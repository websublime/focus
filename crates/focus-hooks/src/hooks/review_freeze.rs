//! PreToolUse: the write-freeze between a terminal review receipt and the
//! gate. A write to a reviewer-bearing stage's declared artifact would void
//! the fresh terminal receipt that covers it; the hook refuses that write
//! before the invalidation happens. Mirrors `aidlc-review-freeze.ts`.
//!
//! Fail-open everywhere, as upstream: malformed stdin, no audit ledger, an
//! unreadable state or graph, an unknown tool, or a conversation that has
//! not joined the workflow all allow the call. The off switch
//! `FOCUS_DISABLE_REVIEW_FREEZE_HOOK=1` disables the check entirely.
//!
//! What the freeze decides from is the review receipt scan
//! (`freshReviewReceipts`), which weighs the ledger's review rows against
//! the attempt window, the source bindings and the Unit claims: the
//! attestation's territory (M5). Focus does not weigh review receipts yet,
//! so a write that would reach that scan (a declared artifact of an open
//! reviewer-bearing stage, in a ledger that holds a review row) is refused
//! by name; a ledger with no review row holds no receipt, and the write
//! passes as upstream passes it.

use std::time::SystemTime;

use focus_audit::read::block_field;
use focus_audit::time::iso_timestamp;
use focus_guards::shell::{CommandShell, Host, write_targets};
use focus_js::EngineError;
use focus_lifecycle::gate::reviewed_artifact_unit;
use focus_lifecycle::registry::intent_repos;
use focus_lifecycle::stage;
use focus_state::progress::{CheckboxState, parse_checkboxes};
use focus_workspace::Layout;
use serde_json::Value;

use super::{open, refused};
use crate::health::{cursor_selection, hooks_health_dir, record_drop, write_status_file};
use crate::{Outcome, context, payload};

const HOOK: &str = "review-freeze";

/// The rows a review leaves in the ledger: a receipt the freeze would weigh.
const REVIEW_ROWS: [&str; 2] = ["REVIEW_REQUESTED", "REVIEW_COMPLETED"];

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let parsed = payload::parse(input);
    let session = parsed
        .as_ref()
        .and_then(|map| map.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    // A conversation that has not joined the selected workflow is not held
    // to its review freeze.
    let workflow = match context::enter(layout, session.as_deref()) {
        Ok(workflow) => workflow,
        Err(err) => return refused(&err),
    };
    if workflow.outside_gate() {
        return Outcome::exit(0);
    }
    // The heartbeat says the host ran this hook, so it comes before the off
    // switch: the freeze switched off never looks like a host running no
    // hooks.
    if let Some(selection) = workflow
        .selection
        .clone()
        .or_else(|| cursor_selection(layout))
    {
        write_status_file(
            &hooks_health_dir(layout, &selection),
            "review-freeze.last",
            &iso_timestamp(SystemTime::now()),
            false,
        );
    }
    // The deterministic off switch; a settings file the resolution refuses
    // is upstream's thrown read, which ends the hook (exit 1).
    match focus_workspace::settings::Places::of(&layout.project).flag(
        &focus_workspace::env::full_name("DISABLE_REVIEW_FREEZE_HOOK"),
        &|name| std::env::var(name).ok(),
    ) {
        Ok(Some(on)) if on == "1" => return Outcome::exit(0),
        Ok(_) => {}
        Err(err) => {
            return Outcome {
                code: 1,
                stdout: String::new(),
                stderr: format!("focus: {err}\n"),
            };
        }
    }
    let Some(parsed) = parsed else {
        return Outcome::exit(0);
    };
    let tool = payload::text(Some(&parsed), "tool_name").to_owned();
    let tool_input = parsed.get("tool_input").and_then(Value::as_object);
    let cwd = parsed.get("cwd").and_then(Value::as_str).map_or_else(
        || layout.project.to_string_lossy().into_owned(),
        str::to_owned,
    );
    // Set by the adapter that ran the tool, outside the agent's input.
    let shell = if parsed.get("focus_shell").and_then(Value::as_str) == Some("powershell") {
        CommandShell::PowerShell
    } else {
        CommandShell::Posix
    };
    let targets = write_targets(&Host::current(), &tool, tool_input, &cwd, shell);
    if targets.is_empty() {
        return Outcome::exit(0);
    }
    // No audit ledger means no receipts to protect: the common case outside
    // a workflow, decided before any state or graph read.
    let Some(selection) = workflow.selection.as_ref() else {
        return Outcome::exit(0);
    };
    let wf = match open(layout, selection, &workflow.invoker) {
        Ok(wf) => wf,
        Err(err) => return refused(&err),
    };
    let shards = wf
        .ledger()
        .shards(&wf.selector(), &wf.invoker, None)
        .unwrap_or_default();
    if shards.is_empty() {
        return Outcome::exit(0);
    }
    let content = match wf.read_state() {
        Ok(content) => content,
        Err(err) => {
            record_drop(layout, selection, HOOK, &err.to_string());
            return Outcome::exit(0);
        }
    };
    // Only open reviewer-bearing stages can hold a receipt the gate still
    // depends on: a completed or skipped stage's artifacts are its record.
    let open_slugs: Vec<String> = parse_checkboxes(&content)
        .into_iter()
        .filter(|checkbox| {
            !matches!(
                checkbox.state,
                CheckboxState::Completed | CheckboxState::Skipped
            )
        })
        .map(|checkbox| checkbox.slug)
        .collect();
    let repos = intent_repos(layout, &selection.space, selection.intent.as_deref());
    let stages = match focus_graph::runtime::load_stages_all(&layout.stage_graph()) {
        Ok(stages) => stages,
        Err(err) => {
            record_drop(layout, selection, HOOK, &err.to_string());
            return Outcome::exit(0);
        }
    };
    for stage in stages.iter().filter(|stage| stage.enabled) {
        if stage::reviewer(stage).is_none() || !open_slugs.contains(&stage.slug) {
            continue;
        }
        for file in &targets {
            if reviewed_artifact_unit(stage, file, &repos).is_none() {
                continue;
            }
            // The write reaches the receipt scan. Its review class resolves
            // to `none` for a scope that reviews nothing; upstream still
            // runs the scan, which answers empty for that class.
            let reviews: Vec<&str> = wf
                .events()
                .iter()
                .filter(|event| {
                    REVIEW_ROWS.contains(&event.event.as_str())
                        && block_field(&event.block, "Stage") == Some(stage.slug.as_str())
                })
                .map(|event| {
                    if event.event == "REVIEW_COMPLETED" {
                        "a completed review"
                    } else {
                        "a review request"
                    }
                })
                .collect();
            if reviews.is_empty() {
                // No receipt: nothing to freeze for this stage.
                continue;
            }
            return refused(&EngineError::new(format!(
                "\"{file}\" is the output document of \"{}\", whose ledger holds {}: whether a fresh terminal review still covers it is the review receipt scan's answer, and Focus does not weigh review receipts yet. Record the review's outcome at the gate, or write elsewhere.",
                stage.slug,
                reviews.join(" and ")
            )));
        }
    }
    Outcome::exit(0)
}

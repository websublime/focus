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
//! (`freshReviewReceipts`): a terminal verdict of the current attempt over
//! the document as it is, or a recovery review still pending, freezes it.
//! The refusal is a guard's (`REVIEW_FREEZE_ACTIVE`): a
//! `REVIEW_FREEZE_BLOCKED` row, the sentence with the way on, and the
//! recovery question left in the refusal's record for the next `next` to
//! ask (exit 2). The fence stands aside when the Guard Policy or the
//! person's own switch lowers it for this piece of work: one line to the
//! person, one row in the ledger, and the write goes through.
//!
//! A stage that runs for each unit freezes unit by unit, off each unit's
//! receipt and claim: with review rows in its ledger, a write to one of
//! its documents is refused by name until units arrive.
//!
//! # What no check tells apart
//!
//! - The receipts and the summary's coverage handed to the guard's attempt:
//!   it would read both the same.
//! - The fingerprints of what the attempt read, handed to the refusal's
//!   record: they enter only the signature of the series, which the
//!   comparison settles by order.

use std::time::SystemTime;

use focus_audit::read::block_field;
use focus_audit::time::iso_timestamp;
use focus_graph::runtime::Stage;
use focus_guards::Decision;
use focus_guards::fences::Fence;
use focus_guards::shell::{CommandShell, Host, write_targets};
use focus_guards::workflow::Guards;
use focus_js::EngineError;
use focus_lifecycle::admission::{memory_strict_holds_guard_policy, resolve_review_class};
use focus_lifecycle::gate::reviewed_artifact_unit;
use focus_lifecycle::guard_refusal::{self, Coverage};
use focus_lifecycle::registry::intent_repos;
use focus_lifecycle::review_receipts::{self, Receipts};
use focus_lifecycle::summary_evidence::{self, Evidence};
use focus_lifecycle::{guard_attempt, guard_series, stage};
use focus_state::progress::{CheckboxState, parse_checkboxes};
use focus_workspace::Layout;
use serde_json::Value;

use super::{append, dropped, is_refusal, open, record_stood_aside, refused, stood_aside_output};
use crate::health::{cursor_selection, hooks_health_dir, record_drop, write_status_file};
use crate::{Outcome, context, payload};
use focus_workspace::env;

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
    let repos = match intent_repos(layout, &selection.space, selection.intent.as_deref()) {
        Ok(repos) => repos,
        Err(err) => return refused(&err),
    };
    let stages = match focus_graph::runtime::load_stages_all(&layout.stage_graph()) {
        Ok(stages) => stages,
        Err(err) => {
            record_drop(layout, selection, HOOK, &err.to_string());
            return Outcome::exit(0);
        }
    };
    let mut frozen: Option<Frozen<'_>> = None;
    'stages: for stage in stages.iter().filter(|stage| stage.enabled) {
        if stage::reviewer(stage).is_none() || !open_slugs.contains(&stage.slug) {
            continue;
        }
        // The receipt scan runs once, for a stage a target is an output of.
        let mut receipts: Option<Receipts> = None;
        for file in &targets {
            if reviewed_artifact_unit(stage, file, &repos).is_none() {
                continue;
            }
            if stage::has_unit_marker(stage) {
                // Not ported: a stage that runs for each unit freezes unit
                // by unit, off each unit's receipt and claim.
                if wf.events().iter().any(|event| {
                    REVIEW_ROWS.contains(&event.event.as_str())
                        && block_field(&event.block, "Stage") == Some(stage.slug.as_str())
                }) {
                    return refused(&EngineError::new(format!(
                        "\"{file}\" is an output document of \"{}\", which runs for each unit and whose ledger holds review rows: which unit's review still covers it is read off the units' rows and claims, and Focus does not read those yet. Record the review's outcome at the gate, or write elsewhere.",
                        stage.slug
                    )));
                }
                continue;
            }
            if receipts.is_none() {
                let class = resolve_review_class(&wf, stage, &content);
                match review_receipts::fresh(
                    &wf,
                    &content,
                    stage,
                    review_receipts::Options {
                        review_class: Some(class),
                        ..Default::default()
                    },
                ) {
                    Ok(read) => receipts = Some(read),
                    Err(err) if is_refusal(&err) => return refused(&err),
                    Err(err) => {
                        record_drop(layout, selection, HOOK, &err.to_string());
                        return Outcome::exit(0);
                    }
                }
            }
            let Some(read) = receipts.as_ref() else {
                continue;
            };
            // `judgeFreeze`: a terminal verdict covers the document, and so
            // does a recovery review still pending: the reviewer writes
            // beside the document, never in it.
            if read.stage_pending.is_some_and(|pending| pending.recovery)
                || read.stage_verdict.is_some()
            {
                frozen = Some(Frozen {
                    stage,
                    target: file.clone(),
                    receipts: read.clone(),
                });
                break 'stages;
            }
        }
    }
    let Some(frozen) = frozen else {
        return Outcome::exit(0);
    };
    let slug = frozen.stage.slug.as_str();

    // The fence stands aside when it is lowered for this piece of work, by
    // the Guard Policy's word or by the person's own switch. The receipt
    // and its verdict are untouched: the person is told in one line and the
    // ledger keeps the row.
    let agent_type = parsed.get("agent_type").and_then(Value::as_str);
    let subagent_type = tool_input
        .and_then(|input| input.get("subagent_type"))
        .and_then(Value::as_str);
    match Guards::read(layout, &wf.selector(), &workflow.invoker) {
        Err(err) if is_refusal(&err) => return refused(&err),
        Err(err) => record_drop(layout, selection, HOOK, &err.to_string()),
        Ok(guards) => {
            // (Upstream hands over the state it has read; the guards read
            // the same file.)
            let decided = guards.decide_for(
                Fence::ReviewFreeze,
                Guards::actor_from(agent_type, subagent_type),
            );
            if decided.decision == Decision::StandAside {
                let stdout = stood_aside_output(layout, &decided, &frozen.target);
                record_stood_aside(&wf, &decided, Some(slug), &tool, &frozen.target);
                return Outcome {
                    code: 0,
                    stdout,
                    stderr: String::new(),
                };
            }
        }
    }

    // The refusal goes in the run's record, so it shows when the freeze
    // bit. Best effort: the block is already decided.
    if wf
        .ledger()
        .audit_file(&wf.selector(), &wf.invoker)
        .is_ok_and(|shard| shard.exists())
    {
        let fields = [
            ("Tool", tool.clone()),
            ("Target", frozen.target.clone()),
            ("Stage", slug.to_owned()),
        ];
        if let Err(err) = append(&wf, "REVIEW_FREEZE_BLOCKED", &fields)
            && let Some(outcome) = dropped(layout, selection, HOOK, &err)
        {
            return outcome;
        }
    }

    // The attempt as the evaluator sees it, from the receipts the freeze
    // has just read. The summary's coverage is the evidence's own word.
    let coverage = match summary_evidence::check(
        &wf,
        frozen.stage,
        summary_evidence::Asked {
            workflow: None,
            state: Some(&content),
            scope: None,
            unit: None,
        },
    ) {
        Ok(Evidence::Ok { .. }) => Coverage::Current,
        Ok(Evidence::Refused { coverage, .. }) => coverage.into(),
        Err(err) => return refused(&err),
    };
    let project_dir = layout.project.to_string_lossy().into_owned();
    let snapshot = match guard_attempt::guard_attempt_state(
        &wf,
        &project_dir,
        &content,
        frozen.stage,
        guard_attempt::Options {
            receipts: Some(frozen.receipts.clone()),
            summary_coverage: Some(coverage),
            ..Default::default()
        },
    ) {
        Ok(snapshot) => snapshot,
        Err(err) => return refused(&err),
    };
    let unattended = guard_refusal::unattended_says(env::var("UNATTENDED").as_deref());
    let named =
        |value: Option<&str>| value.is_some_and(|text| !focus_js::text::trim(text).is_empty());
    let blocked_action = format!("artifact-write:{}", frozen.target);
    let evaluated = match guard_refusal::evaluate(
        &wf,
        &guard_refusal::Input {
            code: "REVIEW_FREEZE_ACTIVE",
            blocked_action: &blocked_action,
            stage: slug,
            unit: None,
            project_dir: Some(&project_dir),
            state: &content,
            invariant: "A terminal review continues to cover the bytes it certified.",
            user_message: "",
            attempt: &snapshot.attempt,
            unattended,
            // This refusal is the fence holding, so the question carries
            // the switch that lowers it beside the workflow's own ways on:
            // not for a subagent, nor where the team keeps the policy
            // strict.
            fence: Some(Fence::ReviewFreeze),
            fence_switch_withheld: named(agent_type)
                || named(subagent_type)
                || memory_strict_holds_guard_policy(&wf, &content),
            summary: None,
            shell: None,
        },
    ) {
        Ok(evaluated) => evaluated,
        Err(err) => return refused(&EngineError::new(err)),
    };
    // The way on the evaluator can take now, or the stage's recovery
    // guidance; and when that cannot be read, the words that always hold.
    let guidance = evaluated
        .remedies
        .iter()
        .find(|remedy| remedy.executable_now)
        .map(|remedy| remedy.action.clone())
        .or_else(|| {
            guard_refusal::recovery_guidance(&wf, &content, slug, None, unattended, None).ok()
        })
        .unwrap_or_else(|| FALLBACK_GUIDANCE.to_owned());
    let refusal = guard_refusal::Refusal {
        user_message: format!(
            "review-freeze: \"{}\" is this stage's output document for stage \"{slug}\", and its latest review is final. Writing it now would make that review no longer cover the document. If this is a reviewer suggestion, quote it at the gate instead of applying it. {guidance}",
            frozen.target
        ),
        ..evaluated
    };
    Outcome {
        code: 2,
        stdout: String::new(),
        stderr: format!(
            "{}\n",
            guard_series::hook_note(&wf, &refusal, &snapshot.attempt, &snapshot.resources)
        ),
    }
}

/// A write the freeze holds: the stage whose review it would void, the
/// path, and the receipts that say so.
struct Frozen<'a> {
    stage: &'a Stage,
    target: String,
    receipts: Receipts,
}

/// `REVIEW_FREEZE_FALLBACK_GUIDANCE`.
const FALLBACK_GUIDANCE: &str = "Record the person's Request Changes decision, with what they said should change (ask only if they have not said), before editing the document; that unlocks it for revision and a fresh review.";

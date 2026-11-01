//! PostToolUse: keep the state file's current stage in step with the
//! host's task list (a `TaskUpdate` to in_progress whose `activeForm` ends
//! in `[slug]`), or, for a host that gives no task payload
//! (`tool_input.source = "ide-audit-sync"`), with the latest `STAGE_STARTED`
//! of the audit, forward only. Mirrors `aidlc-sync-workflow-state.ts`.

use std::time::SystemTime;

use focus_audit::read::find_all_events;
use focus_audit::time::iso_timestamp;
use focus_js::re;
use focus_js::text::trim;
use focus_state::fields::get_field;
use focus_state::progress::{CheckboxState, parse_checkboxes};
use focus_workspace::Layout;
use serde_json::{Value, json};

use super::{is_refusal, open, refused};
use crate::health::{debug, hooks_health_dir, write_status_file};
use crate::status::set_status;
use crate::{Outcome, context, payload};

/// `latestStartedStageSlug`: the stage of the last `STAGE_STARTED` that is
/// not a single-stage workflow's.
#[must_use]
pub fn latest_started_stage_slug(audit: &str) -> Option<String> {
    let started: Vec<_> = find_all_events(audit, "STAGE_STARTED", None)
        .into_iter()
        .filter(|found| !re::test(r"^\*\*Workflow\*\*:\s*single-stage:", "m", &found.block))
        .collect();
    let last = started.last()?;
    re::find(r"^\*\*Stage\*\*:\s*([a-z][a-z0-9-]*)\s*$", "m", &last.block)
        .and_then(|found| re::group(&last.block, &found, 1).map(str::to_owned))
}

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let parsed = payload::parse(input);
    let session = parsed
        .as_ref()
        .and_then(|map| map.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    // A conversation that has not joined the selected workflow does not
    // move its stage.
    let workflow = match context::enter(layout, session.as_deref()) {
        Ok(workflow) => workflow,
        Err(err) => return refused(&err),
    };
    if workflow.stands_outside() {
        return Outcome::exit(0);
    }
    let selection = workflow.selection.as_ref().expect("a selection inside");
    debug(layout, selection, "sync-workflow-state", "invoked", &[]);
    let Some(parsed) = parsed else {
        return Outcome::exit(0);
    };
    let wf = match open(layout, selection, &workflow.invoker) {
        Ok(wf) => wf,
        Err(err) => return refused(&err),
    };
    // The state file must exist (it does not before a workflow is created).
    if !wf.state_path().exists() {
        return Outcome::exit(0);
    }
    let tool_input = parsed.get("tool_input").and_then(Value::as_object);
    let source = payload::text(tool_input, "source");
    let slug: String = if source == "ide-audit-sync" {
        // A forward-only mirror: it may only nudge Current Stage toward the
        // latest STAGE_STARTED, never backward.
        let Ok(state) = wf.read_state() else {
            return Outcome::exit(0);
        };
        let status = trim(get_field(&state, "Status").unwrap_or("")).to_owned();
        let current = trim(get_field(&state, "Current Stage").unwrap_or("")).to_owned();
        let audit_slug = latest_started_stage_slug(&wf.audit_text());
        debug(
            layout,
            selection,
            "sync-workflow-state",
            "ide-audit-sync",
            &[
                ("auditSlug", json!(audit_slug)),
                ("current", json!(current)),
                ("status", json!(status)),
            ],
        );
        // Nothing to do from this record's files: the front gate skips the
        // next call until one of them changes.
        let noop = || {
            write_status_file(
                &hooks_health_dir(layout, selection),
                "sync-workflow-state.noop",
                &iso_timestamp(SystemTime::now()),
                false,
            );
            Outcome::exit(0)
        };
        if status != "Running" {
            return noop();
        }
        if current.is_empty() || current == "none" {
            return noop();
        }
        let Some(audit_slug) = audit_slug.filter(|slug| !slug.is_empty()) else {
            return noop();
        };
        if current == audit_slug {
            return noop();
        }
        // Never sync backward: a stage the workflow has completed or skipped
        // is legitimately ahead of the audit tail.
        if let Some(done) = parse_checkboxes(&state)
            .into_iter()
            .find(|checkbox| checkbox.slug == audit_slug)
            && matches!(
                done.state,
                CheckboxState::Completed | CheckboxState::Skipped
            )
        {
            debug(
                layout,
                selection,
                "sync-workflow-state",
                "skip: audit slug already done/skipped",
                &[
                    ("auditSlug", json!(audit_slug)),
                    ("auditState", json!(done.state.name())),
                ],
            );
            return noop();
        }
        audit_slug
    } else {
        let status = payload::text(tool_input, "status");
        if status != "in_progress" {
            return Outcome::exit(0);
        }
        let active_form = match payload::text_field(tool_input, "activeForm", "sync-workflow-state")
        {
            Ok(text) => text,
            Err(err) => return refused(&err),
        };
        if active_form.is_empty() {
            return Outcome::exit(0);
        }
        let Some(found) = re::find(r"\[([a-z][a-z0-9-]*)\]$", "", active_form) else {
            return Outcome::exit(0);
        };
        re::group(active_form, &found, 1).unwrap_or("").to_owned()
    };
    // Health heartbeat.
    write_status_file(
        &hooks_health_dir(layout, selection),
        "sync-workflow-state.last",
        &iso_timestamp(SystemTime::now()),
        false,
    );
    // Update the state through the shared implementation; the hook owns
    // this mutation.
    debug(
        layout,
        selection,
        "sync-workflow-state",
        "set-status",
        &[("slug", json!(slug))],
    );
    match set_status(&wf, &slug, None, None) {
        Ok(_) => Outcome::exit(0),
        Err(err) if is_refusal(&err) => refused(&err),
        Err(err) => {
            debug(
                layout,
                selection,
                "sync-workflow-state",
                "set-status failed",
                &[("error", json!(err.to_string()))],
            );
            Outcome::exit(0)
        }
    }
}

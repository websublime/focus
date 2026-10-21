//! PreCompact: validate the state file's structure and emit
//! `SESSION_COMPACTED` before the host compacts the conversation, with a
//! recovery breadcrumb under the record's engine folder. Mirrors
//! `aidlc-validate-state.ts`.

use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_state::fields::get_field;
use focus_workspace::Layout;
use focus_workspace::records::valid_session_id;

use super::{append, dropped, is_refusal, open, refused};
use crate::engine::{refuse_directive_marker, refuse_rules_delivery, write_engine_file_no_follow};
use crate::health::{hooks_health_dir, write_status_file};
use crate::{Outcome, context, payload};

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let parsed = payload::parse(input);
    let session = payload::session_id(parsed.as_ref());
    // The compacted chat no longer holds the rule text it was handed,
    // whatever workflow it works on.
    if let Some(session) = session.as_deref().and_then(|id| valid_session_id(Some(id)))
        && let Err(err) = refuse_rules_delivery(layout, session)
    {
        return refused(&err);
    }
    // A compaction in a conversation that has not joined the selected
    // workflow leaves that workflow's heartbeat, breadcrumb and ledger alone.
    let workflow = match context::enter(layout, session.as_deref()) {
        Ok(workflow) => workflow,
        Err(err) => return refused(&err),
    };
    if workflow.stands_outside() {
        return Outcome::exit(0);
    }
    let selection = workflow.selection.as_ref().expect("a selection inside");
    let wf = match open(layout, selection, &workflow.invoker) {
        Ok(wf) => wf,
        Err(err) => return refused(&err),
    };
    // Health heartbeat.
    let health_dir = hooks_health_dir(layout, selection);
    write_status_file(
        &health_dir,
        "validate-state.last",
        &iso_timestamp(SystemTime::now()),
        false,
    );
    if !wf.state_path().exists() {
        return Outcome::exit(0);
    }
    let Ok(content) = wf.read_state() else {
        return Outcome::exit(0);
    };
    if let Err(err) = refuse_directive_marker(layout, selection, "a compaction") {
        return refused(&err);
    }
    // Validate the state file has its required sections.
    let mut missing: Vec<&str> = Vec::new();
    if !content.contains("## Stage Progress") {
        missing.push("Stage Progress");
    }
    if !content.contains("## Current Status") {
        missing.push("Current Status");
    }
    let mut stderr = String::new();
    if !missing.is_empty() {
        stderr.push_str(&format!(
            "WARNING: focus-state.md missing sections: {}\n",
            missing.join(", ")
        ));
    }
    let state_status = if missing.is_empty() {
        "valid (all required sections present)".to_owned()
    } else {
        format!("INVALID — missing sections: {}", missing.join(", "))
    };
    // The recovery breadcrumb, never through a link; one that cannot be kept
    // never fails the compaction.
    let current_stage = get_field(&content, "Current Stage")
        .unwrap_or("")
        .to_owned();
    let timestamp = iso_timestamp(SystemTime::now());
    let _ = write_engine_file_no_follow(
        layout,
        selection,
        "recovery.md",
        &format!(
            "# FOCUS Recovery Breadcrumb\n**Last validated**: {timestamp}\n**Current stage**: {current_stage}\n**State file**: {state_status}\n"
        ),
    );
    // Emit SESSION_COMPACTED if an audit file exists for this workflow. Which
    // shard that is can depend on a record Focus does not read (a unit
    // claim): that is said, not passed over as no shard.
    let shard = match wf.ledger().audit_file(&wf.selector(), &wf.invoker) {
        Ok(shard) => Some(shard),
        Err(err) if is_refusal(&err) => return refused(&err),
        Err(_) => None,
    };
    if shard.is_some_and(|shard| shard.exists()) {
        let fields = [
            ("Current Stage", current_stage),
            (
                "State Validity",
                if missing.is_empty() {
                    "valid"
                } else {
                    "invalid"
                }
                .to_owned(),
            ),
        ];
        if let Err(err) = append(&wf, "SESSION_COMPACTED", &fields)
            && let Some(outcome) = dropped(layout, selection, "validate-state", &err)
        {
            return outcome;
        }
    }
    Outcome {
        code: 0,
        stdout: String::new(),
        stderr,
    }
}

//! The verbs of `state` that only leave a row: `acknowledge-compaction`
//! and `practices-event` (`handleAcknowledgeCompaction` and
//! `handlePracticesEvent`, `aidlc-state.ts` L7 803 and L7 867).
//!
//! Upstream routes these through its tool so that every row of the ledger
//! is written by code, never by a stage's prose.

use focus_js::{EngineError, Result, re, text as js_text};
use focus_state::fields::get_field;
use serde_json::{Value, json};

use crate::workflow::Workflow;

/// Whether the latest `SESSION_COMPACTED` of the ledger's tail is still
/// waiting: nothing of the work, and no recovery, was recorded after it.
#[must_use]
pub fn compaction_pending(audit: &str) -> bool {
    if audit.is_empty() {
        return false;
    }
    let lines: Vec<&str> = audit.split('\n').collect();
    let tail = lines[lines.len().saturating_sub(400)..].join("\n");
    tail.rfind("**Event**: SESSION_COMPACTED").is_some_and(|at| {
        !re::test(
            r"\*\*Event\*\*: (STAGE_STARTED|STAGE_COMPLETED|GATE_APPROVED|SESSION_RESUMED|RECOVERY_COMPLETED)",
            "",
            &tail[at..],
        )
    })
}

/// `handleAcknowledgeCompaction`: the person was shown the prompt that
/// follows a compaction and chose; `RECOVERY_COMPLETED` closes the window.
///
/// # Errors
///
/// No choice, or one that is none; no compaction waiting; the state file
/// or the ledger cannot be read or written.
pub fn acknowledge_compaction(wf: &Workflow, args: &[String]) -> Result<Value> {
    let mut choice = "";
    let mut at = 0;
    while at < args.len() {
        if args[at] == "--choice" && at + 1 < args.len() {
            choice = &args[at + 1];
            at += 1;
        }
        at += 1;
    }
    if choice.is_empty() {
        return Err(EngineError::new(
            "Usage: focus engine state acknowledge-compaction --choice <continue|review|restart>",
        ));
    }
    if !["continue", "review", "restart"].contains(&choice) {
        return Err(EngineError::new(format!(
            "Invalid --choice: {choice}. Valid: continue, review, restart"
        )));
    }
    let content = wf.read_state()?;
    let current = get_field(&content, "Current Stage")
        .filter(|stage| !stage.is_empty())
        .unwrap_or("unknown");
    if !compaction_pending(&wf.audit_text()) {
        return Err(EngineError::new(
            "No pending compaction to acknowledge (latest SESSION_COMPACTED already followed by stage activity or recovery).",
        ));
    }
    let held = wf.lock("acknowledging a compaction")?;
    wf.emit_plain(
        &held,
        "RECOVERY_COMPLETED",
        &[
            ("Choice", choice.to_owned()),
            ("Current Stage", current.to_owned()),
        ],
    )?;
    Ok(json!({ "acknowledged": true, "choice": choice, "current_stage": current }))
}

/// `handlePracticesEvent`: one of the three rows the practices-discovery
/// stage leaves, with the fields it gives (`--field "Key: Value"`, as often
/// as needed; a later value of a key replaces an earlier one where the
/// first stood). A field's name is checked by the ledger's writer.
///
/// # Errors
///
/// No `--type`, one that is none, or `affirmed`, which only the promotion
/// of the practices records.
pub fn practices_event(wf: &Workflow, args: &[String]) -> Result<Value> {
    let mut kind = "";
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    while at < args.len() {
        if args[at] == "--type" && at + 1 < args.len() {
            kind = &args[at + 1];
            at += 1;
        } else if args[at] == "--field" && at + 1 < args.len() {
            let given = &args[at + 1];
            if let Some(colon) = given.find(':').filter(|colon| *colon > 0) {
                let key = js_text::trim(&given[..colon]).to_owned();
                let value = js_text::trim(&given[colon + 1..]).to_owned();
                match fields.iter_mut().find(|(known, _)| *known == key) {
                    Some(field) => field.1 = value,
                    None => fields.push((key, value)),
                }
            }
            at += 1;
        }
        at += 1;
    }
    if kind.is_empty() {
        return Err(EngineError::new(
            "Usage: focus engine state practices-event --type <discovered|override|empty> [--field \"Key: Value\"]...",
        ));
    }
    let event = match kind {
        "discovered" => "PRACTICES_DISCOVERED",
        "override" => "PRACTICES_OVERRIDE",
        "empty" => "PRACTICES_SECTION_EMPTY",
        "affirmed" => {
            return Err(EngineError::new(
                "PRACTICES_AFFIRMED is reserved for practices-promote so the audit receipt cannot be minted without successful memory promotion.",
            ));
        }
        other => {
            return Err(EngineError::new(format!(
                "Invalid --type: {other}. Must be discovered, override, or empty."
            )));
        }
    };
    // `__proto__` names no key of an object of upstream's: it is dropped,
    // uncounted, where the ledger's writer would have refused the name. (The
    // writer itself lists the fields as an object does, a key that is a
    // number first.)
    fields.retain(|(key, _)| key != "__proto__");
    let row: Vec<(&str, String)> = fields
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect();
    let held = wf.lock("recording a practices event")?;
    wf.emit_plain(&held, event, &row)?;
    Ok(json!({ "emitted": event, "fields_count": fields.len() }))
}

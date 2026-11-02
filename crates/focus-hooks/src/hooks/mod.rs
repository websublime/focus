//! The observers: each hook as upstream's `run(input)`, answering with an
//! [`Outcome`](crate::Outcome).

pub mod log_subagent;
pub mod rebuild_stage_graph;
pub mod review_freeze;
pub mod reviewer_scope;
pub mod session_end;
pub mod session_start;
pub mod sync_workflow_state;
pub mod validate_state;
pub mod write_audit_log;

use focus_js::EngineError;
use focus_lifecycle::workflow::Workflow;
use focus_workspace::Layout;
use focus_workspace::records::{Invoker, Selection};

use crate::Outcome;

/// A refusal by name: what upstream would have done quietly, Focus says and
/// exits 1 on, as the rule for input it does not port.
pub(crate) fn refused(err: &EngineError) -> Outcome {
    Outcome {
        code: 1,
        stdout: String::new(),
        stderr: format!("{err}\n"),
    }
}

/// Whether an error is a refusal by name (input Focus does not port yet,
/// worded `Focus does not …` or `Focus cannot …` throughout the crates),
/// which a hook says rather than swallow into a drop.
pub(crate) fn is_refusal(err: &EngineError) -> bool {
    let text = err.to_string();
    text.contains("Focus does not") || text.contains("Focus cannot")
}

/// An emission that failed: a refusal by name is said (exit 1); anything
/// else is recorded as a drop, as upstream's hooks do, and the hook goes on.
pub(crate) fn dropped(
    layout: &Layout,
    selection: &Selection,
    hook: &str,
    err: &EngineError,
) -> Option<Outcome> {
    if is_refusal(err) {
        return Some(refused(err));
    }
    crate::health::record_drop(layout, selection, hook, &err.to_string());
    None
}

/// The record a hook appends to, opened with the invoker the hook resolved
/// under.
pub(crate) fn open(
    layout: &Layout,
    selection: &Selection,
    invoker: &Invoker,
) -> Result<Workflow, EngineError> {
    Workflow::open(layout.clone(), selection.clone(), invoker.clone())
}

/// `writeGuardStoodAside`: the one line the person hears when a fence
/// stands aside, as Claude Code shows it (`systemMessage`) or as the plain
/// line other hosts read; nothing under Guard Policy off.
pub(crate) fn stood_aside_output(
    layout: &Layout,
    decided: &focus_guards::FenceDecision,
    detail: &str,
) -> String {
    if !decided.speaks() {
        return String::new();
    }
    let line = decided.stood_aside_line(Some(detail), true);
    if layout.harness == focus_harness::Harness::Claude {
        format!(
            "{}\n",
            focus_js::json::stringify(&serde_json::json!({ "systemMessage": line }))
        )
    } else {
        format!("{line}\n")
    }
}

/// `recordGuardStoodAside`: one `GUARD_STOOD_ASIDE` row, when this clone's
/// shard exists. Best effort.
pub(crate) fn record_stood_aside(
    wf: &Workflow,
    decided: &focus_guards::FenceDecision,
    stage: Option<&str>,
    tool: &str,
    details: &str,
) {
    let shard = wf.ledger().audit_file(&wf.selector(), &wf.invoker);
    if !shard.is_ok_and(|shard| shard.exists()) {
        return;
    }
    let mut fields: Vec<(&str, String)> = vec![
        ("Guard", decided.fence.name().to_owned()),
        ("Authority", decided.authority.covered.as_str().to_owned()),
        ("Grant", decided.authority.grant_source().to_owned()),
        ("Actor", decided.authority.actor.as_str().to_owned()),
    ];
    if let Some(stage) = stage {
        fields.push(("Stage", stage.to_owned()));
    }
    fields.push(("Tool", tool.to_owned()));
    fields.push(("Details", details.to_owned()));
    let _ = append(wf, "GUARD_STOOD_ASIDE", &fields);
}

/// `appendAuditEntry`: one row under the audit lock.
pub(crate) fn append(
    wf: &Workflow,
    event: &str,
    fields: &[(&str, String)],
) -> Result<(), EngineError> {
    let held = wf.lock(&format!("appending {event}"))?;
    wf.emit_plain(&held, event, fields)
}

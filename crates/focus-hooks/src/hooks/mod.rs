//! The observers: each hook as upstream's `run(input)`, answering with an
//! [`Outcome`](crate::Outcome).

pub mod log_subagent;
pub mod session_end;
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

/// `appendAuditEntry`: one row under the audit lock.
pub(crate) fn append(
    wf: &Workflow,
    event: &str,
    fields: &[(&str, String)],
) -> Result<(), EngineError> {
    let held = wf.lock(&format!("appending {event}"))?;
    wf.emit_plain(&held, event, fields)
}

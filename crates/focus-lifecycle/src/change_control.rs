//! The rows that record what the Guard Policy did: a change after an
//! approval that it let through (`CHANGE_ACCEPTED`), and a change of the
//! policy itself (`GUARD_POLICY_SET`).
//!
//! Under a policy that is not strict, the row of an accepted change is the
//! evidence that stands in for the refusal, and the row of a policy that
//! moved is how the move is traced. Neither is best effort: a ledger that
//! cannot be written refuses the checkpoint instead of going on unrecorded
//! (`governedGuardPolicy`, `recordAcceptedChanges`, `acceptedChangeFields`
//! and `changeControlLedgerAppend`, `aidlc-lib.ts` L40 155 to L40 370).
//!
//! A change is said once: the same recorded and current values for the same
//! checkpoint, stage and unit never give a second row or a second line for
//! the person. A summary's pair is a receipt and a time shared by every
//! output of one confirmation, so there the path is part of what the change
//! is.

use focus_audit::read::block_field;
use focus_evidence::source::listing::render_changed_paths;
use focus_guards::policy::{GuardPolicy, Resolution};
use focus_guards::workflow::Guards;
use focus_js::{EngineError, Result, bail};
use focus_workspace::env;
use focus_workspace::lock::WorkspaceLock;

use crate::workflow::Workflow;

/// `AcceptedChange`: a change the Guard Policy let through, to record and
/// to say once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Accepted<'a> {
    /// `plan-approval`, `review-receipt` or `summary-confirmation`.
    pub checkpoint: &'a str,
    pub stage: &'a str,
    pub unit: Option<&'a str>,
    /// The paths that changed; `None` when only digests are known.
    pub changed: Option<&'a [String]>,
    /// What the approval recorded, and what is there now.
    pub recorded: &'a str,
    pub current: &'a str,
    /// The one line the person hears.
    pub notice: &'a str,
}

/// `acceptedChangeFields`: what the row of an accepted change carries.
#[must_use]
pub fn accepted_change_fields(change: &Accepted<'_>) -> Vec<(&'static str, String)> {
    let mut fields = vec![("Stage", change.stage.to_owned())];
    if let Some(unit) = change.unit.filter(|unit| !unit.is_empty()) {
        fields.push(("Unit", unit.to_owned()));
    }
    fields.extend([
        ("Checkpoint", change.checkpoint.to_owned()),
        (
            "Changed",
            change
                .changed
                .map_or_else(|| "(paths unavailable)".to_owned(), render_changed_paths),
        ),
        ("Recorded", change.recorded.to_owned()),
        ("Current", change.current.to_owned()),
        ("Details", change.notice.to_owned()),
    ]);
    fields
}

/// `acceptedChangeMatchesRow`: the row already says this change.
fn matches_row(change: &Accepted<'_>, block: &str) -> bool {
    let fields = accepted_change_fields(change);
    let said = |name: &str| {
        fields
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, value)| value.as_str())
    };
    block_field(block, "Stage") == said("Stage")
        && block_field(block, "Unit") == change.unit
        && block_field(block, "Checkpoint") == said("Checkpoint")
        && block_field(block, "Recorded") == said("Recorded")
        && block_field(block, "Current") == said("Current")
        && (change.checkpoint != "summary-confirmation"
            || block_field(block, "Changed") == said("Changed"))
}

/// `changeControlLedgerAppend`: one of the two rows, written under the lock
/// the caller holds. A record with no state file has nowhere to write to,
/// and the test seam fails every append.
fn append(
    wf: &Workflow,
    held: &WorkspaceLock,
    event: &str,
    fields: &[(&'static str, String)],
) -> Result<()> {
    if !wf.state_path().exists() {
        bail!(
            "Guard Policy has no intent record to write to: {} is missing.",
            wf.state_path()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
    }
    if let Some(fault) = env::var("TEST_CHANGE_CONTROL_LEDGER_FAULT")
        .map(|fault| focus_js::text::trim(&fault).to_owned())
        .filter(|fault| !fault.is_empty())
    {
        bail!("injected ledger fault: {fault}");
    }
    wf.emit_plain(held, event, fields)
}

/// `recordAcceptedChanges`: the rows for the changes the ledger does not
/// carry yet, and the lines for the person for exactly those.
///
/// # Errors
///
/// A row cannot be written: an acceptance that cannot be recorded is not
/// one, so the caller refuses instead of going on.
pub fn record_accepted_changes(
    wf: &Workflow,
    held: &WorkspaceLock,
    changes: &[Accepted<'_>],
) -> Result<Vec<String>> {
    if changes.is_empty() {
        return Ok(Vec::new());
    }
    let events = wf.events();
    let mut notices = Vec::new();
    for change in changes {
        if events
            .iter()
            .any(|row| row.event == "CHANGE_ACCEPTED" && matches_row(change, &row.block))
        {
            continue;
        }
        append(wf, held, "CHANGE_ACCEPTED", &accepted_change_fields(change)).map_err(|error| {
            EngineError::new(format!(
                "Cannot continue under a relaxed or off Guard Policy: the accepted change for \"{}\"{} could not be recorded in the audit ledger ({error}). Repair the ledger, then run the same command again.",
                change.stage,
                change
                    .unit
                    .filter(|unit| !unit.is_empty())
                    .map(|unit| format!(" (unit {unit})"))
                    .unwrap_or_default()
            ))
        })?;
        notices.push(change.notice.to_owned());
    }
    Ok(notices)
}

/// `governedGuardPolicy`: the policy in force, with its move traced: when
/// it is not the last one the ledger recorded (or, with none recorded, the
/// state's own), the row that says so is written.
///
/// # Errors
///
/// The policy cannot be read, or the row cannot be written.
pub fn governed_guard_policy(
    wf: &Workflow,
    held: &WorkspaceLock,
    state: &str,
) -> Result<Resolution> {
    let resolution = Guards::read(&wf.layout, &wf.selector(), &wf.invoker)?
        .with_state(state)
        .policy(false)?;
    if !wf.state_path().exists() {
        return Ok(resolution);
    }
    let mut previous: Option<GuardPolicy> = None;
    for row in wf.events() {
        if row.event != "GUARD_POLICY_SET" && row.event != "CHANGE_CONTROL_SET" {
            continue;
        }
        if let Some(value) = block_field(&row.block, "New Value").and_then(GuardPolicy::parse) {
            previous = Some(value);
        }
    }
    let previous = previous.unwrap_or(resolution.state_value);
    if previous != resolution.value {
        let fields = [
            ("Old Value", previous.as_str().to_owned()),
            ("New Value", resolution.value.as_str().to_owned()),
            ("Source", resolution.source.clone()),
        ];
        append(wf, held, "GUARD_POLICY_SET", &fields).map_err(|error| {
            EngineError::new(format!(
                "Cannot record the Guard Policy change ({} to {}, source {}) in the audit ledger: {error}",
                previous.as_str(),
                resolution.value.as_str(),
                resolution.source
            ))
        })?;
    }
    Ok(resolution)
}

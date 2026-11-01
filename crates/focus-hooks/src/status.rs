//! `setStatus` in `aidlc-utility.ts`: the state file moved to a stage by
//! the sync hook, which owns this mutation.

use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_js::{Result, bail};
use focus_lifecycle::workflow::Workflow;
use focus_state::fields::set_field;
use focus_state::progress::{CheckboxState, set_checkbox};

use crate::engine::refuse_directive_marker;

/// What `setStatus` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub phase: String,
    pub stage: String,
    pub agent: String,
}

/// `setStatus(projectDir, { stage, phase?, agent? })`.
///
/// # Errors
///
/// No state file; no stage named; an unknown stage; the record carries the
/// orchestrator's directive marker (M5), which the move would refresh; or
/// the file cannot be written.
pub fn set_status(
    wf: &Workflow,
    stage: &str,
    phase: Option<&str>,
    agent: Option<&str>,
) -> Result<Moved> {
    if !wf.state_path().exists() {
        bail!(
            "No state file found. Start a workflow first by describing what to build (/focus \"build the auth service\")."
        );
    }
    if stage.is_empty() {
        bail!("--stage is required for set-status");
    }
    let entry = wf.stage(stage)?;
    let phase = phase
        .filter(|phase| !phase.is_empty())
        .unwrap_or(&entry.phase)
        .to_uppercase();
    let agent = agent
        .filter(|agent| !agent.is_empty())
        .unwrap_or(focus_lifecycle::stage::lead_agent(entry))
        .to_owned();
    let previous = wf.read_state()?;
    refuse_directive_marker(&wf.layout, &wf.selection, "set-status")?;
    let mut content = previous;
    content = set_field(&content, "Lifecycle Phase", &phase);
    content = set_field(&content, "Active Agent", &agent);
    content = set_field(&content, "Status", "Running");
    content = set_field(&content, "Last Updated", &iso_timestamp(SystemTime::now()));
    content = set_field(&content, "Current Stage", stage);
    content = set_field(&content, "In Progress", stage);
    content = set_checkbox(&content, stage, CheckboxState::InProgress);
    wf.write_state(&content)?;
    Ok(Moved {
        phase,
        stage: stage.to_owned(),
        agent,
    })
}

//! Parking a workflow and taking it up again: `park` records where the
//! work stopped (`Parked`, `Parked At Stage`, and `Parked By: person` when
//! the person asked), `unpark` clears it.
//!
//! A team's units are parked one at a time from a checkout's unit scope
//! stamp, and that stamp, the claim registry it is checked against and the
//! fan-out it belongs to are the units' lifecycle (M4): with Unit Ownership
//! team, both verbs refuse by name.
//!
//! Mirrors `handlePark`, `parkWorkflow` and `handleUnpark` in
//! `aidlc-state.ts`.

use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_js::text::trim;
use focus_js::{Result, bail};
use focus_state::fields::{get_field, remove_field, set_field, set_or_insert_field};
use focus_workspace::env;
use serde_json::{Map, Value, json};

use crate::presence;
use crate::workflow::Workflow;

const RUNTIME_HEADING: &str = "## Runtime State";

/// `humanTurnMintAllowed`: an unattended run mints no human turn.
fn attended_session() -> bool {
    env::var("UNATTENDED").as_deref() != Some("1")
}

fn autonomous(content: &str) -> bool {
    get_field(content, "Construction Autonomy Mode").map(trim) == Some("autonomous")
}

/// `isTeamUnitOwnership`.
fn team_unit_ownership(content: &str) -> bool {
    get_field(content, "Unit Ownership").map(trim) == Some("team")
}

fn refuse_team_units(content: &str, verb: &str) -> Result<()> {
    if team_unit_ownership(content) {
        bail!(
            "Unit Ownership is team, so `{verb}` works on the unit this checkout claimed, read from its unit scope stamp and checked against the claim registry: Focus does not run units yet."
        );
    }
    Ok(())
}

/// `handlePark`.
///
/// # Errors
///
/// What upstream refuses, in its words; what Focus refuses by name.
pub fn park(wf: &Workflow) -> Result<Value> {
    let initial = wf.read_state()?;
    let attended = attended_session();
    // Their stop outlives the run's own approvals after it.
    let replied = attended && presence::person_spoke_since_gate(wf, false, false, true);
    let missed_reply = attended
        && !replied
        && presence::hook_activation(wf).is_some_and(|activation| {
            activation
                .get("missedReply")
                .is_some_and(|value| !value.is_null())
                || activation.get("missesReplies") == Some(&Value::Bool(true))
        })
        && autonomous(&initial);
    let mut result = park_workflow(wf, &initial, replied || missed_reply)?;
    if missed_reply {
        result.insert(
            "note".to_owned(),
            json!("No reply from the person was on record (this host can miss one), so this stop is recorded as theirs. Tell them in one line that the work is parked and resumes when they ask."),
        );
    }
    Ok(Value::Object(result))
}

/// `parkWorkflow`.
fn park_workflow(wf: &Workflow, initial: &str, attended: bool) -> Result<Map<String, Value>> {
    let autonomous = autonomous(initial);
    // An unattended run has nobody to resume it, so it never parks itself
    // under the autonomous grant, whatever the caller says.
    if autonomous && !attended_session() {
        bail!(
            "Refusing to park: {}=1 is set and Construction Autonomy Mode is autonomous, so nobody is here to resume the run; it keeps moving.",
            env::full_name("UNATTENDED")
        );
    }
    if !attended && autonomous {
        bail!(
            "Refusing to park: Construction Autonomy Mode is autonomous and no reply from the person is on record since the last decision, so the run keeps moving. When the person asks to stop, park then: their stop wins over the autonomous grant. If their stop is not on record, run `focus engine bolt set-autonomy --mode gated` (Construction then stops for approval at each Bolt), then park."
        );
    }
    refuse_team_units(initial, "park")?;
    let held = wf.lock("parking the workflow")?;
    let mut content = wf.read_state()?;
    let status = get_field(&content, "Status");
    if status == Some("Completed") {
        bail!("Workflow is already Completed - nothing to park.");
    }
    if status == Some("Archived") {
        bail!(
            "Workflow is Archived - nothing to park. Bring it back first with `/focus intent unarchive <name>`."
        );
    }
    let current = get_field(&content, "Current Stage")
        .unwrap_or("")
        .to_owned();
    if current.is_empty() {
        bail!("State file has no Current Stage - cannot park.");
    }
    let timestamp = iso_timestamp(SystemTime::now());
    wf.emit_plain(&held, "WORKFLOW_PARKED", &[("Stage", current.clone())])?;
    content = set_or_insert_field(&content, RUNTIME_HEADING, "Parked", &timestamp)?;
    content = set_or_insert_field(&content, RUNTIME_HEADING, "Parked At Stage", &current)?;
    // A person's park is theirs to resume.
    content = if attended {
        set_or_insert_field(&content, RUNTIME_HEADING, "Parked By", "person")?
    } else {
        remove_field(&content, "Parked By")
    };
    content = set_field(&content, "Last Updated", &timestamp);
    wf.write_state(&content)?;
    let mut out = Map::new();
    out.insert("parked".to_owned(), json!(true));
    out.insert("stage".to_owned(), json!(current));
    out.insert("timestamp".to_owned(), json!(timestamp));
    Ok(out)
}

/// `handleUnpark`: idempotent.
///
/// # Errors
///
/// The state cannot be read or written, or the row cannot be appended.
pub fn unpark(wf: &Workflow) -> Result<Value> {
    let preflight = wf.read_state()?;
    refuse_team_units(&preflight, "unpark")?;
    let held = wf.lock("unparking the workflow")?;
    let mut content = wf.read_state()?;
    let was_parked = get_field(&content, "Parked")
        .map(trim)
        .is_some_and(|parked| !parked.is_empty());
    content = remove_field(&content, "Parked");
    content = remove_field(&content, "Parked At Stage");
    content = remove_field(&content, "Parked By");
    if was_parked {
        let timestamp = iso_timestamp(SystemTime::now());
        wf.emit_plain(&held, "WORKFLOW_UNPARKED", &[])?;
        content = set_field(&content, "Last Updated", &timestamp);
    }
    wf.write_state(&content)?;
    Ok(json!({"unparked": true, "was_parked": was_parked}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_autonomy_mode_and_the_ownership_are_read_trimmed() {
        assert!(autonomous(
            "- **Construction Autonomy Mode**:  autonomous \n"
        ));
        assert!(!autonomous("- **Construction Autonomy Mode**: gated\n"));
        assert!(team_unit_ownership("- **Unit Ownership**: team\n"));
        assert!(!team_unit_ownership("- **Unit Ownership**: solo\n"));
        assert!(!team_unit_ownership(""));
    }
}

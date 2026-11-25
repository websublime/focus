//! How Construction runs, as the person set it: the walking skeleton's
//! stance, the iteration over the units, the checkpoints, the execution and
//! the command that verifies a unit (`handleSetSkeletonStance`,
//! `handleSetConstructionIteration`, `handleSetConstructionPolicy` and
//! `handleSetConstructionVerificationCommand`, `aidlc-state.ts` L1 126 to
//! L1 384), and the two verbs of a team's units that answer without a team
//! (`handleSetUnitGateRhythm`, L1 549, and `handleSyncUnitScopeStage`,
//! L2 121).
//!
//! These are preferences of the run: none moves the lifecycle or grants
//! autonomy. During Construction a change of iteration, checkpoints or
//! execution is the person's: they asked for it in a message since the
//! last decision, or chose it at a policy question whose receipt
//! (`CONSTRUCTION_POLICY_RECORDED`) is the latest word about that field.
//! The verification command is never believed off the state alone: the
//! latest `VERIFICATION_COMMAND_RECORDED` of this workflow has to carry its
//! digest and the person's approval.
//!
//! # What is refused by name
//!
//! `Unit Ownership: team` is deferred (the plan, section 11). With it, the
//! units' gate rhythm and a claimed unit's stage are a team's, and both
//! verbs refuse by name. Without it they refuse as upstream does.

use focus_audit::read::{block_field, has_unsafe_single_line_character};
use focus_audit::shards::ShardEvent;
use focus_js::text::{to_lower_case, trim};
use focus_js::{EngineError, Result, bail, re};
use focus_state::fields::{get_field, set_or_insert_field};
use focus_workspace::lock::WorkspaceLock;
use focus_workspace::records::active_intent_uuid;
use focus_workspace::{env, files, links};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::presence;
use crate::review_attempt::{definitely_before, is_team_unit_ownership, maximal};
use crate::workflow::Workflow;

const RUNTIME_HEADING: &str = "## Runtime State";

/// `SKELETON_STANCES`.
const SKELETON_STANCES: [&str; 3] = ["on", "off", "scope-dependent"];

const ITERATIONS: [&str; 2] = ["unit-major", "stage-major"];

const RHYTHMS: [&str; 2] = ["per-stage", "unit-end"];

/// `CHECKPOINT_GLOSS`.
const CHECKPOINT_GLOSS: &str = "a stop after each Unit for you to check and approve it";

/// `VERIFICATION_COMMAND_CHECKPOINT`.
const VERIFICATION_COMMAND: &str = "Construction Verification Command";

/// `CONSTRUCTION_POLICY_CHECKPOINT`.
const POLICY_CHECKPOINT: &str = "Construction Policy";

/// `PERSONS_WORDS_FIELD`.
const PERSONS_WORDS_FIELD: &str = "Person Reply";

/// `CONSTRUCTION_POLICY_RECOVERY`.
const POLICY_RECOVERY: &str = "When the person asks for this Construction change, run focus engine state set-construction-checkpoints, set-construction-execution, or set-construction-iteration with the value they asked for, then say its notice line to them. Do not ask them to confirm it, and never change it on your own.";

/// `VERIFICATION_COMMAND_RECOVERY`.
const VERIFICATION_RECOVERY: &str = "Write the proposed command to <record>/verification-command.txt with the harness file-write tool (never shell echo or a heredoc); never interpolate repo-derived command text into a shell line. Record the human choice with focus engine log decision --stage \"<stage>\" --checkpoint verification-command --command-file verification-command.txt --decision \"Use this command to verify each completed Unit?\" --options \"Approve,Request Changes\", then wait for the human's offered choice in that session and run focus engine log answer --stage \"<stage>\" --checkpoint verification-command --command-file verification-command.txt --details \"Approve\". Both commands find the session they run in. Apply the receipt with focus engine state set-construction-verification-command --command-file verification-command.txt.";

/// `handleSetSkeletonStance`: the stance the conductor classified. It is
/// metadata the next step reads, not a transition: no row is left.
///
/// # Errors
///
/// No stance, or one that is none; the state cannot be read or written.
pub fn set_skeleton_stance(wf: &Workflow, args: &[String]) -> Result<Value> {
    let Some(stance) = args.first() else {
        bail!(
            "Usage: focus engine state set-skeleton-stance <{}>",
            SKELETON_STANCES.join("|")
        );
    };
    if !SKELETON_STANCES.contains(&stance.as_str()) {
        bail!(
            "Invalid skeleton stance \"{stance}\". Valid: {}.",
            SKELETON_STANCES.join(", ")
        );
    }
    let _held = wf.lock("setting the skeleton stance")?;
    let content = wf.read_state()?;
    let updated = set_or_insert_field(&content, RUNTIME_HEADING, "Skeleton Stance", stance)?;
    wf.write_state(&updated)?;
    Ok(json!({ "updated": true, "skeleton_stance": stance }))
}

/// What the person hears of one value of a setting.
struct Line {
    now: String,
    was: &'static str,
    already: String,
}

fn line(value: &str) -> Option<Line> {
    let checkpoints = |state: &str| Line {
        now: format!("Construction checkpoints ({CHECKPOINT_GLOSS}) are {state} for this work now"),
        was: if state == "on" {
            "they were on"
        } else {
            "they were off"
        },
        already: format!(
            "Construction checkpoints ({CHECKPOINT_GLOSS}) are already {state} for this work"
        ),
    };
    let walk = |how: &str, was: &'static str| Line {
        now: format!("Construction now {how}"),
        was,
        already: format!("Construction already {how}"),
    };
    Some(match value {
        "enabled" => checkpoints("on"),
        "disabled" => checkpoints("off"),
        "unit-major" => walk("builds one Unit at a time", "it built one Unit at a time"),
        "stage-major" => walk("goes stage by stage", "it went stage by stage"),
        "swarm" => walk(
            "builds the Units in parallel",
            "it built the Units in parallel",
        ),
        "serial" => walk(
            "builds the Units one after another",
            "it built the Units one after another",
        ),
        _ => return None,
    })
}

/// `constructionPolicyNotice`: the one line the person hears for a setting
/// they asked for: what it is now, what it was, and that it can be switched
/// back; or that it already was so.
fn notice(before: &str, after: &str, field: &str) -> String {
    let previous = get_field(before, field).map_or("", trim);
    let value = get_field(after, field).map_or("", trim);
    let now = line(value);
    if previous == value {
        return now.map_or_else(
            || format!("{field} is already {value}."),
            |now| format!("{}.", now.already),
        );
    }
    let Some(now) = now else {
        return format!("{field} is now {value}.");
    };
    let was = line(previous).map_or_else(String::new, |was| format!(" ({})", was.was));
    format!("{}{was}. You can switch back any time.", now.now)
}

/// The rows of the workflow itself: a stage run by itself keeps its own.
fn workflow_rows(wf: &Workflow) -> Option<Vec<ShardEvent>> {
    let mut unreadable = Vec::new();
    let rows = wf
        .ledger()
        .read_events(&wf.selector(), &wf.invoker, Some(&mut unreadable))
        .ok()?;
    if !unreadable.is_empty() {
        return None;
    }
    Some(
        rows.into_iter()
            .filter(|row| {
                !block_field(&row.block, "Workflow")
                    .is_some_and(|workflow| workflow.starts_with("single-stage:"))
            })
            .collect(),
    )
}

/// The one start of the workflow nothing is after, when there is one.
fn workflow_start(rows: &[ShardEvent]) -> Option<&ShardEvent> {
    let starts: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| row.event == "WORKFLOW_STARTED")
        .collect();
    match maximal(&starts).as_slice() {
        [start] => Some(*start),
        _ => None,
    }
}

/// `authorizedConstructionPolicyChange`: the person chose this value at a
/// policy question. Applying a receipt sets its value, and going back to
/// another needs a newer receipt for the field, so one that was used never
/// becomes live again.
fn receipt_authorizes(wf: &Workflow, state: &str, field: &str, value: &str) -> bool {
    if get_field(state, field) == Some(value) || active_intent_uuid(&wf.layout, None).is_none() {
        return false;
    }
    let Some(rows) = workflow_rows(wf) else {
        return false;
    };
    let Some(start) = workflow_start(&rows) else {
        return false;
    };
    let about: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| {
            matches!(
                row.event.as_str(),
                "DECISION_RECORDED" | "QUESTION_ANSWERED" | "CONSTRUCTION_POLICY_RECORDED"
            ) && block_field(&row.block, "Checkpoint") == Some(POLICY_CHECKPOINT)
                && block_field(&row.block, "Field") == Some(field)
        })
        .collect();
    let [receipt] = maximal(&about)[..] else {
        return false;
    };
    receipt.event == "CONSTRUCTION_POLICY_RECORDED"
        && definitely_before(start, receipt)
        && block_field(&receipt.block, "Value") == Some(value)
        // (Not blank: a field is read without the spaces around it.)
        && block_field(&receipt.block, "Session").is_some_and(|session| !session.is_empty())
        && block_field(&receipt.block, "User Input") == Some("Approve")
}

/// Why a change during Construction is the person's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Authority {
    Receipt,
    Asked,
}

/// `constructionPolicyChangeAuthority`.
fn authority(wf: &Workflow, state: &str, field: &str, value: &str) -> Option<Authority> {
    if receipt_authorizes(wf, state, field, value) {
        return Some(Authority::Receipt);
    }
    // `personAskedSinceGate`. (Upstream asks it of an attended run only; the
    // one caller has refused an unattended run without a receipt before any
    // answer of this is used.)
    presence::person_spoke_since_gate(wf, false, true, true).then_some(Authority::Asked)
}

/// `requireHumanConstructionPolicyChange`: during Construction a change is
/// the person's. The words of the turn that asked, for the record.
fn require_person(wf: &Workflow, state: &str, field: &str, value: &str) -> Result<Option<String>> {
    if get_field(state, "Lifecycle Phase")
        .map(to_lower_case)
        .as_deref()
        != Some("construction")
    {
        return Ok(None);
    }
    // An unattended run never changes it on its own, whatever switch lowers
    // the presence check: only a choice the person recorded earlier counts.
    if env::var("UNATTENDED").as_deref() == Some("1")
        && authority(wf, state, field, value) != Some(Authority::Receipt)
    {
        bail!(
            "An unattended run does not change {field}: it changes when the person asks for it. {POLICY_RECOVERY}"
        );
    }
    if presence::human_presence_guard_disabled(wf) {
        return Ok(None);
    }
    match authority(wf, state, field, value) {
        None => bail!(
            "No message from the person since the last decision asks to change {field}. Change it only when they ask for it, never to clear a refusal or on your own. {POLICY_RECOVERY}"
        ),
        Some(Authority::Asked) => {
            Ok(presence::latest_person_turn_words(wf).filter(|words| !words.is_empty()))
        }
        Some(Authority::Receipt) => Ok(None),
    }
}

/// A change that was applied: the row (`emitConstructionPolicySet`: the
/// value it found, and the iteration and checkpoints it leaves in force,
/// which the attempt floor reads), then the state.
fn apply(
    wf: &Workflow,
    held: &WorkspaceLock,
    content: &str,
    updated: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    if updated != content {
        let words = require_person(wf, content, field, value)?;
        let current = |state: &str, name: &str| {
            get_field(state, name)
                .map(trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("unset")
                .to_owned()
        };
        let mut fields = vec![
            ("Field", field.to_owned()),
            ("Value", value.to_owned()),
            ("Previous Value", current(content, field)),
            (
                "Construction Iteration",
                current(updated, "Construction Iteration"),
            ),
            (
                "Construction Checkpoints",
                current(updated, "Construction Checkpoints"),
            ),
        ];
        if let Some(words) = words {
            fields.push((PERSONS_WORDS_FIELD, words));
        }
        wf.emit_plain(held, "CONSTRUCTION_POLICY_SET", &fields)?;
    }
    wf.write_state(updated)
}

/// `handleSetConstructionIteration`: how the per-unit stages walk the
/// units.
///
/// # Errors
///
/// No value, or one that is none; a team's units leaving `unit-major`;
/// `unit-major` over a swarm; during Construction, a change the person did
/// not ask for.
pub fn set_iteration(wf: &Workflow, args: &[String]) -> Result<Value> {
    let Some(value) = args.first() else {
        bail!(
            "Usage: focus engine state set-construction-iteration <{}>",
            ITERATIONS.join("|")
        );
    };
    if !ITERATIONS.contains(&value.as_str()) {
        bail!(
            "Invalid construction iteration \"{value}\". Valid: {}.",
            ITERATIONS.join(", ")
        );
    }
    let held = wf.lock("setting how Construction runs")?;
    let content = wf.read_state()?;
    if value != "unit-major" && is_team_unit_ownership(&content) {
        bail!(
            "Construction Iteration cannot leave unit-major while Unit Ownership is team. Set unit ownership to solo first."
        );
    }
    if value == "unit-major"
        && get_field(&content, "Construction Checkpoints") == Some("enabled")
        && get_field(&content, "Construction Execution") == Some("swarm")
    {
        bail!("Select Construction Execution: serial before switching to unit-major iteration.");
    }
    let field = "Construction Iteration";
    let updated = set_or_insert_field(&content, RUNTIME_HEADING, field, value)?;
    apply(wf, &held, &content, &updated, field, value)?;
    Ok(json!({
        "updated": true,
        "construction_iteration": value,
        "notice": notice(&content, &updated, field),
    }))
}

/// The two settings `handleSetConstructionPolicy` takes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    Checkpoints,
    Execution,
}

impl Policy {
    const fn field(self) -> &'static str {
        match self {
            Self::Checkpoints => "Construction Checkpoints",
            Self::Execution => "Construction Execution",
        }
    }

    const fn values(self) -> [&'static str; 2] {
        match self {
            Self::Checkpoints => ["enabled", "disabled"],
            Self::Execution => ["serial", "swarm"],
        }
    }
}

/// `handleSetConstructionPolicy`: the checkpoints, or the execution.
///
/// # Errors
///
/// Not exactly one value, or one that is none; an execution chosen without
/// checkpoints, or a swarm over `unit-major`; during Construction, a change
/// the person did not ask for.
pub fn set_policy(wf: &Workflow, policy: Policy, args: &[String]) -> Result<Value> {
    let field = policy.field();
    let [value] = args else {
        bail!("{field} requires exactly one value.");
    };
    let held = wf.lock("setting how Construction runs")?;
    let content = wf.read_state()?;
    // `setConstructionPolicyField`.
    if !policy.values().contains(&value.as_str()) {
        bail!("{field} must be one of: {}.", policy.values().join(", "));
    }
    if policy == Policy::Execution {
        if get_field(&content, "Construction Checkpoints") != Some("enabled") {
            bail!("Enable Construction Checkpoints before selecting Construction Execution.");
        }
        if value == "swarm" && get_field(&content, "Construction Iteration") == Some("unit-major") {
            bail!("Select stage-major iteration before choosing swarm execution.");
        }
    }
    let updated = set_or_insert_field(&content, RUNTIME_HEADING, field, value)?;
    apply(wf, &held, &content, &updated, field, value)?;
    Ok(json!({
        "updated": true,
        "field": field,
        "value": value,
        "notice": notice(&content, &updated, field),
    }))
}

/// A command as it is recorded: its text, and the digest that names it.
struct Command {
    text: String,
    sha256: String,
}

/// `verificationCommandDetails`.
fn command_details(command: &str) -> Result<Command> {
    if re::test(r"[\p{Cf}\p{Zl}\p{Zp} ]", "u", command) {
        bail!(
            "Construction verification command must contain no display-spoofing characters. Put complex checks in a script and record its invocation."
        );
    }
    let canonical = trim(command);
    if canonical.is_empty()
        || canonical.encode_utf16().count() > 1024
        || has_unsafe_single_line_character(canonical)
        || canonical
            .chars()
            .any(|ch| ('\u{80}'..='\u{9f}').contains(&ch))
    {
        bail!(
            "Construction verification command must be nonblank, at most 1024 characters, and contain no control characters. Put multiline checks in a script and record its invocation."
        );
    }
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(Command {
        text: canonical.to_owned(),
        sha256: digest.iter().map(|byte| format!("{byte:02x}")).collect(),
    })
}

/// `readVerificationCommandFile`: the command in a file of the record,
/// reached through no link.
fn command_file(wf: &Workflow, file: &str) -> Result<Command> {
    let Some(root) = wf.record_dir() else {
        bail!("Verification command file requires an active intent record.");
    };
    let anchor = std::fs::canonicalize(&root)
        .map_err(|err| EngineError::new(format!("{}: {err}", root.display())))?;
    // `recordFileTargetOrThrow`.
    let target = links::target_in_chain(&anchor, file)?;
    let bytes = files::read_regular_within(&target, "verification command file", Some(16 * 1024))?;
    command_details(&String::from_utf8_lossy(&bytes))
}

/// `authorizedVerificationCommand`: the state's command, when the latest
/// receipt of this workflow carries its digest and the person's approval.
fn command_is_authorized(wf: &Workflow, state: &str) -> bool {
    let Some(field) = get_field(state, VERIFICATION_COMMAND).filter(|field| !field.is_empty())
    else {
        return false;
    };
    if active_intent_uuid(&wf.layout, None).is_none() {
        return false;
    }
    let Ok(command) = command_details(field) else {
        return false;
    };
    let Some(rows) = workflow_rows(wf) else {
        return false;
    };
    let Some(start) = workflow_start(&rows) else {
        return false;
    };
    let receipts: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| row.event == "VERIFICATION_COMMAND_RECORDED")
        .collect();
    let [receipt] = maximal(&receipts)[..] else {
        return false;
    };
    definitely_before(start, receipt)
        && block_field(&receipt.block, "Checkpoint") == Some(VERIFICATION_COMMAND)
        && block_field(&receipt.block, "Command SHA-256") == Some(command.sha256.as_str())
        && block_field(&receipt.block, "User Input") == Some("Approve")
}

/// `handleSetConstructionVerificationCommand`: the command the person
/// approved, written to the state its receipt authorizes.
///
/// # Errors
///
/// Neither one command nor `--command-file <path>`; a command a line
/// cannot hold; no receipt of the person's approval for it.
pub fn set_verification_command(wf: &Workflow, args: &[String]) -> Result<Value> {
    let from_file = args.len() == 2 && args[0] == "--command-file";
    if !from_file && (args.len() != 1 || args[0] == "--command-file") {
        bail!(
            "Usage: focus engine state set-construction-verification-command --command-file <record-relative path> (or one positional command argument)."
        );
    }
    let command = if from_file {
        command_file(wf, &args[1])?
    } else {
        command_details(&args[0])?
    };
    let _held = wf.lock("setting the verification command")?;
    let content = wf.read_state()?;
    let previous = get_field(&content, VERIFICATION_COMMAND);
    // A field that exists is replaced through a pattern, where a dollar
    // sign has a meaning: each is written so that it stays one.
    let value = if previous.is_none() {
        command.text.clone()
    } else {
        command.text.replace('$', "$$")
    };
    let updated = set_or_insert_field(&content, RUNTIME_HEADING, VERIFICATION_COMMAND, &value)?;
    if !command_is_authorized(wf, &updated) {
        bail!(
            "No current VERIFICATION_COMMAND_RECORDED with matching Command SHA-256 and User Input: Approve authorizes this command. {VERIFICATION_RECOVERY}"
        );
    }
    wf.write_state(&updated)?;
    let mut out = Map::new();
    out.insert("updated".to_owned(), true.into());
    out.insert("command_sha256".to_owned(), command.sha256.into());
    out.insert("command_label".to_owned(), command.text.clone().into());
    // A new command checks the units still to be approved; the ones already
    // approved keep their approval.
    if previous.is_some_and(|previous| !trim(previous).is_empty() && previous != command.text) {
        out.insert(
            "notice".to_owned(),
            format!("Using `{}` from here on.", command.text).into(),
        );
    }
    Ok(Value::Object(out))
}

/// `handleSetUnitGateRhythm`: when a team's units stop for approval.
///
/// # Errors
///
/// No rhythm, or one that is none; units that are not a team's; and, by
/// name, units that are.
pub fn set_unit_gate_rhythm(wf: &Workflow, args: &[String]) -> Result<Value> {
    let Some(value) = args.first() else {
        bail!(
            "Usage: focus engine state set-unit-gate-rhythm <{}>",
            RHYTHMS.join("|")
        );
    };
    if !RHYTHMS.contains(&value.as_str()) {
        bail!(
            "Invalid unit gate rhythm \"{value}\". Valid: {}.",
            RHYTHMS.join(", ")
        );
    }
    let _held = wf.lock("setting the units' gate rhythm")?;
    let content = wf.read_state()?;
    if !is_team_unit_ownership(&content) {
        bail!(
            "Unit gate rhythm requires Unit Ownership: team. Run focus engine state set-unit-ownership team first."
        );
    }
    bail!(
        "Unit Ownership is team, and the gate rhythm of a team's units is frozen by what its units have done since the workflow started or was jumped: Focus does not run a team's units."
    )
}

/// `handleSyncUnitScopeStage`: a claimed unit's checkout brought to the
/// stage it works on.
///
/// # Errors
///
/// No stage or no unit; units that are not a team's (no claim is live);
/// and, by name, units that are.
pub fn sync_unit_scope_stage(
    wf: &Workflow,
    slug: Option<&str>,
    unit: Option<&str>,
) -> Result<Value> {
    if slug.is_none_or(str::is_empty) || unit.is_none_or(str::is_empty) {
        bail!("Usage: focus engine state sync-unit-scope-stage <stage> --unit <unit>");
    }
    let _held = wf.lock("syncing a unit's stage")?;
    let content = wf.read_state()?;
    // `validateLiveUnitScope`: without a team there is no claim.
    if !is_team_unit_ownership(&content) {
        bail!("Scoped Unit stage sync requires a live Unit claim.");
    }
    bail!(
        "Unit Ownership is team, and a unit's stage is synced from the scope stamp of the checkout that claimed it, checked against the claim registry: Focus does not run a team's units."
    )
}

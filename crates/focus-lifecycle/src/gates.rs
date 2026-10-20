//! The gate verbs: opening a stage's approval gate, recording the person's
//! approval or their request for changes, and re-entering the gate after a
//! revision.
//!
//! Human judgement ends at the gate reply; everything after it is
//! bookkeeping. `gate-start` marks the stage awaiting approval and records
//! what the person was asked; `approve` records the decision, closes the
//! stage and moves on as `advance` or `complete-workflow` would, under the
//! same lock; `reject` records the request with the person's feedback and
//! marks the stage revising; `revise` re-enters the gate.
//!
//! What proves a person is there is the human-turn hook's record (M4): the
//! `HUMAN_TURN` rows are read here as upstream reads them, and the words
//! the hook keeps beside them are refused by name when on disk. With the
//! presence guard off (`FOCUS_SKIP_HUMAN_PRESENCE_GUARD=1`) no reply is
//! asked for, as upstream asks for none.
//!
//! A refusal "for the agent" (the question is still open, or the person
//! decided otherwise) carries its guidance on the error, so the router
//! can hand it back to the agent rather than the person.
//!
//! Mirrors `handleGateStart`, `handleApprove`, `verifyApprovalDecision`,
//! `handleReject` and `handleRevise` in `aidlc-state.ts`.

use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_graph::runtime::Stage;
use focus_intent::plan::next_in_scope_stage;
use focus_js::EngineError;
use focus_js::text::{parse_int, trim};
use focus_state::fields::{get_field, set_field};
use focus_state::progress::{CheckboxState, count_checkboxes, parse_checkboxes, set_checkbox};
use serde_json::{Map, Value, json};

use crate::admission::{self, Action};
use crate::gate;
use crate::presence;
use crate::reply::{self, DecisionKind};
use crate::stage;
use crate::transitions;
use crate::validity;
use crate::workflow::Workflow;

/// Why a gate verb stopped: a refusal in the engine's words, or one for
/// the agent, with its guidance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Error(EngineError),
    /// `DecisionNotMadeError`: the question is still open
    /// (`question-open`), or the person made another choice
    /// (`person-decided`).
    ForAgent {
        message: String,
        person_decided: bool,
    },
}

impl Refused {
    /// The sentence, either way.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Error(err) => err.message(),
            Self::ForAgent { message, .. } => message,
        }
    }

    /// `agent_guidance` on the error JSON.
    #[must_use]
    pub fn agent_guidance(&self) -> Option<&'static str> {
        match self {
            Self::Error(_) => None,
            Self::ForAgent { person_decided, .. } => Some(if *person_decided {
                "person-decided"
            } else {
                "question-open"
            }),
        }
    }
}

impl From<EngineError> for Refused {
    fn from(err: EngineError) -> Self {
        Self::Error(err)
    }
}

type Outcome = std::result::Result<Value, Refused>;

/// `return Err(Refused::Error(EngineError::new(format!(...))))`.
macro_rules! refuse {
    ($($arg:tt)*) => {
        return Err(Refused::Error(EngineError::new(format!($($arg)*))))
    };
}

fn for_agent(message: String, person_decided: bool) -> Refused {
    Refused::ForAgent {
        message,
        person_decided,
    }
}

fn slug_state(content: &str, slug: &str) -> Option<CheckboxState> {
    parse_checkboxes(content)
        .into_iter()
        .find(|line| line.slug == slug)
        .map(|line| line.state)
}

fn validate_slug_in_state(
    content: &str,
    slug: &str,
    allowed: &[CheckboxState],
) -> Result<(), Refused> {
    let Some(actual) = slug_state(content, slug) else {
        return Err(EngineError::new(format!("Stage not found in state file: {slug}")).into());
    };
    if !allowed.contains(&actual) {
        return Err(EngineError::new(format!(
            "Stage {slug} is in state '{}' but command requires one of: {}",
            actual.name(),
            allowed
                .iter()
                .map(|state| state.name())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .into());
    }
    Ok(())
}

/// `teamGateContext`, as far as a serial stage goes: `--unit` needs
/// `Unit Ownership: team`, and a team gate on a stage that is not a
/// per-unit Construction stage does not apply (with `--unit`, an error;
/// without, the ordinary gate). A team gate on a per-unit Construction
/// stage is the units' (M4).
fn refuse_team_gate(state: &str, stage: &Stage, unit: Option<&str>) -> Result<(), Refused> {
    let team = get_field(state, "Unit Ownership").map(trim) == Some("team");
    if !team {
        if unit.is_some() {
            refuse!("--unit gate decisions require Unit Ownership: team.");
        }
        return Ok(());
    }
    if stage.phase != "construction" || !stage::is_per_unit(stage) {
        if unit.is_some() {
            refuse!(
                "Stage \"{}\" is not a per-unit Construction stage; team unit gates do not apply.",
                stage.slug
            );
        }
        return Ok(());
    }
    refuse!(
        "Unit Ownership is team and \"{}\" runs per unit of work, so this gate is a unit's gate, which needs the unit DAG and the units' lifecycle: Focus does not run units yet.",
        stage.slug
    )
}

/// A record with an active directive marker: the orchestrator's (M5), and
/// what a gate reads off it for a guard-recovery answer.
fn refuse_directive_marker(wf: &Workflow) -> Result<(), Refused> {
    let marker = wf
        .selection
        .engine_dir(&wf.layout)
        .join("active-directive.json");
    if marker.exists() {
        refuse!(
            "{} is the orchestrator's directive marker, which a gate decision reads for a guard-recovery answer: Focus does not read it yet.",
            marker.display()
        );
    }
    Ok(())
}

/// `revisionCountOf`.
fn revision_count_of(content: &str) -> f64 {
    let parsed = get_field(content, "Revision Count")
        .filter(|raw| !raw.is_empty())
        .map_or(Some(0.0), parse_int);
    parsed.filter(|n| n.is_finite()).unwrap_or(0.0)
}

/// What `gate-start` was asked.
#[derive(Debug, Clone, Default)]
pub struct GateStart<'a> {
    pub artifacts: Option<&'a str>,
    pub recovered: bool,
    pub person_approves: bool,
    pub override_blocking_sensors: bool,
    pub user_input: Option<&'a str>,
    pub approves_together: Option<&'a str>,
    pub unit: Option<&'a str>,
}

/// `handleGateStart`.
///
/// # Errors
///
/// Anything upstream refuses, in its words; what Focus refuses by name.
pub fn gate_start(wf: &Workflow, slug: &str, ask: &GateStart<'_>, operation: &str) -> Outcome {
    let preflight = wf.read_state()?;
    let stage = wf.stage(slug)?;
    refuse_team_gate(&preflight, stage, ask.unit)?;
    validate_slug_in_state(
        &preflight,
        slug,
        &[CheckboxState::InProgress, CheckboxState::AwaitingApproval],
    )?;
    // The person's own approval over an unfinished review is the reviewer's
    // business (M5); it is read as upstream reads it and changes nothing here.
    let _person_approves = ask.recovered
        && ask.person_approves
        && (presence::human_presence_guard_disabled(wf) || presence::human_replied_since_gate(wf));
    admission::admit(
        wf,
        &preflight,
        stage,
        Action::PresentApprovalGate,
        operation,
        false,
    )?;
    gate::refuse_gate_sensors(wf, stage, &preflight, ask.artifacts, true)?;

    let held = wf.lock("opening a gate")?;
    gate::withdraw_protected_questions(wf, &held)?;
    let mut content = wf.read_state()?;
    validate_slug_in_state(
        &content,
        slug,
        &[CheckboxState::InProgress, CheckboxState::AwaitingApproval],
    )?;
    let already_awaiting = slug_state(&content, slug) == Some(CheckboxState::AwaitingApproval);
    admission::admit(
        wf,
        &content,
        stage,
        Action::PresentApprovalGate,
        operation,
        false,
    )?;
    if already_awaiting {
        return Ok(json!({
            "slug": slug,
            "new_state": "awaiting-approval",
            "already_awaiting_approval": true,
            "revalidated": true,
        }));
    }
    let together = gate::approves_together_flag(ask.approves_together, &content, slug)?;
    content = set_checkbox(&content, slug, CheckboxState::AwaitingApproval);
    let timestamp = iso_timestamp(SystemTime::now());
    content = set_field(&content, "Last Updated", &timestamp);
    let mut fields = vec![("Stage", slug.to_owned())];
    fields.extend(gate::approves_together_fields(&together));
    fields.extend(gate::gate_ask_fields(wf, stage, &content, &together));
    // An empty list is no list, as upstream's falsy test reads it.
    if let Some(artifacts) = ask.artifacts.filter(|list| !list.is_empty()) {
        fields.push(("Artifacts", artifacts.to_owned()));
    }
    if ask.recovered {
        fields.push(("Recovered", "true".to_owned()));
    }
    wf.emit(&held, "STAGE_AWAITING_APPROVAL", &fields)?;
    wf.write_state(&content)?;
    let mut out = Map::new();
    out.insert("slug".to_owned(), json!(slug));
    out.insert("new_state".to_owned(), json!("awaiting-approval"));
    out.insert("timestamp".to_owned(), json!(timestamp));
    if !together.is_empty() {
        out.insert("approves_together".to_owned(), json!(together));
    }
    Ok(Value::Object(out))
}

/// `handleRevise`.
///
/// # Errors
///
/// As [`gate_start`].
pub fn revise(wf: &Workflow, slug: &str, ask: &GateStart<'_>, operation: &str) -> Outcome {
    let preflight = wf.read_state()?;
    let stage = wf.stage(slug)?;
    refuse_team_gate(&preflight, stage, ask.unit)?;
    validate_slug_in_state(&preflight, slug, &[CheckboxState::Revising])?;
    admission::admit(
        wf,
        &preflight,
        stage,
        Action::PresentApprovalGate,
        operation,
        false,
    )?;
    gate::refuse_gate_sensors(wf, stage, &preflight, None, true)?;

    let held = wf.lock("re-entering a gate")?;
    let mut content = wf.read_state()?;
    validate_slug_in_state(&content, slug, &[CheckboxState::Revising])?;
    admission::admit(
        wf,
        &content,
        stage,
        Action::PresentApprovalGate,
        operation,
        false,
    )?;
    let together = gate::approves_together_flag(ask.approves_together, &content, slug)?;
    content = set_checkbox(&content, slug, CheckboxState::AwaitingApproval);
    let timestamp = iso_timestamp(SystemTime::now());
    content = set_field(&content, "Last Updated", &timestamp);
    let mut fields = vec![
        ("Stage", slug.to_owned()),
        ("Details", "Re-entering gate after revision".to_owned()),
    ];
    fields.extend(gate::approves_together_fields(&together));
    fields.extend(gate::gate_ask_fields(wf, stage, &content, &together));
    wf.emit(&held, "STAGE_AWAITING_APPROVAL", &fields)?;
    wf.write_state(&content)?;
    let mut out = Map::new();
    out.insert("slug".to_owned(), json!(slug));
    out.insert("new_state".to_owned(), json!("awaiting-approval"));
    out.insert("timestamp".to_owned(), json!(timestamp));
    if !together.is_empty() {
        out.insert("approves_together".to_owned(), json!(together));
    }
    Ok(Value::Object(out))
}

/// What `verifyApprovalDecision` settled.
struct Decision {
    /// The approval as recorded (`User Input`); none when nothing was
    /// passed and no reply is asked for.
    approval_input: Option<String>,
    /// The earlier stage whose approval covers this one.
    together: Option<String>,
}

/// `verifyApprovalDecision`: the person's approval, read as syntax.
fn verify_approval_decision(
    wf: &Workflow,
    content: &str,
    stage: &Stage,
    user_input: Option<&str>,
) -> Result<Decision, Refused> {
    let slug = &stage.slug;
    let presence_off = presence::human_presence_guard_disabled(wf);
    let mut approval_input: Option<String> = user_input.map(|input| trim(input).to_owned());
    let together = if presence_off {
        None
    } else {
        gate::approved_together_cover(wf, slug)
    };
    if !presence_off
        && let Some(marker) = reply::self_attributed_decision_marker(
            approval_input.as_deref(),
            DecisionKind::Approval,
        )
    {
        return Err(for_agent(
            format!(
                "Cannot approve \"{slug}\" because --user-input says the choice came from the assistant ({}: \"{}\"). This check looks for explicit assistant provenance; it does not prove who wrote the reply. Approval is the human's decision: end the turn and let them answer, and if something is blocking completion, explain that blocker at the gate in project terms instead of recording their decision.",
                marker.category, marker.phrase
            ),
            false,
        ));
    }
    if !presence_off {
        let revision_count = revision_count_of(content);
        if let Some(input) = approval_input.as_deref().filter(|input| !input.is_empty())
            && reply::is_non_answer(Some(input))
        {
            return Err(for_agent(
                format!(
                    "Cannot approve \"{slug}\" because the reply {} is cancellation boilerplate, not a decision. Re-present the original held gate with every offered choice and wait for the human to choose one.",
                    reply::format_received_reply(Some(input))
                ),
                false,
            ));
        }
        // The person's exact pick is in the gate words (M4), refused above
        // when on disk; with none, the input names the approval.
        approval_input = Some(
            reply::stage_gate_approval(approval_input.as_deref(), revision_count >= 3.0).to_owned(),
        );
    }
    if !presence_off && together.is_none() {
        let replied_since_decision = presence::human_replied_since_gate(wf);
        let replied_since_shown = presence::person_replied_since_presentation(wf, slug, true);
        if replied_since_decision && replied_since_shown == Some(false) {
            return Err(for_agent(
                format!(
                    "Cannot approve \"{slug}\" because no new human reply has been received for this approval question: the person's last reply came before the gate was shown, so it does not answer it. Show the gate and end your turn; their next reply answers it."
                ),
                false,
            ));
        }
        if !replied_since_decision {
            return Err(for_agent(
                format!(
                    "Cannot approve \"{slug}\" because no new human reply has been received for this approval question. Wait for the human to type their choice, then retry the approval.{}{}",
                    presence::command_turn_hint(wf),
                    presence::unattended_human_presence_hint(wf)
                ),
                false,
            ));
        }
    }
    if !presence_off && user_input.map(trim).is_none_or(str::is_empty) {
        return Err(for_agent(
            format!(
                "Cannot approve \"{slug}\" because no choice was passed. The person replied after the question was shown: report the choice their reply makes with --user-input (for example \"Approve\"), or ask them if their reply is unclear."
            ),
            true,
        ));
    }
    Ok(Decision {
        approval_input: approval_input.filter(|input| !input.is_empty()),
        together,
    })
}

/// `revisionBackstopDisabled`.
fn revision_backstop_disabled(wf: &Workflow) -> bool {
    focus_workspace::settings::Places::of(&wf.layout.project)
        .flag(
            &focus_workspace::env::full_name("SKIP_REVISION_BACKSTOP"),
            &|name| std::env::var(name).ok(),
        )
        .ok()
        .flatten()
        .as_deref()
        == Some("1")
}

/// `guardPolicyAcceptsChanges`: the policy is not strict; unreadable reads
/// as strict.
fn guard_policy_accepts_changes(wf: &Workflow, state: &str) -> bool {
    focus_guards::workflow::Guards::read(&wf.layout, &wf.selector(), &wf.invoker)
        .map(|guards| guards.with_state(state).accepts_changes())
        .unwrap_or(false)
}

/// `handleApprove`.
///
/// # Errors
///
/// As [`gate_start`]; a refusal for the agent carries its guidance.
pub fn approve(
    wf: &Workflow,
    slug: &str,
    user_input: Option<&str>,
    unit: Option<&str>,
    operation: &str,
) -> Outcome {
    let preflight = wf.read_state()?;
    let stage = wf.stage(slug)?;
    refuse_team_gate(&preflight, stage, unit)?;
    validate_slug_in_state(&preflight, slug, &[CheckboxState::AwaitingApproval])?;
    admission::refuse_construction(stage)?;
    gate::refuse_gate_words(wf)?;
    verify_approval_decision(wf, &preflight, stage, user_input)?;
    admission::verify_stage_artifacts(wf, stage, Action::Complete)?;
    admission::verify_summary_confirmation(wf, stage, &preflight)?;
    let preflight_backstop = !revision_backstop_disabled(wf)
        && !guard_policy_accepts_changes(wf, &preflight)
        && gate::unrecorded_revision_since_gate_open(wf, stage);
    if preflight_backstop {
        gate::refuse_gate_sensors(wf, stage, &preflight, None, true)?;
    }
    transitions::refuse_usage_rollup(wf, Some(slug))?;

    let held = wf.lock("approving a gate")?;
    let mut content = wf.read_state()?;
    validate_slug_in_state(&content, slug, &[CheckboxState::AwaitingApproval])?;
    let decision = verify_approval_decision(wf, &content, stage, user_input)?;
    admission::verify_stage_artifacts(wf, stage, Action::Complete)?;
    admission::verify_summary_confirmation(wf, stage, &content)?;
    let listed_together: Vec<String> = if decision.together.is_some() {
        Vec::new()
    } else {
        match gate::approves_together_stages(&content, slug) {
            None => Vec::new(),
            Some(waiting) => {
                let shown = gate::open_gate_approves_together(wf, slug);
                if shown.first().map(String::as_str) == Some(slug) {
                    shown.into_iter().filter(|s| waiting.contains(s)).collect()
                } else {
                    Vec::new()
                }
            }
        }
    };
    let backstop_now = !revision_backstop_disabled(wf)
        && !guard_policy_accepts_changes(wf, &content)
        && gate::unrecorded_revision_since_gate_open(wf, stage);
    if backstop_now && !preflight_backstop {
        refuse!(
            "Refusing to approve \"{slug}\": revision evidence changed during the gate preflight. Retry the approval so revised bytes can be checked before re-entry."
        );
    }
    if backstop_now {
        let revision_count = revision_count_of(&content) + 1.0;
        let revision_count = focus_js::num::to_string(revision_count);
        content = set_field(&content, "Revision Count", &revision_count);
        content = set_checkbox(&content, slug, CheckboxState::Revising);
        content = set_field(&content, "Last Updated", &iso_timestamp(SystemTime::now()));
        wf.emit(
            &held,
            "GATE_REJECTED",
            &[
                ("Stage", slug.to_owned()),
                ("Recovered", "true".to_owned()),
                (
                    "Details",
                    "Backfilled by the revision backstop: the artifact was revised at an open gate with no reject recorded".to_owned(),
                ),
            ],
        )?;
        wf.emit(
            &held,
            "STAGE_REVISING",
            &[
                ("Stage", slug.to_owned()),
                ("Revision count", revision_count.clone()),
                ("Recovered", "true".to_owned()),
            ],
        )?;
        wf.write_state(&content)?;
        admission::verify_summary_confirmation(wf, stage, &content)?;
        admission::verify_pipeline_link(wf, stage)?;
        if !admission::reviewer_gate_guard_disabled() {
            admission::verify_reviewer(wf, stage, &content, true)?;
        }
        let mut fields = vec![
            ("Stage", slug.to_owned()),
            ("Recovered", "true".to_owned()),
            (
                "Details",
                "Re-entering gate after backfilled revision".to_owned(),
            ),
        ];
        fields.extend(gate::gate_ask_fields(wf, stage, &content, &[]));
        wf.emit(&held, "STAGE_AWAITING_APPROVAL", &fields)?;
        content = set_checkbox(&content, slug, CheckboxState::AwaitingApproval);
        content = set_field(&content, "Last Updated", &iso_timestamp(SystemTime::now()));
        wf.write_state(&content)?;
    }
    admission::admit(wf, &content, stage, Action::Complete, operation, true)?;
    gate::refuse_review_findings(wf, stage)?;
    let (_, definition) = wf.scope_definition(
        &content,
        "Refusing to advance after approve \u{2014} fix the state file first.",
    )?;
    let timestamp = iso_timestamp(SystemTime::now());
    content = set_checkbox(&content, slug, CheckboxState::Completed);
    content = set_field(&content, "Last Updated", &timestamp);
    let completed_count = count_checkboxes(&content, CheckboxState::Completed);
    content = set_field(&content, "Completed", &completed_count.to_string());
    content = set_field(&content, "Last Completed Stage", slug);
    let validation = validity::audit_fields(wf, stage, &content);
    let warning = validation
        .iter()
        .find(|(name, _)| *name == validity::VALIDATION_WARNING_FIELD)
        .map(|(_, value)| value.clone());
    let mut gate_fields = vec![("Stage", slug.to_owned())];
    if let Some(input) = &decision.approval_input {
        gate_fields.push(("User Input", input.clone()));
    }
    match &decision.together {
        Some(first) => gate_fields.push(gate::approved_together_with_field(first)),
        None => gate_fields.extend(gate::approves_together_fields(&listed_together)),
    }
    gate_fields.push(gate::brief_digest_field(wf, stage));
    if let Some(accepted) = gate::decisions_accepted_open_field(wf, slug) {
        gate_fields.push(accepted);
    }
    wf.emit(&held, "GATE_APPROVED", &gate_fields)?;
    let mut completed_fields = vec![("Stage", slug.to_owned())];
    completed_fields.extend(
        validation
            .iter()
            .map(|(name, value)| (*name, value.clone())),
    );
    completed_fields.push((
        "Details",
        format!("Stage {} approved by gate", stage::name(stage)),
    ));
    wf.emit(&held, "STAGE_COMPLETED", &completed_fields)?;
    wf.write_state(&content)?;
    let next = next_in_scope_stage(slug, definition, &wf.install.graph, Some(&content)).is_some();
    let answer = if next {
        transitions::advance_locked(wf, &held, slug, None, operation, warning.as_deref())?
    } else {
        transitions::complete_workflow_locked(wf, &held, slug, None, operation, warning.as_deref())?
    };
    Ok(answer)
}

/// What `reject` was asked.
#[derive(Debug, Clone, Default)]
pub struct Reject<'a> {
    pub feedback: Option<&'a str>,
    pub reject_findings: Vec<String>,
    pub reopen_findings: Vec<String>,
    pub unit: Option<&'a str>,
}

/// `handleReject`.
///
/// # Errors
///
/// As [`approve`].
pub fn reject(wf: &Workflow, slug: &str, ask: &Reject<'_>, _operation: &str) -> Outcome {
    let feedback = ask.feedback.map(trim).filter(|text| !text.is_empty());
    let _held = wf.lock("recording a rejection")?;
    let mut content = wf.read_state()?;
    let stage = wf.stage(slug)?;
    refuse_team_gate(&content, stage, ask.unit)?;
    validate_slug_in_state(
        &content,
        slug,
        &[CheckboxState::AwaitingApproval, CheckboxState::InProgress],
    )?;
    admission::refuse_construction(stage)?;
    refuse_directive_marker(wf)?;
    gate::refuse_gate_words(wf)?;
    let Some(feedback) = feedback else {
        return Err(for_agent(
            format!(
                "Refusing to reject \"{slug}\": Request Changes requires nonblank revision feedback in --feedback (or --reason through `focus engine orchestrate report`). Ask \"What should change?\", end the turn, and pass their answer."
            ),
            false,
        ));
    };
    if reply::is_non_answer(Some(feedback)) {
        return Err(for_agent(
            format!(
                "Refusing to reject \"{slug}\": revision feedback {} is cancellation boilerplate. Re-present the original held gate with every offered choice and wait for the human to choose one.",
                reply::format_received_reply(Some(feedback))
            ),
            false,
        ));
    }
    let presence_off = presence::human_presence_guard_disabled(wf);
    let replied_to_gate =
        presence::person_replied_since_presentation(wf, slug, false) != Some(false);
    if !presence_off && (!presence::human_replied_since_gate(wf) || !replied_to_gate) {
        return Err(for_agent(
            format!(
                "Cannot request changes for \"{slug}\" because no new human reply has been received for this approval question. Wait for the human to type Request Changes and their feedback, then retry.{}{}",
                presence::command_turn_hint(wf),
                presence::unattended_human_presence_hint(wf)
            ),
            false,
        ));
    }
    if !presence_off
        && let Some(marker) =
            reply::self_attributed_decision_marker(Some(feedback), DecisionKind::Rejection)
    {
        return Err(for_agent(
            format!(
                "Cannot request changes for \"{slug}\" because --feedback says it was written by the assistant ({}: \"{}\"). Requesting changes is the human's decision. Explain why another review is needed at the approval question and wait for the human to choose.",
                marker.category, marker.phrase
            ),
            false,
        ));
    }
    if !ask.reject_findings.is_empty() || !ask.reopen_findings.is_empty() {
        if stage::reviewer(stage).is_none() {
            refuse!(
                "Cannot reject review findings for stage \"{slug}\": the stage has no reviewer."
            );
        }
        refuse!(
            "--reject-finding and --reopen-finding record the person's decision on a review's findings, which Focus does not read yet."
        );
    }
    gate::refuse_review_findings(wf, stage)?;
    let revision_count = revision_count_of(&content) + 1.0;
    let revision_count = focus_js::num::to_string(revision_count);
    content = set_field(&content, "Revision Count", &revision_count);
    content = set_checkbox(&content, slug, CheckboxState::Revising);
    let timestamp = iso_timestamp(SystemTime::now());
    content = set_field(&content, "Last Updated", &timestamp);
    wf.emit(
        &_held,
        "GATE_REJECTED",
        &[
            ("Stage", slug.to_owned()),
            ("Feedback", feedback.to_owned()),
        ],
    )?;
    wf.emit(
        &_held,
        "STAGE_REVISING",
        &[
            ("Stage", slug.to_owned()),
            ("Revision count", revision_count.clone()),
            ("Feedback", feedback.to_owned()),
        ],
    )?;
    wf.write_state(&content)?;
    Ok(json!({
        "slug": slug,
        "new_state": "revising",
        "revision_count": focus_js::num::from_str(&revision_count),
        "timestamp": timestamp,
    }))
}

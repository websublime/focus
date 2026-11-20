//! The guard chain a lifecycle action passes before it changes anything.
//!
//! Listed once, so the enforcing transition and the router that previews
//! it (M5) see the same refusal: the workflow is not archived; the stage's
//! declared artifacts exist, and a code-producing stage left source work;
//! the stage's summary confirmation, its reviewer's receipt and its
//! pipeline's handoffs are recorded when owed; and its Construction
//! checkpoints are approved.
//!
//! The evidence checks read `aidlc-log` records. The summary confirmation
//! is weighed ([`crate::summary_evidence`]): one that is not there, or no
//! longer the current one, is a guard's refusal, and a change the Guard
//! Policy keeps is recorded and said once, on a line of its own before the
//! command's answer ([`Workflow::say_change_notices`]). A pipeline's
//! handoffs are weighed ([`crate::pipeline_evidence`]): each link of the
//! chain with no receipt of this attempt that counts is owed, and the
//! refusal is a guard's. And the reviewer's receipt is weighed for a stage
//! reviewed as one ([`crate::review_receipts`]): a terminal verdict of the
//! current attempt over the outputs as they are lets the stage through; a
//! request with no verdict, a verdict over other bytes, a recovery pending
//! or spent and a record that cannot be verified are each a guard's
//! refusal; and the person's own approval goes over a review that did not
//! finish, unless their team keeps the policy strict ([`PersonApproval`]).
//! A stage that runs for each unit, or that writes source code, owes a
//! review weighed against the units' rows and claims: both are refused by
//! name until units arrive, as a Construction stage is one step earlier
//! (its checkpoints, units and swarms are M4). What an approval records
//! of a review it went over comes with the findings a gate records
//! (slice 3e-v).
//!
//! # What no check tells apart
//!
//! In the reviewer's precondition: two readings it hands over, which the reader
//! would take the same: the review class given to the receipts (they
//! resolve it as the precondition does), and the receipts given to the
//! guard's attempt (it reads them with the same class). The paths of a
//! change after a review: the receipts name them only where the source
//! changed, which a stage that writes source code owes and is refused by
//! name; for an output they are never known. And the person's
//! approval carried by `approve`: with it the approval ends in the
//! findings' refusal by name, without it in the reviewer's, where upstream
//! approves; which of the two Focus says is a test of the command, since
//! no comparison holds either (`what_a_gate_cannot_weigh_yet_is_refused_by_name`).
//!
//! Three things in the summary's admission, each said here because a
//! mutant of it survives the differential:
//!
//! - **The state handed to the evidence.** Every caller hands over the
//!   state it has just read or just written, so an evidence that read the
//!   file again would read the same text.
//! - **The unit of a change the policy kept.** A transition asks for the
//!   stage's evidence, and a stage that runs for each unit and owes a
//!   confirmation is refused by name one reader earlier
//!   ([`crate::summary_evidence`]): the unit is always none until units
//!   arrive.
//! - **The check repeated under the lock at an approval.** Upstream
//!   weighs the summary before the lock, again under it, and a third time
//!   in the chain every completion passes; when the revision backstop
//!   fires, once more after its rows. Within one process the first
//!   answer is every later one: the repeats are for a second process
//!   that changes the record between the check and the lock. What the
//!   first one decides alone is the order of the refusals before the
//!   lock, which is compared (`gates/summary_unconfirmed_ahead_of_*`).
//!
//! The rows of a change the policy keeps are written under the workspace
//! lock. A check that runs before its transition takes the lock (upstream
//! checks a gate's opening and its approval once outside the lock and once
//! under it) takes the lock for those rows and lets go of it; under the
//! lock the transition hands its own over, because the lock is not taken
//! twice by one process.
//!
//! A guard that refuses carries its code and the invariant it holds. The
//! error's message is the sentence; the refusal, evaluated on the attempt
//! as the guard sees it, rides on the error for the tool's entry point to
//! print with its recovery question and to count in the series of the
//! stage ([`crate::guard_series`]).
//!
//! Mirrors `admitStageAction`, `verifyGateOpeningGuards`,
//! `verifyStageArtifacts`, `verifySummaryConfirmationPrecondition`,
//! `verifyReviewerPrecondition`, `verifyPipelineLinkPrecondition`,
//! `verifyConstructionCheckpointPrecondition`, `artifactGuardDisabled`
//! and `refuseStateGuard` in `aidlc-state.ts`, and `summaryConfirmationOwed`,
//! `resolveReviewClass`, `resolveCeremony` and `reviewerGateGuardDisabled`
//! in `aidlc-lib.ts`.

use focus_graph::runtime::Stage;
use focus_guards::workflow::Guards;
use focus_js::re;
use focus_js::text::trim;
use focus_js::{EngineError, Result, bail};
use focus_state::fields::get_field;
use focus_state::progress::{CheckboxState, parse_checkboxes};
use focus_workspace::lock::WorkspaceLock;
use focus_workspace::settings::Places;
use focus_workspace::{Layout, env};

use crate::artifacts::{produces_artifacts_exist, workspace_has_work};
use crate::change_control::{self, Accepted};
use crate::review_receipts::{self, PendingState, Receipts};
use crate::stage;
use crate::summary_evidence::{self, Evidence};
use crate::workflow::Workflow;

/// What the action is: closing the stage, or opening its approval gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Complete,
    PresentApprovalGate,
}

impl Action {
    /// `reviewerPreconditionPrefix`.
    fn prefix(self, slug: &str) -> String {
        match self {
            Self::Complete => format!("Cannot complete \"{slug}\""),
            Self::PresentApprovalGate => format!("Cannot present \"{slug}\" for approval"),
        }
    }
}

/// `resolveProjectFlag`: the environment, else the settings files.
fn project_flag(layout: &Layout, name: &str) -> Option<String> {
    Places::of(&layout.project)
        .flag(&env::full_name(name), &|variable| {
            std::env::var(variable).ok()
        })
        .ok()
        .flatten()
}

/// `artifactGuardDisabled`.
#[must_use]
pub fn artifact_guard_disabled(layout: &Layout) -> bool {
    project_flag(layout, "SKIP_ARTIFACT_GUARD").as_deref() == Some("1")
}

/// `reviewerGateGuardDisabled`: the environment alone.
#[must_use]
pub fn reviewer_gate_guard_disabled() -> bool {
    env::var("SKIP_REVIEWER_GATE_GUARD").as_deref() == Some("1")
}

/// `assertWorkflowNotArchived`.
///
/// # Errors
///
/// The state's `Status` is `Archived`.
pub fn assert_not_archived(state: &str, operation: &str) -> Result<()> {
    if get_field(state, "Status") == Some("Archived") {
        bail!(
            "Workflow is Archived, so {operation} is refused. Bring it back first with `/focus intent unarchive <name>`."
        );
    }
    Ok(())
}

impl Action {
    /// The action as a refusal names it (`ReviewerPreconditionAction`).
    fn word(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::PresentApprovalGate => "present-approval-gate",
        }
    }
}

/// `refuseStateGuard`: a guard's refusal, with its reason code and the
/// invariant it holds: the sentence is the error's message, and the
/// refusal rides on it for the tool's entry point to print with its
/// recovery question ([`crate::guard_series::printed`]).
fn guard_refusal(
    wf: &Workflow,
    stage: &Stage,
    action: Action,
    code: &str,
    invariant: &str,
    message: &str,
) -> EngineError {
    let state = match wf.read_state() {
        Ok(state) => state,
        Err(error) => return error,
    };
    crate::guard_series::guard_refused(
        wf,
        &state,
        stage,
        crate::guard_series::Refused {
            code,
            blocked_action: action.word(),
            invariant,
            user_message: message,
            unit: None,
        },
        crate::guard_attempt::Options::default(),
    )
}

/// `verifyStageArtifacts`: the declared artifacts exist, and a
/// code-producing stage has source work.
///
/// # Errors
///
/// `REQUIRED_ARTIFACTS_MISSING` or `REQUIRED_SOURCE_WORK_MISSING`.
pub fn verify_stage_artifacts(wf: &Workflow, stage: &Stage, action: Action) -> Result<()> {
    if artifact_guard_disabled(&wf.layout) {
        return Ok(());
    }
    if !produces_artifacts_exist(wf, stage)? {
        return Err(guard_refusal(
            wf,
            stage,
            action,
            "REQUIRED_ARTIFACTS_MISSING",
            "Every applicable required output exists before certification.",
            &format!(
                "{}: none of its declared artifacts exist under the intent's record directory. The stage protocol requires {} to produce output before the gate. Produce the artifacts before completing. (declared: {})",
                action.prefix(&stage.slug),
                stage::name(stage),
                if stage.produces.is_empty() {
                    "none".to_owned()
                } else {
                    stage.produces.join(", ")
                }
            ),
        ));
    }
    if stage::workspace_requires(stage) && !workspace_has_work(wf)? {
        return Err(guard_refusal(
            wf,
            stage,
            action,
            "REQUIRED_SOURCE_WORK_MISSING",
            "A workspace-producing stage contains application source work.",
            &format!(
                "{}: it is a code-producing stage (workspace_requires) but no source work is evident outside the focus/ workspace tree. In a git workspace this means no uncommitted change and no code in the last commit; otherwise no source file exists. Planning docs alone do not satisfy {} - write the code to the workspace (a source change not committed yet counts, as does code in the last commit), or the person can choose Request Changes and say what is missing.",
                action.prefix(&stage.slug),
                stage::name(stage)
            ),
        ));
    }
    Ok(())
}

/// `parseCeremonyStateLine`: `on` or `off`, read off a state line.
fn ceremony_state_value(raw: Option<&str>) -> Option<&'static str> {
    let raw = trim(raw?);
    let found = re::find(r"^(on|off)\b(?:\s*\((.*)\))?\s*$", "i", raw)?;
    match re::group(raw, &found, 1)?.to_lowercase().as_str() {
        "on" => Some("on"),
        _ => Some("off"),
    }
}

/// `resolveCeremony(...).value`: the kill switch `switch`, else the
/// state's line `field`, else the scope's setting for `key`, else on.
#[must_use]
pub fn ceremony_setting(
    wf: &Workflow,
    state: &str,
    key: &str,
    field: &str,
    switch: &str,
) -> &'static str {
    ceremony_setting_of(
        wf,
        get_field(state, "Scope"),
        Some(state),
        key,
        field,
        switch,
    )
}

/// [`ceremony_setting`] for a caller that names the scope itself and may
/// have no state: a stage run by itself reads the scope its own attempt
/// recorded and never the main workflow's settings.
#[must_use]
pub fn ceremony_setting_of(
    wf: &Workflow,
    scope: Option<&str>,
    state: Option<&str>,
    key: &str,
    field: &str,
    switch: &str,
) -> &'static str {
    if project_flag(&wf.layout, switch).as_deref() == Some("1") {
        return "off";
    }
    if let Some(value) = state.and_then(|state| ceremony_state_value(get_field(state, field))) {
        return value;
    }
    let scope = scope
        .map(|scope| trim(scope).to_lowercase())
        .unwrap_or_default();
    match wf.install.definition(&scope) {
        Some(definition) if definition.meta.ceremony_setting(key) == "off" => "off",
        _ => "on",
    }
}

/// The `summary_confirmation` ceremony's setting.
#[must_use]
pub fn summary_confirmation_setting(wf: &Workflow, state: &str) -> &'static str {
    ceremony_setting(
        wf,
        state,
        "summary_confirmation",
        "Summary Confirmation",
        "DISABLE_SUMMARY_CONFIRMATION",
    )
}

/// `summaryConfirmationOwed`: whether the stage owes a summary confirmation.
#[must_use]
pub fn summary_confirmation_owed(wf: &Workflow, stage: &Stage, state: &str) -> bool {
    summary_confirmation_owed_of(wf, stage, Some(state), None)
}

/// [`summary_confirmation_owed`] for a caller that may have no state and
/// may name the scope itself, as a stage run by itself does.
#[must_use]
pub fn summary_confirmation_owed_of(
    wf: &Workflow,
    stage: &Stage,
    state: Option<&str>,
    scope: Option<&str>,
) -> bool {
    if project_flag(&wf.layout, "SKIP_SUMMARY_CONFIRMATION_GUARD").as_deref() == Some("1") {
        return false;
    }
    let scope = scope.or_else(|| get_field(state.unwrap_or_default(), "Scope"));
    if ceremony_setting_of(
        wf,
        scope,
        state,
        "summary_confirmation",
        "Summary Confirmation",
        "DISABLE_SUMMARY_CONFIRMATION",
    ) == "off"
    {
        return false;
    }
    if stage.phase == "initialization" {
        return false;
    }
    if stage.phase == "construction" && state.is_some_and(is_autonomous_mode) {
        return false;
    }
    stage::summary_confirmation(stage).is_some()
}

/// `isAutonomousMode`.
pub fn is_autonomous_mode(state: &str) -> bool {
    get_field(state, "Construction Autonomy Mode").map(trim) == Some("autonomous")
}

/// `observeChangeControl`: a check of the admission read the Guard Policy,
/// because something had changed after an approval. The transition reads
/// it again as the one that changes things (a move of the policy is
/// recorded), records the changes the check let through, and says them
/// once, on a line of their own.
///
/// `held` is the workspace lock when the caller holds it; a check that
/// runs before the transition takes the lock takes it for these rows.
///
/// # Errors
///
/// The policy cannot be read, or a row cannot be written.
fn observe_change_control(
    wf: &Workflow,
    held: Option<&WorkspaceLock>,
    state: &str,
    accepted: &[Accepted<'_>],
) -> Result<()> {
    let taken;
    let held = match held {
        Some(held) => held,
        None => {
            taken = wf.lock("recording what the Guard Policy let through")?;
            &taken
        }
    };
    change_control::governed_guard_policy(wf, held, state)?;
    let notices = change_control::record_accepted_changes(wf, held, accepted)?;
    wf.say_change_notices(&notices);
    Ok(())
}

/// `verifySummaryConfirmationPrecondition`: the stage's outputs descend
/// from a summary confirmation a person gave and that is still the current
/// one, when the stage owes one.
///
/// # Errors
///
/// The confirmation is not there or no longer current: a guard's refusal,
/// with the summary's coverage. Or the evidence cannot be read.
pub fn verify_summary_confirmation(
    wf: &Workflow,
    held: Option<&WorkspaceLock>,
    stage: &Stage,
    state: &str,
) -> Result<()> {
    let evidence = summary_evidence::check(
        wf,
        stage,
        summary_evidence::Asked {
            workflow: None,
            state: Some(state),
            scope: None,
            unit: None,
        },
    )?;
    let (Evidence::Ok {
        change_control_read,
        ..
    }
    | Evidence::Refused {
        change_control_read,
        ..
    }) = &evidence;
    if *change_control_read {
        let accepted: Vec<Accepted<'_>> = match &evidence {
            Evidence::Ok {
                accepted_changes, ..
            } => accepted_changes
                .iter()
                .map(|change| Accepted {
                    checkpoint: "summary-confirmation",
                    stage: &change.stage,
                    unit: change.unit.as_deref(),
                    changed: Some(&change.changed),
                    recorded: &change.recorded,
                    current: &change.current,
                    notice: &change.notice,
                })
                .collect(),
            Evidence::Refused { .. } => Vec::new(),
        };
        observe_change_control(wf, held, state, &accepted)?;
    }
    let Evidence::Refused {
        code,
        message,
        coverage,
        ..
    } = &evidence
    else {
        return Ok(());
    };
    Err(crate::guard_series::guard_refused(
        wf,
        state,
        stage,
        crate::guard_series::Refused {
            code,
            blocked_action: "summary-confirmation",
            invariant: SUMMARY_INVARIANT,
            user_message: message,
            unit: None,
        },
        crate::guard_attempt::Options {
            summary_coverage: Some((*coverage).into()),
            ..Default::default()
        },
    ))
}

/// The invariant the summary's admission holds.
pub const SUMMARY_INVARIANT: &str =
    "Generated outputs descend from a current human-backed summary confirmation.";

/// `resolveReviewClass`: the stage's class, capped by the state's
/// `Review Override` or the scope's `review_cap`.
#[must_use]
pub fn resolve_review_class(wf: &Workflow, stage: &Stage, state: &str) -> &'static str {
    let as_class = |value: Option<&str>| -> Option<&'static str> {
        match value? {
            "none" => Some("none"),
            "advisory" => Some("advisory"),
            "adversarial" => Some("adversarial"),
            _ => None,
        }
    };
    let rank = |class: &str| match class {
        "none" => 0,
        "advisory" => 1,
        _ => 2,
    };
    let Some(declared) = as_class(Some(stage::review_class(stage).unwrap_or("adversarial"))) else {
        return "none";
    };
    let scope = get_field(state, "Scope").unwrap_or("");
    let cap = as_class(get_field(state, "Review Override")).or_else(|| {
        as_class(
            wf.install
                .definition(scope)
                .and_then(|definition| definition.meta.review_cap.as_deref()),
        )
    });
    match cap {
        Some(cap) if rank(cap) < rank(declared) => cap,
        _ => declared,
    }
}

/// The person's own approval, carried through the admission chain
/// (`PersonApproval`). It goes over a review the agent asked for that has
/// no verdict yet, and over a verdict that is the `NOT-READY` no reviewer
/// gave; the chain notes here when it did, so the approval is recorded and
/// said that way.
#[derive(Debug, Default)]
pub struct PersonApproval {
    over_unfinished_review: std::cell::Cell<bool>,
}

impl PersonApproval {
    /// Whether the approval went over a review that did not finish.
    /// (Nothing reads it yet: what an approval records of it comes with
    /// the findings a gate records, slice 3e-v.)
    #[must_use]
    pub fn over_unfinished_review(&self) -> bool {
        self.over_unfinished_review.get()
    }
}

/// What a transition carries through the chain: its hold on the workspace
/// (a check that runs before the transition takes the lock has none), and
/// the person's own approval when the call is one.
#[derive(Clone, Copy, Default)]
pub struct Carried<'a> {
    pub held: Option<&'a WorkspaceLock>,
    pub person: Option<&'a PersonApproval>,
}

impl<'a> Carried<'a> {
    /// Under the caller's lock, with nobody's approval.
    #[must_use]
    pub fn under(held: &'a WorkspaceLock) -> Self {
        Self {
            held: Some(held),
            person: None,
        }
    }
}

/// `personMayApproveOverUnfinishedReview`: off, relaxed and a strict set
/// for this piece of work let the person approve over a review that did not
/// finish; a strict the team keeps in memory does not, and neither does a
/// policy that cannot be read.
fn person_may_approve_over_unfinished_review(wf: &Workflow, state: &str) -> bool {
    Guards::read(&wf.layout, &wf.selector(), &wf.invoker)
        .and_then(|guards| guards.with_state(state).policy(false))
        .is_ok_and(|policy| policy.memory_strict.is_none())
}

/// `approvableUnfinishedReview`, for the stage: a request with no verdict
/// yet that the person may approve over, the recovery review they asked for
/// again included. (Upstream asks here too whether the result could not be
/// verified, which still finishes first. The one caller has refused that
/// before it asks this, so the test is not repeated.)
fn approvable_unfinished_review(
    wf: &Workflow,
    state: &str,
    receipts: &Receipts,
    person: Option<&PersonApproval>,
) -> bool {
    let Some(person) = person else {
        return false;
    };
    if receipts.stage_pending.is_none() {
        return false;
    }
    if !receipts.awaiting_verdict.has("") || !person_may_approve_over_unfinished_review(wf, state) {
        return false;
    }
    person.over_unfinished_review.set(true);
    true
}

/// `reviewerPreconditionPrefix`.
fn reviewer_prefix(slug: &str, action: Action) -> String {
    match action {
        Action::PresentApprovalGate => format!("Cannot present \"{slug}\" for approval"),
        Action::Complete => format!("Cannot complete \"{slug}\""),
    }
}

/// `verifyReviewerPrecondition`, for a stage reviewed as one: a stage with
/// a reviewer cannot open its gate or complete until the review ran, which
/// a terminal verdict of the current attempt over the current outputs
/// proves. Hard on the review having happened, soft on what it said.
///
/// A recovery of a stage already closed (`require_receipt` off) weighs what
/// is pending and not whether a receipt exists. `action` is how the refusal
/// names what it blocks. The person's own approval goes over a review that
/// did not finish, unless their team keeps the policy strict.
///
/// # Errors
///
/// The review is owed, pending, unverifiable or stale: a guard's refusal.
/// Or the stage runs for each unit, or writes source code: neither is
/// weighed yet, and both are refused by name.
pub fn verify_reviewer(
    wf: &Workflow,
    carried: Carried<'_>,
    stage: &Stage,
    state: &str,
    action: Action,
    require_receipt: bool,
) -> Result<()> {
    let Some(reviewer) = stage::reviewer(stage) else {
        return Ok(());
    };
    let class = resolve_review_class(wf, stage, state);
    if class == "none" {
        return Ok(());
    }
    let slug = stage.slug.as_str();
    if stage::has_unit_marker(stage) || stage::is_per_unit(stage) {
        bail!(
            "\"{slug}\" runs for each unit and owes a review from {reviewer}: which units owe one is read off their rows and the plan of units, and Focus does not read those yet (units arrive with a later slice)."
        );
    }
    if stage::workspace_requires(stage) {
        bail!(
            "\"{slug}\" writes source code and owes a review from {reviewer}: whether the source is still the one reviewed is weighed against each unit's claims, and Focus does not read those yet (units arrive with a later slice)."
        );
    }
    let receipts = review_receipts::fresh(
        wf,
        state,
        stage,
        review_receipts::Options {
            review_class: Some(class),
            ..Default::default()
        },
    )?;
    if receipts.change_control_read {
        let accepted: Vec<Accepted<'_>> = receipts
            .accepted_changes
            .iter()
            .map(|change| Accepted {
                checkpoint: "review-receipt",
                stage: &change.stage,
                unit: change.unit.as_deref(),
                changed: change.changed.as_deref(),
                recorded: &change.recorded,
                current: &change.current,
                notice: &change.notice,
            })
            .collect();
        observe_change_control(wf, carried.held, state, &accepted)?;
    }
    let prefix = reviewer_prefix(slug, action);
    let pending = receipts.stage_pending;
    // A refusal here is a guard's, evaluated on the attempt with the
    // receipts just read.
    let refuse = |code: &str, invariant: &str, message: &str| {
        crate::guard_series::guard_refused(
            wf,
            state,
            stage,
            crate::guard_series::Refused {
                code,
                blocked_action: action.word(),
                invariant,
                user_message: message,
                unit: None,
            },
            crate::guard_attempt::Options {
                receipts: Some(receipts.clone()),
                ..Default::default()
            },
        )
    };
    if let Some(pending) = pending.filter(|pending| pending.verification_failed) {
        let message = format!(
            "{prefix} because the recorded review could not be verified and the review must be run again. Retry the pending request with `focus engine log review --stage {slug} --reviewer {reviewer} --iteration {} --retry-pending`, then record the verdict with the same command plus `--verdict <READY|NOT-READY>`.",
            focus_js::num::to_string(pending.iteration)
        );
        return Err(refuse(
            "REVIEW_EVIDENCE_MISSING",
            "A recorded review remains available and matches its completion.",
            &message,
        ));
    }
    if pending.is_some_and(|pending| pending.recovery)
        && !approvable_unfinished_review(wf, state, &receipts, carried.person)
    {
        let message = format!(
            "{prefix}: the recovery review is still in progress. Finish that review and record its result before presenting the gate or completing the stage."
        );
        return Err(refuse(
            "REVIEW_RECOVERY_PENDING",
            "A pending recovery request receives its matching verdict.",
            &message,
        ));
    }
    // A recovery of a stage already closed asks no more: whether a receipt
    // exists is what closing it asked.
    if !require_receipt {
        return Ok(());
    }
    // (`noteVerdictNotFinished` marks here the approval over a verdict
    // that is the NOT-READY no reviewer gave. It decides nothing: it is
    // what the approval then records, which comes with the findings a
    // gate records, slice 3e-v.)
    if receipts.stage_verdict.is_some()
        || approvable_unfinished_review(wf, state, &receipts, carried.person)
    {
        return Ok(());
    }
    if receipts.stage_stale {
        // `staleReviewPreconditionError`.
        if receipts
            .stage_stale_progress
            .is_some_and(|progress| progress.recovery_spent)
        {
            let unattended =
                crate::guard_refusal::unattended_says(env::var("UNATTENDED").as_deref());
            let guidance =
                crate::guard_refusal::recovery_guidance(wf, state, slug, None, unattended, None)
                    .map_err(EngineError::new)?;
            let message = format!(
                "{prefix}: this stage's output document changed again after the one recovery review by {reviewer}. {guidance}{}",
                if crate::guard_refusal::request_changes_reset_is_executable(state, slug) {
                    " Only a human Request Changes decision resets the review attempt; do not record that rejection on the human's behalf."
                } else {
                    ""
                }
            );
            return Err(refuse(
                "ARTIFACT_RECOVERY_SPENT",
                "The current output remains covered by the bounded review attempt.",
                &message,
            ));
        }
        let message = format!(
            "{prefix} because an output document changed after {reviewer} reviewed it. Run one recovery review pass with `focus engine log review --stage {slug} --reviewer {reviewer} --iteration <next ordinal>`, then record the verdict with the same command plus `--verdict <READY|NOT-READY>`. After that review, stop editing this stage's output documents. If the recovery pass was already spent, present the situation to the human at the approval gate; a human Request Changes decision resets the review attempt. Do not record a rejection on the human's behalf."
        );
        return Err(refuse(
            "ARTIFACT_REVIEW_STALE",
            "The current output remains covered by reviewer evidence.",
            &message,
        ));
    }
    // `reviewerPreconditionError`.
    if let Some(pending) = pending.filter(|pending| pending.state == PendingState::RetryRequired) {
        let message = format!(
            "{prefix} because the review request is still waiting for a verifiable recorded result. Run `focus engine log review --stage {slug} --reviewer {reviewer} --iteration {} --retry-pending`, then record the verdict with the same command plus `--verdict <READY|NOT-READY>`.",
            focus_js::num::to_string(pending.iteration)
        );
        return Err(refuse(
            "REVIEW_EVIDENCE_MISSING",
            "A pending review request receives a verifiable recorded result.",
            &message,
        ));
    }
    // A request that can never finish is requested again at its own pass.
    let ordinal = pending
        .filter(|pending| pending.state == PendingState::Outstanding)
        .map_or_else(
            || "<next ordinal>".to_owned(),
            |pending| focus_js::num::to_string(pending.iteration),
        );
    let message = match action {
        Action::PresentApprovalGate => format!(
            "Cannot present \"{slug}\" for approval because {reviewer} has not reviewed the current output. Apply any fixes first, then request the review with `focus engine log review --stage {slug} --reviewer {reviewer} --iteration {ordinal}` and record its verdict with the same command plus `--verdict <READY|NOT-READY>`. After recording the verdict, do not edit this stage's output documents; include suggestions from a READY review in the approval summary instead."
        ),
        Action::Complete => format!(
            "Cannot complete \"{slug}\" because {reviewer} has not reviewed the current output. Apply any fixes first, then request the review with `focus engine log review --stage {slug} --reviewer {reviewer} --iteration {ordinal}` and record its verdict with the same command plus `--verdict <READY|NOT-READY>`. After recording the verdict, do not edit this stage's output documents; include suggestions from a READY review in the approval summary instead."
        ),
    };
    Err(refuse(
        "REVIEW_EVIDENCE_MISSING",
        "Reviewer-bearing stages have current terminal review evidence.",
        &message,
    ))
}

/// `verifyPipelineLinkPrecondition`: a pipeline stage owes its handoffs,
/// each link of its chain recorded in the current attempt, unless the
/// evidence is switched off.
///
/// # Errors
///
/// A handoff is owed: a guard's refusal that names each one, by
/// repository. Or the registry names the repositories with something that
/// is not a list of names, or the state cannot be read.
pub fn verify_pipeline_link(wf: &Workflow, stage: &Stage) -> Result<()> {
    if stage::mode(stage) != Some("pipeline")
        || project_flag(&wf.layout, "DISABLE_ENSEMBLE_EVIDENCE").as_deref() == Some("1")
    {
        return Ok(());
    }
    let owing = crate::pipeline_evidence::owed(wf, stage)?;
    if owing.missing.is_empty() {
        return Ok(());
    }
    let missing: Vec<String> = owing
        .missing
        .iter()
        .map(|owed| match owed.repo.as_deref() {
            Some(repo) if !repo.is_empty() => format!("{repo}:{}", owed.link),
            _ => owed.link.clone(),
        })
        .collect();
    let message = format!(
        "Cannot complete \"{}\" because these pipeline handoffs have not been recorded for the current run: {}. Run `focus engine log` link after each agent returns{}, or set {}=1 only to recover a legitimately-run in-flight pipeline.",
        stage.slug,
        missing.join(", "),
        if owing.repos.is_empty() {
            ""
        } else {
            " with --repo <repo>"
        },
        env::full_name("DISABLE_ENSEMBLE_EVIDENCE")
    );
    // The state as the record holds it now: the guard reads it again.
    let state = wf.read_state()?;
    Err(crate::guard_series::guard_refused(
        wf,
        &state,
        stage,
        crate::guard_series::Refused {
            code: "PIPELINE_EVIDENCE_MISSING",
            blocked_action: "complete",
            invariant: "Every required pipeline handoff is recorded in the current attempt.",
            user_message: &message,
            unit: None,
        },
        crate::guard_attempt::Options::default(),
    ))
}

/// What stands in for Construction's checkpoints, units and swarms (M4).
///
/// # Errors
///
/// The stage is a Construction stage.
pub fn refuse_construction(stage: &Stage) -> Result<()> {
    if stage.phase == "construction" {
        bail!(
            "\"{}\" is a Construction stage, which runs per unit of work with checkpoints, unit gates and swarms: Focus does not run Construction yet. Skip the stage, or wait for the units milestone.",
            stage.slug
        );
    }
    Ok(())
}

/// `verifyGateOpeningGuards`: what opening a gate checks.
///
/// # Errors
///
/// Any guard above refuses.
pub fn verify_gate_opening_guards(
    wf: &Workflow,
    carried: Carried<'_>,
    stage: &Stage,
    state: &str,
) -> Result<()> {
    verify_stage_artifacts(wf, stage, Action::PresentApprovalGate)?;
    verify_summary_confirmation(wf, carried.held, stage, state)?;
    verify_pipeline_link(wf, stage)?;
    if !reviewer_gate_guard_disabled() {
        verify_reviewer(wf, carried, stage, state, Action::PresentApprovalGate, true)?;
    }
    Ok(())
}

/// `admitStageAction`: the chain, for a serial stage. `operation` is how
/// the archived refusal names the command; `approving` is the `approve`
/// entry point, which checks the artifacts, the summary and the pipeline
/// before the reviewer, where a recovery checks the reviewer first.
///
/// # Errors
///
/// Any guard refuses; the stage is a Construction stage.
pub fn admit(
    wf: &Workflow,
    carried: Carried<'_>,
    state: &str,
    stage: &Stage,
    action: Action,
    operation: &str,
    approving: bool,
) -> Result<()> {
    assert_not_archived(state, operation)?;
    refuse_construction(stage)?;
    if action != Action::Complete {
        verify_gate_opening_guards(wf, carried, stage, state)?;
        return Ok(());
    }
    let already_completed = parse_checkboxes(state)
        .iter()
        .any(|line| line.slug == stage.slug && line.state == CheckboxState::Completed);
    if approving {
        verify_stage_artifacts(wf, stage, Action::Complete)?;
        verify_summary_confirmation(wf, carried.held, stage, state)?;
        verify_pipeline_link(wf, stage)?;
        verify_reviewer(wf, carried, stage, state, Action::Complete, true)?;
        return Ok(());
    }
    verify_reviewer(
        wf,
        carried,
        stage,
        state,
        Action::Complete,
        !already_completed,
    )?;
    if !already_completed {
        verify_stage_artifacts(wf, stage, Action::Complete)?;
        verify_summary_confirmation(wf, carried.held, stage, state)?;
        verify_pipeline_link(wf, stage)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use focus_harness::Harness;
    use focus_workspace::records::{Invoker, Selection};
    use serde_json::json;

    use super::*;

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// An installed project with one record, opened as a workflow.
    fn workflow(name: &str, state: &str) -> (Workflow, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("focus-admission-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream/installed/claude");
        copy_dir(&fixtures, &root);
        let record = root.join("focus/spaces/default/intents/260105-add-login");
        std::fs::create_dir_all(&record).unwrap();
        std::fs::write(record.join("focus-state.md"), state).unwrap();
        let layout = Layout::new(root.clone(), Harness::Claude);
        let selection = Selection {
            space: "default".to_owned(),
            intent: Some("260105-add-login".to_owned()),
            session_id: None,
            binding: None,
        };
        (
            Workflow::open(layout, selection, Invoker::default()).unwrap(),
            root,
        )
    }

    fn stage_of(node: serde_json::Value) -> Stage {
        focus_graph::runtime::load_from_value(&json!([node])).remove(0)
    }

    const STATE: &str = "- **Scope**: feature\n- **Project Type**: Greenfield\n- **Review Override**: \n- **Summary Confirmation**: on (from scope feature)\n";

    #[test]
    fn a_code_producing_stage_needs_source_work() {
        let (wf, root) = workflow("source", STATE);
        // What the distribution lays at the project's root is a file like any other, and counts.
        assert!(workspace_has_work(&wf).unwrap());
        std::fs::remove_file(root.join(".mcp.json")).unwrap();
        assert!(workspace_has_work(&wf).unwrap());
        std::fs::remove_file(root.join(".gitignore")).unwrap();
        assert!(!workspace_has_work(&wf).unwrap());
        // No artifacts declared: the stage passes on them; the source work is what is missing.
        let stage = stage_of(
            json!({"slug": "builder", "phase": "ideation", "name": "Builder", "workspace_requires": true, "produces": []}),
        );
        let refused = verify_stage_artifacts(&wf, &stage, Action::Complete).unwrap_err();
        assert!(refused.message().starts_with("Cannot complete \"builder\": it is a code-producing stage (workspace_requires) but no source work is evident outside the focus/ workspace tree."), "{refused}");
        let gate = verify_stage_artifacts(&wf, &stage, Action::PresentApprovalGate).unwrap_err();
        assert!(
            gate.message().starts_with(
                "Cannot present \"builder\" for approval: it is a code-producing stage"
            ),
            "{gate}"
        );
        // The refusal rides on the error with its code and its invariant,
        // as the tool's entry point prints them. (No command reaches this
        // guard yet: the stages that write code are Construction's, which
        // are refused by name one step earlier, so these two are not yet
        // compared with upstream's.)
        let carried = |error: &EngineError| {
            let guard = error
                .detail::<crate::guard_series::GuardRefused>()
                .expect("a guard's refusal");
            (
                guard.refusal.code.clone(),
                guard.refusal.blocked_action.clone(),
                guard.refusal.invariant.clone(),
            )
        };
        assert_eq!(
            carried(&refused),
            (
                "REQUIRED_SOURCE_WORK_MISSING".to_owned(),
                "complete".to_owned(),
                "A workspace-producing stage contains application source work.".to_owned()
            )
        );
        assert_eq!(carried(&gate).1, "present-approval-gate");
        // A planning doc under the workspace is not source work; a file under src/ is.
        std::fs::write(root.join("focus/notes.md"), "x").unwrap();
        assert!(verify_stage_artifacts(&wf, &stage, Action::Complete).is_err());
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        assert!(verify_stage_artifacts(&wf, &stage, Action::Complete).is_ok());
        // A stage that does not produce code is not asked.
        let plain = stage_of(
            json!({"slug": "writer", "phase": "ideation", "name": "Writer", "produces": []}),
        );
        std::fs::remove_dir_all(root.join("src")).unwrap();
        assert!(verify_stage_artifacts(&wf, &plain, Action::Complete).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_summary_is_owed_by_the_phases_that_ask_one() {
        let (wf, root) = workflow("summary", STATE);
        let at = |phase: &str, declared: bool| {
            let mut node = json!({"slug": "s", "phase": phase, "name": "S"});
            if declared {
                node["summary_confirmation"] = json!("required");
            }
            stage_of(node)
        };
        assert!(!summary_confirmation_owed(
            &wf,
            &at("initialization", true),
            STATE
        ));
        assert!(summary_confirmation_owed(&wf, &at("ideation", true), STATE));
        assert!(summary_confirmation_owed(
            &wf,
            &at("operation", true),
            STATE
        ));
        assert!(!summary_confirmation_owed(
            &wf,
            &at("operation", false),
            STATE
        ));
        assert!(summary_confirmation_owed(
            &wf,
            &at("construction", true),
            STATE
        ));
        let autonomous = format!("{STATE}- **Construction Autonomy Mode**: autonomous\n");
        assert!(!summary_confirmation_owed(
            &wf,
            &at("construction", true),
            &autonomous
        ));
        assert!(summary_confirmation_owed(
            &wf,
            &at("operation", true),
            &autonomous
        ));
        let off = STATE.replace("on (from scope feature)", "off (set by you)");
        assert!(!summary_confirmation_owed(&wf, &at("ideation", true), &off));
        // With no line, the scope decides; an unknown scope reads as on.
        let no_line = STATE.replace("- **Summary Confirmation**: on (from scope feature)\n", "");
        assert_eq!(summary_confirmation_setting(&wf, &no_line), "on");
        assert_eq!(
            summary_confirmation_setting(&wf, &no_line.replace("feature", "nope")),
            "on"
        );
        // Owed and not there: a guard's refusal, with what the guard holds
        // (the whole refusal is compared with upstream in the differential).
        let refused =
            verify_summary_confirmation(&wf, None, &at("ideation", true), STATE).unwrap_err();
        let guard = refused
            .detail::<crate::guard_series::GuardRefused>()
            .expect("a guard's refusal");
        assert_eq!(
            guard.refusal.code, "SUMMARY_QUESTIONS_MISSING",
            "{refused:?}"
        );
        assert_eq!(guard.refusal.blocked_action, "summary-confirmation");
        assert_eq!(guard.refusal.invariant, SUMMARY_INVARIANT);
        assert!(verify_summary_confirmation(&wf, None, &at("ideation", true), &off).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_pipeline_owes_its_handoffs_and_a_reviewed_stage_its_receipt() {
        let (wf, root) = workflow("pipeline", STATE);
        let pipeline =
            stage_of(json!({"slug": "p", "phase": "inception", "name": "P", "mode": "pipeline"}));
        // Owed and not recorded: a guard's refusal, with what the guard
        // holds (the whole refusal is compared in the differential).
        let refused = verify_pipeline_link(&wf, &pipeline).unwrap_err();
        let guard = refused
            .detail::<crate::guard_series::GuardRefused>()
            .expect("a guard's refusal");
        assert_eq!(
            guard.refusal.code, "PIPELINE_EVIDENCE_MISSING",
            "{refused:?}"
        );
        assert_eq!(guard.refusal.blocked_action, "complete");
        assert_eq!(
            guard.refusal.invariant,
            "Every required pipeline handoff is recorded in the current attempt."
        );
        for mode in ["inline", "subagent", "mob"] {
            let other =
                stage_of(json!({"slug": "o", "phase": "inception", "name": "O", "mode": mode}));
            assert!(verify_pipeline_link(&wf, &other).is_ok(), "{mode}");
        }
        let reviewed = stage_of(
            json!({"slug": "r", "phase": "inception", "name": "R", "reviewer": "focus-product-lead-agent", "review_class": "advisory"}),
        );
        assert_eq!(resolve_review_class(&wf, &reviewed, STATE), "advisory");
        assert_eq!(
            resolve_review_class(
                &wf,
                &reviewed,
                &STATE.replace("- **Review Override**: \n", "- **Review Override**: none\n")
            ),
            "none"
        );
        assert_eq!(
            resolve_review_class(
                &wf,
                &reviewed,
                &STATE.replace(
                    "- **Review Override**: \n",
                    "- **Review Override**: adversarial\n"
                )
            ),
            "advisory"
        );
        // Owed and not there: a guard's refusal, said for the action it
        // blocks (the whole refusal is compared in the differential). A
        // recovery of a stage already closed does not ask for the receipt.
        let weigh = |stage: &Stage, action: Action, require: bool| {
            verify_reviewer(&wf, Carried::default(), stage, STATE, action, require)
        };
        let refused = weigh(&reviewed, Action::Complete, true).unwrap_err();
        let guard = refused
            .detail::<crate::guard_series::GuardRefused>()
            .expect("a guard's refusal");
        assert_eq!(guard.refusal.code, "REVIEW_EVIDENCE_MISSING", "{refused:?}");
        assert_eq!(guard.refusal.blocked_action, "complete");
        assert!(refused.message().starts_with("Cannot complete \"r\" because focus-product-lead-agent has not reviewed the current output."));
        let refused = weigh(&reviewed, Action::PresentApprovalGate, true).unwrap_err();
        assert!(
            refused
                .message()
                .starts_with("Cannot present \"r\" for approval because")
        );
        assert!(weigh(&reviewed, Action::Complete, false).is_ok());
        let unreviewed = stage_of(json!({"slug": "u", "phase": "inception", "name": "U"}));
        assert!(weigh(&unreviewed, Action::Complete, true).is_ok());
        let construction = stage_of(json!({"slug": "c", "phase": "construction", "name": "C"}));
        assert!(
            refuse_construction(&construction)
                .unwrap_err()
                .message()
                .contains("is a Construction stage")
        );
        assert!(refuse_construction(&unreviewed).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_ceremony_line_reads_on_or_off() {
        assert_eq!(ceremony_state_value(Some("off (set by you)")), Some("off"));
        assert_eq!(ceremony_state_value(Some("ON")), Some("on"));
        assert_eq!(
            ceremony_state_value(Some("  on (from scope feature)  ")),
            Some("on")
        );
        assert_eq!(ceremony_state_value(Some("offline")), None);
        assert_eq!(ceremony_state_value(Some("")), None);
        assert_eq!(ceremony_state_value(None), None);
    }

    #[test]
    fn an_archived_workflow_is_refused_by_the_operation() {
        assert!(assert_not_archived("- **Status**: Running\n", "advance").is_ok());
        assert_eq!(
            assert_not_archived("- **Status**: Archived\n", "focus engine state advance")
                .unwrap_err()
                .message(),
            "Workflow is Archived, so focus engine state advance is refused. Bring it back first with `/focus intent unarchive <name>`."
        );
    }
}

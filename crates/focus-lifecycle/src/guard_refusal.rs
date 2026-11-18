//! What a guard that refuses offers as the way on.
//!
//! `evaluateGuardRefusal` (`aidlc-lib.ts` L30 482): from the code a guard
//! refused with, the stage's state and what the attempt looks like (the
//! review budget, the one recovery, the review in flight, and whether the
//! summary, the review and the source still cover the bytes on disk), the
//! ways on, in the order they are offered: what the workflow can do by
//! itself first, the reset of the stage's lifecycle after, the fence's own
//! switch last. Each remedy says what the conductor does (`action`),
//! whether it can be done now, whether it needs the person, and, for the
//! ones put to the person, the name and the one line they read.
//!
//! The evaluator reads nothing of the project: the same input gives the
//! same refusal, which is what lets the tool that refuses and the router
//! that asks agree.
//!
//! # What is not here yet
//!
//! Two inputs of upstream's have no counterpart, because nothing in Focus
//! can give them yet: the gate of a unit owned by a team (`teamGate`,
//! deferred with team ownership) and an autonomous bolt (`autonomousBolt`,
//! which the swarm gives). Their remedies (`restore-scope`, `abort-bolt`,
//! and a report that names a team's unit) come with them.
//!
//! The summary's questions file is handed in by the caller, who reads it
//! (upstream reads it here when it has a project directory).

use focus_evidence::source::walk::REGISTRY;
use focus_guards::fences::Fence;
use focus_guards::guard_operation::{
    self, EngineInvocation, GuardRecoveryOperation, NATIVE_INVOCATION, RemedyWordingContext,
    RenderMode, RenderShell,
};
use focus_js::text::trim;
use focus_js::{num, re};
use focus_state::fields::get_field;
use focus_state::progress::{CheckboxState, parse_checkboxes, parse_stage_suffixes};
use serde_json::Value;

use crate::review_attempt::is_team_unit_ownership;
use crate::workflow::Workflow;
use crate::{stage, summary_evidence};

/// The entry skill, as a person types it.
const ENTRY_SKILL: &str = "/focus";

/// Whether the one recovery review of an attempt is still there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    Available,
    Pending,
    Spent,
}

/// Whether a piece of evidence still covers the bytes on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Current,
    Stale,
    Missing,
    /// The source could not be bound at all (the source's coverage only).
    Unbindable,
}

/// A review request that waits for its verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingReview {
    pub iteration: f64,
    pub retryable: bool,
    /// `None` when the attempt's reader did not say.
    pub verdict_recordable: Option<bool>,
    /// The exact command that records the verdict.
    pub record_verdict: Option<String>,
}

/// The pass a review resumes at, with its exact request when it is known.
#[derive(Debug, Clone, PartialEq)]
pub struct NextReview {
    pub iteration: f64,
    pub request: Option<String>,
}

/// `GuardAttemptState`: what the current attempt looks like to a guard.
#[derive(Debug, Clone, PartialEq)]
pub struct Attempt {
    /// The passes used and the passes the stage allows.
    pub review_budget: Option<(f64, f64)>,
    pub recovery: Recovery,
    pub pending_review: Option<PendingReview>,
    /// The pass whose reviewer asked for repairs.
    pub repair_review: Option<f64>,
    pub next_review: Option<NextReview>,
    /// The exact request for the next permitted review when none is in
    /// flight.
    pub request_review: Option<String>,
    pub summary_coverage: Coverage,
    pub review_coverage: Coverage,
    pub source_coverage: Coverage,
}

/// The summary check that refused: its remedy names the same questions file
/// and the same isolated identity the receipt must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub questions_file: Option<String>,
    pub isolated: bool,
}

/// `GuardRefusalInput`.
#[derive(Debug, Clone)]
pub struct Input<'a> {
    pub code: &'a str,
    pub blocked_action: &'a str,
    pub stage: &'a str,
    pub unit: Option<&'a str>,
    /// Named in the commands a remedy spells, when the guard has one.
    pub project_dir: Option<&'a str>,
    pub state: &'a str,
    pub invariant: &'a str,
    pub user_message: &'a str,
    pub attempt: &'a Attempt,
    /// The operator said no person is watching.
    pub unattended: bool,
    /// Set when the refusal is a fence holding: its switch is offered last.
    pub fence: Option<Fence>,
    /// Policy or the actor makes the switch unavailable.
    pub fence_switch_withheld: bool,
    pub summary: Option<&'a Summary>,
    /// The shell the commands are quoted for; `None` is this platform's.
    pub shell: Option<RenderShell>,
}

/// How a remedy is carried out: as its exact command, by the person's
/// follow-up, or as work the conductor does through the protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interaction {
    Command,
    HumanInput,
    ExternalWork,
}

impl Interaction {
    /// Upstream's word.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::HumanInput => "human-input",
            Self::ExternalWork => "external-work",
        }
    }
}

/// `GuardRemedy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remedy {
    pub op: &'static str,
    /// What the person reads; the conductor's own work carries neither.
    pub label: Option<String>,
    pub description: Option<String>,
    pub action: String,
    pub operation: Option<GuardRecoveryOperation>,
    pub command: Option<String>,
    pub requires_human: bool,
    pub executable_now: bool,
    pub interaction: Interaction,
}

/// `GuardRefusal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: String,
    pub blocked_action: String,
    pub stage: String,
    pub unit: Option<String>,
    /// The stage's state, in upstream's words.
    pub state: &'static str,
    pub invariant: String,
    pub user_message: String,
    pub remedies: Vec<Remedy>,
}

/// A remedy before its interaction and its words are worked out.
struct Offer {
    op: &'static str,
    action: String,
    operation: Option<GuardRecoveryOperation>,
    command: Option<String>,
    requires_human: bool,
    executable_now: bool,
}

impl Offer {
    fn work(op: &'static str, action: String, executable_now: bool) -> Self {
        Self {
            op,
            action,
            operation: None,
            command: None,
            requires_human: false,
            executable_now,
        }
    }

    fn person(op: &'static str, action: String) -> Self {
        Self {
            requires_human: true,
            ..Self::work(op, action, true)
        }
    }
}

/// What the evaluator needs of the install to spell a command.
struct Spelling<'a> {
    wf: &'a Workflow,
    shell: RenderShell,
    project_dir: Option<&'a str>,
}

impl Spelling<'_> {
    /// `renderEngineInvocation`.
    fn engine(&self, route: &str, args: Vec<String>) -> String {
        guard_operation::render_engine_invocation(
            &EngineInvocation {
                route: route.to_owned(),
                args,
            },
            RenderMode::Native,
            self.wf.layout.harness.dir(),
            self.shell,
        )
        .unwrap_or_default()
    }

    /// `guardOperation`: the operation with its own command.
    fn operation(&self, operation: &GuardRecoveryOperation) -> String {
        guard_operation::render(
            operation,
            RenderMode::Native,
            self.wf.layout.harness.dir(),
            self.shell,
        )
        .unwrap_or_default()
    }

    /// A remedy the person picks, run as the operation's command.
    fn command(
        &self,
        op: &'static str,
        action: String,
        operation: GuardRecoveryOperation,
    ) -> Offer {
        Offer {
            command: Some(self.operation(&operation)),
            operation: Some(operation),
            ..Offer::person(op, action)
        }
    }
}

/// `aidlcToolInvocation("orchestrate")`.
fn orchestrate() -> String {
    format!("{NATIVE_INVOCATION} engine orchestrate")
}

/// `guardLifecycleState`: the stage's checkbox, `pending` with none.
fn lifecycle_state(state: &str, slug: &str) -> CheckboxState {
    parse_checkboxes(state)
        .into_iter()
        .find(|entry| entry.slug == slug)
        .map_or(CheckboxState::Pending, |entry| entry.state)
}

/// `unitMajorConstructionStageSlugs`: the per-unit Construction stages the
/// plan runs, in graph order, the skipped ones left out.
fn unit_major_stage_slugs<'a>(
    wf: &'a Workflow,
    scope: &str,
    state: &str,
    include_completed: bool,
) -> Vec<&'a str> {
    let Some(mapping) = wf.install.definition(scope) else {
        return Vec::new();
    };
    let overrides = parse_stage_suffixes(state);
    let boxes = parse_checkboxes(state);
    wf.install
        .graph
        .iter()
        .filter(|node| {
            if node.phase != "construction" || !stage::has_unit_marker(node) {
                return false;
            }
            // (A stage listed twice is the last line's: a `Map` built from
            // the lines.)
            let checkbox = boxes
                .iter()
                .rfind(|entry| entry.slug == node.slug)
                .map(|entry| entry.state);
            if checkbox == Some(CheckboxState::Skipped)
                || (checkbox == Some(CheckboxState::Completed) && !include_completed)
            {
                return false;
            }
            let action = overrides
                .iter()
                .rfind(|(slug, _)| *slug == node.slug)
                .map(|(_, action)| *action)
                .or_else(|| mapping.stages.get(&node.slug).and_then(Value::as_str));
            action == Some("EXECUTE")
        })
        .map(|node| node.slug.as_str())
        .collect()
}

/// `constructionCheckpointsApply`: Construction's checkpoints are on, the
/// units are no team's, and a stage of the walk writes code.
fn construction_checkpoints_apply(wf: &Workflow, state: &str) -> bool {
    if get_field(state, "Construction Checkpoints") != Some("enabled")
        || is_team_unit_ownership(state)
    {
        return false;
    }
    unit_major_stage_slugs(
        wf,
        get_field(state, "Scope").unwrap_or_default(),
        state,
        true,
    )
    .iter()
    .any(|slug| {
        wf.install
            .stage(slug)
            .is_some_and(stage::workspace_requires)
    })
}

/// Where a refusal sits in a solo walk one unit at a time.
struct Walk {
    /// The unit the refusal is about: the one it names, or the active unit
    /// working this stage.
    unit: Option<String>,
    /// `next` routes that unit's step again.
    live: bool,
    first_stage: bool,
}

/// `soloUnitMajorRefusal`: `None` outside a solo unit-major walk, or at a
/// stage that is not one of its block.
fn solo_unit_major(wf: &Workflow, input: &Input<'_>) -> Option<Walk> {
    let field = |name: &str| get_field(input.state, name).map(trim);
    if is_team_unit_ownership(input.state) || field("Construction Iteration") != Some("unit-major")
    {
        return None;
    }
    let block = unit_major_stage_slugs(wf, field("Scope").unwrap_or_default(), input.state, true);
    if !block.contains(&input.stage) {
        return None;
    }
    // Read from the state file, so only a valid unit name counts.
    let active =
        field("Active Unit").filter(|unit| re::test("^[A-Za-z0-9][A-Za-z0-9._-]*$", "", unit));
    let active_here = active.is_some() && field("Unit Stage") == Some(input.stage);
    let unit = input.unit.or(if active_here { active } else { None });
    Some(Walk {
        live: unit.is_some() && (active.is_none() || (active_here && active == unit)),
        unit: unit.map(str::to_owned),
        first_stage: block.first() == Some(&input.stage),
    })
}

/// `unitStepRedoClears`: redoing a unit's step resets no attempt, so it
/// clears a refusal about the step's own work and never one about its
/// review attempt.
fn unit_step_redo_clears(code: &str, attempt: &Attempt) -> bool {
    if code == "REVIEW_FREEZE_ACTIVE" || attempt.pending_review.is_some() {
        return false;
    }
    if attempt
        .review_budget
        .is_some_and(|(used, limit)| used >= limit)
    {
        return false;
    }
    !(attempt.recovery == Recovery::Spent && attempt.review_coverage != Coverage::Missing)
}

/// `unitStepInProgress`: the unit a refusal is about when it is part way
/// through its step in a solo unit-major walk.
fn unit_step_in_progress(wf: &Workflow, input: &Input<'_>) -> Option<String> {
    let walk = solo_unit_major(wf, input)?;
    let unit = walk.unit.filter(|_| walk.live)?;
    let field = |name: &str| get_field(input.state, name).map(trim);
    (field("Active Unit") == Some(unit.as_str()) && field("Unit State") == Some("in-progress"))
        .then_some(unit)
}

/// `unitMajorResetCost`: what a stage-wide reset throws away in a walk one
/// unit at a time, said where it is offered.
fn unit_major_reset_cost(jump: bool, stage: &str) -> String {
    if jump {
        " Construction runs one unit at a time here, so this also throws away the work every unit has finished: each unit redoes its steps and needs its reviews, Plan Approvals and checkpoint approval again.".to_owned()
    } else {
        format!(
            " Construction runs one unit at a time here, so this also throws away every unit's finished \"{stage}\" work: each unit that already did it does it again and needs its review and checkpoint approval again."
        )
    }
}

/// `scopesRunningStage`: the scopes whose plan runs this stage, by name.
fn scopes_running_stage(wf: &Workflow, slug: &str) -> Vec<String> {
    let mut names: Vec<String> = wf
        .install
        .definitions
        .iter()
        .filter(|definition| definition.stages.get(slug).and_then(Value::as_str) == Some("EXECUTE"))
        .map(|definition| definition.meta.name.clone())
        .collect();
    names.sort_by(|left, right| focus_js::order::cmp_utf16(left, right));
    names
}

/// `scopeChoice`: which scope the agent switches to.
fn scope_choice(scopes: &[String]) -> String {
    match scopes.len() {
        1 => format!("`{}`, the one scope that fits", scopes[0]),
        2..=4 => format!(
            "the one the person picks from {} (ask them which, in plain words)",
            scopes.join(", ")
        ),
        _ => "the scope the person picks (ask them which, in plain words)".to_owned(),
    }
}

/// `restartStageRemedy`.
fn restart_stage(spell: &Spelling<'_>, stage: &str) -> Offer {
    spell.command(
        "restart-stage",
        format!(
            "Restart this stage with {ENTRY_SKILL} --stage {stage}; the recorded answers survive, and the stage will ask for confirmation again."
        ),
        GuardRecoveryOperation::RestartStage {
            stage: stage.to_owned(),
        },
    )
}

/// `reopenUnitStepRemedy`.
fn reopen_unit_step(spell: &Spelling<'_>, stage: &str, unit: &str) -> Offer {
    spell.command(
        "reopen-unit-step",
        format!(
            "Start \"{stage}\" again for unit \"{unit}\" only, then re-run next. Unit \"{unit}\" does that step again; the other units keep their finished work, reviews, Plan Approvals and checkpoint approvals."
        ),
        GuardRecoveryOperation::ReopenUnit {
            stage: stage.to_owned(),
            unit: unit.to_owned(),
        },
    )
}

/// `finishUnitStepRemedy`: the unit's step finished with the review it has,
/// when no review pass is left.
fn finish_unit_step(spell: &Spelling<'_>, stage: &str, unit: &str) -> Offer {
    let operation = GuardRecoveryOperation::RecordUnitCompletion {
        stage: stage.to_owned(),
        unit: unit.to_owned(),
    };
    let command = spell.operation(&operation);
    spell.command(
        "record-unit-completion",
        format!(
            "Finish \"{stage}\" for unit \"{unit}\" with the review it has: run `{command}`, then re-run next. The open findings go to the person when unit \"{unit}\"'s work comes up for approval."
        ),
        operation,
    )
}

/// `lifecycleResetRemedies`: the ways to reset the stage's lifecycle, by
/// the state it is in.
fn lifecycle_reset(
    wf: &Workflow,
    spell: &Spelling<'_>,
    input: &Input<'_>,
    state: CheckboxState,
) -> Vec<Offer> {
    let entry = parse_checkboxes(input.state)
        .into_iter()
        .find(|entry| entry.slug == input.stage);
    if entry.is_some_and(|entry| entry.suffix.starts_with("SKIP")) {
        return vec![Offer::person(
            "change-scope",
            format!(
                "This stage is excluded from the current plan. Switch to a scope that includes it: run `{} next --scope <scope>` with {}, then restart {}.",
                orchestrate(),
                scope_choice(&scopes_running_stage(wf, input.stage)),
                input.stage
            ),
        )];
    }
    let walk = solo_unit_major(wf, input);
    let cost = |jump: bool| {
        if walk.is_some() {
            unit_major_reset_cost(jump, input.stage)
        } else {
            String::new()
        }
    };
    match state {
        CheckboxState::Pending | CheckboxState::Skipped => {
            let Some(walk) = &walk else {
                return vec![restart_stage(spell, input.stage)];
            };
            let live_unit = walk.unit.as_deref().filter(|_| walk.live);
            if let Some(unit) = live_unit
                && unit_step_redo_clears(input.code, input.attempt)
            {
                return vec![Offer::work(
                    "redo-unit-step",
                    format!(
                        "Redo \"{}\" for unit \"{unit}\" only: continue with {ENTRY_SKILL} and do that step again for unit \"{unit}\". The other units keep their finished work, reviews, Plan Approvals and checkpoint approvals.",
                        input.stage
                    ),
                    true,
                )];
            }
            // Restarting the first stage of the block is no forward jump, so
            // it is still offered with its cost. A later one either lands
            // back on the same step or starts every unit's finished work
            // over: the unit on this step starts it again on its own, unless
            // this work allows no review at all.
            if !walk.first_stage {
                return match live_unit {
                    Some(unit)
                        if input.attempt.review_budget.map(|(_, limit)| limit) != Some(0.0) =>
                    {
                        vec![reopen_unit_step(spell, input.stage, unit)]
                    }
                    _ => Vec::new(),
                };
            }
            let mut restart = restart_stage(spell, input.stage);
            restart.action.push_str(&cost(true));
            vec![restart]
        }
        CheckboxState::InProgress | CheckboxState::AwaitingApproval => {
            if input.unattended {
                return vec![Offer::person(
                    "unset-unattended",
                    format!(
                        "Halt unattended execution and ask a human what should change. Unset {}, ask \"What should change?\" for stage \"{}\", and end the turn. Only after the human answers may their exact text be submitted as the Request Changes reason.{}",
                        focus_workspace::env::full_name("UNATTENDED"),
                        input.stage,
                        cost(false)
                    ),
                )];
            }
            vec![Offer::person(
                "request-changes",
                format!(
                    "When the person already said what should change for stage \"{}\", submit Request Changes with their exact text unchanged as the report reason. Otherwise ask \"What should change?\" and end the turn, then submit their answer the same way. Either way that unlocks revision and a fresh review.{}",
                    input.stage,
                    cost(false)
                ),
            )]
        }
        CheckboxState::Revising => {
            let mut args: Vec<String> = ["report", "--stage", input.stage, "--result", "revised"]
                .map(str::to_owned)
                .to_vec();
            if let Some(project) = spell.project_dir {
                args.extend(["--project-dir".to_owned(), project.to_owned()]);
            }
            let finish = spell.engine("orchestrate", args);
            vec![
                Offer::work(
                    "finish-revision",
                    format!(
                        "Finish the current revision without restarting the stage by running `{finish}`. This reopens the approval gate without re-running the stage or re-asking anything."
                    ),
                    input.attempt.summary_coverage == Coverage::Current
                        && input.attempt.review_coverage == Coverage::Current,
                ),
                spell.command(
                    "redo-jump",
                    format!(
                        "Restart the stage from the top with {ENTRY_SKILL} --stage {}. This costs more than finishing the current revision: your recorded answers survive, but you re-confirm the summary once and then save every output document again, so each one descends from the new confirmation.{}",
                        input.stage,
                        cost(true)
                    ),
                    GuardRecoveryOperation::RestartStage {
                        stage: input.stage.to_owned(),
                    },
                ),
            ]
        }
        CheckboxState::Completed => {
            let lead = if input.attempt.source_coverage == Coverage::Unbindable {
                format!(
                    "This stage is already approved; repair {REGISTRY} or the workspace source boundary so the application source can be checked, or jump back with {ENTRY_SKILL} --stage {} to redo it.",
                    input.stage
                )
            } else {
                format!(
                    "This stage is already approved; restore the reviewed source state, or jump back with {ENTRY_SKILL} --stage {} to redo it.",
                    input.stage
                )
            };
            vec![spell.command(
                "restore-or-jump",
                lead + &cost(true),
                GuardRecoveryOperation::RestartStage {
                    stage: input.stage.to_owned(),
                },
            )]
        }
    }
}

/// `reviewRequestAction`: a review request spelled out for the conductor:
/// the exact request, then the dispatch and the verdict it returns.
fn review_request_action(wf: &Workflow, lead: &str, request: Option<&str>) -> String {
    match request {
        None => format!("{lead}."),
        Some(request) => format!(
            "{lead}: run `{request}`, then dispatch the reviewer named in it as a subagent and have it write the `reviewFile` that command returns: that file is the reviewer's, so never write it yourself and never stand in for it (`{}/focus-common/protocols/stage-protocol-reviewer.md` step 1 says what to pass it). When its verdict is back, record it with the `recordVerdict` command the request returns.",
            wf.layout.harness.dir()
        ),
    }
}

/// `evaluateGuardRefusal`: the refusal with its ways on.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn evaluate(wf: &Workflow, input: &Input<'_>) -> Refusal {
    let state = lifecycle_state(input.state, input.stage);
    let attempt = input.attempt;
    let spell = Spelling {
        wf,
        shell: input.shell.unwrap_or(if cfg!(windows) {
            RenderShell::PowerShell
        } else {
            RenderShell::Posix
        }),
        project_dir: input.project_dir.filter(|project| !project.is_empty()),
    };
    let open_for_work = matches!(
        state,
        CheckboxState::InProgress | CheckboxState::AwaitingApproval
    );
    let summary_current = attempt.summary_coverage == Coverage::Current;
    let unit = input.unit.filter(|unit| !unit.is_empty());
    let mut offers: Vec<Offer> = Vec::new();

    // The gate needs this unit's completion receipt and nothing but the
    // receipt is missing: recording it is the way forward.
    if input.code == "UNIT_COMPLETION_MISSING"
        && open_for_work
        && let Some(unit) = unit
    {
        let operation = GuardRecoveryOperation::RecordUnitCompletion {
            stage: input.stage.to_owned(),
            unit: unit.to_owned(),
        };
        let command = spell.operation(&operation);
        offers.push(spell.command(
            "record-unit-completion",
            format!(
                "Record Unit \"{unit}\"'s completion for \"{}\" from the artifacts already on disk by running `{command}`, then present its gate again.",
                input.stage
            ),
            operation,
        ));
    }
    if let Some(pending) = &attempt.pending_review {
        let iteration = num::to_string(pending.iteration);
        if pending.verdict_recordable != Some(false) && summary_current {
            // The closing call is spelled out: asking for a review and
            // recording its verdict are one command with `--verdict` added,
            // which the request's own output does not let anyone guess.
            let how = match pending
                .record_verdict
                .as_deref()
                .filter(|command| !command.is_empty())
            {
                Some(command) => format!(": `{command}`."),
                None => {
                    " using the recordVerdict command returned by the review request.".to_owned()
                }
            };
            offers.push(Offer::work(
                "record-verdict",
                format!(
                    "Record the verdict for pending review iteration {iteration} if the reviewer returned{how}"
                ),
                true,
            ));
        }
        if pending.retryable {
            offers.push(Offer::work(
                "retry-pending",
                format!("Retry pending review iteration {iteration} with --retry-pending."),
                summary_current,
            ));
        }
    }
    if let Some(repaired) = attempt.repair_review {
        offers.push(Offer::work(
            "apply-repairs-then-request",
            format!(
                "Apply the reviewer's requested repairs, then request review iteration {}.",
                num::to_string(repaired + 1.0)
            ),
            summary_current && open_for_work,
        ));
        // Another pass is the reviewer's call; the person may want the gate
        // now.
        offers.push(Offer {
            executable_now: summary_current && open_for_work,
            ..spell.command(
                "review-advisory-gate",
                "If the person wants to decide now, set reviews to advisory for this piece of work, then present the reviewer's open findings at the approval gate for them to decide.".to_owned(),
                GuardRecoveryOperation::ReviewAdvisory,
            )
        });
    }
    if let Some(next) = &attempt.next_review {
        offers.push(Offer::work(
            "request-review",
            review_request_action(
                wf,
                &format!(
                    "Request review iteration {} against the current artifact and source bytes",
                    num::to_string(next.iteration)
                ),
                next.request.as_deref(),
            ),
            summary_current && open_for_work,
        ));
    }
    if attempt.source_coverage == Coverage::Unbindable && state != CheckboxState::Revising {
        offers.push(Offer::work(
            "repair-source-boundary",
            format!("Repair {REGISTRY} or the workspace source boundary so the application source can be checked, then request a fresh review."),
            state != CheckboxState::Completed,
        ));
    }
    let budget_available = attempt
        .review_budget
        .is_none_or(|(used, limit)| used < limit);
    let review_current = attempt.review_coverage == Coverage::Current;
    // A unit part way through its step in a solo unit-major walk: no gate
    // opens before that step is done. The ways on are the unit's own.
    let mid_step = if !budget_available && review_current {
        unit_step_in_progress(wf, input)
    } else {
        None
    };
    if let Some(unit) = &mid_step {
        offers.push(finish_unit_step(&spell, input.stage, unit));
        offers.push(reopen_unit_step(&spell, input.stage, unit));
    } else if !budget_available && review_current && open_for_work {
        // With Construction's checkpoints the unit's checkpoint is its
        // gate: `next` shows it again.
        let walk = if construction_checkpoints_apply(wf, input.state) {
            solo_unit_major(wf, input)
        } else {
            None
        };
        let checkpoint_unit = walk.and_then(|walk| walk.unit.filter(|_| walk.live));
        let mut args = vec!["next".to_owned()];
        if let Some(project) = spell.project_dir {
            args.extend(["--project-dir".to_owned(), project.to_owned()]);
        }
        let show = spell.engine("orchestrate", args);
        offers.push(Offer::person(
            "present-approval-gate",
            match checkpoint_unit {
                None => "Present the unresolved review findings at the approval gate for the human instead of starting another review pass.".to_owned(),
                Some(unit) => format!(
                    "Present the unresolved review findings at unit \"{unit}\"'s checkpoint for the human instead of starting another review pass: run `{show}`, which shows that checkpoint again, and ask it with the findings."
                ),
            },
        ));
    }
    let no_review_in_flight = attempt.pending_review.is_none()
        && attempt.repair_review.is_none()
        && attempt.next_review.is_none();
    if attempt.recovery == Recovery::Available
        && attempt.review_coverage == Coverage::Stale
        && no_review_in_flight
    {
        offers.push(Offer::work(
            "start-recovery-review",
            "Start the one stale-review recovery review against the current artifact and source state.".to_owned(),
            summary_current && open_for_work,
        ));
    } else if budget_available
        && attempt.review_coverage == Coverage::Missing
        && no_review_in_flight
    {
        offers.push(Offer::work(
            "request-review",
            review_request_action(
                wf,
                "Request the next permitted review for the current attempt",
                attempt.request_review.as_deref(),
            ),
            summary_current && open_for_work,
        ));
    }
    if !summary_current && !review_current {
        let commands = summary_evidence::commands_for(
            wf,
            input.stage,
            unit,
            input
                .summary
                .and_then(|summary| summary.questions_file.as_deref()),
            input.summary.is_some_and(|summary| summary.isolated),
            "Looks correct",
            spell.shell,
        );
        let (decision, answer) = commands
            .map(|commands| (commands.decision, commands.answer))
            .unwrap_or_default();
        offers.push(Offer {
            // The summary's owner accepts a fresh confirmation during
            // revision too.
            executable_now: open_for_work || state == CheckboxState::Revising,
            ..Offer::person(
                "reconfirm-summary",
                format!(
                    "Present the current consolidated summary and record it with the checkpoint flags (a plain decision or answer never counts): `{decision}`, with exactly one blank `[Answer]:` line in the summary section; end the turn; after the human's fresh reply run `{answer}`. Then regenerate or re-save the produced artifacts."
                ),
            )
        });
    }
    for reset in lifecycle_reset(wf, &spell, input, state) {
        if !offers.iter().any(|offered| offered.op == reset.op) {
            offers.push(reset);
        }
    }
    // The fence's own way out, always last: letting the workflow finish the
    // step is nearly always the right answer.
    if let Some(fence) = input.fence.filter(|_| !input.fence_switch_withheld) {
        offers.push(spell.command(
            "lower-fence",
            format!(
                "Turn the {} check off for this piece of work. After the command succeeds, tell the person in one line that it is off for this piece of work, comes back on for the next one, and that they can ask you to turn it back on; then retry what was refused.",
                fence.name()
            ),
            GuardRecoveryOperation::LowerFence { fence },
        ));
    }

    let stage_name = wf
        .install
        .stage(input.stage)
        .map_or(input.stage, stage::name);
    let target = match unit {
        Some(unit) => format!("{stage_name} for {unit}"),
        None => stage_name.to_owned(),
    };
    let wording = RemedyWordingContext {
        code: input.code,
        stage: stage_name,
        target: &target,
        unit,
        fence: input.fence,
        source_unbindable: attempt.source_coverage == Coverage::Unbindable,
        every_unit: solo_unit_major(wf, input).is_some(),
    };
    Refusal {
        code: input.code.to_owned(),
        blocked_action: input.blocked_action.to_owned(),
        stage: input.stage.to_owned(),
        unit: unit.map(str::to_owned),
        state: state.name(),
        invariant: input.invariant.to_owned(),
        user_message: input.user_message.to_owned(),
        remedies: offers
            .into_iter()
            .map(|offer| {
                let interaction = if offer.operation.is_some() {
                    Interaction::Command
                } else if offer.requires_human {
                    Interaction::HumanInput
                } else {
                    Interaction::ExternalWork
                };
                let words = (interaction != Interaction::ExternalWork)
                    .then(|| guard_operation::remedy_wording(offer.op, &wording))
                    .flatten();
                let (label, description) = words.unzip();
                Remedy {
                    op: offer.op,
                    label,
                    description,
                    action: offer.action,
                    operation: offer.operation,
                    command: offer.command,
                    requires_human: offer.requires_human,
                    executable_now: offer.executable_now,
                    interaction,
                }
            })
            .collect(),
    }
}

/// `requestChangesResetIsExecutable`: whether a Request Changes can reset
/// the attempt now: the stage is at work or at its gate.
#[must_use]
pub fn request_changes_reset_is_executable(state: &str, slug: &str) -> bool {
    matches!(
        lifecycle_state(state, slug),
        CheckboxState::InProgress | CheckboxState::AwaitingApproval
    )
}

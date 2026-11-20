//! `log review`: a review of a stage's output is asked for.
//!
//! `focus engine log review --stage <slug> --reviewer <agent> --iteration
//! <n> [--single] [--retry-pending]` writes the `REVIEW_REQUESTED` row a
//! reviewer is dispatched on, and answers with the file the reviewer writes
//! and the exact command that closes the request (`handleReview`,
//! `aidlc-log.ts` L2 975, the half without `--verdict`).
//!
//! The pass is the engine's: the request names the pass it means and is
//! refused unless that is the next one of the attempt. A stage allows as
//! many passes as its review class gives; a review the person asked for is
//! never refused for want of passes. A request waits for its verdict before
//! another starts, and can be dispatched again once (`--retry-pending`)
//! against the bytes it was asked of. One that can never finish, because
//! its outputs or the source changed before the verdict, is replaced by a
//! new request at the same pass. And a review that a later write made stale
//! gets one recovery pass in an attempt.
//!
//! When one of the readers read the Guard Policy (something had changed),
//! what the policy did is recorded before anything else is decided: the
//! policy's own move, and each change it let through, which the answer then
//! says once (`change_notices`). A verdict that is refused after a change
//! was let through says it too, before its sentence and in a field of its
//! own ([`Refused`]), as upstream's does (`emitError`); a request that is
//! refused says none, as upstream's says none.
//!
//! # What is not here yet
//!
//! Refused by name, each until the slice that brings it:
//!
//! - `--unit`: the unit's claim, the plan of units, its source and its
//!   manifest (slice 5).
//!
//! A guard that refuses says its sentence; the question upstream prints
//! after it, and the series of the refusal it keeps, come with slices 4b
//! and 4c.
//!
//! Upstream also refuses a pending request that has no valid binding. No
//! pending request is without one (the count of an attempt passes over a
//! request row that binds nothing), so that refusal is never said, and it
//! is not here.
//!
//! # Tests that change no answer
//!
//! Mutating each condition here against the differential leaves five that
//! no check tells apart:
//!
//! - **The newest review of the source is the stage's.** With no unit asked
//!   for, the receipts name no unit as the one whose source is newest: the
//!   question is slice 5's.
//! - **The next pass is counted over every request, the budget over the
//!   ones since a unit started a step again.** The two counts differ only
//!   for a unit.
//! - **A recovery is offered only when no pass waits.** A pass that waits
//!   refuses the request before the offer is read, or is replaced.
//! - **The slot is looked at again after the old draft is removed.** In a
//!   tree that stands still the second look sees what the first saw.
//! - **A retry accepts a review appended to the artifact after the
//!   request.** That is a request of the retired appendix protocol, which
//!   no check writes; the reader it asks is compared by the corpus.

use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::review::{self as review_paths, RequestBinding};
use focus_evidence::source::snapshot::{self, UNBINDABLE};
use focus_evidence::source::state::same_workspace_source;
use focus_guards::guard_operation::RenderShell;
use focus_js::{EngineError, Result, bail, num, re};
use focus_lifecycle::artifacts::{self, Dag};
use focus_lifecycle::change_control::{self, Accepted};
use focus_lifecycle::guard_refusal::{
    recovery_guidance, request_changes_reset_is_executable, unattended_says,
};
use focus_lifecycle::review_artifacts::{self, Options as ArtifactOptions};
use focus_lifecycle::review_attempt::{self, Accounting, Reviewed, definitely_before};
use focus_lifecycle::review_pending::{self, Asked as PendingAsked, Status};
use focus_lifecycle::review_receipts::{self, Options as ReceiptOptions};
use focus_lifecycle::review_request::{
    ReviewCommand, recovery_spent_message, request_command, request_note, units_block_repair,
    verdict_command,
};
use focus_lifecycle::summary_evidence::{self, Asked as SummaryAsked, Evidence};
use focus_lifecycle::workflow::Workflow;
use focus_lifecycle::{admission, gate, presence, stage};
use focus_workspace::{env, links, observer};
use serde_json::{Value, json};

use crate::flags;

/// Why a request stops: a refusal in upstream's words (`ReviewRefusal`), or
/// anything else that failed under the lock, which upstream reports as a
/// failure of the audit's emission.
pub(crate) enum Stop {
    Refusal(String),
    Failed(String),
}

impl From<EngineError> for Stop {
    fn from(error: EngineError) -> Self {
        Self::Failed(error.to_string())
    }
}

pub(crate) type Step<T> = std::result::Result<T, Stop>;

pub(crate) fn refuse<T>(message: String) -> Step<T> {
    Err(Stop::Refusal(message))
}

/// What a guard of the review refuses with.
pub(crate) struct GuardSaid<'a> {
    pub(crate) code: &'a str,
    pub(crate) blocked_action: &'a str,
    pub(crate) invariant: &'a str,
    pub(crate) message: &'a str,
}

/// `refuseReviewGuard`: a guard's refusal as the review prints it: the
/// sentence, then the recovery question, with the refusal counted in its
/// series. The attempt is read with what the command already has in hand
/// (`options`); `resources` are the ones the series is signed with, the
/// attempt's own when the guard names none.
pub(crate) fn guard_text(
    wf: &Workflow,
    asked: &Asked<'_>,
    state: &str,
    node: &focus_graph::runtime::Stage,
    said: &GuardSaid<'_>,
    options: focus_lifecycle::guard_attempt::Options<'_>,
    resources: Option<Vec<String>>,
) -> Step<String> {
    use focus_lifecycle::guard_refusal::{Input, Summary, evaluate, unattended_says};

    let snapshot = focus_lifecycle::guard_attempt::guard_attempt_state(
        wf,
        &asked.project_dir,
        state,
        node,
        focus_lifecycle::guard_attempt::Options {
            single: asked.single,
            ..options
        },
    )?;
    let summary = Summary {
        questions_file: crate::summary::question_file_relative(
            wf,
            node,
            (!asked.single).then_some(state),
            None,
        ),
        isolated: asked.single,
    };
    let refusal = evaluate(
        wf,
        &Input {
            code: said.code,
            blocked_action: said.blocked_action,
            stage: asked.stage,
            unit: None,
            project_dir: Some(&asked.project_dir),
            state,
            invariant: said.invariant,
            user_message: said.message,
            attempt: &snapshot.attempt,
            unattended: unattended_says(env::var("UNATTENDED").as_deref()),
            fence: None,
            fence_switch_withheld: false,
            summary: Some(&summary),
            shell: Some(platform_shell()),
        },
    )
    .map_err(Stop::Failed)?;
    Ok(focus_lifecycle::guard_series::refusal_output(
        wf,
        &refusal,
        &snapshot.attempt,
        &resources.unwrap_or(snapshot.resources),
    ))
}

/// A summary's coverage, in the evaluator's words.
pub(crate) fn guard_coverage(
    coverage: summary_evidence::Coverage,
) -> focus_lifecycle::guard_refusal::Coverage {
    match coverage {
        summary_evidence::Coverage::Missing => focus_lifecycle::guard_refusal::Coverage::Missing,
        summary_evidence::Coverage::Stale => focus_lifecycle::guard_refusal::Coverage::Stale,
    }
}

/// The invariant a review's summary admission holds.
pub(crate) const SUMMARY_INVARIANT: &str =
    "Generated outputs descend from a current human-backed summary confirmation.";

/// The shell a command is quoted for: this platform's.
pub(crate) fn platform_shell() -> RenderShell {
    if cfg!(windows) {
        RenderShell::PowerShell
    } else {
        RenderShell::Posix
    }
}

/// What the person asked on the command line.
pub(crate) struct Asked<'a> {
    pub(crate) stage: &'a str,
    pub(crate) reviewer: &'a str,
    pub(crate) single: bool,
    pub(crate) retry_pending: bool,
    pub(crate) iteration: f64,
    pub(crate) project_dir: String,
    /// `--review-file`, as given: the verdict's way of naming the review
    /// file. A request reads none.
    pub(crate) review_file: Option<&'a str>,
}

impl Asked<'_> {
    pub(crate) fn command(&self, iteration: f64) -> ReviewCommand<'_> {
        ReviewCommand {
            project_dir: &self.project_dir,
            stage: self.stage,
            reviewer: self.reviewer,
            unit: None,
            single: self.single,
            iteration,
            retry_pending: false,
        }
    }
}

/// What a request that was written says back.
#[derive(Default)]
struct Written {
    retried: bool,
    upgraded: bool,
    recovery: bool,
    replaces: Option<String>,
    request_id: String,
    review_file: String,
    /// What the Guard Policy let through, said once.
    notices: Vec<String>,
}

/// `reviewBudgetMessage`.
fn budget_message(stage: &str, ordinal: f64, budget: f64) -> String {
    #[allow(clippy::float_cmp)]
    let one = budget == 1.0;
    format!(
        "Cannot request review pass {} for \"{stage}\" because this stage allows {} review pass{}. {}",
        num::to_string(ordinal),
        num::to_string(budget),
        if one { "" } else { "es" },
        if one {
            "Do not ask the reviewer again; include the findings in the approval summary for the human."
        } else {
            "Present the unresolved findings at the approval gate for the human instead of starting another review."
        }
    )
}

/// `reviewRecoveryAlreadyRequestedMessage`.
fn already_requested_message(stage: &str, iteration: f64, guidance: &str, valid: bool) -> String {
    format!(
        "Cannot request another recovery review for \"{stage}\" because one already exists in this review attempt. If the reviewer has not returned, retry iteration {} with --retry-pending. If its verdict was recorded, {guidance}{}",
        num::to_string(iteration),
        if valid {
            " Only a human Request Changes decision resets the review attempt; do not record that rejection on the human's behalf."
        } else {
            ""
        }
    )
}

/// `reviewSummaryEvidenceMessage`: the summary's own refusal, said of a
/// review.
fn summary_evidence_message(stage: &str, message: &str) -> String {
    let cause = re::replace(
        r#"^Refusing to (?:complete|continue) "[^"]+"(?: for unit "[^"]+")?:\s*"#,
        "",
        message,
        "",
    );
    format!("Cannot start review for \"{stage}\": {cause}")
}

/// `reviewRecoveryGuidance`: the way on a spent recovery ends with; a stage
/// no operation may name gets the plain restart.
fn guidance(wf: &Workflow, state: &str, stage: &str) -> String {
    let unattended = unattended_says(env::var("UNATTENDED").as_deref());
    recovery_guidance(wf, state, stage, None, unattended, None).unwrap_or_else(|_| {
        format!(
            "Restart this stage cleanly with /focus --stage {stage}, then confirm its summary and review the finished output again."
        )
    })
}

/// `personAskedForReview`: the person spoke since the last decision, after
/// the last request for this review. The pass cap and the one recovery
/// bound only the reviews the agent starts on its own.
fn person_asked(wf: &Workflow, stage: &str, reviewer: &str) -> bool {
    if !presence::person_spoke_since_gate(wf, false, true, false) {
        return false;
    }
    let rows = gate::sort_attempt_events(&wf.events());
    let mut turn: Option<&ShardEvent> = None;
    let mut request: Option<&ShardEvent> = None;
    for row in &rows {
        if presence::is_request_turn(&row.event, &row.block) {
            turn = Some(row);
        } else if row.event == "REVIEW_REQUESTED"
            && block_field(&row.block, "Stage") == Some(stage)
            && block_field(&row.block, "Reviewer") == Some(reviewer)
            && block_field(&row.block, "Unit").is_none_or(str::is_empty)
        {
            request = Some(row);
        }
    }
    turn.is_some_and(|turn| request.is_none_or(|request| definitely_before(request, turn)))
}

/// `removeRecordFileNoFollow` and the slot's folder made: the reviewer's
/// file for this request, opened. Any draft an earlier dispatch of the same
/// pass left behind is not this dispatch's review. Never through a link: a
/// redirected slot is not this record's.
fn open_slot(record: &Path, relative: &str) -> Result<()> {
    observer::refuse_write("removeRecordFileNoFollow")?;
    let anchor = std::fs::canonicalize(record)
        .map_err(|err| EngineError::new(format!("{}: {err}", record.display())))?;
    links::refuse_link_in_chain(&anchor, Path::new(relative))?;
    let path = anchor.join(relative);
    if let Err(err) = std::fs::remove_file(&path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        bail!("{}: {err}", path.display());
    }
    links::refuse_link_in_chain(&anchor, Path::new(relative))?;
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)
            .map_err(|err| EngineError::new(format!("{}: {err}", folder.display())))?;
    }
    links::refuse_link_in_chain(&anchor, Path::new(relative))
}

/// `mintReviewRequestId`.
fn mint_request_id() -> Result<String> {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        bail!("no source of random bytes for a review request id");
    }
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    Ok(format!("review:{hex}"))
}

/// Why a review command refused, and what the Guard Policy had let through
/// by then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub message: String,
    /// The changes a verdict accepted and recorded before it was refused:
    /// said with the refusal, because the ledger already holds them and a
    /// second try will not say them again (`emitError`'s `changeNotices`).
    /// A request's refusal carries none, as upstream's carries none.
    pub change_notices: Vec<String>,
}

/// `handleReview`.
///
/// # Errors
///
/// What upstream refuses, in its words; and what is not ported, by name.
pub fn review(
    open: &dyn Fn() -> Result<Workflow>,
    args: &[String],
) -> std::result::Result<Value, Refused> {
    let mut notices = Vec::new();
    asked(open, args, &mut notices).map_err(|refusal| Refused {
        message: refusal.to_string(),
        change_notices: notices,
    })
}

/// The command; `notices` takes what a verdict's Guard Policy let through,
/// whether or not the verdict is then recorded.
fn asked(
    open: &dyn Fn() -> Result<Workflow>,
    args: &[String],
    notices: &mut Vec<String>,
) -> Result<Value> {
    let (_, flags) = flags::parse(args)?;
    let Some(stage) = flags.given("stage") else {
        bail!("Missing --stage <slug>");
    };
    let Some(reviewer) = flags.given("reviewer") else {
        bail!("Missing --reviewer <agent>");
    };
    if flags.given("intent").is_some() || flags.given("space").is_some() {
        bail!(
            "The review command does not accept --intent/--space selectors. Switch to the target workspace first."
        );
    }
    let wf = open()?;
    if flags.given("unit").is_some() {
        bail!(
            "log review --unit is not ported yet: Focus records the review of a stage, and not yet the review of one unit (its claim, the plan of units, the unit's source and its manifest)."
        );
    }
    if wf.selection.intent.is_none() {
        bail!("Cannot resolve the active intent for review logging.");
    }
    if let Some(said) = flags.get("verdict") {
        return record_verdict(&wf, &flags, stage, reviewer, said, notices);
    }
    let Some(pass) = flags
        .given("iteration")
        .filter(|pass| re::test("^[1-9][0-9]*$", "", pass))
    else {
        bail!("Starting a review requires --iteration <positive integer>.");
    };
    let asked = Asked {
        stage,
        reviewer,
        single: flags.get("single") == Some("true"),
        retry_pending: flags.get("retry-pending") == Some("true"),
        // (`Number` of a run of digits: past what a number holds exactly it
        // rounds, as the parse does.)
        iteration: pass.parse::<f64>().unwrap_or(f64::INFINITY),
        project_dir: wf.layout.project.to_string_lossy().into_owned(),
        review_file: None,
    };
    // Construction walks its units from Units Generation's units block, so
    // a block the engine cannot read is fixed before the review, while the
    // document can still change. Writing it is the agent's step, so it comes
    // back as a print with the same request to run again.
    if stage == "units-generation"
        && !asked.single
        && let Dag::Malformed { reason, detail } = artifacts::resolve_bolt_dag(&wf)
    {
        let again = request_command(
            &wf,
            &ReviewCommand {
                retry_pending: asked.retry_pending,
                ..asked.command(asked.iteration)
            },
            platform_shell(),
        );
        return Ok(json!({
            "kind": "print",
            "message": format!("{} Then run `{again}` again.", units_block_repair(&reason, &detail)),
        }));
    }

    let mut fields: Vec<(&'static str, String)> = vec![
        ("Stage", stage.to_owned()),
        ("Reviewer", reviewer.to_owned()),
    ];
    if asked.single {
        fields.push(("Workflow", format!("single-stage:{stage}")));
    }
    fields.push(("Iteration", pass.to_owned()));
    let written = match request(&wf, &asked, fields) {
        Ok(written) => written,
        Err(Stop::Refusal(message)) => bail!("{message}"),
        Err(Stop::Failed(message)) => bail!("Audit emission failed: {message}"),
    };

    // A request is half of the exchange: the slot stays open until the same
    // command runs again with `--verdict`. So the request hands back the
    // exact command that closes it.
    let record_verdict = verdict_command(&wf, &asked.command(asked.iteration), platform_shell());
    let mut out = serde_json::Map::new();
    out.insert("emitted".to_owned(), json!("REVIEW_REQUESTED"));
    out.insert("stage".to_owned(), json!(stage));
    out.insert(
        "agent_note".to_owned(),
        json!(request_note(
            reviewer,
            &written.review_file,
            &record_verdict
        )),
    );
    if written.retried {
        out.insert("retry".to_owned(), json!("pending-request"));
    }
    if written.upgraded {
        out.insert("upgrade".to_owned(), json!("legacy-request"));
    }
    if written.recovery {
        out.insert("recovery".to_owned(), json!("stale-receipt"));
    }
    if let Some(replaces) = &written.replaces {
        out.insert("replaces".to_owned(), json!(replaces));
    }
    out.insert("requestId".to_owned(), json!(written.request_id));
    out.insert("reviewFile".to_owned(), json!(written.review_file));
    out.insert("recordVerdict".to_owned(), json!(record_verdict));
    if !written.notices.is_empty() {
        out.insert("change_notices".to_owned(), json!(written.notices));
    }
    Ok(Value::Object(out))
}

/// The verdict of a review: the flags, then
/// [`crate::review_verdict::verdict`] under the lock, then what closes the
/// reviewer's window.
fn record_verdict(
    wf: &Workflow,
    flags: &flags::Flags,
    stage: &str,
    reviewer: &str,
    said: &str,
    notices: &mut Vec<String>,
) -> Result<Value> {
    if flags.get("retry-pending") == Some("true") {
        bail!(
            "--retry-pending cannot be combined with --verdict. Run the request with --retry-pending first (no --verdict), dispatch the reviewer again, then record its verdict with --verdict."
        );
    }
    let Some(pass) = flags
        .given("iteration")
        .filter(|pass| re::test("^[1-9][0-9]*$", "", pass))
    else {
        bail!("Recording a review verdict requires --iteration <positive integer>.");
    };
    let verdict = focus_js::text::to_upper_case(said);
    if !matches!(verdict.as_str(), "READY" | "NOT-READY") {
        bail!("Unknown --verdict \"{said}\". Accepted: READY, NOT-READY.");
    }
    let asked = Asked {
        stage,
        reviewer,
        single: flags.get("single") == Some("true"),
        retry_pending: false,
        iteration: pass.parse::<f64>().unwrap_or(f64::INFINITY),
        project_dir: wf.layout.project.to_string_lossy().into_owned(),
        review_file: flags.get("review-file"),
    };
    let mut fields: Vec<(&'static str, String)> = vec![
        ("Stage", stage.to_owned()),
        ("Reviewer", reviewer.to_owned()),
    ];
    if asked.single {
        fields.push(("Workflow", format!("single-stage:{stage}")));
    }
    fields.push(("Iteration", pass.to_owned()));
    fields.push(("Verdict", verdict.clone()));
    let recorded = match crate::review_verdict::verdict(wf, &asked, &verdict, fields, notices) {
        Ok(recorded) => recorded,
        Err(Stop::Refusal(message)) => bail!("{message}"),
        Err(Stop::Failed(message)) => bail!("Audit emission failed: {message}"),
    };
    // The verdict closes the window the reviewer's scope is enforced in, so
    // the dispatch record the conductor wrote goes with it. Best effort:
    // the verdict is recorded either way.
    let _ = std::fs::remove_file(
        wf.selection
            .engine_dir(&wf.layout)
            .join("reviewer-dispatch.json"),
    );
    let mut out = serde_json::Map::new();
    out.insert("emitted".to_owned(), json!("REVIEW_COMPLETED"));
    out.insert("stage".to_owned(), json!(stage));
    out.insert("reviewRecord".to_owned(), json!(recorded.review_record));
    if let Some(copy) = &recorded.review_markdown {
        out.insert("reviewMarkdown".to_owned(), json!(copy));
    }
    if !notices.is_empty() {
        out.insert("change_notices".to_owned(), json!(notices));
    }
    Ok(Value::Object(out))
}

/// Sets `name` among the row's fields, where it already is or at the end
/// (an object's key keeps its place when it is set again).
pub(crate) fn set(fields: &mut Vec<(&'static str, String)>, name: &'static str, value: String) {
    match fields.iter_mut().find(|(known, _)| *known == name) {
        Some(field) => field.1 = value,
        None => fields.push((name, value)),
    }
}

/// What a request and a verdict both read before anything else
/// (`loadContext`): the stage and its declared reviewer, the attempt's
/// window and count, and whether the outputs a stage owes are asked for.
pub(crate) struct Context<'w> {
    pub(crate) state: String,
    pub(crate) node: &'w focus_graph::runtime::Stage,
    pub(crate) view: review_attempt::Window,
    pub(crate) record: Option<std::path::PathBuf>,
    pub(crate) workflow: Option<String>,
    pub(crate) attempt: Accounting,
    pub(crate) class: &'static str,
    pub(crate) merged: std::collections::BTreeSet<String>,
    pub(crate) require_required_artifacts: bool,
}

/// `loadContext`, without the receipts only a request scans and without
/// the admissibility of a unit, which no ported command names.
pub(crate) fn load_context<'w>(
    wf: &'w Workflow,
    slug: &str,
    reviewer: &str,
    single: bool,
) -> Step<Context<'w>> {
    let state = wf.read_state()?;
    let Some(node) = wf
        .install
        .stage(slug)
        .filter(|node| stage::reviewer(node).is_some())
    else {
        return refuse(format!(
            "Cannot record review: stage \"{slug}\" has no declared reviewer."
        ));
    };
    if stage::review_artifact(node).is_none_or(str::is_empty) {
        return refuse(format!(
            "Cannot record review: stage \"{slug}\" has no declared review_artifact."
        ));
    }
    let declared = stage::reviewer(node).unwrap_or_default();
    if reviewer != declared {
        return refuse(format!(
            "Cannot record review for \"{slug}\": reviewer \"{reviewer}\" does not match the declared reviewer \"{declared}\"."
        ));
    }
    let per_unit = stage::has_unit_marker(node);
    let dag = per_unit.then(|| artifacts::resolve_bolt_dag(wf));
    let view = review_attempt::window(wf, &state, node, wf.events());
    let record = wf.record_dir();
    let workflow = single.then(|| format!("single-stage:{slug}"));
    let attempt: Accounting = review_attempt::accounting(
        &view,
        &state,
        &Reviewed {
            slug,
            per_unit,
            workspace_requires: stage::workspace_requires(node),
            reviewer,
            unit: None,
            workflow: workflow.as_deref(),
        },
        &wf.install.graph,
        &|named| focus_evidence::review_record::read(record.as_deref(), named),
    )
    .map_err(Stop::Failed)?;
    if let Some(ambiguity) = &attempt.ambiguity {
        return refuse(format!(
            "Cannot record review for \"{slug}\": {ambiguity} makes the current review attempt chronology ambiguous. Record a fresh stage/jump boundary, then request the review again."
        ));
    }
    let class = admission::resolve_review_class(wf, node, &state);
    let merged: std::collections::BTreeSet<String> =
        view.merged_bolt_units.iter().cloned().collect();
    let require_required_artifacts = !admission::artifact_guard_disabled(&wf.layout)
        && !(dag
            .as_ref()
            .is_some_and(|dag| !matches!(dag, Dag::Ok { .. }))
            && merged.is_empty());
    Ok(Context {
        state,
        node,
        view,
        record,
        workflow,
        attempt,
        class,
        merged,
        require_required_artifacts,
    })
}

/// The request, under the audit lock: the ordinal is counted and the row
/// appended in one hold, so two requests cannot both take a pass.
#[allow(clippy::too_many_lines)]
fn request(
    wf: &Workflow,
    asked: &Asked<'_>,
    mut fields: Vec<(&'static str, String)>,
) -> Step<Written> {
    let held = wf.lock("log review")?;
    let Asked {
        stage: slug,
        reviewer,
        single,
        retry_pending,
        iteration,
        ..
    } = *asked;
    let shell = platform_shell();
    let Context {
        state,
        node,
        view,
        record,
        workflow,
        attempt,
        class,
        merged,
        require_required_artifacts,
    } = load_context(wf, slug, reviewer, single)?;
    let budget = match class {
        "none" => 0.0,
        "advisory" => 1.0,
        _ => node
            .raw
            .get("reviewer_max_iterations")
            .and_then(Value::as_f64)
            .unwrap_or(2.0),
    };
    let receipts = review_receipts::fresh(
        wf,
        &state,
        node,
        ReceiptOptions {
            review_class: Some(class),
            window: Some(&view),
            ..ReceiptOptions::default()
        },
    )?;
    let artifact_options = ArtifactOptions {
        require_required_artifacts,
        merged_bolt_units: Some(&merged),
    };

    // --- The summary's admission ---------------------------------------
    let summary = summary_evidence::check(
        wf,
        node,
        SummaryAsked {
            workflow: workflow.as_deref(),
            state: Some(&state),
            scope: None,
            unit: None,
        },
    )?;
    let pending: Option<Status> = review_pending::pending_review_request_status(
        wf,
        node,
        None,
        &attempt,
        PendingAsked {
            require_required_artifacts,
            merged_bolt_units: Some(&merged),
            single,
            source_state: None,
        },
    )?;
    // What the Guard Policy did is recorded, not passed over in silence.
    let summary_read = match &summary {
        Evidence::Ok {
            change_control_read,
            ..
        }
        | Evidence::Refused {
            change_control_read,
            ..
        } => *change_control_read,
    };
    let mut notices = Vec::new();
    if receipts.change_control_read || summary_read {
        change_control::governed_guard_policy(wf, &held, &state)?;
        let mut accepted: Vec<Accepted<'_>> = receipts
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
        if let Evidence::Ok {
            accepted_changes, ..
        } = &summary
        {
            accepted.extend(accepted_changes.iter().map(|change| Accepted {
                checkpoint: "summary-confirmation",
                stage: &change.stage,
                unit: change.unit.as_deref(),
                changed: Some(&change.changed),
                recorded: &change.recorded,
                current: &change.current,
                notice: &change.notice,
            }));
        }
        notices = change_control::record_accepted_changes(wf, &held, &accepted)?;
    }
    if let Evidence::Refused {
        code,
        message,
        coverage,
        ..
    } = &summary
    {
        return refuse(guard_text(
            wf,
            asked,
            &state,
            node,
            &GuardSaid {
                code,
                blocked_action: "review-request",
                invariant: SUMMARY_INVARIANT,
                message: &summary_evidence_message(slug, message),
            },
            focus_lifecycle::guard_attempt::Options {
                receipts: Some(receipts.clone()),
                summary_coverage: Some(guard_coverage(*coverage)),
                review_budget: Some(budget),
                pending_status: Some(pending),
                accounting: Some(&attempt),
                require_required_artifacts: Some(require_required_artifacts),
                ..Default::default()
            },
            None,
        )?);
    }

    // --- What is stale, and what the one recovery is --------------------
    let expected = attempt.request_count as f64 + 1.0;
    let same_scope = receipts.newest_source_unit.is_none();
    let source_stale = same_scope && receipts.source_stale;
    let artifact_stale = receipts.stage_stale;
    let scope_stale = env::var("SKIP_SOURCE_FRESHNESS").as_deref() != Some("1")
        && !single
        && (source_stale || artifact_stale);
    let source_recovery_spent = source_stale
        && (receipts.source_recovery_spent
            || receipts
                .source_stale_progress
                .is_some_and(|progress| progress.recovery_spent));
    let recovery_spent = attempt.recovery_spent || source_recovery_spent;
    // A review the person asked for is never refused for want of passes.
    let mut asked_for: Option<bool> = None;
    let mut person = || *asked_for.get_or_insert_with(|| person_asked(wf, slug, reviewer));
    let again = |pass: f64| request_command(wf, &asked.command(pass), shell);
    let start_again = |pass: f64| {
        #[allow(clippy::float_cmp)]
        let replaceable =
            pending.is_some_and(|status| status.iteration == pass && status.replaceable);
        if replaceable {
            format!(
                " It never got a verdict, so request it again instead: `{}`.",
                again(pass)
            )
        } else {
            String::new()
        }
    };
    let spent = || {
        recovery_spent_message(
            slug,
            Some(&guidance(wf, &state, slug)),
            request_changes_reset_is_executable(&state, slug),
        )
    };
    let already_requested = || {
        already_requested_message(
            slug,
            attempt.recovery_iteration.unwrap_or(iteration),
            &guidance(wf, &state, slug),
            request_changes_reset_is_executable(&state, slug),
        )
    };
    // A guard of the attempt refuses with its question: the ways on are
    // read off the attempt as the command has it in hand.
    let attempt_guard = |code: &str, invariant: &str, message: String| -> Step<String> {
        guard_text(
            wf,
            asked,
            &state,
            node,
            &GuardSaid {
                code,
                blocked_action: "review-request",
                invariant,
                message: &message,
            },
            focus_lifecycle::guard_attempt::Options {
                receipts: Some(receipts.clone()),
                review_budget: Some(budget),
                pending_status: Some(pending),
                accounting: Some(&attempt),
                ..Default::default()
            },
            Some(vec![
                format!(
                    "source:{}",
                    receipts
                        .newest_source_fingerprint
                        .as_deref()
                        .unwrap_or("none")
                ),
                format!("artifact-stale:{artifact_stale}"),
            ]),
        )
    };
    let spent_guard = || {
        attempt_guard(
            "REVIEW_RECOVERY_SPENT",
            "The stale-receipt recovery slot is single-use within an attempt.",
            spent(),
        )
    };
    let already_guard = || {
        attempt_guard(
            "REVIEW_RECOVERY_ALREADY_REQUESTED",
            "Only one stale-receipt recovery request exists in an attempt.",
            already_requested(),
        )
    };
    let attempt_id = review_paths::attempt_id(&attempt.floor);
    // The reviewer's slot for this request, opened; the row names it.
    let open_draft = |fields: &mut Vec<(&'static str, String)>, id: &str| -> Step<String> {
        let Some(record) = record.as_deref() else {
            return refuse("Cannot resolve the active intent record.".to_owned());
        };
        let draft = review_paths::draft_relative_path(slug, None, &attempt_id, iteration, Some(id))
            .map_err(Stop::Failed)?;
        if let Err(error) = open_slot(record, &draft) {
            return refuse(format!(
                "Cannot start review for \"{slug}\": the review slot {draft} cannot be opened ({error})."
            ));
        }
        set(fields, "Review File", draft.clone());
        let from_project = focus_js::path::relative(
            focus_js::path::Flavour::host(),
            &asked.project_dir,
            &asked.project_dir,
            &record.join(&draft).to_string_lossy(),
        )
        .replace('\\', "/");
        Ok(from_project)
    };
    // The source the request is bound to, with the listing kept behind the
    // fingerprint so a change after the review can be told as paths.
    let stamp_source = |fields: &mut Vec<(&'static str, String)>| -> Step<()> {
        if !stage::workspace_requires(node) {
            return Ok(());
        }
        let source = review_pending::current_source(wf)?;
        set(
            fields,
            "Source Fingerprint",
            source.as_ref().map_or_else(
                || UNBINDABLE.to_owned(),
                |source| source.fingerprint.clone(),
            ),
        );
        if let Some(source) = &source {
            snapshot::write_workspace_snapshot(&wf.layout, &wf.selection, slug, source)?;
        }
        Ok(())
    };
    let emit = |fields: &[(&'static str, String)]| -> Step<()> {
        Ok(wf.emit_plain(&held, "REVIEW_REQUESTED", fields)?)
    };
    let pending_of = |pass: f64| -> Option<&review_attempt::PendingRequest> {
        #[allow(clippy::float_cmp)]
        attempt
            .pending_requests
            .iter()
            .find(|(known, _)| *known == pass)
            .map(|(_, request)| request)
    };

    // --- The same pass, dispatched again --------------------------------
    if retry_pending {
        let Some(pending_request) = pending_of(iteration) else {
            if scope_stale {
                if recovery_spent {
                    return refuse(spent_guard()?);
                }
                return refuse(format!(
                    "Cannot retry the prior review for \"{slug}\" because it completed before the stage output or project source changed. Start the one recovery pass with `{}`.",
                    again(expected)
                ));
            }
            if recovery_spent {
                return refuse(already_guard()?);
            }
            return refuse(format!(
                "Cannot retry review iteration {} for \"{slug}\" because no pending request with that number exists. Start the expected review pass instead.",
                num::to_string(iteration)
            ));
        };
        let binding: &RequestBinding = &pending_request.binding;
        if pending_request.retried {
            return refuse(format!(
                "Refusing review retry for \"{slug}\": REVIEW_REQUESTED iteration {} already used its one pending-request retry. Do not dispatch it again; record the bounded incomplete-review NOT-READY fallback or start the next permitted review iteration.",
                num::to_string(iteration)
            ));
        }
        let Some(taken) = review_artifacts::snapshot(wf, node, None, artifact_options) else {
            return refuse(format!(
                "Cannot retry review for \"{slug}\": the declared artifact set could not be captured as one stable snapshot. Restore regular artifact files and retry."
            ));
        };
        if !review_artifacts::request_artifacts_current(binding, &taken)
            && !review_artifacts::appended_after_request(binding, &taken)
        {
            return refuse(format!(
                "Refusing review retry for \"{slug}\": declared artifacts no longer match the bytes from REVIEW_REQUESTED iteration {}. A retry re-dispatches that exact request and cannot rebaseline changed content. Restore the requested artifact bytes before retrying.{}",
                num::to_string(iteration),
                start_again(iteration)
            ));
        }
        // A request written before review records, or before source binding
        // on a stage that writes the workspace, is modernized by this one
        // retry: it gains a request id and the source fingerprint, keeps
        // its artifact fingerprint, and is marked so the ledger shows it.
        let workspace = stage::workspace_requires(node);
        let mut upgraded =
            binding.request_id.is_none() || (workspace && binding.source_fingerprint.is_none());
        if workspace {
            stamp_source(&mut fields)?;
            let current = fields
                .iter()
                .find(|(name, _)| *name == "Source Fingerprint")
                .map(|(_, value)| value.clone());
            if binding.source_fingerprint.is_some()
                && !same_workspace_source(binding.source_fingerprint.as_deref(), current.as_deref())
            {
                return refuse(format!(
                    "Refusing review retry for \"{slug}\": workspace source no longer matches REVIEW_REQUESTED iteration {}. A retry cannot rebaseline source changed while review was pending.{}",
                    num::to_string(iteration),
                    start_again(iteration)
                ));
            }
        }
        if binding
            .legacy_appendix
            .as_ref()
            .is_some_and(|legacy| legacy.prior_appendix && legacy.challenge.is_none())
        {
            upgraded = true;
        }
        let request_id = match &binding.request_id {
            Some(id) => id.clone(),
            None => mint_request_id()?,
        };
        set(&mut fields, "Retry", "pending-request".to_owned());
        set(
            &mut fields,
            "Artifact Fingerprint",
            binding.artifact_fingerprint.clone(),
        );
        set(&mut fields, "Request Id", request_id.clone());
        if let Some(legacy) = &binding.legacy_appendix {
            // The retry pairs with the original request under the legacy
            // matcher: its appendix binding is echoed unchanged.
            for (name, value) in review_paths::legacy_appendix_echo_fields(legacy) {
                set(&mut fields, name, value);
            }
        }
        if let Some(source) = &binding.source_fingerprint {
            set(&mut fields, "Source Fingerprint", source.clone());
        }
        if let Some(cause) = &binding.recovery_cause {
            set(&mut fields, "Recovery Cause", cause.clone());
        }
        if upgraded {
            set(&mut fields, "Upgrade", "legacy-request".to_owned());
        }
        let review_file = open_draft(&mut fields, &request_id)?;
        emit(&fields)?;
        return Ok(Written {
            retried: true,
            upgraded,
            request_id,
            review_file,
            notices,
            ..Written::default()
        });
    }

    // --- A pending request that can never finish is replaced ------------
    if let Some(replaced_pass) = pending
        .filter(|status| status.replaceable)
        .map(|status| status.iteration)
    {
        #[allow(clippy::float_cmp)]
        if iteration != replaced_pass {
            return refuse(format!(
                "Cannot start review iteration {} for \"{slug}\": iteration {} never got a verdict and its outputs or source changed since, so it is requested again as iteration {}: `{}`.",
                num::to_string(iteration),
                num::to_string(replaced_pass),
                num::to_string(replaced_pass),
                again(replaced_pass)
            ));
        }
        let Some(taken) = review_artifacts::snapshot(wf, node, None, artifact_options) else {
            return refuse(format!(
                "Cannot start review for \"{slug}\": a required output document is missing or unreadable. Create every required output document for this stage, then retry the review."
            ));
        };
        let replaced = pending_of(replaced_pass).map(|request| &request.binding);
        let replaces = replaced
            .and_then(|binding| binding.request_id.clone())
            .unwrap_or_else(|| "none".to_owned());
        set(
            &mut fields,
            "Artifact Fingerprint",
            taken.fingerprint.clone(),
        );
        let request_id = mint_request_id()?;
        set(&mut fields, "Request Id", request_id.clone());
        set(&mut fields, "Replaces Request Id", replaces.clone());
        // A replaced recovery request stays the attempt's recovery request.
        let cause = replaced
            .and_then(|binding| binding.recovery_cause.clone())
            .filter(|cause| !cause.is_empty());
        if let Some(cause) = &cause {
            set(&mut fields, "Recovery", "stale-receipt".to_owned());
            set(&mut fields, "Recovery Cause", cause.clone());
        }
        stamp_source(&mut fields)?;
        let review_file = open_draft(&mut fields, &request_id)?;
        emit(&fields)?;
        return Ok(Written {
            recovery: cause.is_some(),
            replaces: Some(replaces),
            request_id,
            review_file,
            notices,
            ..Written::default()
        });
    }

    // --- The next pass ---------------------------------------------------
    let recovery_eligible = scope_stale && attempt.pending_iterations.is_empty() && !recovery_spent;
    if scope_stale && recovery_spent && !person() {
        return refuse(spent_guard()?);
    }
    if recovery_spent && !person() {
        return refuse(already_guard()?);
    }
    // A pending request comes before the budget: the stage is waiting on
    // its verdict, not out of passes.
    if !attempt.pending_iterations.is_empty() {
        let mut waiting = attempt.pending_iterations.clone();
        waiting.sort_by(f64::total_cmp);
        let listed: Vec<String> = waiting.into_iter().map(num::to_string).collect();
        return refuse(attempt_guard(
            "REVIEW_VERDICT_PENDING",
            "A review request receives its verdict before another request starts.",
            format!(
                "Cannot start another review for \"{slug}\" because iteration {} is still waiting for a verdict. Record that verdict, or repeat the same iteration with --retry-pending if the reviewer did not run.",
                listed.join(", ")
            ),
        )?);
    }
    // The budget is measured against the attempt's own count, never against
    // the pass the caller claims.
    let pass = attempt.budget_count as f64 + 1.0;
    if !recovery_eligible && pass > budget && !person() {
        return refuse(attempt_guard(
            "REVIEW_BUDGET_EXHAUSTED",
            "Review requests do not exceed the configured attempt budget.",
            budget_message(slug, pass, budget),
        )?);
    }
    #[allow(clippy::float_cmp)]
    if iteration != expected {
        return refuse(format!(
            "Cannot start review iteration {} for \"{slug}\" because the next iteration is {}. Retry with --iteration {}.",
            num::to_string(iteration),
            num::to_string(expected),
            num::to_string(expected)
        ));
    }
    if recovery_eligible {
        set(&mut fields, "Recovery", "stale-receipt".to_owned());
        set(
            &mut fields,
            "Recovery Cause",
            if artifact_stale && source_stale {
                "artifact+source"
            } else if artifact_stale {
                "artifact"
            } else {
                "source"
            }
            .to_owned(),
        );
    }
    let Some(taken) = review_artifacts::snapshot(wf, node, None, artifact_options) else {
        return refuse(format!(
            "Cannot start review for \"{slug}\": a required output document is missing or unreadable. Create every required output document for this stage, then retry the review."
        ));
    };
    set(
        &mut fields,
        "Artifact Fingerprint",
        taken.fingerprint.clone(),
    );
    let request_id = mint_request_id()?;
    set(&mut fields, "Request Id", request_id.clone());
    stamp_source(&mut fields)?;
    let review_file = open_draft(&mut fields, &request_id)?;
    emit(&fields)?;
    Ok(Written {
        recovery: recovery_eligible,
        request_id,
        review_file,
        notices,
        ..Written::default()
    })
}

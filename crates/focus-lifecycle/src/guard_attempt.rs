//! What the current attempt of a stage looks like to a guard.
//!
//! `guardAttemptState` (`aidlc-lib.ts` L30 773): the one place the attempt
//! is said in the evaluator's words ([`crate::guard_refusal::Attempt`]).
//! It reads the readers of the attempt once (the window, the receipts that
//! still stand, the count of the attempt's reviews, what can be done with
//! the request that waits) and answers: the budget used, the one recovery,
//! the review in flight with its exact commands, and whether the summary,
//! the review and the source still cover what is on disk. With the attempt
//! go the receipts it was read from and the names of the resources whose
//! change is real progress, which the series of a refusal is signed with.
//!
//! A caller that already holds one of the readings hands it over, and it is
//! not read again.
//!
//! # `humanAuthorityState`
//!
//! Upstream's reader of the person's authority gives two answers:
//! `unattended`, which is [`crate::guard_refusal::unattended_says`], and
//! `freshTurn` (whether the person acted since the gate). Nothing upstream
//! reads `freshTurn`: the evaluator never asks for it. It is not computed
//! here.
//!
//! # What is not here yet
//!
//! While units are owned by a team, the count of a unit's reviews leaves
//! out the rows of an earlier claim of that unit
//! (`reviewAttemptEventMatchesCurrentClaim`). Team claims are deferred
//! with team ownership: an attempt asked of a unit whose rows would have
//! to be weighed that way is refused by name.
//!
//! The readers this one calls refuse what they do not read yet, each by
//! its own name (see [`crate::review_receipts`] and
//! [`crate::review_pending`]).
//!
//! # Tests that change no answer
//!
//! Mutating each condition here against the oracle corpus and the tests
//! leaves four that nothing can tell apart, each said again at its line:
//!
//! - **The project's source is shared by the two readers that weigh it.**
//!   Each takes it for itself when it is handed none, and reads the same.
//! - **The request that waits is not asked as a stage run by itself.**
//!   That matters only to a unit's request in a stage that writes code,
//!   which the receipts refuse one reader earlier.
//! - **A row of the unit's that says so in its `Unit` field**, and **only
//!   the rows the count reads**: under a team the receipts refuse any row
//!   with that field first, and the rows of a unit's own bolt are all rows
//!   the count reads.
//!
//! Two of upstream's tests are not written, because the receipts already
//! answer them: whether the source's stale progress says its recovery is
//! spent (the receipts fill it from the answer they give for the source,
//! which is asked), and whether the newest reviewed source's fingerprint
//! is the empty text (one they keep never is).
//!
//! # A stage with no phase
//!
//! Upstream's type lets a caller hand a partial stage with no `phase`, and
//! such a stage is not reviewable. A stage of the compiled graph always
//! has one, so the test is not written.

use focus_audit::read::block_field;
use focus_audit::units::bolt_slug_for_unit;
use focus_graph::runtime::Stage;
use focus_guards::guard_operation::RenderShell;
use focus_js::{EngineError, Result, bail};
use serde_json::{Map, Value, json};

use crate::guard_refusal::{Attempt, Coverage, NextReview, PendingReview, Recovery};
use crate::review_attempt::{
    self, ACCOUNTING_EVENTS, Accounting, Reviewed, Window, is_team_unit_ownership,
};
use crate::review_pending::{self, Asked as PendingAsked, Status};
use crate::review_receipts::{self, Options as ReceiptOptions, PendingState, Receipts};
use crate::review_request::{ReviewCommand, request_command, verdict_command};
use crate::workflow::Workflow;
use crate::{admission, stage};

/// What a caller already has in hand, and what it asks about.
#[derive(Debug, Clone, Default)]
pub struct Options<'a> {
    /// The unit the attempt is of; `None` for the stage's.
    pub unit: Option<&'a str>,
    /// The receipts, when the caller weighed them.
    pub receipts: Option<Receipts>,
    /// The attempt's window, when the caller took it.
    pub window: Option<&'a Window>,
    /// What the summary's check said; `None` is `current`.
    pub summary_coverage: Option<Coverage>,
    /// The passes the stage's review class allows, when the caller counted
    /// them: the budget is said only then.
    pub review_budget: Option<f64>,
    /// What can be done with the request that waits, when the caller asked:
    /// `Some(None)` says none was found.
    pub pending_status: Option<Option<Status>>,
    /// The count of the attempt's reviews, when the caller took it.
    pub accounting: Option<&'a Accounting>,
    /// Whether the stage's owed outputs are checked, when the caller
    /// decided; `None` reads the switch.
    pub require_required_artifacts: Option<bool>,
    /// The stage is run by itself: the commands say so.
    pub single: bool,
    /// The shell the commands are quoted for; `None` is this platform's.
    pub shell: Option<RenderShell>,
    /// Whether the check of the source's freshness is switched off; `None`
    /// reads the process's environment.
    pub skip_source_freshness: Option<bool>,
    /// Whether the check of a stage's owed outputs is switched off; `None`
    /// reads the environment and the project's settings.
    pub skip_artifact_guard: Option<bool>,
}

/// `GuardAttemptSnapshot`.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub attempt: Attempt,
    /// The receipts the attempt was read from; `None` for a stage nobody
    /// reviews, when the caller handed none.
    pub receipts: Option<Receipts>,
    /// The resources whose change means real progress, each as
    /// `<name>:<what it stands at>`.
    pub resources: Vec<String>,
}

/// The attempt as upstream's object, in the order its keys are written.
#[must_use]
pub fn attempt_value(attempt: &Attempt) -> Value {
    let coverage = |coverage: Coverage| match coverage {
        Coverage::Current => "current",
        Coverage::Stale => "stale",
        Coverage::Missing => "missing",
        Coverage::Unbindable => "unbindable",
    };
    let mut out = Map::new();
    if let Some(floor) = &attempt.floor {
        out.insert("floor".to_owned(), json!(floor));
    }
    if let Some((used, limit)) = attempt.review_budget {
        out.insert(
            "reviewBudget".to_owned(),
            json!({ "used": used, "limit": limit }),
        );
    }
    out.insert(
        "recovery".to_owned(),
        json!(match attempt.recovery {
            Recovery::Available => "available",
            Recovery::Pending => "pending",
            Recovery::Spent => "spent",
        }),
    );
    if let Some(pending) = &attempt.pending_review {
        let mut said = Map::new();
        said.insert("iteration".to_owned(), json!(pending.iteration));
        said.insert("retryable".to_owned(), json!(pending.retryable));
        if let Some(recordable) = pending.verdict_recordable {
            said.insert("verdictRecordable".to_owned(), json!(recordable));
        }
        if let Some(command) = &pending.record_verdict {
            said.insert("recordVerdict".to_owned(), json!(command));
        }
        out.insert("pendingReview".to_owned(), Value::Object(said));
    }
    if let Some(iteration) = attempt.repair_review {
        out.insert("repairReview".to_owned(), json!({ "iteration": iteration }));
    }
    if let Some(next) = &attempt.next_review {
        let mut said = Map::new();
        said.insert("iteration".to_owned(), json!(next.iteration));
        if let Some(request) = &next.request {
            said.insert("request".to_owned(), json!(request));
        }
        out.insert("nextReview".to_owned(), Value::Object(said));
    }
    if let Some(request) = &attempt.request_review {
        out.insert("requestReview".to_owned(), json!(request));
    }
    out.insert(
        "summaryCoverage".to_owned(),
        json!(coverage(attempt.summary_coverage)),
    );
    out.insert(
        "reviewCoverage".to_owned(),
        json!(coverage(attempt.review_coverage)),
    );
    out.insert(
        "sourceCoverage".to_owned(),
        json!(coverage(attempt.source_coverage)),
    );
    Value::Object(out)
}

/// `reviewAttemptEventMatchesCurrentClaim`: every row counts while units
/// are not owned by a team, or the attempt is the stage's.
///
/// # Errors
///
/// Units are owned by a team and a row is one of the unit's: whether it
/// belongs to the unit's current claim is not read yet.
fn refuse_team_claims(view: &Window, state: &str, unit: Option<&str>) -> Result<()> {
    let Some(unit) = unit else {
        return Ok(());
    };
    if !is_team_unit_ownership(state) {
        return Ok(());
    }
    let slug = bolt_slug_for_unit(unit).ok();
    // (A row that names the unit in its `Unit` field never gets this far
    // while the receipts are weighed here: they refuse it first, in their
    // own words. What does is the start or the end of the unit's own bolt,
    // which names it with no such field, and is always a row the count
    // reads.)
    let of_the_unit = |row: &focus_audit::shards::ShardEvent| {
        let field = |name: &str| block_field(&row.block, name);
        match field("Unit") {
            Some(named) => named == unit,
            None => {
                matches!(
                    row.event.as_str(),
                    "BOLT_STARTED" | "BOLT_COMPLETED" | "BOLT_FAILED"
                ) && field("Bolt names") == Some(unit)
                    && field("Bolt slug") == slug.as_deref()
            }
        }
    };
    if view
        .all_events
        .iter()
        .any(|row| ACCOUNTING_EVENTS.contains(&row.event.as_str()) && of_the_unit(row))
    {
        bail!(
            "The review attempt of unit \"{unit}\" is counted while units are owned by a team, and Focus does not read team claims yet: whether a row of the unit belongs to its current claim cannot be told."
        );
    }
    Ok(())
}

/// `guardAttemptState`: the attempt of `stage` (or of one unit of it) as a
/// guard sees it, for the piece of work `wf` selects. `project_dir` is the
/// project as the commands name it.
///
/// # Errors
///
/// As [`review_receipts::fresh`], [`review_attempt::accounting`] and
/// [`review_pending::pending_review_request_status`], for the readings the
/// caller did not hand over; and a unit's attempt while units are owned by
/// a team (see the module docs).
#[allow(clippy::too_many_lines)]
pub fn guard_attempt_state(
    wf: &Workflow,
    project_dir: &str,
    state: &str,
    stage: &Stage,
    options: Options<'_>,
) -> Result<Snapshot> {
    let Options {
        unit,
        receipts,
        window,
        summary_coverage,
        review_budget,
        pending_status,
        accounting,
        require_required_artifacts,
        single,
        shell,
        skip_source_freshness,
        skip_artifact_guard,
    } = options;
    let shell = shell.unwrap_or_else(crate::guard_refusal::platform_shell);
    let taken_window;
    let view = match window {
        Some(view) => view,
        None => {
            taken_window = review_attempt::window(wf, state, stage, wf.events());
            &taken_window
        }
    };
    // (A reviewer declared as the empty name is still a declaration here:
    // the attempt is counted, for nobody. Only the next request needs a
    // name to ask for.)
    let declared = stage.raw.get("reviewer").and_then(Value::as_str);
    let writes_workspace = stage::workspace_requires(stage);
    // The project's source is taken once, and shared by the two readers
    // that weigh it. (Each takes it for itself when it is not handed one,
    // and reads the same: sharing saves a walk and changes no answer.)
    let source = match declared {
        Some(_) if writes_workspace => Some(review_pending::current_source(wf)?),
        _ => None,
    };
    let source_state = source.as_ref().map(Option::as_ref);
    let receipts = match (receipts, declared) {
        (Some(given), _) => Some(given),
        (None, Some(_)) => Some(review_receipts::fresh(
            wf,
            state,
            stage,
            ReceiptOptions {
                review_class: Some(admission::resolve_review_class(wf, stage, state)),
                window: Some(view),
                source_state,
                skip_source_freshness,
                skip_artifact_guard,
            },
        )?),
        (None, None) => None,
    };
    let taken_accounting;
    let accounting: Option<&Accounting> = match (accounting, declared) {
        (Some(given), _) => Some(given),
        (None, Some(reviewer)) => {
            refuse_team_claims(view, state, unit)?;
            let record = wf.record_dir();
            taken_accounting = review_attempt::accounting(
                view,
                state,
                &Reviewed {
                    slug: &stage.slug,
                    per_unit: stage::has_unit_marker(stage),
                    workspace_requires: writes_workspace,
                    reviewer,
                    unit,
                    workflow: None,
                },
                &wf.install.graph,
                &|named| focus_evidence::review_record::read(record.as_deref(), named),
            )
            .map_err(EngineError::new)?;
            Some(&taken_accounting)
        }
        (None, None) => None,
    };
    let pending_status = match (pending_status, accounting) {
        (Some(given), _) => given,
        (None, None) => None,
        (None, Some(accounting)) => {
            let merged = view.merged_bolt_units.iter().cloned().collect();
            review_pending::pending_review_request_status(
                wf,
                stage,
                unit,
                accounting,
                PendingAsked {
                    require_required_artifacts: require_required_artifacts.unwrap_or_else(|| {
                        !skip_artifact_guard
                            .unwrap_or_else(|| admission::artifact_guard_disabled(&wf.layout))
                    }),
                    merged_bolt_units: Some(&merged),
                    // (Upstream does not say here that a stage is run by
                    // itself. Where that would matter, a unit's request in
                    // a stage that writes code, the receipts have refused
                    // the unit's own source one reader earlier.)
                    single: false,
                    source_state,
                },
            )?
        }
    };

    let of = receipts.as_ref();
    let verdict_stands = match unit {
        None => of.is_some_and(|receipts| receipts.stage_verdict.is_some()),
        Some(unit) => of.is_some_and(|receipts| receipts.unit_verdicts.get(unit).is_some()),
    };
    let stale = match unit {
        None => of.is_some_and(|receipts| receipts.stage_stale),
        Some(unit) => of.is_some_and(|receipts| receipts.unit_stale.has(unit)),
    };
    let pending = match unit {
        None => of.and_then(|receipts| receipts.stage_pending.as_ref()),
        Some(unit) => of.and_then(|receipts| receipts.unit_pending.get(unit)),
    };
    let stale_progress = match unit {
        None => of.and_then(|receipts| receipts.stage_stale_progress.as_ref()),
        Some(unit) => of.and_then(|receipts| receipts.unit_stale_progress.get(unit)),
    };
    // (Upstream also asks the source's stale progress whether its recovery
    // is spent. The receipts fill that from the answer they give for the
    // source as a whole, which is asked here.)
    let recovery_spent = accounting.is_some_and(|accounting| accounting.recovery_spent)
        || stale_progress.is_some_and(|progress| progress.recovery_spent)
        || of.is_some_and(|receipts| receipts.source_recovery_spent);
    let lowest_pending = accounting.and_then(|accounting| {
        accounting
            .pending_iterations
            .iter()
            .copied()
            .min_by(f64::total_cmp)
    });

    let command = |iteration: f64| ReviewCommand {
        project_dir,
        stage: &stage.slug,
        reviewer: declared.unwrap_or_default(),
        unit,
        single,
        iteration,
        retry_pending: false,
    };
    // The exact request for a pass, named in the way on that asks for it.
    let request_at = |iteration: f64| request_command(wf, &command(iteration), shell);
    let status_of = |iteration: f64| pending_status.filter(|status| status.iteration == iteration);
    let waits = |iteration: f64| PendingReview {
        iteration,
        retryable: status_of(iteration).is_none_or(|status| status.retryable),
        verdict_recordable: Some(
            status_of(iteration).is_none_or(|status| status.verdict_recordable),
        ),
        record_verdict: Some(verdict_command(wf, &command(iteration), shell)),
    };
    let next_at = |iteration: f64| NextReview {
        iteration,
        request: Some(request_at(iteration)),
    };
    // What is in flight: a repair the reviewer asked for, the next request,
    // or a request that waits. A pending request that can never finish (its
    // outputs or source changed before a verdict) is requested again at
    // the same pass.
    let replaced = |iteration: f64| status_of(iteration).is_some_and(|status| status.replaceable);
    let waiting = match pending {
        Some(pending) => Some(pending.iteration),
        None => lowest_pending,
    };
    let state = pending.map(|pending| pending.state);
    let repair_review = pending
        .filter(|_| state == Some(PendingState::RepairRequired))
        .map(|pending| pending.iteration);
    let asks_again = |iteration: f64| {
        state == Some(PendingState::Outstanding)
            || (state != Some(PendingState::RepairRequired) && replaced(iteration))
    };
    let next_review = waiting
        .filter(|iteration| asks_again(*iteration))
        .map(next_at);
    let pending_review = waiting
        .filter(|iteration| state != Some(PendingState::RepairRequired) && !asks_again(*iteration))
        .map(waits);

    let floor = match accounting {
        Some(accounting) => accounting.floor.clone(),
        None => view
            .floor_idx
            .and_then(|index| view.events.get(index))
            .map_or_else(String::new, |row| {
                format!("{}:{}:{}:{}", row.event, row.timestamp, row.shard, row.pos)
            }),
    };
    #[allow(clippy::cast_precision_loss)]
    let attempt = Attempt {
        floor: Some(floor),
        review_budget: review_budget
            .zip(accounting)
            .map(|(limit, accounting)| (accounting.budget_count as f64, limit)),
        recovery: if pending.is_some_and(|pending| pending.recovery) {
            Recovery::Pending
        } else if recovery_spent {
            Recovery::Spent
        } else {
            Recovery::Available
        },
        pending_review,
        repair_review,
        next_review,
        // The pass the review log expects next (its request count plus
        // one).
        request_review: accounting
            .filter(|_| declared.is_some_and(|reviewer| !reviewer.is_empty()))
            .map(|accounting| request_at(accounting.request_count as f64 + 1.0)),
        summary_coverage: summary_coverage.unwrap_or(Coverage::Current),
        review_coverage: if verdict_stands {
            Coverage::Current
        } else if stale || of.is_some_and(|receipts| receipts.source_stale) {
            Coverage::Stale
        } else {
            Coverage::Missing
        },
        source_coverage: match of {
            Some(receipts) if receipts.source_stale_reason == Some("boundary-unbindable") => {
                Coverage::Unbindable
            }
            Some(receipts) if receipts.source_stale => Coverage::Stale,
            // (A fingerprint the receipts keep is never the empty text.)
            Some(receipts) if receipts.newest_source_fingerprint.is_some() => Coverage::Current,
            _ => Coverage::Missing,
        },
    };
    let none = || "none".to_owned();
    let resources = vec![
        format!(
            "source:{}",
            of.and_then(|receipts| receipts.newest_source_fingerprint.clone())
                .unwrap_or_else(none)
        ),
        format!(
            "stage-review:{}",
            of.and_then(|receipts| receipts.stage_iteration)
                .map_or_else(none, focus_js::num::to_string)
        ),
        format!(
            "unit-review:{}",
            unit.and_then(|unit| of.and_then(|receipts| receipts.unit_iterations.get(unit)))
                .copied()
                .map_or_else(none, focus_js::num::to_string)
        ),
    ];
    Ok(Snapshot {
        attempt,
        receipts,
        resources,
    })
}

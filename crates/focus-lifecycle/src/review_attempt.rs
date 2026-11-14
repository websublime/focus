//! The attempt of a stage a review belongs to.
//!
//! A review is of one attempt of its stage: the work done since the stage
//! last started over. A request, its verdict and the receipts a gate weighs
//! all count from the same boundary, read off the ledger: the creation of
//! the work, a jump that reaches the stage, a rejection of its gate, a start
//! of the stage. For a stage that runs once for each unit of work the same
//! reading says which units have a bolt open and which have one merged
//! (`reviewAttemptWindow`, `aidlc-lib.ts` L18 300 to L18 528, with
//! `stageStartsUnderUnitFlooring`, L34 846).
//!
//! # Order
//!
//! Rows are read by instant, then by position inside a shard, then by the
//! order of the shards (`sortAttemptEvents`). Whether one row is before
//! another is a narrower question (`attemptEventDefinitelyBefore`): position
//! decides inside a shard and the instant decides across shards, so two rows
//! of different shards at one instant are neither before the other.
//!
//! # Tests that change no answer
//!
//! Upstream asks them and so does this module; the mutation sweep found no
//! ledger that tells them apart.
//!
//! - Bolt rows of one instant and the same kind are applied in the order of
//!   their shards: the queues are met in that order already, so the earlier
//!   shard is the one held.
//! - The starts recorded under unit-major flooring are worked out only when
//!   the walk is not unit-major now: they are asked about nowhere else.
//! - Among those starts, one already left out is not looked at again, and a
//!   start of a stage run by itself is passed over: neither is a boundary
//!   whatever the answer.
//! - A change of policy counts as between a start and a later change only
//!   when the start is plainly before it. A change the start is not plainly
//!   before is itself one the start is tested against, and fails that test,
//!   so the start is kept either way.

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_audit::units::bolt_slug_for_unit;
use focus_evidence::authorization::jump_reaches;
use focus_evidence::review::{self, RecordRef, RequestBinding};
use focus_evidence::review_record::{self, Record};
use focus_graph::runtime::Stage;
use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{num, re};
use focus_state::fields::get_field;

use crate::artifacts::uses_stage_level_per_unit_artifacts;
use crate::gate::sort_attempt_events;
use crate::workflow::Workflow;

/// `REVIEW_RECEIPT_EVENTS`: the rows a review's receipt is read against.
const RECEIPT_EVENTS: [&str; 14] = [
    "WORKFLOW_STARTED",
    "STAGE_STARTED",
    "STAGE_JUMPED",
    "GATE_REJECTED",
    "SESSION_STARTED",
    "SESSION_RESUMED",
    "BOLT_STARTED",
    "BOLT_COMPLETED",
    "BOLT_FAILED",
    "AUDIT_MERGED",
    "ARTIFACT_CREATED",
    "ARTIFACT_UPDATED",
    "REVIEW_REQUESTED",
    "REVIEW_COMPLETED",
];

/// `UNIT_LIFECYCLE_EVENTS`.
const UNIT_LIFECYCLE_EVENTS: [&str; 5] = [
    "UNIT_STARTED",
    "UNIT_PAUSED",
    "UNIT_RESUMED",
    "UNIT_COMPLETED",
    "UNIT_SKIPPED",
];

/// `AttemptView` (`ReviewAttemptWindow`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Every row of the ledger, as it was read.
    pub all_events: Vec<ShardEvent>,
    /// The receipt rows, in order.
    pub events: Vec<ShardEvent>,
    /// The boundary of the attempt in `events`; `None` when the ledger has
    /// none.
    pub floor_idx: Option<usize>,
    /// The units whose bolt of this attempt was merged, in the order found.
    pub merged_bolt_units: Vec<String>,
    /// The units whose bolt of this attempt is open, or completed and not
    /// merged yet.
    pub open_bolt_units: Vec<String>,
}

/// How the state says the units of a per-unit stage are walked.
struct Units {
    major: bool,
    team: bool,
    scoped_rejections: bool,
}

/// `isTeamUnitOwnership`.
#[must_use]
pub fn is_team_unit_ownership(state: &str) -> bool {
    get_field(state, "Unit Ownership") == Some("team")
}

fn checkpoints_enabled(state: &str) -> bool {
    get_field(state, "Construction Checkpoints") == Some("enabled")
}

fn iteration_is_unit_major(state: &str) -> bool {
    get_field(state, "Construction Iteration") == Some("unit-major")
}

/// `unitScopedLifecycleFloors`: whether a unit's receipts are floored for
/// the unit alone: units owned by a team, Construction checkpoints, and a
/// solo unit-major Construction.
#[must_use]
pub fn unit_scoped_lifecycle_floors(state: &str) -> bool {
    is_team_unit_ownership(state) || checkpoints_enabled(state) || iteration_is_unit_major(state)
}

/// `attemptEventDefinitelyBefore`.
#[must_use]
pub fn definitely_before(earlier: &ShardEvent, later: &ShardEvent) -> bool {
    if earlier.shard == later.shard {
        earlier.pos < later.pos
    } else {
        cmp_utf16(&earlier.timestamp, &later.timestamp) == std::cmp::Ordering::Less
    }
}

fn single_stage(block: &str) -> bool {
    block_field(block, "Workflow").is_some_and(|workflow| workflow.starts_with("single-stage:"))
}

/// A field that is there and says something (`!!auditBlockField(...)`).
fn said<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    block_field(block, name).filter(|value| !value.is_empty())
}

/// `constructionPolicyRowComplete`.
fn policy_row_complete(row: &ShardEvent) -> bool {
    matches!(
        block_field(&row.block, "Field"),
        Some("Construction Iteration" | "Construction Checkpoints" | "Construction Execution")
    ) && [
        "Value",
        "Previous Value",
        "Construction Iteration",
        "Construction Checkpoints",
    ]
    .iter()
    .all(|name| said(&row.block, name).is_some())
}

/// `constructionPolicyFoundUnitMajor`: whether a recorded change of policy
/// found unit-major flooring in force before it.
fn policy_found_unit_major(row: &ShardEvent) -> bool {
    let before = |field: &str| {
        if block_field(&row.block, "Field") == Some(field) {
            block_field(&row.block, "Previous Value")
        } else {
            block_field(&row.block, field)
        }
    };
    before("Construction Iteration") == Some("unit-major")
        || before("Construction Checkpoints") == Some("enabled")
}

/// `stageStartsUnderUnitFlooring`: the stage starts recorded while
/// unit-major flooring was in force, by their place in `rows`. A unit's
/// stage-major floor leaves them out, so a switch back keeps the units that
/// were finished under unit-major.
fn starts_under_unit_flooring(rows: &[ShardEvent], graph: &[Stage]) -> Vec<bool> {
    let mut ignored = vec![false; rows.len()];
    let changes: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| row.event == "CONSTRUCTION_POLICY_SET" && policy_row_complete(row))
        .collect();
    if changes.is_empty() {
        return ignored;
    }
    let before = definitely_before;
    // A start plainly before every change that may be the first after it,
    // all of which found unit-major flooring.
    for (index, start) in rows.iter().enumerate() {
        if start.event != "STAGE_STARTED" {
            continue;
        }
        let after: Vec<&ShardEvent> = changes
            .iter()
            .copied()
            .filter(|change| !before(change, start))
            .collect();
        if after.is_empty() {
            continue;
        }
        let first_after: Vec<&ShardEvent> = after
            .iter()
            .copied()
            .filter(|change| {
                !after
                    .iter()
                    .any(|other| before(start, other) && before(other, change))
            })
            .collect();
        let candidates = if first_after.is_empty() {
            &after
        } else {
            &first_after
        };
        if candidates
            .iter()
            .all(|change| before(start, change) && policy_found_unit_major(change))
        {
            ignored[index] = true;
        }
    }
    // The first start of a stage in its attempt, after a change that found
    // unit-major flooring and after rows of the stage's units: the walk
    // finished the units before the stage's own start was recorded.
    let unit_rows: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| UNIT_LIFECYCLE_EVENTS.contains(&row.event.as_str()))
        .collect();
    let boundaries: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| row.event == "WORKFLOW_STARTED" || row.event == "STAGE_JUMPED")
        .collect();
    for (index, start) in rows.iter().enumerate() {
        if start.event != "STAGE_STARTED" || ignored[index] {
            continue;
        }
        let Some(slug) = said(&start.block, "Stage") else {
            continue;
        };
        if single_stage(&start.block) {
            continue;
        }
        let opened = |row: &ShardEvent| {
            before(row, start)
                && !boundaries.iter().any(|boundary| {
                    (boundary.event == "WORKFLOW_STARTED"
                        || jump_reaches(&boundary.block, slug, graph))
                        && before(row, boundary)
                        && before(boundary, start)
                })
        };
        let first = !rows.iter().enumerate().any(|(other_index, other)| {
            other_index != index
                && other.event == "STAGE_STARTED"
                && block_field(&other.block, "Stage") == Some(slug)
                && !single_stage(&other.block)
                && opened(other)
        });
        if first
            && changes
                .iter()
                .any(|change| before(change, start) && policy_found_unit_major(change))
            && unit_rows
                .iter()
                .any(|row| block_field(&row.block, "Stage") == Some(slug) && opened(row))
        {
            ignored[index] = true;
        }
    }
    ignored
}

/// The units a bolt's row names (`boltEventUnits`).
fn bolt_units(event: &ShardEvent) -> Vec<&str> {
    let field = if event.event == "BOLT_FAILED" {
        "Failed Bolt"
    } else {
        "Bolt names"
    };
    block_field(&event.block, field)
        .unwrap_or_default()
        .split(',')
        .map(trim)
        .filter(|unit| !unit.is_empty())
        .collect()
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum BoltState {
    Open,
    Completed,
    Merged,
    Failed,
}

struct BoltAttempt {
    unit: String,
    slug: Option<String>,
    state: BoltState,
}

/// One bolt row applied to the attempts so far. A terminal row pairs with
/// its start by the bolt's slug when both carry one, and by the unit's name
/// otherwise. A completion alone ends a name-only attempt; one that involves
/// a slug waits for the merge that confirms it.
fn apply_bolt_event(attempts: &mut Vec<BoltAttempt>, event: &ShardEvent) {
    let slug = block_field(&event.block, "Bolt slug");
    if event.event == "AUDIT_MERGED" {
        // A merge confirms the newest completion of its slug that waits for
        // one: it revives no failed attempt and invents none.
        let Some(slug) = slug else {
            return;
        };
        if let Some(attempt) = attempts.iter_mut().rev().find(|attempt| {
            attempt.slug.as_deref() == Some(slug) && attempt.state == BoltState::Completed
        }) {
            attempt.state = BoltState::Merged;
        }
        return;
    }
    let units = bolt_units(event);
    if event.event == "BOLT_STARTED" {
        for unit in units {
            attempts.push(BoltAttempt {
                unit: unit.to_owned(),
                slug: slug.map(str::to_owned),
                state: BoltState::Open,
            });
        }
        return;
    }
    let completion = event.event == "BOLT_COMPLETED";
    for unit in units {
        let mut found: Option<usize> = None;
        if let Some(slug) = slug {
            found = attempts.iter().position(|attempt| {
                attempt.state == BoltState::Open && attempt.slug.as_deref() == Some(slug)
            });
            if found.is_none() {
                // A confirmed merge is final for its attempt.
                found = attempts.iter().rposition(|attempt| {
                    attempt.slug.as_deref() == Some(slug) && attempt.state != BoltState::Merged
                });
            }
        }
        if found.is_none() {
            // A completion with no slug pairs name-only attempts alone; a
            // failure with none still closes the newest open attempt.
            found = attempts.iter().position(|attempt| {
                attempt.state == BoltState::Open
                    && attempt.unit == unit
                    && ((slug.is_none() && !completion) || attempt.slug.is_none())
            });
        }
        if found.is_none() && slug.is_none() {
            found = attempts.iter().rposition(|attempt| {
                attempt.unit == unit
                    && attempt.state != BoltState::Merged
                    && (!completion || attempt.slug.is_none())
            });
        }
        let Some(found) = found else {
            continue;
        };
        let attempt = &mut attempts[found];
        if !completion {
            attempt.state = BoltState::Failed;
            continue;
        }
        if attempt.slug.is_none()
            && let Some(slug) = slug
        {
            // A completion that carries a slug binds it to a name-only
            // start, for the merge to confirm.
            attempt.slug = Some(slug.to_owned());
        }
        attempt.state = if attempt.slug.is_some() {
            BoltState::Completed
        } else {
            BoltState::Merged
        };
    }
}

/// The bolt rows of one instant, in the order they are applied: shard by
/// shard in each shard's own order, a start before a failure before a
/// completion before a merge, and the earlier shard on a tie.
fn bolt_rows_in_order<'a>(group: &[&'a ShardEvent]) -> Vec<&'a ShardEvent> {
    let priority = |event: &ShardEvent| match event.event.as_str() {
        "BOLT_STARTED" => 0,
        "BOLT_FAILED" => 1,
        "BOLT_COMPLETED" => 2,
        _ => 3,
    };
    let mut queues: Vec<std::collections::VecDeque<&ShardEvent>> = Vec::new();
    for event in group {
        if !matches!(
            event.event.as_str(),
            "BOLT_STARTED" | "BOLT_COMPLETED" | "BOLT_FAILED" | "AUDIT_MERGED"
        ) {
            continue;
        }
        match queues.iter_mut().find(|queue| {
            queue
                .front()
                .is_some_and(|first| first.shard == event.shard)
        }) {
            Some(queue) => queue.push_back(event),
            None => queues.push(std::collections::VecDeque::from([*event])),
        }
    }
    let mut ordered = Vec::new();
    while !queues.is_empty() {
        let mut selected = 0;
        for index in 1..queues.len() {
            let (candidate, held) = (queues[index][0], queues[selected][0]);
            let (ahead, same) = (
                priority(candidate) < priority(held),
                priority(candidate) == priority(held),
            );
            if ahead || (same && candidate.shard_index < held.shard_index) {
                selected = index;
            }
        }
        ordered.push(queues[selected].pop_front().unwrap());
        if queues[selected].is_empty() {
            queues.remove(selected);
        }
    }
    ordered
}

/// `reviewAttemptWindow` over the ledger `rows`, for a stage that is per
/// unit or not, whose per-unit artifacts are at stage level or not.
#[must_use]
pub fn window_over(
    rows: Vec<ShardEvent>,
    state: &str,
    slug: &str,
    per_unit: bool,
    stage_level_artifacts: bool,
    graph: &[Stage],
) -> Window {
    let receipts: Vec<ShardEvent> = rows
        .iter()
        .filter(|row| RECEIPT_EVENTS.contains(&row.event.as_str()))
        .cloned()
        .collect();
    let events = sort_attempt_events(&receipts);
    let artifact_per_unit = per_unit && !stage_level_artifacts;
    let units = Units {
        major: artifact_per_unit && (iteration_is_unit_major(state) || checkpoints_enabled(state)),
        team: artifact_per_unit && (is_team_unit_ownership(state) || checkpoints_enabled(state)),
        // A rejection that names a unit starts a new attempt for that unit
        // alone wherever floors are per unit.
        scoped_rejections: artifact_per_unit && unit_scoped_lifecycle_floors(state),
    };
    // A stage start recorded under unit-major flooring is no boundary after
    // a switch back.
    let unit_floored: Vec<&ShardEvent> = if artifact_per_unit && !units.major {
        let ignored = starts_under_unit_flooring(&rows, graph);
        rows.iter()
            .zip(ignored)
            .filter_map(|(row, ignored)| ignored.then_some(row))
            .collect()
    } else {
        Vec::new()
    };
    let floored = |event: &ShardEvent| {
        unit_floored
            .iter()
            .any(|row| row.shard == event.shard && row.pos == event.pos)
    };

    let mut floor_idx: Option<usize> = None;
    let mut index = 0;
    while index < events.len() {
        let event = &events[index];
        let mut boundary = event.event == "WORKFLOW_STARTED"
            || (event.event == "STAGE_JUMPED" && jump_reaches(&event.block, slug, graph));
        if !boundary && block_field(&event.block, "Stage") == Some(slug) {
            boundary = (event.event == "GATE_REJECTED"
                && !(units.scoped_rejections && said(&event.block, "Unit").is_some()))
                || (event.event == "STAGE_STARTED"
                    && !units.major
                    && !floored(event)
                    && !single_stage(&event.block));
        }
        if !boundary {
            index += 1;
            continue;
        }
        let tied_across_shards = events.iter().any(|candidate| {
            candidate.timestamp == event.timestamp && candidate.shard != event.shard
        });
        // An attempt owned by a team floors the whole instant of a boundary
        // tied across shards: nothing orders its rows.
        if tied_across_shards && units.team {
            let mut end = index;
            while end + 1 < events.len() && events[end + 1].timestamp == event.timestamp {
                end += 1;
            }
            floor_idx = Some(end);
            index = end;
        } else {
            floor_idx = Some(index);
        }
        index += 1;
    }

    let mut attempts: Vec<BoltAttempt> = Vec::new();
    if per_unit {
        let mut start = floor_idx.map_or(0, |floor| floor + 1);
        while start < events.len() {
            let mut end = start + 1;
            while end < events.len() && events[end].timestamp == events[start].timestamp {
                end += 1;
            }
            let group: Vec<&ShardEvent> = events[start..end].iter().collect();
            for event in bolt_rows_in_order(&group) {
                apply_bolt_event(&mut attempts, event);
            }
            start = end;
        }
    }
    let mut merged_bolt_units: Vec<String> = Vec::new();
    let mut open_bolt_units: Vec<String> = Vec::new();
    for attempt in &attempts {
        let set = match attempt.state {
            BoltState::Merged => &mut merged_bolt_units,
            BoltState::Open | BoltState::Completed => &mut open_bolt_units,
            BoltState::Failed => continue,
        };
        if !set.contains(&attempt.unit) {
            set.push(attempt.unit.clone());
        }
    }
    Window {
        all_events: rows,
        events,
        floor_idx,
        merged_bolt_units,
        open_bolt_units,
    }
}

/// `reviewAttemptWindow`: the review attempt of `stage`, read off the
/// ledger `rows` and the record's state.
#[must_use]
pub fn window(wf: &Workflow, state: &str, stage: &Stage, rows: Vec<ShardEvent>) -> Window {
    let per_unit = crate::stage::has_unit_marker(stage);
    let stage_level = per_unit
        && uses_stage_level_per_unit_artifacts(
            wf,
            get_field(state, "Scope").unwrap_or_default(),
            state,
        );
    window_over(
        rows,
        state,
        &stage.slug,
        per_unit,
        stage_level,
        &wf.install.graph,
    )
}

/// `REVIEW_ACCOUNTING_EVENTS`: the rows the count of an attempt's reviews is
/// read against.
const ACCOUNTING_EVENTS: [&str; 10] = [
    "WORKFLOW_STARTED",
    "STAGE_STARTED",
    "STAGE_COMPLETED",
    "STAGE_JUMPED",
    "GATE_REJECTED",
    "BOLT_STARTED",
    "BOLT_COMPLETED",
    "BOLT_FAILED",
    "REVIEW_REQUESTED",
    "REVIEW_COMPLETED",
];

/// A review request that waits for its verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    pub binding: RequestBinding,
    /// Whether the request was dispatched again against its own binding.
    pub retried: bool,
    /// Whether the request's row names a review file of its own (rows from
    /// before per-request review files do not).
    pub own_review_file: bool,
}

/// `ReviewAttemptAccounting`: the reviews of one attempt of a stage, for one
/// reviewer and one unit (or the stage).
#[derive(Debug, Clone, PartialEq)]
pub struct Accounting {
    /// The row the attempt starts at, as text; empty when the ledger has no
    /// boundary. The id of the attempt's review files is taken from it.
    pub floor: String,
    /// The requests of the attempt.
    pub request_count: usize,
    /// The requests the review budget counts: fewer when the unit started
    /// again a step it had finished.
    pub budget_count: usize,
    pub bolt_started: bool,
    pub bolt_batch: Option<String>,
    pub bolt_slug: Option<String>,
    /// The passes with a request and no verdict yet, in the order asked.
    pub pending_iterations: Vec<f64>,
    /// The pending request of each such pass.
    pub pending_requests: Vec<(f64, PendingRequest)>,
    pub recovery_iteration: Option<f64>,
    pub recovery_spent: bool,
    /// Why the attempt's order cannot be told: a tie across shards.
    pub ambiguity: Option<String>,
}

/// What the accounting is asked about.
pub struct Reviewed<'a> {
    pub slug: &'a str,
    pub per_unit: bool,
    pub workspace_requires: bool,
    pub reviewer: &'a str,
    pub unit: Option<&'a str>,
    /// `single-stage:<slug>` for a stage run by itself.
    pub workflow: Option<&'a str>,
}

/// The names a row lists in `field`, as written between the commas.
fn listed<'a>(block: &'a str, field: &str) -> Vec<&'a str> {
    block_field(block, field)
        .unwrap_or_default()
        .split(',')
        .map(trim)
        .collect()
}

/// `reviewAttemptAccounting`: the requests of the current attempt, counted.
/// The boundaries are the window's, read again with the unit in mind: the
/// start of the unit's own bolt is a stronger floor than the stage's, and
/// its end closes it. `read` hands over the review record a completion
/// names.
///
/// # Errors
///
/// `unit` is no unit name.
pub fn accounting(
    view: &Window,
    state: &str,
    reviewed: &Reviewed,
    graph: &[Stage],
    read: &dyn Fn(&RecordRef) -> Option<Record>,
) -> Result<Accounting, String> {
    let Reviewed {
        slug,
        per_unit,
        workspace_requires,
        reviewer,
        unit,
        workflow,
    } = *reviewed;
    let rows: Vec<ShardEvent> = view
        .all_events
        .iter()
        .filter(|row| ACCOUNTING_EVENTS.contains(&row.event.as_str()))
        .cloned()
        .collect();
    let events = sort_attempt_events(&rows);
    let tied_across_shards = |index: usize| {
        events.iter().any(|candidate| {
            candidate.timestamp == events[index].timestamp && candidate.shard != events[index].shard
        })
    };
    // Tied with rows of other shards that are all the work's own boundaries.
    let tied_only_to_workflow_boundary = |index: usize| {
        let mut others = events.iter().filter(|other| {
            other.timestamp == events[index].timestamp && other.shard != events[index].shard
        });
        let mut saw_boundary = false;
        others.all(|other| {
            saw_boundary = true;
            other.event == "WORKFLOW_STARTED" || other.event == "STAGE_JUMPED"
        }) && saw_boundary
    };

    let unit_major = per_unit && (iteration_is_unit_major(state) || checkpoints_enabled(state));
    let team = per_unit && (is_team_unit_ownership(state) || checkpoints_enabled(state));
    let scoped_rejections = per_unit && unit_scoped_lifecycle_floors(state);
    let unit_floored: Vec<&ShardEvent> = if per_unit && !unit_major {
        let ignored = starts_under_unit_flooring(&view.all_events, graph);
        view.all_events
            .iter()
            .zip(ignored)
            .filter_map(|(row, ignored)| ignored.then_some(row))
            .collect()
    } else {
        Vec::new()
    };
    let floored = |event: &ShardEvent| {
        unit_floored
            .iter()
            .any(|row| row.shard == event.shard && row.pos == event.pos)
    };

    let expected_bolt_slug = unit.map(bolt_slug_for_unit).transpose()?;
    let mut floor: Option<usize> = None;
    let mut bolt_started = false;
    let mut bolt_batch: Option<String> = None;
    let mut bolt_slug: Option<String> = None;
    let mut ambiguity: Option<String> = None;
    for (index, entry) in events.iter().enumerate() {
        let field = |name: &str| block_field(&entry.block, name);
        let tie = |what: &str| format!("cross-shard {what} tie at {}", entry.timestamp);
        if let Some(workflow) = workflow {
            // A stage run by itself counts from its own last completion.
            if entry.event == "STAGE_COMPLETED"
                && field("Stage") == Some(slug)
                && field("Workflow") == Some(workflow)
            {
                floor = Some(index);
            }
            continue;
        }
        let mut closes_bolt = false;
        if entry.event == "WORKFLOW_STARTED"
            || (entry.event == "STAGE_JUMPED" && jump_reaches(&entry.block, slug, graph))
        {
            // Only an attempt owned by a team is undone by a tied boundary.
            ambiguity = (team && tied_across_shards(index)).then(|| tie("boundary"));
            closes_bolt = true;
        } else if let Some(unit) = unit.filter(|_| entry.event == "BOLT_STARTED") {
            let started_slug = field("Bolt slug");
            if !listed(&entry.block, "Bolt names").contains(&unit)
                || started_slug.is_some_and(|slug| Some(slug) != expected_bolt_slug.as_deref())
            {
                continue;
            }
            ambiguity = tied_across_shards(index).then(|| tie("Bolt boundary"));
            floor = Some(index);
            bolt_started = true;
            bolt_batch = field("Batch number").map(str::to_owned);
            bolt_slug = started_slug.map(str::to_owned);
            continue;
        } else if let Some(unit) =
            unit.filter(|_| entry.event == "BOLT_COMPLETED" || entry.event == "BOLT_FAILED")
        {
            let names = listed(
                &entry.block,
                if entry.event == "BOLT_FAILED" {
                    "Failed Bolt"
                } else {
                    "Bolt names"
                },
            );
            let paired = match (bolt_slug.as_deref(), field("Bolt slug")) {
                (Some(started), Some(ended)) => started == ended,
                _ => names.contains(&unit),
            };
            if !paired {
                continue;
            }
            ambiguity = tied_across_shards(index).then(|| tie("Bolt boundary"));
            closes_bolt = true;
        } else if entry.event == "GATE_REJECTED" {
            let stages = match field("Gate Stages").or(field("Stage")) {
                Some(stages) => stages.split(',').map(trim).collect(),
                None => vec![""],
            };
            if !stages.contains(&slug) {
                continue;
            }
            let rejected_unit = field("Unit");
            // A rejection with no unit asks for changes to the whole stage:
            // outside team and checkpoint ownership it starts every unit's
            // attempt again.
            if scoped_rejections
                && match unit {
                    Some(unit) => rejected_unit != Some(unit) && (team || rejected_unit.is_some()),
                    None => rejected_unit.is_some(),
                }
            {
                continue;
            }
            ambiguity = tied_across_shards(index).then(|| tie("gate boundary"));
            closes_bolt = true;
        } else if entry.event == "STAGE_STARTED"
            && field("Stage") == Some(slug)
            && !unit_major
            && !floored(entry)
            && !single_stage(&entry.block)
        {
            ambiguity = tied_across_shards(index).then(|| tie("stage boundary"));
            closes_bolt = true;
        }
        if closes_bolt {
            floor = Some(index);
            bolt_started = false;
            bolt_batch = None;
            bolt_slug = None;
        }
    }

    // A tie among the rows of the unit's own bolt, once that bolt is merged
    // and none is open, orders nothing that matters.
    const BOLT_TIE: &str = "cross-shard Bolt boundary tie at ";
    if let Some(unit) = unit
        && let Some(instant) = ambiguity
            .as_deref()
            .and_then(|said| said.strip_prefix(BOLT_TIE))
        && view.merged_bolt_units.iter().any(|merged| merged == unit)
        && !view.open_bolt_units.iter().any(|open| open == unit)
    {
        let tied: Vec<&ShardEvent> = view
            .events
            .iter()
            .filter(|event| event.timestamp == instant)
            .collect();
        let several_shards = tied.iter().any(|event| event.shard != tied[0].shard);
        let lifecycle_only = tied.len() > 1
            && several_shards
            && tied.iter().all(|event| match event.event.as_str() {
                "AUDIT_MERGED" => true,
                "BOLT_STARTED" | "BOLT_COMPLETED" => {
                    listed(&event.block, "Bolt names").contains(&unit)
                }
                "BOLT_FAILED" => listed(&event.block, "Failed Bolt").contains(&unit),
                _ => false,
            });
        if lifecycle_only {
            ambiguity = None;
            bolt_started = false;
            bolt_batch = None;
            bolt_slug = None;
        }
    }

    // A solo unit-major walk hands a unit's finished step back when its work
    // is gone, and the unit starts that step again: that run of the step has
    // the stage's review passes, and its one recovery, again.
    let mut restart: Option<ShardEvent> = None;
    if let Some(unit) = unit
        && unit_major
        && !is_team_unit_ownership(state)
        && workflow.is_none()
    {
        let steps: Vec<ShardEvent> = view
            .all_events
            .iter()
            .filter(|row| {
                (row.event == "UNIT_STARTED" || row.event == "UNIT_COMPLETED")
                    && block_field(&row.block, "Stage") == Some(slug)
                    && block_field(&row.block, "Unit") == Some(unit)
            })
            .cloned()
            .collect();
        let mut finished = false;
        for row in sort_attempt_events(&steps) {
            if floor.is_some_and(|floor| !definitely_before(&events[floor], &row)) {
                continue;
            }
            if row.event == "UNIT_COMPLETED" {
                finished = true;
            } else if finished {
                restart = Some(row);
                finished = false;
            }
        }
    }

    let mut request_count = 0;
    let mut budget_count = 0;
    let mut recovery_iteration: Option<f64> = None;
    let mut recovery_spent = false;
    let mut pending_iterations: Vec<f64> = Vec::new();
    let mut pending_requests: Vec<(f64, PendingRequest)> = Vec::new();
    for (index, entry) in events
        .iter()
        .enumerate()
        .skip(floor.map_or(0, |floor| floor + 1))
    {
        if entry.event != "REVIEW_REQUESTED" && entry.event != "REVIEW_COMPLETED" {
            continue;
        }
        let field = |name: &str| block_field(&entry.block, name);
        if field("Stage") != Some(slug) || field("Reviewer") != Some(reviewer) {
            continue;
        }
        if said(&entry.block, "Unit") != unit {
            continue;
        }
        let of_another_run = match (workflow, said(&entry.block, "Workflow")) {
            (Some(workflow), row) => row != Some(workflow),
            (None, row) => row.is_some_and(|row| row.starts_with("single-stage:")),
        };
        if of_another_run {
            continue;
        }
        if tied_across_shards(index) && (team || !tied_only_to_workflow_boundary(index)) {
            ambiguity = Some(format!(
                "cross-shard review authority tie at {}",
                entry.timestamp
            ));
            continue;
        }
        let Some(iteration) = field("Iteration")
            .filter(|pass| re::test("^[1-9][0-9]*$", "", pass))
            .map(num::from_str)
        else {
            continue;
        };
        let held = pending_requests
            .iter()
            .position(|(pass, _)| *pass == iteration);
        if entry.event == "REVIEW_REQUESTED" {
            let Some(binding) = review::request_binding(&entry.block) else {
                continue;
            };
            let previous = held.map(|held| &pending_requests[held].1);
            // A replacement takes the pass of the request it replaces: it
            // neither counts again nor inherits a retry.
            let replacement = review::request_replaces(
                &entry.block,
                previous.map(|previous| Some(&previous.binding)),
            );
            let since_restart = restart
                .as_ref()
                .is_none_or(|restart| definitely_before(restart, entry));
            let retry = field("Retry") == Some("pending-request");
            if !retry && !replacement {
                request_count += 1;
                if since_restart {
                    budget_count += 1;
                }
            }
            if field("Recovery") == Some("stale-receipt") && since_restart {
                recovery_iteration = Some(iteration);
                recovery_spent = true;
            }
            if !pending_iterations.contains(&iteration) {
                pending_iterations.push(iteration);
            }
            let request = PendingRequest {
                retried: !replacement
                    && (previous.is_some_and(|previous| previous.retried)
                        || (retry && review::binding_is_modern(&binding, workspace_requires))),
                own_review_file: field("Review File").is_some(),
                binding,
            };
            match held {
                Some(held) => pending_requests[held].1 = request,
                None => pending_requests.push((iteration, request)),
            }
        } else if let Some(held) = held
            && review_record::completion_carries_verified_review(
                &pending_requests[held].1.binding,
                &entry.block,
                read,
            )
        {
            pending_requests.remove(held);
            pending_iterations.retain(|pass| *pass != iteration);
        }
    }
    Ok(Accounting {
        floor: floor.map_or_else(String::new, |floor| {
            let row = &events[floor];
            format!("{}:{}:{}:{}", row.event, row.timestamp, row.shard, row.pos)
        }),
        request_count,
        budget_count,
        bolt_started,
        bolt_batch,
        bolt_slug,
        pending_iterations,
        pending_requests,
        recovery_iteration,
        recovery_spent,
        ambiguity,
    })
}

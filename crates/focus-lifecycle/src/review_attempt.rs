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

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::authorization::jump_reaches;
use focus_graph::runtime::Stage;
use focus_js::order::cmp_utf16;
use focus_js::text::trim;
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
    get_field(state, "Unit Ownership").map(trim) == Some("team")
}

fn checkpoints_enabled(state: &str) -> bool {
    get_field(state, "Construction Checkpoints") == Some("enabled")
}

fn iteration_is_unit_major(state: &str) -> bool {
    get_field(state, "Construction Iteration").map(trim) == Some("unit-major")
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
        let tied_across_shards = events.iter().enumerate().any(|(other, candidate)| {
            other != index
                && candidate.timestamp == event.timestamp
                && candidate.shard != event.shard
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

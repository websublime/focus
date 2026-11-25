//! The floor of a stage's current attempt, as the token a receipt carries.
//!
//! A unit's lifecycle receipts (`UNIT_STARTED` … `UNIT_COMPLETED`) and a
//! swarm's are stamped with a `Run floor`: the last boundary of the stage's
//! attempt when they were written (the creation of the work, a jump that
//! reaches the stage, a rejection of its gate, a start of it), with its
//! place among the boundaries of its kind. A receipt counts while the floor
//! read now is the one it carries, so a new boundary retires every receipt
//! of the attempt before it, even one written in the same second.
//!
//! Mirrors `latestMainWorkflowStageRunFloorFromRows`,
//! `stageStartsUnderStageFlooring` and `unitLifecycleRunFloorForProject`
//! (`aidlc-lib.ts` L34 707, L34 810 and L34 669).
//! `stageStartsUnderUnitFlooring` is [`crate::review_attempt`]'s.

use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::authorization::jump_reaches;
use focus_graph::runtime::Stage;
use focus_js::order::cmp_utf16;
use sha2::{Digest, Sha256};

use crate::gate::sort_attempt_events;
use crate::review_attempt::{
    definitely_before, policy_found_unit_major, policy_row_complete, starts_under_unit_flooring,
};
use crate::review_receipts::gate_stages;

/// The floor of a ledger with no boundary of the stage.
const UNSTARTED: &str = "unstarted#0";

fn single_stage(block: &str) -> bool {
    block_field(block, "Workflow").is_some_and(|workflow| workflow.starts_with("single-stage:"))
}

/// `stageStartsUnderStageFlooring`: the stage starts recorded while
/// stage-major flooring was in force, by their place in `rows`. Each
/// `CONSTRUCTION_POLICY_SET` row says what it found, so a start reads the
/// policy of the first change after it; a start after the last change
/// follows the caller's own flooring and is not counted here. Rows of
/// different shards in one second cannot be ordered, so a start counts
/// unless every change that may be the first after it found unit-major
/// flooring.
#[must_use]
pub fn starts_under_stage_flooring(rows: &[ShardEvent]) -> Vec<bool> {
    let mut counted = vec![false; rows.len()];
    let changes: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| row.event == "CONSTRUCTION_POLICY_SET" && policy_row_complete(row))
        .collect();
    let before = definitely_before;
    for (index, start) in rows.iter().enumerate() {
        if start.event != "STAGE_STARTED" {
            continue;
        }
        let after: Vec<&ShardEvent> = changes
            .iter()
            .copied()
            .filter(|change| !before(change, start))
            .collect();
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
        counted[index] = candidates
            .iter()
            .any(|change| !policy_found_unit_major(change));
    }
    counted
}

/// `gateRejectionMatchesAttempt`: a rejection of the stage's gate, of the
/// whole stage or of this unit.
fn rejection_matches(block: &str, slug: &str, unit: Option<&str>) -> bool {
    if !gate_stages(block).contains(&slug) {
        return false;
    }
    match block_field(block, "Unit") {
        None => true,
        Some(of) => unit == Some(of),
    }
}

/// `latestMainWorkflowStageRunFloorFromRows`: the token of the last
/// boundary of `slug`'s attempt in `rows`, which may come in any order.
///
/// Under unit-major flooring only the stage starts recorded under
/// stage-major flooring are boundaries; for a unit's receipts under
/// stage-major flooring (`unit_receipts`), the starts recorded under
/// unit-major flooring are not. A start keeps the ordinal it has among all
/// the starts of the stage, and a jump the one it has among all jumps, so
/// a switch of flooring does not rename a floor.
///
/// When the latest boundaries are rows of different shards in one second,
/// their order cannot be known: the floor is a token made of all of them,
/// which no earlier receipt carries.
#[must_use]
pub fn latest(
    rows: &[ShardEvent],
    slug: &str,
    unit_major: bool,
    unit: Option<&str>,
    unit_receipts: bool,
    graph: &[Stage],
) -> String {
    let stage_start = |row: &ShardEvent| {
        row.event == "STAGE_STARTED"
            && block_field(&row.block, "Stage") == Some(slug)
            && !single_stage(&row.block)
    };
    let stage_floored = unit_major.then(|| starts_under_stage_flooring(rows));
    let unit_floored =
        (!unit_major && unit_receipts).then(|| starts_under_unit_flooring(rows, graph));
    let by_time = |a: &&ShardEvent, b: &&ShardEvent| {
        cmp_utf16(&a.timestamp, &b.timestamp)
            .then(a.shard_index.cmp(&b.shard_index))
            .then(a.pos.cmp(&b.pos))
    };
    let same = |a: &ShardEvent, b: &ShardEvent| std::ptr::eq(a, b);
    // (`sortAttemptEvents` copies its rows: a start is found again by where
    // it was read.)
    let starts: Vec<ShardEvent> = sort_attempt_events(
        &rows
            .iter()
            .filter(|row| stage_start(row))
            .cloned()
            .collect::<Vec<_>>(),
    );
    let start_ordinal = |row: &ShardEvent| {
        starts
            .iter()
            .position(|start| start.shard_index == row.shard_index && start.pos == row.pos)
            .map_or(0, |at| at + 1)
    };
    let mut jumps: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| row.event == "STAGE_JUMPED")
        .collect();
    jumps.sort_by(by_time);
    let jump_ordinal = |row: &ShardEvent| {
        jumps
            .iter()
            .position(|jump| same(jump, row))
            .map_or(0, |at| at + 1)
    };
    let mut boundaries: Vec<&ShardEvent> = rows
        .iter()
        .enumerate()
        .filter(|(index, row)| match row.event.as_str() {
            "WORKFLOW_STARTED" => true,
            "STAGE_JUMPED" => jump_reaches(&row.block, slug, graph),
            "GATE_REJECTED" => rejection_matches(&row.block, slug, unit),
            "STAGE_STARTED" => {
                stage_start(row)
                    && stage_floored.as_ref().is_none_or(|counted| counted[*index])
                    && unit_floored.as_ref().is_none_or(|ignored| !ignored[*index])
            }
            _ => false,
        })
        .map(|(_, row)| row)
        .collect();
    boundaries.sort_by(by_time);
    let Some(last) = boundaries.last() else {
        return UNSTARTED.to_owned();
    };
    let tied: Vec<&ShardEvent> = boundaries
        .iter()
        .copied()
        .filter(|row| row.timestamp == last.timestamp)
        .collect();
    if tied.iter().any(|row| row.shard != tied[0].shard) {
        let mut identity: Vec<String> = tied
            .iter()
            .map(|row| {
                let name = Path::new(&row.shard)
                    .file_name()
                    .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
                let mut fields = vec![
                    name,
                    row.pos.to_string(),
                    row.event.clone(),
                    block_field(&row.block, "Stage").unwrap_or("").to_owned(),
                ];
                if unit.is_some() {
                    fields.push(block_field(&row.block, "Unit").unwrap_or("").to_owned());
                    fields.push(
                        block_field(&row.block, "Gate Stages")
                            .unwrap_or("")
                            .to_owned(),
                    );
                }
                fields.join(":")
            })
            .collect();
        identity.sort_by(|a, b| cmp_utf16(a, b));
        let digest = Sha256::digest(identity.join("|").as_bytes());
        let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        return format!("AMBIGUOUS:{}#{}", last.timestamp, &hex[..12]);
    }
    let (mut created, mut rejected) = (0, 0);
    let mut floor = UNSTARTED.to_owned();
    for row in boundaries {
        let ordinal = match row.event.as_str() {
            "STAGE_STARTED" => start_ordinal(row),
            "STAGE_JUMPED" => jump_ordinal(row),
            "WORKFLOW_STARTED" => {
                created += 1;
                created
            }
            _ => {
                rejected += 1;
                rejected
            }
        };
        floor = format!("{}:{}#{ordinal}", row.event, row.timestamp);
    }
    floor
}

/// `unitLifecycleRunFloorForProject`: the floor a unit's lifecycle receipt
/// carries and is read against. A unit floored per unit (`unit_scoped`), or
/// one with a rejection of its own, is floored on that unit, so a reopened
/// unit owes its redo whatever the flooring; any other reads the stage's.
#[must_use]
pub fn unit_lifecycle(
    rows: &[ShardEvent],
    slug: &str,
    unit_major: bool,
    unit: Option<&str>,
    unit_scoped: bool,
    graph: &[Stage],
) -> String {
    let own_reopen = |name: &str| {
        rows.iter().any(|row| {
            row.event == "GATE_REJECTED"
                && block_field(&row.block, "Unit") == Some(name)
                && gate_stages(&row.block).contains(&slug)
        })
    };
    let floored = unit.filter(|unit| unit_scoped || own_reopen(unit));
    latest(rows, slug, unit_major, floored, true, graph)
}

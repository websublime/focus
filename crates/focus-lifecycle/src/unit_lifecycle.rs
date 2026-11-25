//! The lifecycle of a stage's units, read off the ledger.
//!
//! A stage that runs for each unit leaves a receipt for each step of each
//! unit: started, paused, resumed, completed, or skipped. The receipts of
//! the stage's current attempt say which units are done, which were
//! skipped and why, and which are open (in progress, or paused with what to
//! do next), most recently touched first.
//!
//! A receipt is of the current attempt when the `Run floor` it carries is
//! the one read now for its unit ([`crate::run_floor::unit_lifecycle`]).
//! Receipts of different shards in one second have no order; the reading
//! is then the safest one: a pause stands over a start or a resumption,
//! and either over a completion, which settles a unit only when nothing
//! else may have come after it.
//!
//! A completion made in a wave (`Mode: wave`) also names the fingerprint of
//! the unit's outputs, and holds while they are the ones it recorded.
//!
//! Mirrors `currentUnitLifecycleRows`, `openUnitCheckpoints`,
//! `unitLifecycleSnapshot` and `unitSkippedUnits` (`aidlc-lib.ts` L35 507,
//! L35 677, L35 719 and L35 843).
//!
//! # What is not read
//!
//! A team's claim of a unit (`eventMatchesClaimAttempt`): with
//! `Unit Ownership: team` upstream also drops the receipts of an earlier
//! claim of the unit. Team is deferred, and what would run a team's units
//! refuses by name before it reads this.

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_graph::runtime::Stage;
use focus_js::order::cmp_utf16;
use focus_js::re;
use focus_state::fields::get_field;

use crate::review_attempt::{starts_under_unit_flooring, unit_scoped_lifecycle_floors};
use crate::run_floor;

/// The receipts of a unit's lifecycle.
const UNIT_EVENTS: [&str; 5] = [
    "UNIT_STARTED",
    "UNIT_PAUSED",
    "UNIT_RESUMED",
    "UNIT_COMPLETED",
    "UNIT_SKIPPED",
];

/// `UNIT_TERMINAL_EVENTS`: a unit's lifecycle for a stage ends in a
/// completion or, when the stage does not apply to it, in a skip.
fn terminal(event: &str) -> bool {
    matches!(event, "UNIT_COMPLETED" | "UNIT_SKIPPED")
}

fn single_stage(block: &str) -> bool {
    block_field(block, "Workflow").is_some_and(|workflow| workflow.starts_with("single-stage:"))
}

/// One receipt of the current attempt, with the unit it is of.
#[derive(Debug, Clone, Copy)]
pub struct Receipt<'a> {
    pub row: &'a ShardEvent,
    pub unit: &'a str,
}

fn in_time(a: &ShardEvent, b: &ShardEvent) -> std::cmp::Ordering {
    cmp_utf16(&a.timestamp, &b.timestamp)
        .then(a.shard_index.cmp(&b.shard_index))
        .then(a.pos.cmp(&b.pos))
}

/// `unitMajorLifecycleMode`, off the state: unit-major iteration, or
/// Construction checkpoints.
#[must_use]
pub fn unit_major(state: &str) -> bool {
    get_field(state, "Construction Iteration") == Some("unit-major")
        || get_field(state, "Construction Checkpoints") == Some("enabled")
}

/// `currentUnitLifecycleRows`: the receipts of `slug`'s current attempt in
/// `rows`, in order, one for each unit and instant.
///
/// Under stage-major flooring a receipt older than the stage's last start
/// that counts is dropped as well, so a stage started after its units were
/// finished in a unit-major walk keeps them.
#[must_use]
pub fn current_rows<'a>(
    rows: &'a [ShardEvent],
    slug: &str,
    unit_major: bool,
    state: Option<&str>,
    graph: &[Stage],
) -> Vec<Receipt<'a>> {
    let ignored = if unit_major {
        vec![false; rows.len()]
    } else {
        starts_under_unit_flooring(rows, graph)
    };
    // (Upstream takes the last of the starts that count, sorted one way
    // with none left out and another with some. Either way it is the
    // greatest timestamp among them.)
    let cutoff = rows
        .iter()
        .zip(&ignored)
        .filter(|(row, ignored)| {
            !**ignored
                && row.event == "STAGE_STARTED"
                && block_field(&row.block, "Stage") == Some(slug)
                && !single_stage(&row.block)
        })
        .map(|(row, _)| row.timestamp.as_str())
        .max_by(|a, b| cmp_utf16(a, b))
        .unwrap_or("");
    let unit_scoped = state.is_some_and(unit_scoped_lifecycle_floors);
    let mut floors: Vec<(&str, String)> = Vec::new();
    let mut current: Vec<Receipt<'a>> = Vec::new();
    for row in rows {
        if !UNIT_EVENTS.contains(&row.event.as_str())
            || block_field(&row.block, "Stage") != Some(slug)
        {
            continue;
        }
        let Some(unit) = block_field(&row.block, "Unit").filter(|unit| !unit.is_empty()) else {
            continue;
        };
        let floor = match floors.iter().find(|(known, _)| *known == unit) {
            Some((_, floor)) => floor.clone(),
            None => {
                let floor = run_floor::unit_lifecycle(
                    rows,
                    slug,
                    unit_major,
                    Some(unit),
                    unit_scoped,
                    graph,
                );
                floors.push((unit, floor.clone()));
                floor
            }
        };
        if block_field(&row.block, "Run floor") != Some(floor.as_str()) {
            continue;
        }
        if !unit_major && !cutoff.is_empty() && cmp_utf16(&row.timestamp, cutoff).is_lt() {
            continue;
        }
        current.push(Receipt { row, unit });
    }
    current.sort_by(|a, b| in_time(a.row, b.row));

    // The receipts of one unit in one instant: the last of each shard, and
    // among shards the one that leaves the unit least settled.
    let rank = |event: &str| match event {
        "UNIT_PAUSED" => 2,
        event if terminal(event) => 0,
        _ => 1,
    };
    let mut reduced: Vec<Receipt<'a>> = Vec::new();
    let mut start = 0;
    while start < current.len() {
        let mut end = start + 1;
        while end < current.len() && current[end].row.timestamp == current[start].row.timestamp {
            end += 1;
        }
        let mut units: Vec<&str> = Vec::new();
        for receipt in &current[start..end] {
            if !units.contains(&receipt.unit) {
                units.push(receipt.unit);
            }
        }
        for unit in units {
            let mut by_shard: Vec<Receipt<'a>> = Vec::new();
            for receipt in current[start..end]
                .iter()
                .filter(|receipt| receipt.unit == unit)
            {
                match by_shard
                    .iter_mut()
                    .find(|kept| kept.row.shard == receipt.row.shard)
                {
                    Some(kept) => *kept = *receipt,
                    None => by_shard.push(*receipt),
                }
            }
            by_shard.sort_by(|a, b| {
                rank(&a.row.event)
                    .cmp(&rank(&b.row.event))
                    .then(a.row.shard_index.cmp(&b.row.shard_index))
                    .then(a.row.pos.cmp(&b.row.pos))
            });
            if let Some(last) = by_shard.last() {
                reduced.push(*last);
            }
        }
        start = end;
    }
    reduced.sort_by(|a, b| in_time(a.row, b.row));
    reduced
}

/// `UnitCheckpoint`: a unit that is open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub unit: String,
    pub paused: bool,
    pub reason: Option<String>,
    pub next_action: Option<String>,
    /// The unit this one was paused for: the walk takes that one first.
    pub set_aside_for: Option<String>,
}

impl Checkpoint {
    /// The state as upstream names it.
    #[must_use]
    pub const fn state(&self) -> &'static str {
        if self.paused { "paused" } else { "in-progress" }
    }
}

/// `openUnitCheckpoints`: every unit whose latest receipt of the attempt
/// leaves it open, most recently touched first.
#[must_use]
pub fn open_checkpoints(receipts: &[Receipt<'_>]) -> Vec<Checkpoint> {
    let mut open = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for receipt in receipts.iter().rev() {
        if seen.contains(&receipt.unit) {
            continue;
        }
        seen.push(receipt.unit);
        if terminal(&receipt.row.event) {
            continue;
        }
        let paused = receipt.row.event == "UNIT_PAUSED";
        let field = |name: &str| block_field(&receipt.row.block, name).map(str::to_owned);
        open.push(Checkpoint {
            unit: receipt.unit.to_owned(),
            paused,
            reason: field("Reason"),
            next_action: field("Next Action"),
            set_aside_for: if paused { field("Set Aside For") } else { None },
        });
    }
    open
}

/// `UnitLifecycleMode`: how the units of the attempt were built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    None,
    Serial,
    Wave,
    Mixed,
}

impl Mode {
    /// The mode as upstream names it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Serial => "serial",
            Self::Wave => "wave",
            Self::Mixed => "mixed",
        }
    }
}

/// `UnitLifecycleSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The units with a completion that stands, in the order they got it.
    pub receipts: Vec<String>,
    /// The units skipped, each with the reason.
    pub skipped: Vec<(String, String)>,
    /// Every open unit, most recently touched first.
    pub open: Vec<Checkpoint>,
    /// Whether the stage ever left a receipt, in any attempt: once it has,
    /// an attempt with none stays unsettled.
    pub in_use: bool,
    pub mode: Mode,
}

impl Snapshot {
    /// The most recently touched open unit.
    #[must_use]
    pub fn checkpoint(&self) -> Option<&Checkpoint> {
        self.open.first()
    }
}

/// `waveCompletionHolds`: a completion made in a wave is a receipt while
/// the unit's outputs are the ones it recorded; with Construction
/// checkpoints a later change is the unit checkpoint's to weigh
/// (`keep_changed`), and it holds while every output is still there.
fn wave_completion_holds(
    recorded: Option<&str>,
    current: Option<&str>,
    keep_changed: bool,
) -> bool {
    recorded.is_some_and(|recorded| {
        re::test(r"^sha256:[0-9a-f]{64}$", "", recorded)
            && (current == Some(recorded) || (keep_changed && current.is_some()))
    })
}

/// A unit put last in a list that keeps the order things were added in.
fn add(list: &mut Vec<String>, unit: &str) {
    if !list.iter().any(|known| known == unit) {
        list.push(unit.to_owned());
    }
}

/// `unitLifecycleSnapshot`: what the receipts of `slug`'s current attempt
/// come to. `fingerprint` gives the fingerprint of a unit's outputs now
/// (nothing when they cannot all be read), asked only of a completion made
/// in a wave.
#[must_use]
pub fn snapshot(
    rows: &[ShardEvent],
    slug: &str,
    state: &str,
    fingerprint: &dyn Fn(&str) -> Option<String>,
    keep_changed: bool,
    graph: &[Stage],
) -> Snapshot {
    let receipts_now = current_rows(rows, slug, unit_major(state), Some(state), graph);
    let mut receipts: Vec<String> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    let (mut serial, mut wave) = (false, false);
    for receipt in &receipts_now {
        let unit = receipt.unit;
        let block = &receipt.row.block;
        if receipt.row.event == "UNIT_SKIPPED" {
            receipts.retain(|known| known != unit);
            let reason = block_field(block, "Reason").unwrap_or("").to_owned();
            match skipped.iter_mut().find(|(known, _)| known == unit) {
                Some(entry) => entry.1 = reason,
                None => skipped.push((unit.to_owned(), reason)),
            }
            continue;
        }
        skipped.retain(|(known, _)| known != unit);
        let in_wave = block_field(block, "Mode") == Some("wave");
        if in_wave {
            wave = true;
        } else {
            serial = true;
        }
        if receipt.row.event != "UNIT_COMPLETED" {
            receipts.retain(|known| known != unit);
        } else if !in_wave
            || wave_completion_holds(
                block_field(block, "Artifact Fingerprint"),
                fingerprint(unit).as_deref(),
                keep_changed,
            )
        {
            add(&mut receipts, unit);
        } else {
            receipts.retain(|known| known != unit);
        }
    }
    let in_use = rows.iter().any(|row| {
        matches!(
            row.event.as_str(),
            "UNIT_STARTED" | "UNIT_PAUSED" | "UNIT_RESUMED" | "UNIT_COMPLETED"
        ) && block_field(&row.block, "Stage") == Some(slug)
    });
    Snapshot {
        receipts,
        skipped,
        open: open_checkpoints(&receipts_now),
        in_use,
        mode: match (serial, wave) {
            (true, true) => Mode::Mixed,
            (false, true) => Mode::Wave,
            (true, false) => Mode::Serial,
            (false, false) => Mode::None,
        },
    }
}

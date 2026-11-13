//! The receipts of a pipeline stage's links.
//!
//! A stage in `pipeline` mode hands its work down a chain: the lead agent,
//! then each supporting agent in order, per repository when the piece of
//! work spans several. A link's return is recorded as a
//! `PIPELINE_LINK_COMPLETED` row, and the rows of the stage's current
//! attempt are the evidence that the chain ran in order. This module reads
//! them (`pipelineAttemptFloor`, `currentPipelineLinkReceipts`,
//! `latestPipelineLinkArtifactMtime`, `pipelineReceiptArtifactIsCurrent`
//! and the chain of `pipelineLinkEvidence`, `aidlc-lib.ts` L33 990 to
//! L34 360).
//!
//! # Order
//!
//! The ledger is read in the order of its instants, then of its shards,
//! then of position. An attempt starts at the newest boundary: for the
//! ordinary walk, the creation of the work, a jump that reaches the stage,
//! a rejection of its gate (unless the guard policy keeps the receipts) or
//! a start of the stage; for a stage run by itself, a start in that run. A
//! row at the boundary's own instant is after it only in the boundary's
//! shard and past its position: across shards nothing says which came
//! first, and the receipt does not count.
//!
//! # What is not here
//!
//! Which links a stage still owes, and a repository whose knowledge base
//! was kept instead of rebuilt, are the gate's reading of the same rows
//! (slice 3).

use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_graph::runtime::Stage;
use focus_js::path::{self as jspath, Flavour};
use focus_js::{num, re};
use focus_workspace::{files, links};
use sha2::{Digest, Sha256};

use crate::authorization::jump_reaches;

/// `PipelineLinkReceipt`: one link's return, as its row has it.
#[derive(Debug, Clone, PartialEq)]
pub struct Receipt {
    pub link: String,
    pub repo: Option<String>,
    pub position: Option<String>,
    pub timestamp: String,
    pub artifact_path: Option<String>,
    pub artifact_sha256: Option<String>,
    pub artifact_mtime_ms: Option<f64>,
}

/// `orderedPipelineEvidenceEvents`: the ledger by instant, then shard, then
/// position.
#[must_use]
pub fn ordered(mut events: Vec<ShardEvent>) -> Vec<ShardEvent> {
    events.sort_by(|a, b| {
        focus_js::order::cmp_utf16(&a.timestamp, &b.timestamp)
            .then(a.shard_index.cmp(&b.shard_index))
            .then(a.pos.cmp(&b.pos))
    });
    events
}

/// `PipelineAttemptFloor`: the boundary an attempt starts at.
#[derive(Debug)]
pub struct Floor<'a> {
    pub timestamp: String,
    rows: Vec<&'a ShardEvent>,
}

fn single_stage(workflow: Option<&str>) -> bool {
    workflow.is_some_and(|workflow| workflow.starts_with("single-stage:"))
}

/// Whether a row belongs to the run asked about: the stage run by itself,
/// or the ordinary walk.
fn of_run(workflow: Option<&str>, stage: &str, single_run: bool) -> bool {
    if single_run {
        workflow == Some(format!("single-stage:{stage}").as_str())
    } else {
        !single_stage(workflow)
    }
}

/// `pipelineAttemptFloor`: the newest boundary of the stage's attempt in
/// `events`, which are in order; `None` when the ledger has none.
#[must_use]
pub fn attempt_floor<'a>(
    events: &'a [ShardEvent],
    stage: &str,
    single_run: bool,
    rejection_keeps_receipts: bool,
    graph: &[Stage],
) -> Option<Floor<'a>> {
    let boundaries: Vec<&ShardEvent> = events
        .iter()
        .filter(|row| {
            let workflow = block_field(&row.block, "Workflow");
            let of_stage = block_field(&row.block, "Stage") == Some(stage);
            let resets = match row.event.as_str() {
                "WORKFLOW_STARTED" => true,
                "STAGE_JUMPED" => jump_reaches(&row.block, stage, graph),
                "GATE_REJECTED" => !rejection_keeps_receipts && of_stage,
                _ => false,
            };
            if !single_run && resets && !single_stage(workflow) {
                return true;
            }
            row.event == "STAGE_STARTED" && of_stage && of_run(workflow, stage, single_run)
        })
        .collect();
    let timestamp = boundaries.last()?.timestamp.clone();
    Some(Floor {
        rows: boundaries
            .into_iter()
            .filter(|row| row.timestamp == timestamp)
            .collect(),
        timestamp,
    })
}

/// `pipelineEventAfterFloor`.
#[must_use]
pub fn after_floor(row: &ShardEvent, floor: Option<&Floor>) -> bool {
    let Some(floor) = floor else {
        return true;
    };
    if row.timestamp != floor.timestamp {
        return focus_js::order::cmp_utf16(&row.timestamp, &floor.timestamp)
            == std::cmp::Ordering::Greater;
    }
    let shard = &floor.rows[0].shard;
    if floor.rows.iter().any(|boundary| boundary.shard != *shard) || row.shard != *shard {
        return false;
    }
    floor.rows.iter().all(|boundary| row.pos > boundary.pos)
}

/// `pipelineAttemptStartedAt`: the instant the stage's attempt started, or
/// nothing.
#[must_use]
pub fn attempt_started_at(
    events: &[ShardEvent],
    stage: &str,
    single_run: bool,
    graph: &[Stage],
) -> String {
    attempt_floor(events, stage, single_run, false, graph)
        .map(|floor| floor.timestamp)
        .unwrap_or_default()
}

/// A row's `Artifact Mtime Ms`, when it is a number.
fn mtime_field(block: &str) -> Option<f64> {
    block_field(block, "Artifact Mtime Ms")
        .filter(|value| re::test(r"^[0-9]+(?:\.[0-9]+)?$", "", value))
        .map(num::from_str)
}

/// `currentPipelineLinkReceipts`: the stage's link receipts since its
/// attempt started, in order. `accepts_changes` is the guard policy's: a
/// rejection then keeps the receipts.
#[must_use]
pub fn current_receipts(
    events: &[ShardEvent],
    stage: &str,
    single_run: bool,
    accepts_changes: bool,
    graph: &[Stage],
) -> Vec<Receipt> {
    let floor = attempt_floor(events, stage, single_run, accepts_changes, graph);
    events
        .iter()
        .filter(|row| after_floor(row, floor.as_ref()))
        .filter(|row| {
            row.event == "PIPELINE_LINK_COMPLETED"
                && block_field(&row.block, "Stage") == Some(stage)
                && of_run(block_field(&row.block, "Workflow"), stage, single_run)
        })
        .filter_map(|row| {
            let field = |name: &str| block_field(&row.block, name).map(str::to_owned);
            Some(Receipt {
                link: field("Link").filter(|link| !link.is_empty())?,
                repo: field("Repo"),
                position: field("Position"),
                timestamp: row.timestamp.clone(),
                artifact_path: field("Artifact Path"),
                artifact_sha256: field("Artifact SHA256"),
                artifact_mtime_ms: mtime_field(&row.block),
            })
        })
        .collect()
}

/// `latestPipelineLinkArtifactMtime`: the newest time any receipt of this
/// link and repository recorded for its handoff file, in any attempt.
#[must_use]
pub fn latest_artifact_mtime(
    events: &[ShardEvent],
    stage: &str,
    link: &str,
    repo: Option<&str>,
    single_run: bool,
) -> Option<f64> {
    events
        .iter()
        .filter(|row| {
            row.event == "PIPELINE_LINK_COMPLETED"
                && block_field(&row.block, "Stage") == Some(stage)
                && block_field(&row.block, "Link") == Some(link)
                && block_field(&row.block, "Repo") == repo
                && of_run(block_field(&row.block, "Workflow"), stage, single_run)
        })
        .filter_map(|row| mtime_field(&row.block))
        .fold(None, |latest: Option<f64>, value| {
            Some(latest.map_or(value, |latest| latest.max(value)))
        })
}

/// A file as a receipt binds to it: its bytes, and when it was last
/// written, in milliseconds as the runtime counts them.
pub struct Handoff {
    pub bytes: Vec<u8>,
    pub mtime_ms: f64,
}

/// The handoff file at `relative` under the project: a regular file with
/// one name, reached through no link.
///
/// # Errors
///
/// A folder on the way, or the file, is a link; it is not a regular file,
/// or has a second name; it cannot be read.
pub fn read_handoff(project: &Path, relative: &str) -> focus_js::Result<Handoff> {
    let anchor = std::fs::canonicalize(project)
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", project.display())))?;
    links::refuse_link_in_chain(&anchor, Path::new(relative))?;
    let path = anchor.join(relative);
    let (bytes, written) =
        files::read_regular_with_time(&path, "reverse-engineering developer handoff")?;
    // `mtimeMs`: the seconds as milliseconds, plus the nanoseconds; a time
    // before the epoch counts back from it.
    #[allow(clippy::cast_precision_loss)]
    let millis = |span: std::time::Duration| {
        span.as_secs() as f64 * 1e3 + f64::from(span.subsec_nanos()) / 1e6
    };
    let mtime_ms = match written.duration_since(std::time::UNIX_EPOCH) {
        Ok(since) => millis(since),
        Err(before) => -millis(before.duration()),
    };
    Ok(Handoff { bytes, mtime_ms })
}

/// `sha256:<hex>` of a handoff's bytes.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// `pipelineReceiptArtifactIsCurrent`, for the receipt of the lead's link
/// of `reverse-engineering`: the handoff file it names is inside the
/// record, readable as one, and (unless the guard policy accepts changes)
/// the very file the receipt recorded, by time and by content.
#[must_use]
pub fn artifact_is_current(
    project: &Path,
    record: Option<&Path>,
    receipt: &Receipt,
    accepts_changes: bool,
) -> bool {
    let (Some(named), Some(sha256), Some(mtime_ms)) = (
        receipt
            .artifact_path
            .as_deref()
            .filter(|path| !path.is_empty()),
        receipt.artifact_sha256.as_deref(),
        receipt.artifact_mtime_ms,
    ) else {
        return false;
    };
    if !re::test("^sha256:[0-9a-f]{64}$", "", sha256) {
        return false;
    }
    let Some(record) = record else {
        return false;
    };
    let host = Flavour::host();
    let project_text = project.to_string_lossy();
    let root = record.to_string_lossy();
    let path = jspath::resolve(host, &project_text, &[&project_text, named]);
    if path != root && !path.starts_with(&format!("{root}{}", host.separator())) {
        return false;
    }
    let relative = jspath::relative(host, &project_text, &project_text, &path);
    let Ok(handoff) = read_handoff(project, &relative) else {
        return false;
    };
    if accepts_changes {
        return true;
    }
    (handoff.mtime_ms - mtime_ms).abs() <= 0.01 && sha256 == digest(&handoff.bytes)
}

/// The chain of `pipelineLinkEvidence`: of the attempt's receipts, the ones
/// that count, per repository (or for the project, when the work spans
/// none). A receipt of the first link starts the chain again; each later
/// link counts only as the next of a chain already started, and the last
/// only at the chain's own length. `is_current` says whether the first
/// link's handoff is still the one it recorded.
#[must_use]
pub fn chain(
    receipts: &[Receipt],
    links: &[String],
    repos: &[String],
    is_current: &dyn Fn(&Receipt) -> bool,
) -> Vec<Receipt> {
    let fits = |receipt: &Receipt, index: usize| {
        index + 1 < links.len()
            || receipt
                .position
                .as_deref()
                .is_none_or(|position| position == format!("{0}/{0}", links.len()))
    };
    let scopes: Vec<Option<&str>> = if repos.is_empty() {
        vec![None]
    } else {
        repos.iter().map(|repo| Some(repo.as_str())).collect()
    };
    let mut kept = Vec::new();
    for repo in scopes {
        let mut chain: Vec<&Receipt> = Vec::new();
        for receipt in receipts
            .iter()
            .filter(|receipt| receipt.repo.as_deref() == repo)
        {
            if links.first() == Some(&receipt.link) {
                chain.clear();
                if is_current(receipt) && fits(receipt, 0) {
                    chain.push(receipt);
                }
            } else if !chain.is_empty()
                && links.get(chain.len()) == Some(&receipt.link)
                && fits(receipt, chain.len())
            {
                chain.push(receipt);
            }
        }
        kept.extend(chain.into_iter().cloned());
    }
    kept
}

//! The authorization a summary confirmation gives.
//!
//! A person's "Looks correct" at a stage's summary authorizes the outputs
//! generated from it. The authorization has an id: a hash of the attempt of
//! the stage, the stage, the unit, the workflow, the questions file, what
//! was confirmed in it, and the choice. The receipt row carries it, and the
//! record keeps the one that is active for the stage under its engine
//! directory (`.focus-engine/summary-authorization/<stage>/stage.json`), so
//! that every later write of a stage output can be stamped with it and a
//! completion can ask whether each output descends from it. The same
//! confirmation gives the same id, so "Looks correct" said twice reaffirms;
//! other answers give another, and the outputs are regenerated under it. A
//! "Request changes" withdraws the active one.
//!
//! This module is the id, the attempt it binds to, and the file
//! (`summaryAuthorizationId`, `summaryAttemptFloors`,
//! `summaryAttemptIdentity`, `writeSummaryAuthorization`,
//! `clearSummaryAuthorization` in `aidlc-lib.ts`, L12 979 to L13 190).
//! Reading it back is the gate's, with the receipts (slice 3).
//!
//! # What is not ported
//!
//! A stage that runs per unit of work has an attempt of its own shape (a
//! unit's attempt, and the stage starts that a change of Construction's
//! policy leaves out) and a file per unit. Both come with the units (slice
//! 5): the caller refuses such a stage by name, and nothing here is asked
//! about one.

use std::path::{Path, PathBuf};

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_graph::runtime::Stage;
use focus_js::json::{stringify, stringify_pretty};
use focus_js::{EngineError, Result, bail, re};
use focus_workspace::names::ENGINE_DIR;
use focus_workspace::{atomic, links, observer};
use serde_json::json;
use sha2::{Digest, Sha256};

/// `SUMMARY_AUTHORIZATION_FIELD`: the field of a receipt row and of an
/// output's row.
pub const FIELD: &str = "Summary Authorization Id";

/// `SUMMARY_AUTHORIZATION_DIR`, under a record.
fn dir() -> String {
    format!("{ENGINE_DIR}/summary-authorization")
}

/// `SummaryAuthorization`: what the record keeps for a stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorization {
    pub id: String,
    pub stage: String,
    pub workflow: Option<String>,
    pub attempt: String,
    pub questions_file: String,
    pub questions_sha256: String,
    pub choice: String,
    pub recorded_at: String,
}

/// `summaryAuthorizationId`, for a stage that does not run per unit.
#[must_use]
pub fn id(
    attempt: &str,
    stage: &str,
    workflow: Option<&str>,
    questions_file: &str,
    questions_sha256: &str,
    choice: &str,
) -> String {
    let named = json!([
        attempt,
        stage,
        null,
        workflow,
        questions_file,
        questions_sha256,
        choice
    ]);
    Sha256::digest(stringify(&named).as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `stageJumpReaches`: whether a jump's row starts a new attempt of `slug`.
/// A jump resets its target and every stage after it, so a stage before the
/// target keeps its attempt. A row whose target, or a stage, the graph does
/// not know reaches every stage.
fn jump_reaches(block: &str, slug: &str, graph: &[Stage]) -> bool {
    let index = |name: &str| graph.iter().position(|stage| stage.slug == name);
    match block_field(block, "Target").filter(|name| !name.is_empty()) {
        None => true,
        Some(name) if name == slug => true,
        Some(name) => match (index(name), index(slug)) {
            (Some(target), Some(at)) => at >= target,
            _ => true,
        },
    }
}

/// `latestEventFrontier`: the newest row of each shard, and of those the
/// ones at the newest instant.
#[must_use]
pub fn frontier<'a>(rows: &[&'a ShardEvent]) -> Vec<&'a ShardEvent> {
    let mut newest: Vec<&ShardEvent> = Vec::new();
    for row in rows {
        match newest.iter_mut().find(|kept| kept.shard == row.shard) {
            Some(kept) if row.pos > kept.pos => *kept = row,
            Some(_) => {}
            None => newest.push(row),
        }
    }
    let latest = newest
        .iter()
        .map(|row| row.timestamp.as_str())
        .max_by(|a, b| focus_js::order::cmp_utf16(a, b))
        .unwrap_or("");
    newest
        .iter()
        .filter(|row| row.timestamp == latest)
        .copied()
        .collect()
}

/// `summaryAttemptFloors`, for a stage that does not run per unit: the
/// boundary of the attempt a confirmation binds to. For a stage run by
/// itself (`workflow`), its last completion; otherwise the newest of the
/// creation of the work, a jump that reaches the stage, and a start of the
/// stage. Empty when the ledger has none.
#[must_use]
pub fn attempt_floors<'a>(
    events: &'a [ShardEvent],
    stage: &str,
    workflow: Option<&str>,
    graph: &[Stage],
) -> Vec<&'a ShardEvent> {
    let candidates: Vec<&ShardEvent> = events
        .iter()
        .filter(|row| {
            let of = block_field(&row.block, "Workflow");
            if let Some(workflow) = workflow {
                return row.event == "STAGE_COMPLETED"
                    && of == Some(workflow)
                    && block_field(&row.block, "Stage") == Some(stage);
            }
            if of.is_some_and(|of| of.starts_with("single-stage:")) {
                return false;
            }
            match row.event.as_str() {
                "WORKFLOW_STARTED" => true,
                "STAGE_JUMPED" => jump_reaches(&row.block, stage, graph),
                "STAGE_STARTED" => block_field(&row.block, "Stage") == Some(stage),
                _ => false,
            }
        })
        .collect();
    frontier(&candidates)
}

/// `summaryAttemptIdentity`: a stable name for an attempt's boundary, from
/// the identities of its rows.
#[must_use]
pub fn attempt_identity(floors: &[&ShardEvent]) -> String {
    if floors.is_empty() {
        return "unstarted".to_owned();
    }
    let mut named: Vec<String> = floors
        .iter()
        .map(|row| format!("{}:{}:{}:{}", row.event, row.timestamp, row.shard, row.pos))
        .collect();
    named.sort_by(|a, b| focus_js::order::cmp_utf16(a, b));
    named.join("|")
}

/// `summaryAuthorizationRelativePath`, for a stage that does not run per
/// unit: where the record keeps the stage's active authorization.
///
/// # Errors
///
/// The stage's name is not one.
pub fn relative_path(stage: &str) -> Result<String> {
    if !re::test("^[a-z0-9][a-z0-9-]*$", "", stage) {
        bail!("Invalid stage slug \"{stage}\".");
    }
    Ok(format!("{}/{stage}/stage.json", dir()))
}

fn real(record: &Path) -> Result<PathBuf> {
    std::fs::canonicalize(record)
        .map_err(|err| EngineError::new(format!("{}: {err}", record.display())))
}

/// `summaryAuthorizationTargetOrThrow`: where the stage's authorization is
/// written or removed, reached through no link. Asked before the receipt
/// that depends on it is recorded.
///
/// # Errors
///
/// The stage's name is not one; a folder on the way, or the file, is a
/// link; the record cannot be resolved.
pub fn target(record: &Path, stage: &str) -> Result<PathBuf> {
    let relative = relative_path(stage)?;
    let anchor = real(record)?;
    links::refuse_link_in_chain(&anchor, Path::new(&relative))?;
    Ok(anchor.join(relative))
}

/// `writeRecordFileNoFollow`: `data` at `relative` under the record,
/// through no link, its folders made.
fn write_record_file(record: &Path, relative: &str, data: &[u8]) -> Result<()> {
    let anchor = real(record)?;
    links::refuse_link_in_chain(&anchor, Path::new(relative))?;
    let path = anchor.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| EngineError::new(format!("{}: {err}", parent.display())))?;
    }
    // The folders exist now: none of them, nor the file, may be a link.
    links::refuse_link_in_chain(&anchor, Path::new(relative))?;
    // `writeBufferAtomic`, the primitive upstream writes a record's file with.
    observer::refuse_write("writeBufferAtomic")?;
    atomic::write_atomic(&path, data)
        .map_err(|err| EngineError::new(format!("{}: {err}", path.display())))
}

/// `removeRecordFileNoFollow`: the file at `relative` under the record
/// removed, never through a link; one that is not there is removed already.
fn remove_record_file(record: &Path, relative: &str) -> Result<()> {
    observer::refuse_write("removeRecordFileNoFollow")?;
    let anchor = real(record)?;
    links::refuse_link_in_chain(&anchor, Path::new(relative))?;
    let path = anchor.join(relative);
    match std::fs::remove_file(&path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(EngineError::new(format!("{}: {err}", path.display())))
        }
        _ => Ok(()),
    }
}

/// `writeSummaryAuthorization`: the stage's active authorization.
///
/// # Errors
///
/// As [`target`]; the file cannot be written; the process is an observer.
pub fn write(record: &Path, authorization: &Authorization) -> Result<()> {
    let kept = json!({
        "version": 1,
        "id": authorization.id,
        "stage": authorization.stage,
        "unit": null,
        "workflow": authorization.workflow,
        "attempt": authorization.attempt,
        "questions_file": authorization.questions_file,
        "questions_sha256": authorization.questions_sha256,
        "choice": authorization.choice,
        "recorded_at": authorization.recorded_at,
    });
    write_record_file(
        record,
        &relative_path(&authorization.stage)?,
        format!("{}\n", stringify_pretty(&kept)).as_bytes(),
    )
}

/// `clearSummaryAuthorization`: the stage has no active authorization. The
/// registry's folder is made even so, so that an authorization of an
/// earlier layout cannot come back through a reader's fallback.
///
/// # Errors
///
/// As [`target`]; the process is an observer.
pub fn clear(record: &Path, stage: &str) -> Result<()> {
    let relative = relative_path(stage)?;
    observer::refuse_write("clearSummaryAuthorization")?;
    let anchor = real(record)?;
    let registry = dir();
    links::refuse_link_in_chain(&anchor, Path::new(&registry))?;
    let folder = anchor.join(&registry);
    std::fs::create_dir_all(&folder)
        .map_err(|err| EngineError::new(format!("{}: {err}", folder.display())))?;
    remove_record_file(record, &relative)
}

/// `writeRecordFileNoFollow` and `removeRecordFileNoFollow`, to put a
/// stage's file back as it was when the receipt that depends on it could
/// not be written: `before` is what it held, or nothing.
///
/// # Errors
///
/// As [`write`] and [`clear`].
pub fn restore(record: &Path, stage: &str, before: Option<&[u8]>) -> Result<()> {
    let relative = relative_path(stage)?;
    match before {
        Some(bytes) => write_record_file(record, &relative, bytes),
        None => remove_record_file(record, &relative),
    }
}

//! The review receipts that still stand.
//!
//! `freshReviewReceipts` (`aidlc-lib.ts` L19 409): what the ledger says of a
//! stage's reviews after the boundary of its current attempt, weighed
//! against the bytes on disk. The answer is the verdict that stands for the
//! stage and for each unit, the ones a later write made stale and the pass
//! their next review is, the requests that wait for a verdict and what can
//! be done with each, whether the project's source is still the one the
//! newest review saw, the baseline the stage started from, and the changes
//! the Guard Policy let through instead of holding the review stale.
//!
//! The rows are read in order, once:
//!
//! - a bolt that starts again clears its units (held back while the row is
//!   tied with another shard's, until the instant has passed);
//! - a gate that rejects a unit makes its verdict stale;
//! - a write of an artifact the review covers makes the verdict stale, or,
//!   under a relaxed policy, is named in the change the policy accepts;
//! - a request is kept until its verdict comes;
//! - a verdict counts when it carries the verified review its request
//!   needs and it is terminal: `READY`, or `NOT-READY` at the last pass.
//!   Its artifact fingerprint is compared with the files as they are.
//!
//! # What is not read yet
//!
//! Two things, each refused by name where the answer would rest on it:
//!
//! - A unit's own source binding. A unit's verdict that recorded a `Unit
//!   Source Fingerprint` is weighed against the unit's manifest and the
//!   listing its reviewer saw (`readUnitSourceManifest`,
//!   `readUnitSourceSnapshot`, `sourceClaimCovers`): units arrive with a
//!   later slice. So `freshUnitClaims`, `unitSourceAttributed` and
//!   `unitSourceMoved` have no counterpart here: they are only ever filled
//!   by that reading.
//! - A row that names a unit while units are owned by a team: whether it
//!   belongs to the current claim (`eventMatchesClaimAttempt`) is deferred
//!   with team ownership.

use std::cell::{Cell, OnceCell};
use std::collections::BTreeSet;
use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::authorization::jump_reaches;
use focus_evidence::review::{self, RequestBinding};
use focus_evidence::review_record;
use focus_evidence::source::listing::{Listing, render_changed_paths};
use focus_evidence::source::raw::current_form;
use focus_evidence::source::snapshot::{self, BASELINE_FIELD, UNBINDABLE};
use focus_evidence::source::state::same_workspace_source;
use focus_evidence::source::walk::WorkspaceSource;
use focus_graph::runtime::Stage;
use focus_js::text::trim;
use focus_js::{Result, bail, re};
use focus_state::fields::get_field;
use focus_workspace::names::ENGINE_DIR;
use serde_json::Value;

use crate::artifacts::{self, Dag};
use crate::gate::{produces_artifact_unit, reviewed_artifact_unit};
use crate::gates::guard_policy_accepts_changes;
use crate::review_attempt::{self, Window, definitely_before, is_team_unit_ownership};
use crate::review_pending::{self, pending_request_currency};
use crate::workflow::Workflow;
use crate::{admission, review_artifacts, stage, summary_evidence};

/// `REVIEW_FINISHED_FIELD`: whether a reviewer gave the row's verdict.
const REVIEW_FINISHED_FIELD: &str = "Review Finished";
/// The folder a delegated worktree keeps its own files in, and the main
/// checkout its worktrees.
const HIDDEN_WORKSPACE_DIR: &str = ".focus";

/// A JavaScript `Map` keyed by text: a key set again keeps its place, and
/// one deleted and set again goes to the end. A `Set` is one with no
/// values.
#[derive(Debug, Clone, PartialEq)]
pub struct Ordered<V>(Vec<(String, V)>);

impl<V> Default for Ordered<V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<V> Ordered<V> {
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&V> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.0.iter().map(|(key, value)| (key.as_str(), value))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(key, _)| key.as_str())
    }

    fn set(&mut self, key: &str, value: V) {
        match self.0.iter_mut().find(|(name, _)| name == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_owned(), value)),
        }
    }

    fn delete(&mut self, key: &str) -> bool {
        let before = self.0.len();
        self.0.retain(|(name, _)| name != key);
        self.0.len() != before
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

/// A JavaScript `Set` of text.
pub type Names = Ordered<()>;

/// `StaleReviewProgress`: where a review that has to be taken again
/// resumes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaleProgress {
    pub next_iteration: f64,
    pub recovery_spent: bool,
}

/// What a pass that has no verdict that stands waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    /// The reviewer said `NOT-READY` below the last pass and the artifacts
    /// are the ones reviewed: the stage owes a repair.
    RepairRequired,
    /// The next move is a new request.
    Outstanding,
    /// The request stands and its review has to be dispatched again.
    RetryRequired,
}

impl PendingState {
    /// Upstream's word.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RepairRequired => "repair-required",
            Self::Outstanding => "outstanding",
            Self::RetryRequired => "retry-required",
        }
    }
}

/// `PendingReviewProgress`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pending {
    pub state: PendingState,
    pub iteration: f64,
    pub recovery: bool,
    /// The `NOT-READY` is the one no reviewer gave.
    pub did_not_finish: bool,
    /// A completion answered the request and did not carry its review.
    pub verification_failed: bool,
}

/// A change after a review that the Guard Policy let through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedChange {
    pub stage: String,
    pub unit: Option<String>,
    /// The paths that changed, from the project; `None` when none can be
    /// named.
    pub changed: Option<Vec<String>>,
    /// What the review recorded, and what is there now.
    pub recorded: String,
    pub current: String,
    /// The one line the person hears.
    pub notice: String,
}

/// `SourceBaselineResult`: the listing the stage started from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Baseline {
    /// The ledger is from before baselines.
    #[default]
    Legacy,
    /// The row could not bind the source, or two rows tie on it.
    Unbindable,
    /// A baseline is owed and cannot be read back.
    Invalid,
    Ready(Listing),
}

/// `FreshReviewReceipts`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Receipts {
    /// `READY` or `NOT-READY`.
    pub stage_verdict: Option<String>,
    pub stage_stale: bool,
    pub unit_verdicts: Ordered<String>,
    pub newest_source_fingerprint: Option<String>,
    pub newest_source_unit: Option<String>,
    pub source_stale: bool,
    /// `boundary-unbindable`, `source-unreadable` or `fingerprint-mismatch`.
    pub source_stale_reason: Option<&'static str>,
    pub source_stale_progress: Option<StaleProgress>,
    pub source_recovery_spent: bool,
    pub unit_stale: Names,
    /// The units whose verdict stands though its source could not be
    /// checked.
    pub unit_source_kept: Names,
    pub unit_recheck_reopened: Names,
    pub source_baseline: Baseline,
    pub current_source_listing: Option<Listing>,
    pub stage_stale_progress: Option<StaleProgress>,
    pub unit_stale_progress: Ordered<StaleProgress>,
    pub stage_iteration: Option<f64>,
    pub unit_iterations: Ordered<f64>,
    pub stage_pending: Option<Pending>,
    pub unit_pending: Ordered<Pending>,
    /// The scopes with a request and no verdict: a unit, or `""`.
    pub awaiting_verdict: Names,
    /// The scopes whose verdict no reviewer gave.
    pub unfinished_verdicts: Names,
    pub merged_bolt_units: Vec<String>,
    pub open_bolt_units: Vec<String>,
    pub accepted_changes: Vec<AcceptedChange>,
    /// The Guard Policy was read: something had changed.
    pub change_control_read: bool,
}

/// What a caller already has in hand.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options<'a> {
    /// The review class to weigh under, when the caller resolved it:
    /// `adversarial`, `advisory` or `none`.
    pub review_class: Option<&'a str>,
    /// The attempt's window, when the caller took it.
    pub window: Option<&'a Window>,
    /// The project's source, when the caller took it: `Some(None)` says it
    /// could not be read. `None` takes it here, once, when it is needed.
    pub source_state: Option<Option<&'a WorkspaceSource>>,
}

/// `terminalReviewVerdict`: the verdict when it ends the review. `READY`
/// always does; `NOT-READY` does at the last pass, and at any pass of an
/// advisory review.
#[must_use]
pub fn terminal_review_verdict<'a>(
    verdict: Option<&'a str>,
    iteration: Option<&str>,
    review_class: &str,
    max_iterations: f64,
) -> Option<&'a str> {
    if review_class == "none" {
        return None;
    }
    match verdict? {
        "READY" => verdict,
        "NOT-READY" => iteration
            .filter(|passes| re::test("^[1-9][0-9]*$", "", passes))
            .and_then(|passes| passes.parse::<f64>().ok())
            .filter(|passes| review_class == "advisory" || *passes >= max_iterations)
            .and(verdict),
        _ => None,
    }
}

/// `gateStagesFromBlock`: the stages a gate's row is about.
#[must_use]
pub fn gate_stages(block: &str) -> Vec<&str> {
    match block_field(block, "Gate Stages").filter(|listed| !listed.is_empty()) {
        Some(listed) => listed
            .split(',')
            .map(trim)
            .filter(|name| !name.is_empty())
            .collect(),
        None => block_field(block, "Stage")
            .filter(|name| !name.is_empty())
            .into_iter()
            .collect(),
    }
}

/// `unitPlainName`: a unit's name as a person says it.
#[must_use]
pub fn unit_plain_name(unit: &str) -> String {
    let named = re::replace(r"^u\d+[-_]", "i", unit, "");
    let spaced = re::replace("[-_]+", "g", &named, " ");
    let plain = trim(&spaced);
    if plain.is_empty() {
        unit.to_owned()
    } else {
        plain.to_owned()
    }
}

/// `reviewRecordAbsent`: whether the review record a completion names is
/// simply not in this checkout. A name that is no record's, a link on the
/// way, or anything the system refuses to look at is not absence.
#[must_use]
pub fn review_record_absent(record: Option<&Path>, relative: &str) -> bool {
    if !review::is_record_relative_path(relative) {
        return false;
    }
    let Some(record) = record else {
        return true;
    };
    let mut at = record.to_path_buf();
    for part in relative.split('/') {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(meta) if meta.file_type().is_symlink() => return false,
            Ok(_) => {}
            Err(err) => return err.kind() == std::io::ErrorKind::NotFound,
        }
    }
    false
}

/// `reviewCompletionDidNotFinish`: whether a completion's `NOT-READY` is
/// the one written when no reviewer gave a verdict. A row from before the
/// field is known by its empty review record.
#[must_use]
pub fn review_completion_did_not_finish(record: Option<&Path>, block: &str) -> bool {
    if let Some(marked) = block_field(block, REVIEW_FINISHED_FIELD) {
        return marked == "no";
    }
    if block_field(block, "Verdict") != Some("NOT-READY") {
        return false;
    }
    // `pairedReviewRecordForCompletion`.
    review::record_ref(block)
        .and_then(|named| review_record::read(record, &named))
        .filter(|found| review_record::matches_completion(found, block))
        .is_some_and(|found| found.document.get("body").and_then(Value::as_str) == Some(""))
}

/// `sourceBaselineBoundaryValue`: the `Source Baseline` of a row that can
/// open the stage's work on the source. The outer `None` is a row that
/// cannot; the inner one, a row that can and carries no baseline.
fn baseline_boundary_value<'a>(
    row: &'a ShardEvent,
    slug: &str,
    unit_major: bool,
    graph: &[Stage],
) -> Option<Option<&'a str>> {
    if single_stage(row) {
        return None;
    }
    let qualifies = row.event == "WORKFLOW_STARTED"
        || (row.event == "STAGE_JUMPED" && jump_reaches(&row.block, slug, graph))
        || (row.event == "STAGE_STARTED"
            && !unit_major
            && block_field(&row.block, "Stage") == Some(slug));
    qualifies.then(|| block_field(&row.block, BASELINE_FIELD))
}

/// A row of a stage run by itself.
fn single_stage(row: &ShardEvent) -> bool {
    block_field(&row.block, "Workflow").is_some_and(|name| name.starts_with("single-stage:"))
}

/// `hasDurableSourceBindingEvidence`: a file that only an engine that binds
/// reviews to the source leaves: a snapshot under the record, a unit's
/// source manifest, or a worktree that names this record.
fn has_durable_source_binding_evidence(wf: &Workflow) -> bool {
    if let Some(record) = wf.selection.record_dir(&wf.layout) {
        fn has_snapshot(dir: &Path) -> bool {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return false;
            };
            entries.flatten().any(|entry| {
                let Ok(kind) = entry.file_type() else {
                    return false;
                };
                if kind.is_file() && entry.file_name().to_string_lossy().ends_with(".tsv") {
                    true
                } else {
                    kind.is_dir() && has_snapshot(&entry.path())
                }
            })
        }
        if has_snapshot(&record.join(ENGINE_DIR).join("source-review")) {
            return true;
        }
        let construction = record.join("construction");
        let manifest = |unit: std::fs::DirEntry| {
            unit.path()
                .join("code-generation")
                .join("source-manifest.json")
                .exists()
        };
        if std::fs::read_dir(&construction).is_ok_and(|units| units.flatten().any(manifest)) {
            return true;
        }
    }
    let Some(expected) = wf.selection.relative_record_dir() else {
        return false;
    };
    let hidden = wf.layout.project.join(HIDDEN_WORKSPACE_DIR);
    let mut candidates = vec![hidden.join("worktree-meta.json")];
    if let Ok(worktrees) = std::fs::read_dir(hidden.join("worktrees")) {
        for entry in worktrees.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            // `parseBoltName`: a bolt's worktree, by either form of its name.
            let bolt = re::test("^bolt-([0-9a-f]{8})_([a-z][a-z0-9-]*)$", "", &name)
                || re::test("^bolt-([a-z][a-z0-9-]*)$", "", &name);
            if bolt && entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                candidates.push(
                    entry
                        .path()
                        .join(HIDDEN_WORKSPACE_DIR)
                        .join("worktree-meta.json"),
                );
            }
        }
    }
    candidates.iter().any(|path| {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| {
                focus_js::json::parse(String::from_utf8_lossy(&bytes).as_ref()).ok()
            })
            .is_some_and(|meta: Value| {
                meta.get("intentRecord").and_then(Value::as_str) == Some(expected.as_str())
            })
    })
}

/// `hasModernSourceBindingEvidence`: whether the piece of work was ever
/// bound to its source, by a row of its ledger or by a file beside it. A
/// stage that writes code then owes a baseline.
fn has_modern_source_binding_evidence(wf: &Workflow, rows: &[ShardEvent]) -> bool {
    let has = |row: &ShardEvent, field: &str| block_field(&row.block, field).is_some();
    rows.iter().any(|row| {
        has(row, BASELINE_FIELD)
            || (row.event == "REVIEW_COMPLETED"
                && has(row, "Unit")
                && (has(row, "Unit Source Fingerprint") || has(row, "Unit Source Binding Bypass")))
            || ((row.event == "WORKTREE_CREATED" || row.event == "BOLT_STARTED")
                && (has(row, "Base commit") || has(row, "Base Source Listing")))
            || (row.event == "SWARM_UNIT_CONVERGED"
                && (has(row, "Source Commit") || has(row, "Source Freshness Bypass")))
            || row.event == "SWARM_SOURCE_MERGED"
    }) || has_durable_source_binding_evidence(wf)
}

/// A request that waits for its verdict.
struct Request {
    unit: Option<String>,
    iteration: f64,
    recovery: bool,
    binding: RequestBinding,
    timestamp: String,
    shard: String,
    verification_failed: bool,
}

/// What a unit's standing verdict recorded of the unit's own source.
struct UnitReceipt {
    fingerprint: Option<String>,
    bypass: bool,
    order: usize,
    timestamp: String,
    shard: String,
    iteration: f64,
    recovery: bool,
}

/// The Guard Policy, read the first time something has changed and never
/// again: the answer says whether it was read.
struct Policy<'a> {
    wf: &'a Workflow,
    state: &'a str,
    read: Cell<bool>,
    resolved: OnceCell<bool>,
}

impl Policy<'_> {
    fn relaxed(&self) -> bool {
        *self.resolved.get_or_init(|| {
            self.read.set(true);
            guard_policy_accepts_changes(self.wf, self.state)
        })
    }
}

/// What the rows have said so far.
#[derive(Default)]
struct Scan {
    unit_verdicts: Ordered<String>,
    unfinished_verdicts: Names,
    unit_stale: Names,
    unit_stale_progress: Ordered<StaleProgress>,
    unit_iterations: Ordered<f64>,
    unit_receipt_recovery: Ordered<bool>,
    unit_pending: Ordered<Pending>,
    pending_requests: Ordered<Request>,
    unit_receipts: Ordered<UnitReceipt>,
    stage_verdict: Option<String>,
    newest_source_fingerprint: Option<String>,
    newest_source_unit: Option<String>,
    newest_source_progress: Option<StaleProgress>,
    source_recovery_spent: bool,
    stage_stale: bool,
    stage_stale_progress: Option<StaleProgress>,
    stage_iteration: Option<f64>,
    stage_receipt_recovery: bool,
    stage_pending: Option<Pending>,
}

impl Scan {
    /// Where a unit's review resumes after its verdict fell.
    fn unit_progress(&self, unit: &str) -> StaleProgress {
        StaleProgress {
            next_iteration: self.unit_iterations.get(unit).copied().unwrap_or(0.0) + 1.0,
            recovery_spent: self
                .unit_receipt_recovery
                .get(unit)
                .copied()
                .unwrap_or(false),
        }
    }

    fn drop_requests_of(&mut self, unit: &str) {
        self.pending_requests
            .0
            .retain(|(_, request)| request.unit.as_deref() != Some(unit));
    }

    /// `resetUnitReviewState`: a unit whose bolt starts again starts its
    /// reviews again.
    fn reset_unit(&mut self, unit: &str) {
        self.drop_requests_of(unit);
        self.unit_verdicts.delete(unit);
        self.unit_stale.delete(unit);
        self.unit_stale_progress.delete(unit);
        self.unit_iterations.delete(unit);
        self.unit_receipt_recovery.delete(unit);
        self.unit_pending.delete(unit);
        if self.newest_source_unit.as_deref() == Some(unit) {
            self.newest_source_fingerprint = None;
            self.newest_source_unit = None;
            self.newest_source_progress = None;
            self.source_recovery_spent = false;
        }
    }

    /// A unit's verdict falls: it is stale, and its review resumes at
    /// `progress`.
    fn stale_unit(&mut self, unit: &str, progress: StaleProgress) {
        self.unit_verdicts.delete(unit);
        self.unit_stale.set(unit, ());
        self.unit_stale_progress.set(unit, progress);
    }
}

/// The line a person hears when a change after a review is let through.
fn relaxed_notice(what: &str, unit: Option<&str>, stage_name: &str) -> String {
    let after = match unit.filter(|unit| !unit.is_empty()) {
        Some(unit) => format!("the {} Unit was reviewed", unit_plain_name(unit)),
        None => format!("the {stage_name} review"),
    };
    format!("{what} changed after {after}; carrying on.")
}

/// `eventMatchesClaimAttempt`, for a row that names a unit: every row
/// matches while units are not owned by a team.
///
/// # Errors
///
/// Units are owned by a team, which is not read yet.
fn refuse_team_claims(team: &OnceCell<bool>, wf: &Workflow, row: &ShardEvent) -> Result<()> {
    if block_field(&row.block, "Unit").is_none_or(str::is_empty) {
        return Ok(());
    }
    // (The state file's own word, not the state the caller handed in.)
    let owned = *team.get_or_init(|| {
        wf.read_state()
            .is_ok_and(|state| is_team_unit_ownership(&state))
    });
    if owned {
        bail!(
            "A review row names a unit while units are owned by a team, and Focus does not read team claims yet: whether the row belongs to the current claim cannot be told."
        );
    }
    Ok(())
}

/// `freshReviewReceipts`: the reviews of `stage` that still stand, for the
/// selected piece of work in `state`.
///
/// # Errors
///
/// A unit's verdict is bound to the unit's own source, or a row names a
/// unit while units are owned by a team: neither is read yet (see the
/// module docs). The record's repositories are not a list of names. The
/// project's source is behind something that is not read yet. A snapshot
/// lists a path no key can be made of.
#[allow(clippy::too_many_lines)]
pub fn fresh(wf: &Workflow, state: &str, stage: &Stage, options: Options<'_>) -> Result<Receipts> {
    let mut empty = Receipts::default();
    let Some(reviewer) = stage::reviewer(stage) else {
        return Ok(empty);
    };
    let review_class = options
        .review_class
        .or_else(|| stage::review_class(stage))
        .unwrap_or("adversarial");
    if review_class == "none" {
        return Ok(empty);
    }
    let max_iterations = if review_class == "advisory" {
        1.0
    } else {
        stage
            .raw
            .get("reviewer_max_iterations")
            .and_then(Value::as_f64)
            .unwrap_or(2.0)
    };
    let slug = stage.slug.as_str();
    let graph = &wf.install.graph;
    let per_unit = stage::has_unit_marker(stage)
        && !artifacts::uses_stage_level_per_unit_artifacts(
            wf,
            get_field(state, "Scope").unwrap_or_default(),
            state,
        );
    let checkpoints = get_field(state, "Construction Checkpoints") == Some("enabled");
    let unit_major = per_unit
        && (get_field(state, "Construction Iteration").map(trim) == Some("unit-major")
            || checkpoints);
    let team_ownership = per_unit && (is_team_unit_ownership(state) || checkpoints);
    let unit_scoped_rejections = per_unit && review_attempt::unit_scoped_lifecycle_floors(state);
    let taken;
    let window = match options.window {
        Some(window) => window,
        None => {
            taken = review_attempt::window(wf, state, stage, wf.events());
            &taken
        }
    };
    let events = &window.events;
    empty.merged_bolt_units.clone_from(&window.merged_bolt_units);
    empty.open_bolt_units.clone_from(&window.open_bolt_units);
    let workspace_requires = stage::workspace_requires(stage);
    let modern_evidence = has_modern_source_binding_evidence(wf, &window.all_events);
    if events.is_empty() {
        if workspace_requires && modern_evidence {
            empty.source_baseline = Baseline::Invalid;
        }
        return Ok(empty);
    }
    // (A merge's own row has no reviewer's authority: one at the same
    // instant in another shard makes nothing here ambiguous.)
    let tied = |index: usize| {
        let row = &events[index];
        events.iter().enumerate().any(|(other, candidate)| {
            other != index
                && candidate.event != "AUDIT_MERGED"
                && candidate.timestamp == row.timestamp
                && candidate.shard != row.shard
        })
    };
    let tied_with_session_boundaries_only = |index: usize| {
        let row = &events[index];
        let mut saw = false;
        for (other, candidate) in events.iter().enumerate() {
            if other == index
                || candidate.timestamp != row.timestamp
                || candidate.shard == row.shard
                || candidate.event == "AUDIT_MERGED"
            {
                continue;
            }
            if candidate.event != "SESSION_STARTED" && candidate.event != "SESSION_RESUMED" {
                return false;
            }
            saw = true;
        }
        saw
    };

    let dag = per_unit.then(|| artifacts::resolve_bolt_dag(wf));
    let mut observed: Vec<&String> = Vec::new();
    for unit in window
        .merged_bolt_units
        .iter()
        .chain(&window.open_bolt_units)
    {
        if !observed.contains(&unit) {
            observed.push(unit);
        }
    }
    // The units the stage's reviews can be of: `None` when any can.
    let applicable: Option<Vec<&str>> = match &dag {
        Some(Dag::Ok {
            units, unit_kinds, ..
        }) => Some(
            units
                .iter()
                .filter(|unit| {
                    let kind = unit_kinds.as_ref().and_then(|kinds| {
                        kinds
                            .iter()
                            .find(|(name, _)| name == *unit)
                            .map(|(_, kind)| kind.as_str())
                    });
                    !artifacts::filter_produces_by_kind(
                        stage::produces_kinds(stage),
                        &stage.produces,
                        kind,
                    )
                    .is_empty()
                })
                .map(String::as_str)
                .collect(),
        ),
        Some(Dag::None) if !observed.is_empty() => {
            Some(observed.iter().map(|unit| unit.as_str()).collect())
        }
        _ => None,
    };
    let dag_ok = matches!(dag, Some(Dag::Ok { .. }));
    let dag_none = matches!(dag, Some(Dag::None));

    let recorded_repos = crate::registry::intent_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    let record = wf.selection.record_dir(&wf.layout);
    let read_record = |named: &review::RecordRef| review_record::read(record.as_deref(), named);
    let merged: BTreeSet<String> = window.merged_bolt_units.iter().cloned().collect();
    let policy = Policy {
        wf,
        state,
        read: Cell::new(false),
        resolved: OnceCell::new(),
    };
    let team = OnceCell::new();
    let stage_name = graph
        .iter()
        .find(|node| node.slug == slug)
        .map_or(slug, stage::name);
    let mut scan = Scan::default();
    // Per scope (a unit, or `""`): the change the policy accepts, opened
    // when a terminal verdict's fingerprint is no longer the files', and
    // given the paths written after it.
    let mut accepted_artifact_changes: Ordered<AcceptedChange> = Ordered::default();
    let mut accepted_changes: Vec<AcceptedChange> = Vec::new();
    // Source that cannot be checked against its review on this machine is
    // said once.
    let mut unchecked_source_noticed = false;
    let mut accept_unchecked_source =
        |accepted: &mut Vec<AcceptedChange>, unit: Option<&str>, recorded: &str, current: Option<&str>| {
            if unchecked_source_noticed {
                return;
            }
            unchecked_source_noticed = true;
            accepted.push(AcceptedChange {
                stage: slug.to_owned(),
                unit: unit.map(str::to_owned),
                changed: None,
                recorded: recorded.to_owned(),
                current: current.unwrap_or("(not readable here)").to_owned(),
                notice: format!(
                    "The project source could not be checked against the {stage_name} review on this machine; carrying on."
                ),
            });
        };

    let mut group_timestamp: Option<&str> = None;
    let mut deferred_bolt_units: Vec<String> = Vec::new();
    let start = window.floor_idx.map_or(0, |floor| floor + 1);
    for (index, row) in events.iter().enumerate().skip(start) {
        if group_timestamp.is_some_and(|instant| instant != row.timestamp) {
            for unit in deferred_bolt_units.drain(..) {
                scan.reset_unit(&unit);
            }
        }
        group_timestamp = Some(&row.timestamp);
        let field = |name: &str| block_field(&row.block, name);
        let event_unit = field("Unit").filter(|unit| !unit.is_empty());
        refuse_team_claims(&team, wf, row)?;
        let reviews = row.event == "REVIEW_REQUESTED" || row.event == "REVIEW_COMPLETED";
        if team_ownership
            && reviews
            && let Some(unit) = event_unit
            && events.iter().any(|candidate| {
                candidate.event == "GATE_REJECTED"
                    && candidate.timestamp == row.timestamp
                    && candidate.shard != row.shard
                    && block_field(&candidate.block, "Unit") == Some(unit)
                    && gate_stages(&candidate.block).contains(&slug)
            })
        {
            continue;
        }
        if unit_scoped_rejections && row.event == "GATE_REJECTED" {
            let Some(rejected) = event_unit.filter(|_| gate_stages(&row.block).contains(&slug))
            else {
                continue;
            };
            if scan.unit_verdicts.has(rejected) {
                let progress = scan.unit_progress(rejected);
                scan.stale_unit(rejected, progress);
            }
            scan.unit_iterations.delete(rejected);
            scan.unit_receipt_recovery.delete(rejected);
            scan.unit_pending.delete(rejected);
            scan.drop_requests_of(rejected);
            continue;
        }
        if row.event == "BOLT_STARTED" && per_unit {
            let units = field("Bolt names")
                .unwrap_or_default()
                .split(',')
                .map(trim)
                .filter(|unit| !unit.is_empty());
            if tied(index) {
                for unit in units {
                    if !deferred_bolt_units.iter().any(|held| held == unit) {
                        deferred_bolt_units.push(unit.to_owned());
                    }
                }
            } else {
                for unit in units {
                    scan.reset_unit(unit);
                }
            }
            continue;
        }
        if row.event == "ARTIFACT_CREATED" || row.event == "ARTIFACT_UPDATED" {
            let Some(file) = field("File").filter(|file| !file.is_empty()) else {
                continue;
            };
            let Some(target) = reviewed_artifact_unit(stage, file, &recorded_repos) else {
                continue;
            };
            // The change the policy governs: a write of a reviewed artifact
            // after a terminal verdict for its scope. A write with no
            // verdict in play reads nothing.
            let receipt_in_play = if !per_unit {
                scan.stage_verdict.is_some()
            } else {
                match &target {
                    None => !scan.unit_verdicts.is_empty(),
                    Some(unit) => scan.unit_verdicts.has(unit),
                }
            };
            if receipt_in_play && policy.relaxed() {
                // The verdict stands. Whether the reviewed bytes differ is
                // what the fingerprint on the verdict said; this write only
                // names a path of that change.
                let scopes: Vec<String> = if !per_unit {
                    vec![String::new()]
                } else {
                    match &target {
                        None => accepted_artifact_changes
                            .keys()
                            .map(str::to_owned)
                            .collect(),
                        Some(unit) => vec![unit.clone()],
                    }
                };
                let changed_path = summary_evidence::row_path_from_project(wf, file);
                for scope in scopes {
                    if let Some((_, accepted)) = accepted_artifact_changes
                        .0
                        .iter_mut()
                        .find(|(name, _)| *name == scope)
                        && let Some(changed) = &mut accepted.changed
                        && !changed.contains(&changed_path)
                    {
                        changed.push(changed_path.clone());
                    }
                }
                continue;
            }
            if !per_unit {
                if scan.stage_verdict.is_some() {
                    scan.stage_stale = true;
                    scan.stage_stale_progress = Some(StaleProgress {
                        next_iteration: scan.stage_iteration.unwrap_or(0.0) + 1.0,
                        recovery_spent: scan.stage_receipt_recovery,
                    });
                }
                scan.stage_verdict = None;
                scan.stage_iteration = None;
                scan.stage_receipt_recovery = false;
            } else if let Some(unit) = &target {
                if scan.unit_verdicts.has(unit) {
                    let progress = scan.unit_progress(unit);
                    scan.stale_unit(unit, progress);
                }
                scan.unit_iterations.delete(unit);
                scan.unit_receipt_recovery.delete(unit);
                scan.unit_receipts.delete(unit);
            } else {
                // A path that names no one unit clears every unit's verdict.
                let units: Vec<String> = scan.unit_verdicts.keys().map(str::to_owned).collect();
                for unit in &units {
                    let progress = scan.unit_progress(unit);
                    scan.unit_stale.set(unit, ());
                    scan.unit_stale_progress.set(unit, progress);
                }
                scan.unit_verdicts.clear();
                scan.unit_iterations.clear();
                scan.unit_receipt_recovery.clear();
                scan.unit_receipts.clear();
            }
            continue;
        }
        if !reviews {
            continue;
        }
        if single_stage(row) || field("Stage") != Some(slug) || field("Reviewer") != Some(reviewer)
        {
            continue;
        }
        let Some(iteration_field) =
            field("Iteration").filter(|passes| re::test("^[1-9][0-9]*$", "", passes))
        else {
            continue;
        };
        let iteration: f64 = iteration_field.parse().unwrap_or(f64::INFINITY);
        let unit = event_unit;
        let applies = |unit: &str| {
            applicable
                .as_ref()
                .is_some_and(|units| units.contains(&unit))
        };
        if per_unit && dag_ok && !unit.is_some_and(applies) {
            continue;
        }
        if per_unit && dag_none && applicable.is_some() && unit.is_some_and(|unit| !applies(unit)) {
            continue;
        }
        let scope = unit.unwrap_or_default();
        let request_key = format!("{scope}\0{iteration_field}");
        if row.event == "REVIEW_REQUESTED" {
            if tied(index) && !tied_with_session_boundaries_only(index) && team_ownership {
                continue;
            }
            let Some(binding) = review::request_binding(&row.block) else {
                continue;
            };
            let recovery = scan
                .pending_requests
                .get(&request_key)
                .is_some_and(|previous| previous.recovery)
                || field("Recovery") == Some("stale-receipt");
            if recovery {
                scan.source_recovery_spent = true;
            }
            scan.pending_requests.set(
                &request_key,
                Request {
                    unit: unit.map(str::to_owned),
                    iteration,
                    recovery,
                    binding,
                    timestamp: row.timestamp.clone(),
                    shard: row.shard.clone(),
                    verification_failed: false,
                },
            );
            continue;
        }
        let Some(verdict) = field("Verdict").filter(|said| matches!(*said, "READY" | "NOT-READY"))
        else {
            continue;
        };
        if team_ownership && tied(index) {
            scan.pending_requests.delete(&request_key);
            continue;
        }
        let Some(request) = scan.pending_requests.get(&request_key) else {
            continue;
        };
        if request.timestamp == row.timestamp && request.shard != row.shard {
            continue;
        }
        if !review_record::completion_carries_verified_review(
            &request.binding,
            &row.block,
            &read_record,
        ) {
            let matches_request = review::completion_matches_request(&request.binding, &row.block);
            let record_ref = matches_request
                .then(|| review::record_ref(&row.block))
                .flatten();
            // Under a relaxed policy, a review whose written record is not
            // on this machine keeps its recorded verdict. A record that is
            // here and is not the one recorded is not that review, under
            // every policy.
            let absent = record_ref
                .as_ref()
                .filter(|named| review_record_absent(record.as_deref(), &named.path));
            let Some(named) = absent.filter(|_| policy.relaxed()) else {
                if matches_request
                    && let Some((_, request)) = scan
                        .pending_requests
                        .0
                        .iter_mut()
                        .find(|(key, _)| *key == request_key)
                {
                    request.verification_failed = true;
                }
                continue;
            };
            accepted_changes.push(AcceptedChange {
                stage: slug.to_owned(),
                unit: unit.map(str::to_owned),
                changed: None,
                recorded: named.digest.clone(),
                current: "(review text not on this machine)".to_owned(),
                notice: format!(
                    "The written review for {stage_name}{} is not on this machine; using its recorded verdict.",
                    unit.map(|unit| format!(" (unit {unit})")).unwrap_or_default()
                ),
            });
        }
        let request_recovery = request.recovery;
        scan.pending_requests.delete(&request_key);
        let recorded_fingerprint = field("Artifact Fingerprint");
        let current_fingerprint = review_artifacts::fingerprint(
            wf,
            stage,
            unit,
            Some(Some(state)),
            review_artifacts::Options {
                require_required_artifacts: false,
                merged_bolt_units: Some(&merged),
            },
        );
        let fingerprints = recorded_fingerprint.zip(current_fingerprint.as_deref());
        let fingerprint_matches =
            fingerprints.is_some_and(|(recorded, current)| current_form(recorded) == current);
        let terminal = if request_recovery {
            Some(verdict)
        } else {
            terminal_review_verdict(
                Some(verdict),
                Some(iteration_field),
                review_class,
                max_iterations,
            )
        };
        if recorded_fingerprint.is_some()
            && terminal.is_some()
            && let Some(source) = field("Source Fingerprint").filter(|source| !source.is_empty())
            && !tied(index)
        {
            // An artifact binding on a terminal verdict is evidence of a
            // review bound to the source, even if the artifacts changed
            // since. A `NOT-READY` below the last pass is repair in
            // progress, not a boundary of freshness.
            scan.newest_source_fingerprint = Some(source.to_owned());
            scan.newest_source_unit = unit.map(str::to_owned);
            scan.newest_source_progress = Some(StaleProgress {
                next_iteration: iteration + 1.0,
                recovery_spent: request_recovery,
            });
        }
        let did_not_finish = verdict == "NOT-READY"
            && review_completion_did_not_finish(record.as_deref(), &row.block);
        let Some(terminal) = terminal else {
            if verdict != "NOT-READY" || fingerprints.is_none() {
                continue;
            }
            let pending = if fingerprint_matches {
                Pending {
                    state: PendingState::RepairRequired,
                    iteration,
                    recovery: request_recovery,
                    did_not_finish,
                    verification_failed: false,
                }
            } else {
                Pending {
                    state: PendingState::Outstanding,
                    iteration: iteration + 1.0,
                    recovery: request_recovery,
                    did_not_finish: false,
                    verification_failed: false,
                }
            };
            match unit {
                Some(unit) => {
                    scan.unit_verdicts.delete(unit);
                    scan.unit_iterations.delete(unit);
                    scan.unit_pending.set(unit, pending);
                }
                None => {
                    scan.stage_verdict = None;
                    scan.stage_iteration = None;
                    scan.stage_pending = Some(pending);
                }
            }
            continue;
        };
        if fingerprint_matches {
            // A newer verdict that matches supersedes a change accepted for
            // its scope.
            accepted_artifact_changes.delete(scope);
        } else {
            let progress = StaleProgress {
                next_iteration: iteration + 1.0,
                recovery_spent: request_recovery,
            };
            match fingerprints.filter(|_| policy.relaxed()) {
                // The reviewed content differs from the verdict's. Under a
                // relaxed policy the verdict stands as recorded; the change
                // is carried to the gate, with the paths written after it.
                Some((recorded, current)) => accepted_artifact_changes.set(
                    scope,
                    AcceptedChange {
                        stage: slug.to_owned(),
                        unit: unit.map(str::to_owned),
                        changed: Some(Vec::new()),
                        recorded: recorded.to_owned(),
                        current: current.to_owned(),
                        notice: String::new(),
                    },
                ),
                None => {
                    if fingerprints.is_some() {
                        match unit {
                            Some(unit) => {
                                scan.unit_stale.set(unit, ());
                                scan.unit_stale_progress.set(unit, progress);
                            }
                            None => {
                                scan.stage_stale = true;
                                scan.stage_stale_progress = Some(progress);
                            }
                        }
                    }
                    continue;
                }
            }
        }
        if did_not_finish {
            scan.unfinished_verdicts.set(scope, ());
        } else {
            scan.unfinished_verdicts.delete(scope);
        }
        match unit {
            Some(unit) => {
                scan.unit_verdicts.set(unit, terminal.to_owned());
                scan.unit_stale.delete(unit);
                scan.unit_stale_progress.delete(unit);
                scan.unit_iterations.set(unit, iteration);
                scan.unit_receipt_recovery.set(unit, request_recovery);
                scan.unit_pending.delete(unit);
                scan.unit_receipts.set(
                    unit,
                    UnitReceipt {
                        fingerprint: field("Unit Source Fingerprint").map(str::to_owned),
                        bypass: field("Unit Source Binding Bypass") == Some("true"),
                        order: index,
                        timestamp: row.timestamp.clone(),
                        shard: row.shard.clone(),
                        iteration,
                        recovery: request_recovery,
                    },
                );
            }
            None => {
                scan.stage_verdict = Some(terminal.to_owned());
                scan.stage_iteration = Some(iteration);
                scan.stage_receipt_recovery = request_recovery;
                scan.stage_pending = None;
                scan.stage_stale = false;
                scan.stage_stale_progress = None;
            }
        }
    }
    for unit in deferred_bolt_units.drain(..) {
        scan.reset_unit(&unit);
    }

    // The project's source is taken once, here or for its freshness.
    let source_taken: OnceCell<Option<WorkspaceSource>> = OnceCell::new();
    let current_source = || -> Result<Option<&WorkspaceSource>> {
        if let Some(given) = options.source_state {
            return Ok(given);
        }
        if source_taken.get().is_none() {
            let _ = source_taken.set(review_pending::current_source(wf)?);
        }
        Ok(source_taken.get().and_then(Option::as_ref))
    };
    let require_required_artifacts = !admission::artifact_guard_disabled(&wf.layout);
    let mut awaiting_verdict = Names::default();
    let requests = std::mem::take(&mut scan.pending_requests);
    for (_, request) in &requests.0 {
        let unit = request.unit.as_deref();
        awaiting_verdict.set(unit.unwrap_or_default(), ());
        // A request whose outputs or source changed before its verdict can
        // never finish: the next move is a new request at the same pass.
        let currency = pending_request_currency(
            wf,
            stage,
            unit,
            &request.binding,
            review_pending::Asked {
                require_required_artifacts,
                merged_bolt_units: Some(&merged),
                single: false,
                source_state: if workspace_requires {
                    Some(current_source()?)
                } else {
                    None
                },
            },
        )?;
        let replaceable = currency.readable && !currency.request_current;
        let pending = Pending {
            state: if replaceable {
                PendingState::Outstanding
            } else {
                PendingState::RetryRequired
            },
            iteration: request.iteration,
            recovery: request.recovery,
            did_not_finish: false,
            verification_failed: request.verification_failed,
        };
        match unit.filter(|unit| !unit.is_empty()) {
            Some(unit) => {
                if !request.recovery {
                    scan.unit_verdicts.delete(unit);
                }
                scan.unit_iterations.delete(unit);
                scan.unit_pending.set(unit, pending);
            }
            None => {
                if !request.recovery {
                    scan.stage_verdict = None;
                }
                scan.stage_iteration = None;
                scan.stage_pending = Some(pending);
            }
        }
    }

    let source_freshness_applies = workspace_requires
        && focus_workspace::env::var("SKIP_SOURCE_FRESHNESS").as_deref() != Some("1");
    let needs_current_source = workspace_requires
        && (scan.newest_source_fingerprint.is_some() || !scan.unit_receipts.is_empty());
    let current_state = if needs_current_source {
        current_source()?
    } else {
        None
    };
    let current_fingerprint = current_state.map(|source| source.fingerprint.as_str());
    let current_listing = current_state.map(|source| &source.listing);
    let newest = scan.newest_source_fingerprint.clone();
    let newest_unit = scan.newest_source_unit.clone();
    let source_mismatch = newest.as_deref().is_some_and(|newest| {
        newest != UNBINDABLE
            && current_fingerprint.is_some()
            && !same_workspace_source(Some(newest), current_fingerprint)
    });
    // A boundary that could not be bound, or a workspace that cannot be
    // read, is not a change: a strict policy holds it stale, a relaxed one
    // says once that it could not be checked.
    let source_unchecked = newest
        .as_deref()
        .is_some_and(|newest| newest == UNBINDABLE || current_fingerprint.is_none());
    let accept_unchecked = source_unchecked && policy.relaxed();
    if accept_unchecked && let Some(newest) = &newest {
        accept_unchecked_source(
            &mut accepted_changes,
            newest_unit.as_deref(),
            newest,
            current_fingerprint,
        );
    }
    let source_stale = newest.is_some()
        && ((source_unchecked && !accept_unchecked) || (source_mismatch && !policy.relaxed()));
    // A unit's own binding is compared path by path, and says which of its
    // paths changed; the whole workspace also moves with another unit's
    // build.
    let unit_bound = newest_unit.as_deref().is_some_and(|unit| {
        current_listing.is_some()
            && source_freshness_applies
            && scan
                .unit_receipts
                .get(unit)
                .is_some_and(|receipt| receipt.fingerprint.is_some())
    });
    if source_mismatch
        && !unit_bound
        && let (Some(newest), Some(current)) = (&newest, current_fingerprint)
        && policy.relaxed()
    {
        // The paths that moved since the review, when its listing was kept.
        let paths = snapshot::workspace_changed_paths(
            &wf.layout,
            &wf.selection,
            slug,
            newest,
            current_state,
        )?
        .filter(|paths| !paths.is_empty());
        let what = paths
            .as_ref()
            .map_or_else(|| "The project's code".to_owned(), |paths| {
                render_changed_paths(paths)
            });
        accepted_changes.push(AcceptedChange {
            stage: slug.to_owned(),
            unit: newest_unit.clone(),
            changed: paths,
            recorded: newest.clone(),
            current: current.to_owned(),
            notice: relaxed_notice(&what, newest_unit.as_deref(), stage_name),
        });
    }

    let mut unit_source_kept = Names::default();
    if source_freshness_applies && current_listing.is_some() {
        let mut newest_first: Vec<(&str, &UnitReceipt)> = scan
            .unit_receipts
            .iter()
            .filter(|(unit, _)| scan.unit_verdicts.has(unit))
            .collect();
        newest_first.sort_by(|left, right| right.1.order.cmp(&left.1.order));
        // Verdicts at one instant in two shards have no order: neither can
        // be the newest claimant.
        let ambiguous: Vec<&str> = newest_first
            .iter()
            .filter(|(_, receipt)| {
                newest_first.iter().any(|(_, other)| {
                    !std::ptr::eq(*other, *receipt)
                        && other.timestamp == receipt.timestamp
                        && other.shard != receipt.shard
                })
            })
            .map(|(_, receipt)| receipt.timestamp.as_str())
            .collect();
        let mut fallen: Vec<(String, StaleProgress)> = Vec::new();
        for (unit, receipt) in &newest_first {
            let progress = StaleProgress {
                next_iteration: receipt.iteration + 1.0,
                recovery_spent: receipt.recovery,
            };
            if ambiguous.contains(&receipt.timestamp.as_str()) && !policy.relaxed() {
                fallen.push(((*unit).to_owned(), progress));
                continue;
            }
            // No marker of a unit binding at all is a verdict from before
            // them: it keeps the whole workspace's rule.
            if receipt.fingerprint.is_none() && !receipt.bypass {
                continue;
            }
            // A review recorded under the source bypass, or against a
            // boundary that could not be bound: a strict policy holds it
            // stale, a relaxed one keeps it.
            let unchecked = receipt.bypass || receipt.fingerprint.as_deref() == Some(UNBINDABLE);
            if !unchecked {
                bail!(
                    "The review of unit \"{unit}\" in \"{slug}\" is bound to the unit's own source, and Focus does not read that yet (units arrive with a later slice)."
                );
            }
            if policy.relaxed() {
                accept_unchecked_source(
                    &mut accepted_changes,
                    Some(unit),
                    receipt.fingerprint.as_deref().unwrap_or("(not recorded)"),
                    None,
                );
                if receipt.fingerprint.as_deref() == Some(UNBINDABLE) {
                    unit_source_kept.set(unit, ());
                }
            } else {
                fallen.push(((*unit).to_owned(), progress));
            }
        }
        for (unit, progress) in fallen {
            scan.stale_unit(&unit, progress);
        }
    } else if source_freshness_applies && !scan.unit_receipts.is_empty() {
        let mut fallen: Vec<(String, StaleProgress)> = Vec::new();
        for (unit, receipt) in scan.unit_receipts.iter() {
            if !scan.unit_verdicts.has(unit) || (receipt.fingerprint.is_none() && !receipt.bypass) {
                continue;
            }
            // The workspace cannot be read now: a relaxed policy keeps the
            // verdicts.
            if policy.relaxed() {
                accept_unchecked_source(
                    &mut accepted_changes,
                    Some(unit),
                    receipt.fingerprint.as_deref().unwrap_or("(not recorded)"),
                    None,
                );
                unit_source_kept.set(unit, ());
                continue;
            }
            fallen.push((
                unit.to_owned(),
                StaleProgress {
                    next_iteration: receipt.iteration + 1.0,
                    recovery_spent: receipt.recovery,
                },
            ));
        }
        for (unit, progress) in fallen {
            scan.stale_unit(&unit, progress);
        }
    }

    // A unit's re-check is spent only until the person next approves the
    // unit at its checkpoint: that approval opens a fresh one.
    let mut unit_recheck_reopened = Names::default();
    let stale_units: Vec<String> = scan.unit_stale_progress.keys().map(str::to_owned).collect();
    for unit in stale_units {
        let recheck = events.iter().skip(start).rev().find(|row| {
            let field = |name: &str| block_field(&row.block, name);
            row.event == "REVIEW_REQUESTED"
                && field("Recovery") == Some("stale-receipt")
                && field("Stage") == Some(slug)
                && field("Unit") == Some(unit.as_str())
                && field("Reviewer") == Some(reviewer)
        });
        let Some(recheck) = recheck else {
            continue;
        };
        let mut approved = false;
        for row in &window.all_events {
            let field = |name: &str| block_field(&row.block, name);
            if row.event == "GATE_APPROVED"
                && field("Unit") == Some(unit.as_str())
                && field("User Input") == Some("Approve")
                && matches!(
                    field("Checkpoint"),
                    Some("construction-unit" | "walking-skeleton")
                )
                && gate_stages(&row.block).contains(&slug)
            {
                refuse_team_claims(&team, wf, row)?;
                if definitely_before(recheck, row) {
                    approved = true;
                    break;
                }
            }
        }
        if !approved {
            continue;
        }
        unit_recheck_reopened.set(&unit, ());
        if let Some(progress) = scan.unit_stale_progress.get(&unit).copied() {
            scan.unit_stale_progress.set(
                &unit,
                StaleProgress {
                    recovery_spent: false,
                    ..progress
                },
            );
        }
    }

    // The baseline is anchored to the stage's real entry. A walk unit by
    // unit never trusts `STAGE_STARTED`: source can be written before that
    // row. A rejection resets the reviews and never the baseline.
    let mut source_baseline = Baseline::Legacy;
    if workspace_requires {
        let boundary = events.iter().rposition(|row| {
            !single_stage(row)
                && (row.event == "WORKFLOW_STARTED"
                    || (row.event == "STAGE_JUMPED" && jump_reaches(&row.block, slug, graph)))
        });
        let from = boundary.unwrap_or(0);
        let first_work = events
            .iter()
            .enumerate()
            .skip(from)
            .find(|(_, row)| {
                if single_stage(row) || block_field(&row.block, "Stage") != Some(slug) {
                    return false;
                }
                if row.event == "REVIEW_REQUESTED" {
                    return true;
                }
                (row.event == "ARTIFACT_CREATED" || row.event == "ARTIFACT_UPDATED")
                    && block_field(&row.block, "File")
                        .filter(|file| !file.is_empty())
                        .is_some_and(|file| {
                            let declared: Vec<String> = stage
                                .produces
                                .iter()
                                .chain(&stage.optional_produces)
                                .cloned()
                                .collect();
                            produces_artifact_unit(stage, file, &recorded_repos, &declared)
                                .is_some()
                        })
            })
            .map_or(events.len(), |(index, _)| index);
        let mut baseline_field: Option<&str> = None;
        for index in from..first_work {
            let Some(field) = baseline_boundary_value(&events[index], slug, unit_major, graph)
            else {
                continue;
            };
            // `auditEventIsCrossShardTied`, over the rows that can open the
            // stage: one at the same instant in another shard that says
            // something else leaves no one baseline.
            let tied_on_another = field.is_some()
                && events.iter().enumerate().any(|(other, candidate)| {
                    other != index
                        && candidate.timestamp == events[index].timestamp
                        && candidate.shard != events[index].shard
                        && baseline_boundary_value(candidate, slug, unit_major, graph)
                            .is_some_and(|said| said != field)
                });
            baseline_field = if tied_on_another {
                Some(UNBINDABLE)
            } else {
                field
            };
        }
        source_baseline = match baseline_field {
            Some(UNBINDABLE) => Baseline::Unbindable,
            Some(field) => {
                match snapshot::read_baseline(&wf.layout, &wf.selection, slug, field)? {
                    Some(listing) => Baseline::Ready(listing),
                    None => Baseline::Invalid,
                }
            }
            None if modern_evidence => Baseline::Invalid,
            None => Baseline::Legacy,
        };
    }

    let produces_one = stage.produces.len() == 1;
    let artifact_changes = accepted_artifact_changes.0.into_iter().map(|(_, change)| {
        let changed = change.changed.filter(|paths| !paths.is_empty());
        // An edit with no write on record names no file: the line names the
        // stage's documents.
        let notice = match &changed {
            Some(paths) => relaxed_notice(
                &render_changed_paths(paths),
                change.unit.as_deref(),
                stage_name,
            ),
            None => format!(
                "{} {stage_name} {} reviewed; carrying on.",
                match change.unit.as_deref().filter(|unit| !unit.is_empty()) {
                    Some(unit) => format!("The {} Unit's", unit_plain_name(unit)),
                    None => "The".to_owned(),
                },
                if produces_one {
                    "document changed after it was"
                } else {
                    "documents changed after they were"
                }
            ),
        };
        AcceptedChange {
            changed,
            notice,
            ..change
        }
    });
    let accepted_changes: Vec<AcceptedChange> = artifact_changes.chain(accepted_changes).collect();
    Ok(Receipts {
        stage_verdict: scan.stage_verdict,
        stage_stale: scan.stage_stale,
        unit_verdicts: scan.unit_verdicts,
        source_stale,
        source_stale_reason: source_stale.then(|| {
            if newest.as_deref() == Some(UNBINDABLE) {
                "boundary-unbindable"
            } else if current_fingerprint.is_none() {
                "source-unreadable"
            } else {
                "fingerprint-mismatch"
            }
        }),
        source_stale_progress: scan
            .newest_source_progress
            .filter(|_| source_stale)
            .map(|progress| StaleProgress {
                recovery_spent: scan.source_recovery_spent,
                ..progress
            }),
        source_recovery_spent: scan.source_recovery_spent,
        newest_source_fingerprint: newest,
        newest_source_unit: newest_unit,
        unit_stale: scan.unit_stale,
        unit_source_kept,
        unit_recheck_reopened,
        source_baseline,
        current_source_listing: current_listing
            .filter(|_| source_freshness_applies)
            .cloned(),
        stage_stale_progress: scan.stage_stale_progress,
        unit_stale_progress: scan.unit_stale_progress,
        stage_iteration: scan.stage_iteration,
        unit_iterations: scan.unit_iterations,
        stage_pending: scan.stage_pending,
        unit_pending: scan.unit_pending,
        awaiting_verdict,
        unfinished_verdicts: scan.unfinished_verdicts,
        merged_bolt_units: window.merged_bolt_units.clone(),
        open_bolt_units: window.open_bolt_units.clone(),
        accepted_changes,
        change_control_read: policy.read.get(),
    })
}

//! The list of findings a review's scope stands at, derived from the
//! ledger (`deriveReviewFindingsList`, `aidlc-review-brief.ts` L795).
//!
//! Nothing keeps the list: it is replayed. Each paired review of the scope,
//! in ledger order, rewrites it (the list the engine kept in the review's
//! record, or, for a record from before it kept one, the record's findings
//! applied by id); each decision a gate recorded is applied to the review
//! it names, or by id where it names none; and a Redo from scratch begins a
//! new list. With no review record yet, the `## Review` section an older
//! release left in the artifact seeds it. A review about to be recorded is
//! applied last, without writing anything, so the record stores the list
//! every later reader will replay.
//!
//! The pairing is the receipts' ([`paired_completions`],
//! `pairedReviewCompletions`, `aidlc-lib.ts` L17 587): a completion counts
//! only when it descends from a request and names a record that verifies,
//! so a row a gate would not trust cannot enter the list.
//!
//! # Tests that change no answer
//!
//! - A paired review's record is read again and held to its row
//!   (`pairedReviewRecordForCompletion`), as upstream does: the pairing has
//!   already read it and held it to the row.
//! - The finding a decision is about is looked for by artifact and id,
//!   where what is then done with it applies the decision by artifact and
//!   id again.
//! - A stage whose reviewer is an empty name pairs nothing, as one with no
//!   reviewer: no row's `Reviewer` reads as an empty name either.
//! - A request's row is done with once its binding is kept: were it weighed
//!   as a completion, it would be weighed against itself and name no
//!   record.
//!
//! # Deliberate deviation
//!
//! Upstream reads the artifact that may hold a legacy `## Review` section
//! by its name: through a link, under a second name, and it fails on a
//! folder with what its runtime says. The section seeds findings a gate
//! then weighs, so Focus reads the file as it reads any file of the record
//! that decides something (a plain file with one name, reached with no link
//! on the way) and refuses the list otherwise, naming the file.

use std::collections::HashMap;
use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::review::{self, RequestBinding};
use focus_evidence::review_dispositions::{self as decisions, Read};
use focus_evidence::review_findings::{
    self as findings, Finding, REPORT_RETRY_MESSAGE, is_unreadable_findings_table_finding,
    read_findings_table, report_unreadable_message, unreadable_findings_table_finding,
};
use focus_evidence::review_list::{
    Disposition, apply_disposition, apply_prior_report, apply_review_body, legacy_record_list,
    without_round_marks,
};
use focus_evidence::review_record::{self, Record};
use focus_graph::runtime::Stage;
use focus_js::{EngineError, Result, bail, re};
use focus_markdown::section::extract_markdown_section;

use crate::artifacts::{
    ReviewEntry, ReviewScope, Unlistable, review_entries, workspace_artifact_path,
};
use crate::gate::sort_attempt_events;
use crate::review_artifacts::{Trusted, read_trusted};
use crate::stage;
use crate::workflow::Workflow;

/// `PairedReviewCompletion`: a completion that answers a request, and the
/// record it names (none for a review from before records).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedCompletion {
    /// The Unit of the row; empty for a review of the whole stage.
    pub unit: String,
    pub event: ShardEvent,
    pub named: Option<review::RecordRef>,
}

/// `pairedReviewCompletions`: every paired `REVIEW_COMPLETED` row of the
/// stage among `events` (in ledger order), each request answered once.
#[must_use]
pub fn paired_completions(
    events: &[ShardEvent],
    record: Option<&Path>,
    slug: &str,
    reviewer: Option<&str>,
) -> Vec<PairedCompletion> {
    let mut paired = Vec::new();
    let Some(reviewer) = reviewer.filter(|reviewer| !reviewer.is_empty()) else {
        return paired;
    };
    let mut pending: HashMap<String, RequestBinding> = HashMap::new();
    for event in events {
        let block = event.block.as_str();
        if (event.event != "REVIEW_REQUESTED" && event.event != "REVIEW_COMPLETED")
            || block_field(block, "Stage") != Some(slug)
            || block_field(block, "Reviewer") != Some(reviewer)
        {
            continue;
        }
        // A row with a repeated field has no single reading, and a review
        // of a stage run by itself is for its own gate.
        if !review::names_once(block) || block_field(block, "Workflow").is_some() {
            continue;
        }
        let Some(iteration) =
            block_field(block, "Iteration").filter(|pass| re::test("^[1-9][0-9]*$", "", pass))
        else {
            continue;
        };
        let unit = block_field(block, "Unit").unwrap_or("");
        // By scope and, for a row of the record era, the request it
        // descends from.
        let key = [
            unit,
            "",
            iteration,
            block_field(block, "Request Id").unwrap_or(""),
        ]
        .join("\0");
        if event.event == "REVIEW_REQUESTED" {
            if let Some(binding) = review::request_binding(block) {
                pending.insert(key, binding);
            }
            continue;
        }
        let Some(request) = pending.get(&key) else {
            continue;
        };
        if !review_record::completion_carries_verified_review(request, block, &|named| {
            review_record::read(record, named)
        }) {
            continue;
        }
        pending.remove(&key);
        paired.push(PairedCompletion {
            unit: unit.to_owned(),
            event: event.clone(),
            named: review::record_ref(block),
        });
    }
    paired
}

/// A paired review of the scope: its row, the record it names and the
/// record.
struct Paired {
    block: String,
    named: review::RecordRef,
    record: Record,
}

/// `pairedReviews`: the scope's paired reviews that name a record that
/// reads, in ledger order.
fn paired_reviews(
    events: &[ShardEvent],
    record: Option<&Path>,
    slug: &str,
    reviewer: Option<&str>,
    unit: Option<&str>,
) -> Vec<Paired> {
    paired_completions(events, record, slug, reviewer)
        .into_iter()
        .filter(|paired| paired.unit == unit.unwrap_or(""))
        .filter_map(|paired| {
            let named = paired.named?;
            // `pairedReviewRecordForCompletion`.
            let read = review_record::read(record, &named)
                .filter(|found| review_record::matches_completion(found, &paired.event.block))?;
            Some(Paired {
                block: paired.event.block,
                named,
                record: read,
            })
        })
        .collect()
}

/// What a legacy `## Review` section seeds the list with.
struct Seed {
    verdict: Option<String>,
    findings: Vec<Finding>,
    findings_text: Option<String>,
}

/// The `## Review` section of the entry's file, when there is a file and
/// it holds one.
fn legacy_section(entry: &ReviewEntry) -> Result<Option<String>> {
    match read_trusted(entry) {
        Trusted::Absent => Ok(None),
        Trusted::Refused => bail!(
            "{} is not a plain file of the record reached with no link on the way, and the review an older release may have left in it is not read. Restore the file, or move what is there away.",
            entry.path.as_deref().map_or_else(
                || entry.logical_path.clone(),
                |path| path.display().to_string()
            )
        ),
        Trusted::File(bytes) => {
            let section = extract_markdown_section(&String::from_utf8_lossy(&bytes), "## Review");
            Ok((!section.is_empty()).then_some(section))
        }
    }
}

/// `legacySeed`: the findings of a legacy section, each as a review's
/// (fixed or new: its status was the reviewer's, never a decision), or the
/// one finding that says the section's table cannot be read.
fn legacy_seed(review: &str, artifact: &str, unit: Option<&str>) -> Seed {
    match findings::parse_review_section(review, artifact, unit) {
        Ok(parsed) => Seed {
            verdict: parsed.verdict,
            findings: parsed
                .findings
                .into_iter()
                .map(|finding| {
                    let fixed = finding.status == "Resolved";
                    Finding {
                        status: if fixed { "Resolved" } else { "New" }.to_owned(),
                        resolved_by_reviewer: Some(fixed),
                        resolved_in_review: Some(fixed),
                        ..finding
                    }
                })
                .collect(),
            findings_text: None,
        },
        Err(reason) => Seed {
            verdict: findings::section_verdict(review),
            findings: vec![unreadable_findings_table_finding(artifact, &reason, unit)],
            findings_text: findings::findings_text(review),
        },
    }
}

/// `PendingReviewFindings`: a review about to be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pending<'p> {
    pub artifact: &'p str,
    pub body: &'p str,
    pub verdict: &'p str,
    /// Why the review's findings were already found unreadable.
    pub unreadable_reason: Option<&'p str>,
    /// A report that breaks a rule is recorded as it is (the last pass)
    /// and not sent back.
    pub allow_malformed: bool,
    /// `seedLegacy !== false`.
    pub seed_legacy: bool,
}

/// `DerivedReviewFindingsList`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Derived {
    pub artifact: String,
    pub unit: Option<String>,
    pub verdict: Option<String>,
    pub findings: Vec<Finding>,
    /// The findings as the reviewer wrote them, when their table cannot be
    /// read.
    pub findings_text: Option<String>,
    /// The record of the scope's latest review.
    pub review_record: Option<(String, String)>,
    /// How many findings that review found fixed.
    pub reviewer_resolved_count: usize,
    /// Why the pending review is sent back.
    pub malformed_report: Option<String>,
    /// The latest review gave no verdict of its own.
    pub incomplete_review: bool,
}

fn finding_of<'f>(findings: &'f [Finding], disposition: &Disposition) -> Option<&'f Finding> {
    findings
        .iter()
        .find(|finding| finding.artifact == disposition.artifact && finding.id == disposition.id)
}

fn is_unreadable(finding: &Finding) -> bool {
    is_unreadable_findings_table_finding(&finding.id, &finding.location)
}

/// An unreadable report adds its finding and leaves the rest of the list as
/// it was; only the previous review's notes and marks are dropped with it.
fn with_unreadable(findings: &[Finding], stand_in: Finding) -> Vec<Finding> {
    findings
        .iter()
        .filter(|finding| finding.id != stand_in.id)
        .map(without_round_marks)
        .chain(std::iter::once(stand_in.clone()))
        .collect()
}

fn decisions_of(block: &str) -> Result<Vec<Read>> {
    decisions::of_block(block).map_err(EngineError::new)
}

/// `deriveReviewFindingsList`: the list of the scope (`stage`, `unit`),
/// as the ledger of `wf`'s record replays it, with `pending` applied last.
///
/// # Errors
///
/// What the list would be read from is not read: a decisions field or a
/// record with a list where a text belongs, an artifact that may hold a
/// legacy review and is not a plain file of the record, a folder the
/// stage's artifacts are found by listing that cannot be listed.
#[allow(clippy::too_many_lines)]
pub fn derive(
    wf: &Workflow,
    stage: &Stage,
    artifact: &str,
    unit: Option<&str>,
    pending: Option<&Pending<'_>>,
) -> Result<Derived> {
    let slug = stage.slug.as_str();
    let record_dir = wf.record_dir();
    let events = sort_attempt_events(&wf.events());
    let pairs = paired_reviews(
        &events,
        record_dir.as_deref(),
        slug,
        stage::reviewer(stage),
        unit,
    );
    // (Two rows with the same text are the later pair, as upstream's map
    // keeps them.)
    let pair_by_block: HashMap<&str, &Paired> = pairs
        .iter()
        .map(|pair| (pair.block.as_str(), pair))
        .collect();

    // With no review record yet, a legacy `## Review` section seeds the
    // list. It stays keyed to the artifact that holds the section, as the
    // gate showed it and as decisions made on it name it.
    let entries = if pairs.is_empty() && pending.is_none_or(|pending| pending.seed_legacy) {
        let scope = ReviewScope {
            unit,
            ..ReviewScope::default()
        };
        match review_entries(wf, stage, scope)? {
            Ok(entries) => entries.unwrap_or_default(),
            Err(Unlistable(folder)) => bail!(
                "{} is there and is not a folder that can be listed, and the artifacts of \"{slug}\" are found by listing it. Restore the folder, or move what is there away.",
                folder.display()
            ),
        }
    } else {
        Vec::new()
    };
    let project = wf.layout.project.as_path();
    // The entry that holds a section: the artifact asked about when it
    // does, or the first that does. A file is read when upstream would
    // look at it, and once.
    let mut read: Vec<Option<Option<String>>> = vec![None; entries.len()];
    let mut section_of = |index: usize| -> Result<Option<String>> {
        if read[index].is_none() {
            read[index] = Some(legacy_section(&entries[index])?);
        }
        Ok(read[index].clone().flatten())
    };
    let paths: Vec<String> = entries
        .iter()
        .map(|entry| workspace_artifact_path(project, entry))
        .collect();
    let mut seeded: Option<(usize, String)> = None;
    for index in (0..entries.len()).filter(|index| paths[*index] == artifact) {
        if let Some(section) = section_of(index)? {
            seeded = Some((index, section));
            break;
        }
    }
    if seeded.is_none() {
        for index in 0..entries.len() {
            if let Some(section) = section_of(index)? {
                seeded = Some((index, section));
                break;
            }
        }
    }
    let seeded = seeded.map(|(index, section)| (paths[index].clone(), section));
    let seed_artifact = seeded.as_ref().map_or(artifact, |(path, _)| path.as_str());
    let seed = seeded
        .as_ref()
        .map(|(_, section)| legacy_seed(section, seed_artifact, unit));

    let mut findings: Vec<Finding> = seed
        .as_ref()
        .map(|seed| seed.findings.clone())
        .unwrap_or_default();
    let mut verdict = seed.as_ref().and_then(|seed| seed.verdict.clone());
    let mut findings_text = seed.as_ref().and_then(|seed| seed.findings_text.clone());
    let mut latest: Option<(String, String)> = None;
    let mut resolved_count = 0;
    let mut incomplete = false;
    let mut malformed_report = None;

    // Each decision a gate row names a review record for attaches to that
    // review, wherever the row sorts: shards share timestamps of one second.
    // A decision that names a review outside this list changes nothing.
    let mut named_decisions: Vec<((String, String), Vec<Disposition>)> = Vec::new();
    for event in &events {
        if !decisions::gate_applies_to_scope(&event.event, &event.block, slug, unit) {
            continue;
        }
        for read in decisions_of(&event.block)? {
            let Some(named) = read.disposition.reviewed_record.clone() else {
                continue;
            };
            match named_decisions.iter_mut().find(|(key, _)| *key == named) {
                Some((_, held)) => held.push(read.disposition),
                None => named_decisions.push((named, vec![read.disposition])),
            }
        }
    }
    // Decisions with no named review (older releases, or a list seeded from
    // a legacy section) apply by id where they sit, and carry by id into a
    // list an older release's record rewrote, at the severity the finding
    // had when the decision was made.
    let mut legacy_decisions: Vec<(String, Disposition, Option<String>)> = Vec::new();
    // Decisions already made on this list. A later review's list, written
    // where a decision had not been seen yet (another shard), may still show
    // the finding open; the decision stands on the finding it was made on.
    let mut settled: Vec<Disposition> = Vec::new();

    // Redo overwrites the artifact, so a legacy section in it was written
    // after the last Redo: the seed begins the list that Redo started.
    // (Upstream gives the seed back at the last Redo only. What an earlier
    // one gives back the next one takes away, so each gives it back here.)
    for event in &events {
        if decisions::redo_applies_to_scope(&event.event, &event.block, slug, unit) {
            let again = seed.as_ref();
            findings = again.map(|seed| seed.findings.clone()).unwrap_or_default();
            verdict = again.and_then(|seed| seed.verdict.clone());
            findings_text = again.and_then(|seed| seed.findings_text.clone());
            latest = None;
            resolved_count = 0;
            incomplete = false;
            legacy_decisions.clear();
            settled.clear();
            continue;
        }
        if let Some(pair) = pair_by_block.get(event.block.as_str()) {
            let body = pair
                .record
                .document
                .get("body")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let record_verdict = Some(pair.record.verdict.as_str());
            findings_text = None;
            let derived = review_record::derived_findings(&pair.record, artifact)
                .map_err(EngineError::new)?;
            if let Some(derived) = derived {
                findings = derived;
                for disposition in &settled {
                    // A finding fixed or reopened since carries its own
                    // decision history in the record's list, and a decision
                    // on an unreadable report belongs to that report alone.
                    if finding_of(&findings, disposition).is_some_and(|target| {
                        target.earlier_decision.is_none() && !is_unreadable(target)
                    }) {
                        apply_disposition(&mut findings, disposition, None);
                    }
                }
                if findings.iter().any(is_unreadable)
                    && read_findings_table(body, artifact, record_verdict, unit)
                        .unreadable
                        .is_some()
                {
                    findings_text = findings::findings_text(body);
                }
            } else if !body.is_empty() {
                let recorded =
                    review_record::findings(&pair.record, artifact).map_err(EngineError::new)?;
                let unreadable =
                    read_findings_table(body, artifact, record_verdict, unit).unreadable;
                let stand_in = recorded.iter().find(|finding| is_unreadable(finding));
                if unreadable.is_some() || stand_in.is_some() {
                    let stand_in = stand_in.cloned().unwrap_or_else(|| {
                        unreadable_findings_table_finding(
                            artifact,
                            unreadable.as_deref().unwrap_or("unknown error"),
                            unit,
                        )
                    });
                    findings = with_unreadable(&findings, stand_in);
                    findings_text = findings::findings_text(body);
                } else {
                    findings = legacy_record_list(&findings, &recorded);
                    for (_, disposition, severity) in &legacy_decisions {
                        apply_disposition(&mut findings, disposition, severity.as_deref());
                    }
                }
            } else {
                // An empty record, written when no reviewer gave a verdict,
                // re-checked nothing.
                findings = apply_prior_report(&findings, artifact, unit, &[], false).0;
            }
            let named = (pair.named.path.clone(), pair.named.digest.clone());
            verdict = Some(pair.record.verdict.clone());
            incomplete = body.is_empty();
            for finding in &mut findings {
                finding.review_record = Some(named.clone());
            }
            resolved_count = findings
                .iter()
                .filter(|finding| finding.resolved_in_review == Some(true))
                .count();
            if let Some((_, held)) = named_decisions.iter().find(|(key, _)| *key == named) {
                for disposition in held {
                    apply_disposition(&mut findings, disposition, None);
                    if !disposition.status.starts_with("Reopened: ") {
                        settled.push(disposition.clone());
                    }
                }
            }
            latest = Some(named);
        } else if decisions::gate_applies_to_scope(&event.event, &event.block, slug, unit) {
            for read in decisions_of(&event.block)? {
                let disposition = read.disposition;
                if disposition.reviewed_record.is_some() {
                    continue;
                }
                if !disposition.status.starts_with("Reopened: ") {
                    let key = decisions::key(&disposition.artifact, &disposition.id);
                    let severity =
                        finding_of(&findings, &disposition).map(|current| current.severity.clone());
                    match legacy_decisions
                        .iter_mut()
                        .find(|(held, _, _)| *held == key)
                    {
                        Some(held) => *held = (key, disposition.clone(), severity),
                        None => legacy_decisions.push((key, disposition.clone(), severity)),
                    }
                }
                apply_disposition(&mut findings, &disposition, None);
            }
        }
    }

    if let Some(pending) = pending {
        verdict = Some(pending.verdict.to_owned());
        incomplete = pending.body.is_empty();
        findings_text = None;
        // What a pending review that cannot be read as asked becomes: sent
        // back with the reason, or, on the last pass, recorded as the one
        // finding that says so.
        let mut unreadable = |findings: &mut Vec<Finding>,
                              findings_text: &mut Option<String>,
                              reason: &str,
                              sent_back: String| {
            if pending.allow_malformed {
                *findings = with_unreadable(
                    findings,
                    unreadable_findings_table_finding(pending.artifact, reason, unit),
                );
                *findings_text = findings::findings_text(pending.body);
            } else {
                malformed_report = Some(sent_back);
            }
        };
        if let Some(reason) = pending.unreadable_reason {
            unreadable(
                &mut findings,
                &mut findings_text,
                reason,
                report_unreadable_message(reason),
            );
        } else {
            // A first review (no record and no findings yet in this list)
            // has no prior findings to report on, so a report that leaves
            // out the empty Prior findings table reads the same. A later
            // one must say what became of the open findings.
            let first = latest.is_none() && findings.is_empty();
            match apply_review_body(&findings, pending.artifact, unit, pending.body) {
                Ok(applied) => {
                    findings = applied.findings;
                    if !pending.allow_malformed && applied.prior_missing && !first {
                        malformed_report = Some(report_unreadable_message(
                            "the Prior findings table is missing, and this review has earlier findings to report on",
                        ));
                    } else if !pending.allow_malformed && applied.malformed {
                        malformed_report = Some(report_unreadable_message(
                            "a Prior findings row names an ID the review context does not list, or repeats an ID",
                        ));
                    }
                }
                Err(thrown) => {
                    // The parser names the fault (the rule, the row and the
                    // cell); anything else keeps the generic sentence.
                    let named = if thrown.starts_with("the findings report could not be read") {
                        thrown
                    } else {
                        REPORT_RETRY_MESSAGE.to_owned()
                    };
                    unreadable(&mut findings, &mut findings_text, &named, named.clone());
                }
            }
        }
        resolved_count = findings
            .iter()
            .filter(|finding| finding.resolved_in_review == Some(true))
            .count();
    }

    Ok(Derived {
        // (With no seed the two are one.)
        artifact: if pending.is_none() {
            seed_artifact
        } else {
            artifact
        }
        .to_owned(),
        unit: unit.filter(|unit| !unit.is_empty()).map(str::to_owned),
        verdict,
        findings,
        findings_text,
        review_record: latest,
        reviewer_resolved_count: resolved_count,
        malformed_report,
        incomplete_review: incomplete,
    })
}

/// `entryUnit`: the unit a logical path of a per-unit stage belongs to
/// (`construction/<unit>/<stage>/…`), by upstream's own pattern.
fn entry_unit<'p>(logical_path: &'p str, slug: &str) -> Option<&'p str> {
    let found = focus_js::re::find(r"^construction/([^/]+)/([^/]+)/", "", logical_path)?;
    // (The paths are the ones `review_entries` builds, so the stage a unit's
    // path names is always the one asked about: the comparison is
    // upstream's, and no path here fails it.)
    (focus_js::re::group(logical_path, &found, 2) == Some(slug))
        .then(|| focus_js::re::group(logical_path, &found, 1))
        .flatten()
}

/// `readReviewArtifactContexts`: the list of findings each scope of the
/// stage stands at now, one per unit for a stage that runs for each unit
/// (or the one unit asked for). A scope's list is keyed to the stage's
/// review artifact, or to its first declared one.
///
/// A scope with nothing to show is left out: no review, or a review that
/// gave no verdict of its own and listed nothing, which the gate answers
/// with its own fallback finding.
///
/// # Errors
///
/// What [`derive`] does not read, or the registry's row for the piece of
/// work.
pub fn contexts(wf: &Workflow, stage: &Stage, unit: Option<&str>) -> Result<Vec<Derived>> {
    let slug = stage.slug.as_str();
    let scope = ReviewScope {
        unit,
        ..ReviewScope::default()
    };
    let entries = match review_entries(wf, stage, scope)? {
        Ok(Some(entries)) => entries,
        Ok(None) => return Ok(Vec::new()),
        Err(Unlistable(folder)) => bail!(
            "{} is there and is not a folder that can be listed, and the artifacts of \"{slug}\" are found by listing it. Restore the folder, or move what is there away.",
            folder.display()
        ),
    };
    // Scope by scope, in the order each first appears.
    let mut scopes: Vec<(String, Vec<&crate::artifacts::ReviewEntry>)> = Vec::new();
    for entry in &entries {
        // (A unit asked for is the only one the entries are of, so which of
        // the two is asked first changes nothing; upstream asks in this order.)
        let of = unit
            .or_else(|| entry_unit(&entry.logical_path, slug))
            .unwrap_or("");
        match scopes.iter_mut().find(|(scope, _)| scope == of) {
            Some((_, grouped)) => grouped.push(entry),
            None => scopes.push((of.to_owned(), vec![entry])),
        }
    }
    let project = wf.layout.project.as_path();
    let mut contexts = Vec::new();
    for (scope, grouped) in scopes {
        let Some(entry) = grouped
            .iter()
            .find(|entry| entry.appendix_target)
            .or_else(|| grouped.first())
        else {
            continue;
        };
        let context = derive(
            wf,
            stage,
            &crate::artifacts::workspace_artifact_path(project, entry),
            (!scope.is_empty()).then_some(scope.as_str()),
            None,
        )?;
        // (Upstream asks for the text too. Wherever a list keeps a report's
        // text it holds the finding that says the report was not read, so
        // the question before it has already answered.)
        if (!context.incomplete_review && context.verdict.is_some())
            || !context.findings.is_empty()
            || context.findings_text.is_some()
        {
            contexts.push(context);
        }
    }
    Ok(contexts)
}

#[cfg(test)]
mod tests {
    use super::legacy_section;
    use crate::artifacts::ReviewEntry;

    fn entry(boundary: &std::path::Path, name: &str) -> ReviewEntry {
        ReviewEntry {
            logical_path: name.to_owned(),
            path: Some(boundary.join(name)),
            boundary: boundary.to_path_buf(),
            required: true,
            appendix_target: true,
            summary_input: false,
        }
    }

    /// Deliberate deviation. Upstream reads the artifact that may hold a
    /// legacy `## Review` section by its name: through a link, under a
    /// second name, and it fails on a folder with what its runtime says.
    /// The section seeds findings a gate then weighs, so Focus reads the
    /// file as a file of the record that decides something, and refuses
    /// the list otherwise, naming the file.
    #[test]
    fn an_artifact_that_is_not_a_plain_file_of_the_record_seeds_nothing_and_is_refused() {
        let root = std::env::temp_dir().join(format!("focus-legacy-seed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("record/folder.md")).expect("a folder");
        let record = root.join("record");
        let review = "# Artifact\n\n## Review\n\n**Verdict:** READY\n";
        std::fs::write(record.join("plain.md"), review).expect("a file");
        std::fs::write(record.join("none.md"), "# Artifact\n").expect("a file");
        std::fs::write(root.join("outside.md"), review).expect("a file");
        std::fs::write(record.join("twice.md"), review).expect("a file");
        std::fs::hard_link(record.join("twice.md"), record.join("second-name.md")).expect("a name");

        assert_eq!(
            legacy_section(&entry(&record, "plain.md")).expect("read"),
            Some("\n**Verdict:** READY\n".to_owned())
        );
        assert_eq!(
            legacy_section(&entry(&record, "none.md")).expect("read"),
            None
        );
        assert_eq!(
            legacy_section(&entry(&record, "absent.md")).expect("read"),
            None
        );
        #[cfg(unix)]
        let linked = {
            std::os::unix::fs::symlink(root.join("outside.md"), record.join("link.md"))
                .expect("a link");
            std::os::unix::fs::symlink(record.join("plain.md"), record.join("inner-link.md"))
                .expect("a link");
            vec!["twice.md", "link.md", "inner-link.md"]
        };
        // Off Unix the host does not say how many names a file has, and
        // making a link asks for a privilege: the folder stands for both.
        #[cfg(not(unix))]
        let linked: Vec<&str> = Vec::new();
        for name in ["folder.md"].into_iter().chain(linked) {
            let refusal = legacy_section(&entry(&record, name)).expect_err(name);
            assert!(
                refusal.message().contains(name)
                    && refusal
                        .message()
                        .contains("is not a plain file of the record"),
                "{name}: {refusal}"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}

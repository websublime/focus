//! What a person is shown before they decide: the brief of a review at its
//! gate, and the brief of a summary confirmation (`renderReviewBrief`,
//! `renderSummaryConfirmationBrief`, `reviewInvalidationDetails`,
//! `acceptedReviewChanges` and `keepGateBrief`, `aidlc-review-brief.ts`).
//!
//! The review's brief says what the review came to, why it is shown now,
//! what moved since (a change the Guard Policy let through; for a review
//! shown again because its receipt went stale, what changed and what a
//! backward jump left to check again), the list of findings, and what each
//! answer does. As printed it is kept beside the review's record, so the
//! gate's row can name the digest of what the person was shown.
//!
//! # What is refused by name
//!
//! A brief for one unit of a stage that runs for each unit depends on
//! whether the approval ahead is the unit's own (its team's gate, or its
//! Construction checkpoint), and a recovery review of a unit names the
//! source it was bound to: both are the units' (a later slice).

use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_audit::time::iso_timestamp;
use focus_audit::units::validate_unit_name;
use focus_evidence::authorization::jump_reaches;
use focus_evidence::review_dispositions::path_unit;
use focus_evidence::review_findings::Finding;
use focus_evidence::review_list::next_finding_id;
use focus_evidence::review_shown::{self, Audience};
use focus_graph::runtime::Stage;
use focus_js::order::cmp_utf16;
use focus_js::path::{self as jspath, Flavour};
use focus_js::{EngineError, Result, bail, json, re, text as js_text};
use focus_workspace::names::ENGINE_DIR;
use sha2::{Digest, Sha256};

use crate::artifacts::{
    ReviewEntry, ReviewScope, Unlistable, review_entries, workspace_artifact_path,
};
use crate::gate::sort_attempt_events;
use crate::review_attempt::{definitely_before, maximal};
use crate::review_derived;
use crate::stage;
use crate::workflow::Workflow;

/// `ReviewBriefReason`: why the brief is shown now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    First,
    Revision,
    Stale,
}

impl Why {
    /// The word of `--why`.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        match word {
            "first" => Some(Self::First),
            "revision" => Some(Self::Revision),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Revision => "revision",
            Self::Stale => "stale",
        }
    }
}

/// A scope's list as the brief shows it.
struct Shown {
    artifact: String,
    unit: Option<String>,
    verdict: Option<String>,
    findings: Vec<Finding>,
    findings_text: Option<String>,
    reviewer_resolved: usize,
    incomplete: bool,
}

/// The stage's artifacts, where upstream lists them and falls over on a
/// folder it cannot list.
fn entries(wf: &Workflow, stage: &Stage, unit: Option<&str>) -> Result<Vec<ReviewEntry>> {
    let scope = ReviewScope {
        unit,
        ..ReviewScope::default()
    };
    match review_entries(wf, stage, scope)? {
        Ok(entries) => Ok(entries.unwrap_or_default()),
        Err(Unlistable(folder)) => bail!(
            "{} is there and is not a folder that can be listed, and the artifacts of \"{}\" are found by listing it. Restore the folder, or move what is there away.",
            folder.display(),
            stage.slug
        ),
    }
}

fn ticked(paths: &[String]) -> String {
    paths
        .iter()
        .map(|path| format!("`{path}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The finding a gate supplies for a review that did not finish. (Upstream
/// gives it its unit and its fingerprint too; the brief shows neither.)
fn fallback(artifact: &str, id: String, said: &str) -> Finding {
    Finding {
        artifact: artifact.to_owned(),
        id,
        severity: "Major".to_owned(),
        location: format!("{artifact} > review completion"),
        finding: said.to_owned(),
        required_action: "Request changes and rerun the reviewer.".to_owned(),
        status: "Unresolved".to_owned(),
        ..Finding::default()
    }
}

/// `renderReviewBrief`: what the person reads at the gate of a reviewed
/// stage. `fallback_finding` is what the gate says of a review that gave no
/// verdict of its own.
///
/// # Errors
///
/// What the lists of the stage do not read; a unit of a stage that runs
/// for each unit, and a unit's recovery review bound to its source, by
/// name.
pub fn review_brief(
    wf: &Workflow,
    stage: &Stage,
    why: Why,
    unit: Option<&str>,
    fallback_finding: Option<&str>,
) -> Result<String> {
    let per_unit = stage::is_per_unit(stage);
    if per_unit && unit.is_some_and(|unit| !unit.is_empty()) {
        bail!(
            "The brief of one unit of \"{}\" depends on whether the approval ahead is the unit's own (its team's gate, or its Construction checkpoint), which is read off the units' gates: Focus does not read those yet (units arrive with a later slice).",
            stage.slug
        );
    }
    // (The final gate of a stage that runs for each unit shows every
    // unit's review.)
    let context_unit = if per_unit { None } else { unit };
    let mut contexts: Vec<Shown> = review_derived::contexts(wf, stage, context_unit)?
        .into_iter()
        .map(|context| Shown {
            artifact: context.artifact,
            unit: context.unit,
            verdict: context.verdict,
            findings: context.findings,
            findings_text: context.findings_text,
            reviewer_resolved: context.reviewer_resolved_count,
            incomplete: context.incomplete_review,
        })
        .collect();
    if let Some(said) = fallback_finding {
        // The review that did not finish re-checked nothing, so a list
        // carried from earlier reviews also shows the finding the gate
        // recorded, under the next unused id.
        for context in contexts.iter_mut().filter(|context| context.incomplete) {
            let finding = fallback(&context.artifact, next_finding_id(&context.findings), said);
            context.findings.push(finding);
        }
        if contexts.is_empty() {
            let artifact = match entries(wf, stage, unit)?.first() {
                Some(entry) => workspace_artifact_path(&wf.layout.project, entry),
                None => format!("{}/{}", stage.phase, stage.slug),
            };
            let finding = fallback(&artifact, "R-01".to_owned(), said);
            contexts.push(Shown {
                artifact,
                unit: unit.filter(|unit| !unit.is_empty()).map(str::to_owned),
                // (Upstream gives this list the verdict NOT-READY. Its one
                // finding is open, and only a list with none is asked for
                // its verdict.)
                verdict: None,
                findings: vec![finding],
                findings_text: None,
                reviewer_resolved: 0,
                incomplete: false,
            });
        }
    }
    let open = contexts
        .iter()
        .flat_map(|context| &context.findings)
        .filter(|finding| finding.is_open())
        .count();
    let listed: usize = contexts.iter().map(|context| context.findings.len()).sum();
    let resolved: usize = contexts
        .iter()
        .map(|context| context.reviewer_resolved)
        .sum();
    let outcome = if open > 0 {
        "Concerns remain for your decision.".to_owned()
    } else if resolved > 0 {
        format!(
            "{resolved} {} marked fixed by the reviewer.",
            if resolved == 1 { "finding" } else { "findings" }
        )
    } else if listed > 0 {
        "No open findings remain.".to_owned()
    } else if contexts
        .iter()
        .any(|context| context.verdict.as_deref() == Some("NOT-READY"))
    {
        "The review did not complete with actionable findings.".to_owned()
    } else {
        "No blocking concerns were found.".to_owned()
    };
    let why_now = match why {
        Why::First => "First review completed.",
        Why::Revision => "Revision re-checked.",
        Why::Stale => "Re-check required: the previous review receipt is no longer valid.",
    };
    let mut lines = vec![
        format!("**Stage:** {}", stage::name(stage)),
        format!("**Review outcome:** {outcome}"),
        format!("**Why now:** {why_now}"),
    ];
    // What moved since the verdict and was let through by the Guard
    // Policy: the verdict above is the reviewer's, these lines say what
    // changed after it. (A unit's own brief says only that unit's; that
    // brief is refused above.)
    for (notice, changed) in accepted_review_changes(wf, &stage.slug) {
        lines.push(format!("**Reviewed content differs:** {notice}"));
        if let Some(changed) = changed {
            lines.push(format!("**Changed after review:** {}", ticked(&changed)));
        }
    }
    if why == Why::Stale {
        let details = invalidation_details(wf, stage, &contexts)?;
        if !details.changed_upstream.is_empty() {
            lines.push(format!(
                "**Changed upstream:** {}",
                ticked(&details.changed_upstream)
            ));
        }
        if !details.invalidated_artifacts.is_empty() {
            lines.push(format!(
                "**Downstream artifacts requiring re-check:** {}",
                ticked(&details.invalidated_artifacts)
            ));
        }
        if !details.invalidated_reviews.is_empty() {
            lines.push(format!(
                "**Downstream reviews requiring re-check:** {}",
                ticked(&details.invalidated_reviews)
            ));
        }
    }
    let shown: Vec<review_shown::Context<'_>> = contexts
        .iter()
        .map(|context| review_shown::Context {
            artifact: &context.artifact,
            findings: &context.findings,
            findings_text: context.findings_text.as_deref(),
        })
        .collect();
    lines.push(String::new());
    lines.push(review_shown::findings_context(&shown, Audience::Gate));
    lines.push(String::new());
    lines.push("**Decision options:**".to_owned());
    // Approve says what it accepts: open findings only when there are some.
    lines.push(
        if open > 0 {
            "- **Approve** - continue with the open findings accepted."
        } else {
            "- **Approve** - continue; no findings are open."
        }
        .to_owned(),
    );
    lines.push(
        "- **Request Changes** - return to the listed artifacts so the required actions can be addressed."
            .to_owned(),
    );
    Ok(lines.join("\n"))
}

/// `reviewInvalidationAttemptView`: the ledger in order, and the rows the
/// stage's current attempt starts after.
fn attempt_view<'e>(wf: &Workflow, events: &'e [ShardEvent], slug: &str) -> Vec<&'e ShardEvent> {
    let boundaries: Vec<&ShardEvent> = events
        .iter()
        .filter(|event| {
            event.event == "WORKFLOW_STARTED"
                || (event.event == "STAGE_JUMPED"
                    && jump_reaches(&event.block, slug, &wf.install.graph))
                || ((event.event == "GATE_APPROVED" || event.event == "GATE_REJECTED")
                    && block_field(&event.block, "Stage") == Some(slug))
        })
        .collect();
    maximal(&boundaries)
}

/// `attemptEventAfterFrontier`.
fn after(frontier: &[&ShardEvent], event: &ShardEvent) -> bool {
    frontier
        .iter()
        .all(|boundary| definitely_before(boundary, event))
}

/// `acceptedReviewChanges`: the changes to reviewed content the Guard
/// Policy let through in the stage's current attempt, oldest first: the
/// line each was said with, and the paths it named.
fn accepted_review_changes(wf: &Workflow, slug: &str) -> Vec<(String, Option<Vec<String>>)> {
    let events = sort_attempt_events(&wf.events());
    let floor = attempt_view(wf, &events, slug);
    events
        .iter()
        .filter(|event| {
            event.event == "CHANGE_ACCEPTED"
                && after(&floor, event)
                && block_field(&event.block, "Stage") == Some(slug)
                && block_field(&event.block, "Checkpoint") == Some("review-receipt")
        })
        .map(|event| {
            let changed = block_field(&event.block, "Changed")
                .filter(|changed| *changed != "(paths unavailable)")
                .map(|changed| {
                    changed
                        .split(", ")
                        .map(js_text::trim)
                        .filter(|path| !path.is_empty())
                        .map(str::to_owned)
                        .collect()
                });
            (
                block_field(&event.block, "Details")
                    .unwrap_or("Reviewed content changed after it was reviewed.")
                    .to_owned(),
                changed,
            )
        })
        .collect()
}

/// `parseAuditPathArray`: a row's list of paths; none when it is not one.
fn path_list(value: Option<&str>) -> Vec<String> {
    value
        .filter(|value| !value.is_empty())
        .and_then(|value| json::parse(value).ok())
        .and_then(|parsed| {
            parsed.as_array().map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.as_str().map(str::to_owned))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// The file a row names, from the project (`resolveAuditProjectPath`, then
/// `relative` and `toPosix`). (Upstream resolves the name against the
/// project first, whether it is whole or not; `relative` does that itself.)
fn row_file(project: &str, recorded: &str) -> String {
    let host = Flavour::host();
    let named = recorded
        .strip_prefix("<project-dir>")
        .and_then(|rest| rest.strip_prefix(['/', '\\']))
        .unwrap_or(recorded);
    let relative = jspath::relative(host, project, project, named);
    if host.separator() == "/" {
        relative
    } else {
        relative.replace(host.separator(), "/")
    }
}

/// A set as upstream keeps one: each path once, and sorted at the end.
#[derive(Default)]
struct Paths(Vec<String>);

impl Paths {
    fn add(&mut self, path: &str) {
        if !self.0.iter().any(|known| known == path) {
            self.0.push(path.to_owned());
        }
    }

    fn sorted(mut self) -> Vec<String> {
        self.0.sort_by(|a, b| cmp_utf16(a, b));
        self.0
    }
}

/// `ReviewInvalidationDetails`.
struct Details {
    changed_upstream: Vec<String>,
    invalidated_artifacts: Vec<String>,
    invalidated_reviews: Vec<String>,
}

/// `reviewInvalidationDetails`: for a review shown again because its
/// receipt went stale, what changed under it in the stage's current
/// attempt, and what a backward jump left to check again that no gate has
/// taken since.
fn invalidation_details(wf: &Workflow, stage: &Stage, contexts: &[Shown]) -> Result<Details> {
    let slug = stage.slug.as_str();
    let project = wf.layout.project.to_string_lossy().into_owned();
    let events = sort_attempt_events(&wf.events());
    let floor = attempt_view(wf, &events, slug);
    let in_attempt = |event: &ShardEvent| after(&floor, event);
    let current_artifacts: Vec<String> = entries(wf, stage, None)?
        .iter()
        .map(|entry| workspace_artifact_path(&wf.layout.project, entry))
        .collect();
    let review_of = |artifact: &str| format!("{artifact}#Review");
    let current_reviews: Vec<String> = contexts
        .iter()
        .map(|context| review_of(&context.artifact))
        .collect();
    let mut changed_upstream = Paths::default();
    let mut invalidated_artifacts = Paths::default();
    let mut invalidated_reviews = Paths::default();
    let manifest = re::compile(
        r"(?:^|/)construction/([^/]+)/([^/]+)/source-manifest\.json$",
        "",
    );

    // The writes to the stage's documents after a review (and before the
    // request that replaced it, when one is given), of one unit or of all.
    let mut collect = |frontier: &[&ShardEvent],
                       before: Option<&ShardEvent>,
                       unit: Option<&str>| {
        let reviews_of = |unit: Option<&str>, reviews: &mut Paths| {
            for context in contexts {
                if unit.is_none_or(|unit| unit.is_empty() || context.unit.as_deref() == Some(unit))
                {
                    reviews.add(&review_of(&context.artifact));
                }
            }
        };
        for event in &events {
            if !in_attempt(event)
                || !after(frontier, event)
                || before.is_some_and(|before| !definitely_before(event, before))
            {
                continue;
            }
            if event.event != "ARTIFACT_CREATED" && event.event != "ARTIFACT_UPDATED" {
                continue;
            }
            let Some(file) = block_field(&event.block, "File").filter(|file| !file.is_empty())
            else {
                continue;
            };
            let named = row_file(&project, file);
            if current_artifacts.contains(&named) {
                let changed_unit = path_unit(&named, slug);
                if let (Some(unit), Some(changed)) = (unit, changed_unit.as_deref())
                    && changed != unit
                {
                    continue;
                }
                changed_upstream.add(&named);
                reviews_of(changed_unit.as_deref().or(unit), &mut invalidated_reviews);
                continue;
            }
            if let Some(found) = manifest.find(&named)
                && re::group(&named, &found, 2) == Some(slug)
            {
                let of = re::group(&named, &found, 1).unwrap_or_default();
                if unit.is_some_and(|unit| unit != of) {
                    continue;
                }
                changed_upstream.add(&named);
                reviews_of(Some(of), &mut invalidated_reviews);
            }
        }
    };

    // A recovery is stamped on its request. Each one is paired with the
    // receipt it replaces, so those changes survive the verdict that
    // follows.
    for event in &events {
        if !in_attempt(event)
            || event.event != "REVIEW_REQUESTED"
            || block_field(&event.block, "Stage") != Some(slug)
            || block_field(&event.block, "Recovery") != Some("stale-receipt")
        {
            continue;
        }
        let unit = block_field(&event.block, "Unit");
        let replaced: Vec<&ShardEvent> = events
            .iter()
            .filter(|candidate| {
                in_attempt(candidate)
                    && candidate.event == "REVIEW_COMPLETED"
                    && block_field(&candidate.block, "Stage") == Some(slug)
                    && block_field(&candidate.block, "Unit") == unit
                    && definitely_before(candidate, event)
            })
            .collect();
        let stale = maximal(&replaced);
        if stale.is_empty() {
            continue;
        }
        collect(&stale, Some(event), unit);
        if let (Some(unit), [replaced]) = (unit.filter(|unit| !unit.is_empty()), stale.as_slice()) {
            let bound = |row: &ShardEvent| {
                block_field(&row.block, "Unit Source Fingerprint").is_some_and(|at| !at.is_empty())
            };
            if bound(replaced) && bound(event) {
                bail!(
                    "The recovery review of unit \"{unit}\" of \"{slug}\" names the source it was bound to, and what changed in it is read off the unit's source snapshots: Focus does not read those yet (units arrive with a later slice)."
                );
            }
        }
    }

    // Newly stale receipts, unit by unit: a later receipt of one unit does
    // not move another's window past its own write.
    let mut scopes: Vec<(&str, Vec<&ShardEvent>)> = Vec::new();
    for event in &events {
        if !in_attempt(event)
            || event.event != "REVIEW_COMPLETED"
            || block_field(&event.block, "Stage") != Some(slug)
        {
            continue;
        }
        let key = block_field(&event.block, "Unit").unwrap_or("");
        match scopes.iter_mut().find(|(scope, _)| *scope == key) {
            Some((_, reviews)) => reviews.push(event),
            None => scopes.push((key, vec![event])),
        }
    }
    for (key, reviews) in &scopes {
        collect(
            &maximal(reviews),
            None,
            Some(*key).filter(|key| !key.is_empty()),
        );
    }

    // A backward jump is itself where an attempt starts. What it left to
    // check again stays until the gate of the stage that owns it has taken
    // it; what changed upstream stays as context.
    if let [boundary] = floor.as_slice()
        && boundary.event == "STAGE_JUMPED"
        && block_field(&boundary.block, "Direction") == Some("BACKWARD")
    {
        let jump_changed = path_list(block_field(&boundary.block, "Changed Upstream Artifacts"));
        let jump_artifacts = path_list(block_field(
            &boundary.block,
            "Invalidated Downstream Artifacts",
        ));
        let jump_reviews = path_list(block_field(
            &boundary.block,
            "Invalidated Downstream Reviews",
        ));
        let mut consumed: Vec<String> = Vec::new();
        for event in &events {
            if event.event != "GATE_APPROVED" && event.event != "GATE_REJECTED" {
                continue;
            }
            // Only a gate proven after the jump takes anything.
            if !definitely_before(boundary, event) {
                continue;
            }
            let Some(taken) = block_field(&event.block, "Stage")
                .filter(|taken| !taken.is_empty())
                .and_then(|taken| wf.install.stage(taken))
            else {
                continue;
            };
            for entry in entries(wf, taken, None)? {
                consumed.push(workspace_artifact_path(&wf.layout.project, &entry));
            }
        }
        let pending_artifacts: Vec<&String> = jump_artifacts
            .iter()
            .filter(|path| !consumed.contains(path))
            .collect();
        let pending_reviews: Vec<&String> = jump_reviews
            .iter()
            .filter(|path| !consumed.iter().any(|taken| review_of(taken) == **path))
            .collect();
        let relevant = jump_changed
            .iter()
            .any(|path| current_artifacts.contains(path))
            || pending_artifacts
                .iter()
                .any(|path| current_artifacts.contains(path))
            || pending_reviews
                .iter()
                .any(|path| current_reviews.contains(path));
        if relevant {
            for path in &jump_changed {
                changed_upstream.add(path);
            }
            for path in pending_artifacts {
                invalidated_artifacts.add(path);
            }
            for path in pending_reviews {
                invalidated_reviews.add(path);
            }
        }
    }

    Ok(Details {
        changed_upstream: changed_upstream.sorted(),
        invalidated_artifacts: invalidated_artifacts.sorted(),
        invalidated_reviews: invalidated_reviews.sorted(),
    })
}

/// `keepGateBrief`: the brief as printed, kept beside the review's record
/// (`reviews/<stage>/stage/briefs/<sha256>.md`, with `latest.json` naming
/// the newest), so the gate's row can record the digest of what the person
/// was shown and a reader can open exactly that text later. Best effort:
/// failing to keep it never costs the brief its print.
pub fn keep_gate_brief(wf: &Workflow, stage: &Stage, unit: Option<&str>, why: Why, brief: &str) {
    // A unit's name that is not one is never joined into a path.
    if unit.is_some_and(|unit| validate_unit_name(unit).is_some()) {
        return;
    }
    let Some(root) = wf.record_dir() else {
        return;
    };
    let scope = match unit {
        Some(unit) if stage::is_per_unit(stage) => format!("units/{unit}"),
        _ => "stage".to_owned(),
    };
    let dir = format!("{ENGINE_DIR}/reviews/{}/{scope}/briefs", stage.slug);
    let digest: String = Sha256::digest(brief.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let latest = json::stringify(&serde_json::json!({
        "digest": digest,
        "why": why.word(),
        "at": iso_timestamp(std::time::SystemTime::now()),
    }));
    // Both files are written under the record through no link, as the
    // review's record is.
    if focus_workspace::files::write_record_file_no_follow(
        &root,
        &format!("{dir}/{digest}.md"),
        brief.as_bytes(),
    )
    .is_ok()
    {
        let _ = focus_workspace::files::write_record_file_no_follow(
            &root,
            &format!("{dir}/latest.json"),
            format!("{latest}\n").as_bytes(),
        );
    }
}

/// `renderSummaryConfirmationBrief`: what the person reads before they
/// confirm the consolidated answers of a stage.
///
/// The questions file is only asked whether it is there, inside the
/// record, as upstream asks: nothing of it is read, and nothing is decided
/// by it.
///
/// # Errors
///
/// The questions file is not inside the selected record, or is not there.
pub fn summary_brief(
    wf: &Workflow,
    stage: &Stage,
    questions_file: &str,
    unit: Option<&str>,
) -> Result<String> {
    let host = Flavour::host();
    let project = wf.layout.project.to_string_lossy().into_owned();
    let questions = jspath::resolve(host, &project, &[&project, questions_file]);
    let inside = wf.record_dir().is_some_and(|record| {
        let record = record.to_string_lossy().into_owned();
        questions == record || questions.starts_with(&format!("{record}{}", host.separator()))
    });
    if !inside || !Path::new(&questions).exists() {
        return Err(EngineError::new(format!(
            "Summary confirmation questions file must exist inside the active intent record: {questions_file}"
        )));
    }
    let artifacts: Vec<String> = entries(wf, stage, unit)?
        .iter()
        .map(|entry| workspace_artifact_path(&wf.layout.project, entry))
        .collect();
    let generated = if artifacts.is_empty() {
        "the stage artifacts".to_owned()
    } else {
        ticked(&artifacts)
    };
    let relative = jspath::relative(host, &project, &project, &questions);
    let named = if host.separator() == "/" {
        relative
    } else {
        relative.replace(host.separator(), "/")
    };
    Ok([
        format!("**Stage:** {}", stage::name(stage)),
        format!(
            "**Confirming:** Consolidated answers in `{named}` before generating {generated}."
        ),
        "**Why now:** All stage questions are answered; artifact generation will use this confirmed summary.".to_owned(),
        "**Decision options:**".to_owned(),
        "- **Looks correct** - record this confirmation and generate the named artifacts.".to_owned(),
        "- **Request changes** - nothing is generated yet; say what to change in your answers, and they are updated first.".to_owned(),
    ]
    .join("\n"))
}

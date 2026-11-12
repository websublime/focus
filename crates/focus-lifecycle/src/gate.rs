//! What a gate records and reads: the ask fields on a gate-open row, the
//! stages one approval covers, the revision a person made at an open gate
//! that nobody recorded, and what Focus refuses by name at a gate.
//!
//! A gate-open row says what the person was asked to decide: the stage the
//! approval continues to, the digest of the review brief rendered for it,
//! the latest result of each applicable sensor per declared artifact, and
//! the decisions the artifacts leave to the person. None of it can stop a
//! gate: whatever cannot be read says so.
//!
//! The gate sensors fire through the sensor tool (M7), and the words the
//! person typed at the gate are the human-turn hook's record (M4): a gate
//! that would fire a sensor, or a session with gate words on disk, is
//! refused by name. So is a stage whose review findings the gate would
//! record (M5).
//!
//! Mirrors `gateAskFields`, `latestGateBriefDigest`, `sensorStateAtGate`,
//! `openDecisionsFields`, `parseDecisionsBlock`, `existingDeclaredArtifactPaths`,
//! `gateSensorMatchesOutput`, `fireGateSensors`, `briefDigestField`,
//! `decisionsAcceptedOpenField`, `unrecordedRevisionSinceGateOpen` and
//! `approvesTogetherFlag` in `aidlc-state.ts`, and `approvesTogetherStages`,
//! `approvedTogetherCover`, `openGateApprovesTogether`, `approvesTogetherField`,
//! `withdrawProtectedQuestions` and `producesArtifactFile` in `aidlc-lib.ts`.

use std::path::{Path, PathBuf};

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_graph::runtime::Stage;
use focus_graph::vocabulary::artifact_filename;
use focus_intent::plan::next_in_scope_stage;
use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{Result, bail, glob, json, re};
use focus_state::fields::get_field;
use focus_workspace::names::SESSIONS_DIR;
use focus_workspace::{WorkspaceLock, links};
use serde_json::Value;

use crate::admission;
use crate::artifacts::{KNOWN_CODEKB_STAGES, produces_dirs_for_stage};
use crate::events::Ordered;
use crate::registry::intent_repos;
use crate::stage;
use crate::workflow::Workflow;

/// `sortAttemptEvents`: timestamp, then position within a shard, then
/// shard order.
#[must_use]
pub fn sort_attempt_events(rows: &[ShardEvent]) -> Vec<ShardEvent> {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| {
        cmp_utf16(&a.timestamp, &b.timestamp).then_with(|| {
            if a.shard == b.shard {
                a.pos.cmp(&b.pos)
            } else {
                a.shard_index.cmp(&b.shard_index).then(a.pos.cmp(&b.pos))
            }
        })
    });
    sorted
}

// --- The artifacts at the gate ------------------------------------------------

fn within(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

/// `existingDeclaredArtifactPaths`: the declared artifacts on disk, as
/// canonical paths, plus the ones `artifacts` names.
///
/// # Errors
///
/// A produce directory or an artifact resolves outside the project (a link).
pub fn existing_declared_artifact_paths(
    wf: &Workflow,
    stage: &Stage,
    artifacts: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let names: Vec<String> = stage
        .produces
        .iter()
        .chain(&stage.optional_produces)
        .map(|name| artifact_filename(name))
        .collect();
    let dirs = produces_dirs_for_stage(wf, stage)?;
    let project =
        std::fs::canonicalize(&wf.layout.project).unwrap_or_else(|_| wf.layout.project.clone());
    let mut roots = Vec::new();
    for dir in &dirs {
        if !dir.exists() {
            continue;
        }
        let Ok(canonical) = std::fs::canonicalize(dir) else {
            continue;
        };
        if !canonical.is_dir() {
            continue;
        }
        if !within(&project, &canonical) {
            bail!(
                "Refusing gate sensor dispatch for \"{}\": resolved produce directory \"{}\" escapes the project root after canonicalization.",
                stage.slug,
                dir.display()
            );
        }
        roots.push(canonical);
    }
    let mut found: Vec<PathBuf> = Vec::new();
    let mut add = |candidate: PathBuf| -> Result<()> {
        let name = candidate
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !names.contains(&name) || !candidate.exists() {
            return Ok(());
        }
        let Ok(canonical) = std::fs::canonicalize(&candidate) else {
            return Ok(());
        };
        if !canonical.is_file() {
            return Ok(());
        }
        if !roots.iter().any(|root| within(root, &canonical)) {
            bail!(
                "Refusing gate sensor artifact \"{}\" for \"{}\": the canonical path is outside the stage's resolved produce directories.",
                candidate.display(),
                stage.slug
            );
        }
        if !found.contains(&canonical) {
            found.push(canonical);
        }
        Ok(())
    };
    for dir in &dirs {
        for name in &names {
            add(dir.join(name))?;
        }
    }
    for token in artifacts.unwrap_or("").split(',') {
        let value = trim(token);
        if value.is_empty() {
            continue;
        }
        let base = Path::new(value)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let variants: Vec<String> = if names.contains(&base) {
            vec![value.to_owned()]
        } else {
            vec![value.to_owned(), artifact_filename(value)]
        };
        for variant in &variants {
            let as_path = Path::new(variant);
            if as_path.is_absolute() || variant.contains('/') || variant.contains('\\') {
                add(if as_path.is_absolute() {
                    as_path.to_path_buf()
                } else {
                    wf.layout.project.join(variant)
                })?;
            }
            for dir in &dirs {
                add(dir.join(variant))?;
            }
        }
    }
    Ok(found)
}

/// One applicable sensor of a stage, as the compiled node lists it.
struct Sensor {
    id: String,
    fire_on: Option<String>,
    severity: Option<String>,
    matches: Option<String>,
}

fn sensors_of(stage: &Stage) -> Vec<Sensor> {
    stage
        .raw
        .get("sensors_applicable")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|sensor| Sensor {
            id: sensor
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            fire_on: sensor
                .get("fire_on")
                .and_then(Value::as_str)
                .map(str::to_owned),
            severity: sensor
                .get("default_severity")
                .and_then(Value::as_str)
                .map(str::to_owned),
            matches: sensor
                .get("matches")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
        .collect()
}

/// `gateSensorMatchesOutput`.
fn sensor_matches(sensor: &Sensor, path: &Path) -> bool {
    match &sensor.matches {
        None => true,
        Some(pattern) => glob::matches(pattern, &path.to_string_lossy().replace('\\', "/")),
    }
}

fn sensors_ceremony_off(wf: &Workflow, state: &str) -> bool {
    admission::ceremony_setting(wf, state, "sensors", "Sensors", "DISABLE_SENSORS") == "off"
}

/// `fireGateSensors`, as far as Focus goes: the sensors that would fire
/// at this gate. A gate that would fire one is refused by name (the sensor
/// tool is M7); with the ceremony off, no artifact on disk, or no sensor
/// applicable, nothing fires and nothing is refused.
///
/// # Errors
///
/// A sensor would fire, or an artifact path cannot be settled.
pub fn refuse_gate_sensors(
    wf: &Workflow,
    stage: &Stage,
    state: &str,
    artifacts: Option<&str>,
    person_answers: bool,
) -> Result<()> {
    if sensors_ceremony_off(wf, state) {
        return Ok(());
    }
    let paths = existing_declared_artifact_paths(wf, stage, artifacts)?;
    if paths.is_empty() {
        return Ok(());
    }
    let firing: Vec<String> = sensors_of(stage)
        .iter()
        .filter(|sensor| sensor.fire_on.as_deref() == Some("gate"))
        .filter(|sensor| person_answers || sensor.severity.as_deref() == Some("blocking"))
        .filter(|sensor| paths.iter().any(|path| sensor_matches(sensor, path)))
        .map(|sensor| sensor.id.clone())
        .collect();
    if firing.is_empty() {
        return Ok(());
    }
    bail!(
        "Opening the gate of \"{}\" fires its gate sensors ({}) on the artifacts on disk, through the sensor tool: Focus does not run sensors yet. Turn the ceremony off for this piece of work (`--sensors off` at creation, or {}=1) to open the gate without them.",
        stage.slug,
        firing.join(", "),
        focus_workspace::env::full_name("DISABLE_SENSORS")
    )
}

// --- The ask fields -------------------------------------------------------------

/// `latestGateBriefDigest`: read by name under the engine directory; the
/// brief is the review CLI's rendering (M5), and nothing is decided by it.
fn latest_gate_brief_digest(wf: &Workflow, stage: &Stage) -> Option<String> {
    let latest = wf
        .selection
        .engine_dir(&wf.layout)
        .join("reviews")
        .join(&stage.slug)
        .join("stage")
        .join("briefs")
        .join("latest.json");
    let text = std::fs::read_to_string(latest).ok()?;
    let parsed = json::parse(&text).ok()?;
    let digest = parsed.get("digest")?.as_str()?;
    re::test(r"^[0-9a-f]{64}$", "", digest).then(|| digest.to_owned())
}

/// `briefDigestField`.
#[must_use]
pub fn brief_digest_field(wf: &Workflow, stage: &Stage) -> (&'static str, String) {
    (
        "Brief Digest",
        latest_gate_brief_digest(wf, stage)
            .map_or_else(|| "none".to_owned(), |digest| format!("sha256:{digest}")),
    )
}

const SENSOR_STATE_MAX_ENTRIES: usize = 20;

/// `sensorStateAtGate`.
fn sensor_state_at_gate(wf: &Workflow, stage: &Stage) -> Result<String> {
    let sensors = sensors_of(stage);
    let paths = existing_declared_artifact_paths(wf, stage, None)?;
    if sensors.is_empty() || paths.is_empty() {
        return Ok("none".to_owned());
    }
    struct Latest {
        passed: bool,
        ts: String,
        pos: usize,
        findings: String,
        detail: Option<String>,
    }
    let mut latest: Vec<(String, Latest)> = Vec::new();
    for row in wf.events() {
        if row.event != "SENSOR_PASSED" && row.event != "SENSOR_FAILED" {
            continue;
        }
        if block_field(&row.block, "Stage slug") != Some(&stage.slug) {
            continue;
        }
        let (Some(id), Some(output)) = (
            block_field(&row.block, "Sensor ID"),
            block_field(&row.block, "Output path"),
        ) else {
            continue;
        };
        let key = format!("{id}@{output}");
        if let Some((_, seen)) = latest.iter().find(|(k, _)| *k == key)
            && (cmp_utf16(&seen.ts, &row.timestamp).is_gt()
                || (seen.ts == row.timestamp && seen.pos > row.pos))
        {
            continue;
        }
        let entry = Latest {
            passed: row.event == "SENSOR_PASSED",
            ts: row.timestamp.clone(),
            pos: row.pos,
            findings: block_field(&row.block, "Findings count")
                .unwrap_or("?")
                .to_owned(),
            detail: block_field(&row.block, "Detail path").map(str::to_owned),
        };
        match latest.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = entry,
            None => latest.push((key, entry)),
        }
    }
    let project = &wf.layout.project;
    let canonical_project = std::fs::canonicalize(project).unwrap_or_else(|_| project.clone());
    let mut failed = Vec::new();
    let mut rest = Vec::new();
    for sensor in &sensors {
        for path in &paths {
            if !sensor_matches(sensor, path) {
                continue;
            }
            let rel = path
                .strip_prefix(&canonical_project)
                .or_else(|_| path.strip_prefix(project))
                .unwrap_or(path)
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let key = format!("{}@{rel}", sensor.id);
            match latest.iter().find(|(k, _)| *k == key) {
                None => rest.push(format!("{}@{rel}=not-run", sensor.id)),
                Some((_, result)) if result.passed => {
                    rest.push(format!("{}@{rel}=passed", sensor.id))
                }
                Some((_, result)) => failed.push(format!(
                    "{}@{rel}=failed({}){}",
                    sensor.id,
                    result.findings,
                    result
                        .detail
                        .as_deref()
                        .map_or(String::new(), |detail| format!(" [detail: {detail}]"))
                )),
            }
        }
    }
    let entries: Vec<String> = failed.into_iter().chain(rest).collect();
    if entries.is_empty() {
        return Ok("none".to_owned());
    }
    let shown = &entries[..entries.len().min(SENSOR_STATE_MAX_ENTRIES)];
    let more = entries.len() - shown.len();
    Ok(format!(
        "{}{}",
        shown.join("; "),
        if more > 0 {
            format!("; +{more} more")
        } else {
            String::new()
        }
    ))
}

/// `parseDecisionsBlock`: the ids of a `decisions:` block, or `None` when
/// the block cannot be read.
fn parse_decisions_block(body: &str) -> Option<Vec<String>> {
    let lines: Vec<String> = body
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .map(|line| re::replace(r"\s+$", "", line, ""))
        .filter(|line| !trim(line).is_empty())
        .collect();
    if lines.is_empty() || trim(&lines[0]) != "decisions:" {
        return None;
    }
    let mut ids = Vec::new();
    for line in &lines[1..] {
        if let Some(found) = re::find(r"^\s*-\s+id:\s*(\S+)\s*$", "", line) {
            ids.push(re::group(line, &found, 1)?.to_owned());
            continue;
        }
        if !re::test(r"^\s+(decision|owner|blocking):\s*\S", "", line) {
            return None;
        }
    }
    if ids.is_empty() { None } else { Some(ids) }
}

/// `openDecisionsFields`.
fn open_decisions_fields(wf: &Workflow, stage: &Stage) -> Result<Vec<(&'static str, String)>> {
    let mut ids: Vec<String> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();
    for path in existing_declared_artifact_paths(wf, stage, None)? {
        let text = std::fs::read_to_string(&path)
            .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))?;
        let fence = re::compile(r"```focus-decisions[^\n]*\n([\s\S]*?)```", "g");
        for found in fence.find_iter(&text) {
            let body = re::group(&text, &found, 1).unwrap_or("");
            match parse_decisions_block(body) {
                None => unreadable.push(
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
                Some(parsed) => ids.extend(parsed),
            }
        }
    }
    if !unreadable.is_empty() {
        let mut unique: Vec<String> = Vec::new();
        for name in unreadable {
            if !unique.contains(&name) {
                unique.push(name);
            }
        }
        return Ok(vec![(
            "Open Decisions",
            format!("unreadable ({})", unique.join(", ")),
        )]);
    }
    let mut unique: Vec<String> = Vec::new();
    for id in ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    if unique.is_empty() {
        Ok(vec![("Open Decisions", "0".to_owned())])
    } else {
        Ok(vec![
            ("Open Decisions", unique.len().to_string()),
            ("Decisions", unique.join(", ")),
        ])
    }
}

/// `gateAskFields`: what the gate asked the person to decide. Record-only:
/// whatever cannot be read says so.
#[must_use]
pub fn gate_ask_fields(
    wf: &Workflow,
    stage: &Stage,
    state: &str,
    listed_together: &[String],
) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    let next = get_field(state, "Scope")
        .and_then(|scope| wf.install.definition(scope))
        .and_then(|definition| {
            next_in_scope_stage(
                listed_together
                    .last()
                    .map_or(stage.slug.as_str(), String::as_str),
                definition,
                &wf.install.graph,
                Some(state),
            )
        });
    fields.push((
        "Next Stage",
        next.map_or("none".to_owned(), |next| next.slug.clone()),
    ));
    fields.push(brief_digest_field(wf, stage));
    fields.push((
        "Sensor State",
        sensor_state_at_gate(wf, stage).unwrap_or_else(|_| "none".to_owned()),
    ));
    match open_decisions_fields(wf, stage) {
        Ok(decisions) => fields.extend(decisions),
        Err(_) => fields.push(("Open Decisions", "0".to_owned())),
    }
    fields
}

/// `decisionsAcceptedOpenField`: the ids the gate-open row listed.
#[must_use]
pub fn decisions_accepted_open_field(wf: &Workflow, slug: &str) -> Option<(&'static str, String)> {
    let open = wf.events().into_iter().rfind(|row| {
        row.event == "STAGE_AWAITING_APPROVAL" && block_field(&row.block, "Stage") == Some(slug)
    })?;
    block_field(&open.block, "Decisions")
        .map(|decisions| ("Decisions Accepted Open", decisions.to_owned()))
}

// --- Approves together -----------------------------------------------------------

const APPROVES_TOGETHER_FIELD: &str = "Approves Together";
pub const APPROVED_TOGETHER_WITH_FIELD: &str = "Approved Together With";
const APPROVED_TOGETHER_ENDS: [&str; 8] = [
    "GATE_REJECTED",
    "QUESTION_ANSWERED",
    "QUESTION_UNANSWERED",
    "REQUEST_ROUTED",
    "DECISION_RECORDED",
    "AUTONOMY_MODE_SET",
    "WORKFLOW_STARTED",
    "STAGE_JUMPED",
];

fn approves_together_field(block: &str) -> Vec<String> {
    block_field(block, APPROVES_TOGETHER_FIELD)
        .unwrap_or("")
        .split(',')
        .map(trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `approvesTogetherStages`: the per-unit Construction stages one gate
/// covers. A Construction stage is refused before a gate opens, so here
/// no gate covers more than its own stage.
#[must_use]
pub fn approves_together_stages(_state: &str, _slug: &str) -> Option<Vec<String>> {
    None
}

/// `approvesTogetherFlag`: the stages `--approves-together` names must be
/// exactly the ones the gate covers.
///
/// # Errors
///
/// The list is not the gate's.
pub fn approves_together_flag(raw: Option<&str>, state: &str, slug: &str) -> Result<Vec<String>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let listed: Vec<String> = raw
        .split(',')
        .map(trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let expected = approves_together_stages(state, slug);
    if expected
        .as_ref()
        .is_none_or(|expected| listed.join(",") != expected.join(","))
    {
        bail!(
            "--approves-together for \"{slug}\" must name exactly the stages its gate covers ({}).",
            expected.map_or("none".to_owned(), |expected| expected.join(", "))
        );
    }
    Ok(listed)
}

/// `approvesTogetherFields`.
#[must_use]
pub fn approves_together_fields(stages: &[String]) -> Vec<(&'static str, String)> {
    if stages.len() >= 2 {
        vec![(APPROVES_TOGETHER_FIELD, stages.join(", "))]
    } else {
        Vec::new()
    }
}

/// `approvedTogetherWithField`.
#[must_use]
pub fn approved_together_with_field(first: &str) -> (&'static str, String) {
    (APPROVED_TOGETHER_WITH_FIELD, first.to_owned())
}

/// `approvedTogetherCover`: the earlier stage whose approval listed `slug`
/// and still covers it.
#[must_use]
pub fn approved_together_cover(wf: &Workflow, slug: &str) -> Option<String> {
    let rows = sort_attempt_events(&wf.events());
    for (index, row) in rows.iter().enumerate().rev() {
        if row.event != "GATE_APPROVED" {
            continue;
        }
        let listed = approves_together_field(&row.block);
        let Some(first) = block_field(&row.block, "Stage") else {
            continue;
        };
        if first == slug || !listed.iter().any(|s| s == slug) {
            continue;
        }
        for later in &rows[index + 1..] {
            if APPROVED_TOGETHER_ENDS.contains(&later.event.as_str()) {
                return None;
            }
            if later.event != "GATE_APPROVED" && later.event != "STAGE_AWAITING_APPROVAL" {
                continue;
            }
            let stage = block_field(&later.block, "Stage").unwrap_or("");
            if !listed.iter().any(|s| s == stage) {
                return None;
            }
            if later.event == "GATE_APPROVED" && stage == slug {
                return None;
            }
        }
        return Some(first.to_owned());
    }
    None
}

/// `openGateApprovesTogether`: the stages the open gate's row listed.
#[must_use]
pub fn open_gate_approves_together(wf: &Workflow, slug: &str) -> Vec<String> {
    let rows = sort_attempt_events(&wf.events());
    rows.iter()
        .rev()
        .find(|row| {
            row.event == "STAGE_AWAITING_APPROVAL" && block_field(&row.block, "Stage") == Some(slug)
        })
        .map(|row| approves_together_field(&row.block))
        .unwrap_or_default()
}

// --- The revision backstop ---------------------------------------------------------

/// `producesArtifactFile`: whether a written file is one the stage produces.
pub fn produces_artifact_file(stage: &Stage, file: &str, recorded_repos: &[String]) -> bool {
    produces_file(&stage.slug, &stage.produces, file, recorded_repos)
}

fn produces_file(slug: &str, produces: &[String], file: &str, recorded_repos: &[String]) -> bool {
    if produces.is_empty() {
        return false;
    }
    let norm = file.replace('\\', "/");
    if KNOWN_CODEKB_STAGES.contains(&slug) {
        return produces.iter().any(|name| {
            let filename = format!("/{}", artifact_filename(name));
            let Some(idx) = norm.rfind(&filename) else {
                return false;
            };
            if idx + filename.len() != norm.len() {
                return false;
            }
            let head = &norm[..idx];
            let Some(repo_slash) = head.rfind('/') else {
                return false;
            };
            if !head[..repo_slash].ends_with("/codekb") {
                return false;
            }
            let repo = &head[repo_slash + 1..];
            !repo.is_empty()
                && (recorded_repos.is_empty() || recorded_repos.iter().any(|r| r == repo))
        });
    }
    produces
        .iter()
        .any(|name| norm.ends_with(&format!("/{slug}/{}", artifact_filename(name))))
}

/// `producesArtifactUnit`: `None` when the file is not one of the stage's
/// declared or optional artifacts; otherwise the unit a per-unit path names
/// (`…/construction/<unit>/<slug>/<file>`), `Some(None)` for a stage-level
/// artifact or an ambiguous per-unit path.
#[must_use]
pub fn produces_artifact_unit(
    stage: &Stage,
    file: &str,
    recorded_repos: &[String],
    produces: &[String],
) -> Option<Option<String>> {
    if !produces_file(&stage.slug, produces, file, recorded_repos) {
        return None;
    }
    if !stage::is_per_unit(stage) {
        return Some(None);
    }
    let norm = file.replace('\\', "/");
    for name in produces {
        let suffix = format!("/{}/{}", stage.slug, artifact_filename(name));
        let Some(parent) = norm.strip_suffix(&suffix) else {
            continue;
        };
        const MARKER: &str = "/construction/";
        let Some(idx) = parent.rfind(MARKER) else {
            return Some(None);
        };
        let unit = &parent[idx + MARKER.len()..];
        return Some((!unit.is_empty() && !unit.contains('/')).then(|| unit.to_owned()));
    }
    Some(None)
}

/// `summaryOwnedReviewInput`: a summary stage's questions file, whose
/// confirmation answers do not change the reviewed input.
fn summary_owned_review_input(stage: &Stage, artifact: &str) -> bool {
    stage::summary_confirmation(stage).is_some()
        && artifact.ends_with("-questions")
        && stage.raw.get("review_artifact").and_then(Value::as_str) != Some(artifact)
}

/// `reviewedArtifactUnit`: [`produces_artifact_unit`] over the artifacts a
/// review covers: the stage's declared and optional ones, less a summary
/// stage's own questions file.
#[must_use]
pub fn reviewed_artifact_unit(
    stage: &Stage,
    file: &str,
    recorded_repos: &[String],
) -> Option<Option<String>> {
    let reviewed: Vec<String> = stage
        .produces
        .iter()
        .chain(stage.optional_produces.iter())
        .filter(|name| !summary_owned_review_input(stage, name))
        .cloned()
        .collect();
    produces_artifact_unit(stage, file, recorded_repos, &reviewed)
}

/// `unrecordedRevisionSinceGateOpen`: the person revised the stage's
/// artifact at an open gate and no reject was recorded.
///
/// # Errors
///
/// The row's `repos` is not a list of names.
pub fn unrecorded_revision_since_gate_open(wf: &Workflow, stage: &Stage) -> Result<bool> {
    if wf.audit_text().is_empty() {
        return Ok(false);
    }
    let repos = intent_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    Ok(unrecorded_revision(wf, stage, &repos))
}

fn unrecorded_revision(wf: &Workflow, stage: &Stage, repos: &[String]) -> bool {
    let audit = wf.audit_text();
    const RELEVANT: [&str; 6] = [
        "STAGE_AWAITING_APPROVAL",
        "STAGE_STARTED",
        "GATE_REJECTED",
        "HUMAN_TURN",
        "ARTIFACT_CREATED",
        "ARTIFACT_UPDATED",
    ];
    let field = |block: &str, name: &str| -> Option<String> {
        let label = format!("**{name}**:");
        block
            .split('\n')
            .find_map(|line| line.strip_prefix(label.as_str()))
            .map(|value| trim(value).to_owned())
    };
    let mut events: Vec<(Ordered, Option<String>, Option<String>, bool)> = audit
        .replace("\r\n", "\n")
        .split("\n---\n")
        .enumerate()
        .filter_map(|(position, block)| {
            let event = field(block, "Event")?;
            if !RELEVANT.contains(&event.as_str()) {
                return None;
            }
            Some((
                Ordered {
                    event,
                    block: block.to_owned(),
                    timestamp: field(block, "Timestamp").unwrap_or_default(),
                    position,
                },
                field(block, "Stage"),
                field(block, "File"),
                field(block, "Recovered").as_deref() == Some("true"),
            ))
        })
        .collect();
    events.sort_by(|a, b| {
        cmp_utf16(&a.0.timestamp, &b.0.timestamp).then(a.0.position.cmp(&b.0.position))
    });
    let mut anchor: Option<usize> = None;
    let mut anchor_is_gate_open = false;
    for (index, (event, stage_of, _, recovered)) in events.iter().enumerate() {
        if stage_of.as_deref() != Some(&stage.slug) {
            continue;
        }
        if event.event == "STAGE_AWAITING_APPROVAL" && !recovered {
            anchor = Some(index);
            anchor_is_gate_open = true;
        } else if event.event == "STAGE_STARTED" {
            anchor = Some(index);
            anchor_is_gate_open = false;
        }
    }
    let Some(anchor) = anchor else {
        return false;
    };
    let mut first_human: Option<usize> = None;
    let mut last_human: Option<usize> = None;
    let mut wrote_before_human = false;
    for (index, (event, stage_of, file, _)) in events.iter().enumerate().skip(anchor + 1) {
        if event.event == "GATE_REJECTED" && stage_of.as_deref() == Some(&stage.slug) {
            return false;
        }
        if event.event == "HUMAN_TURN" {
            if first_human.is_none() {
                first_human = Some(index);
            }
            last_human = Some(index);
        } else if first_human.is_none()
            && (event.event == "ARTIFACT_CREATED" || event.event == "ARTIFACT_UPDATED")
            && file
                .as_deref()
                .is_some_and(|file| produces_artifact_file(stage, file, repos))
        {
            wrote_before_human = true;
        }
    }
    let (Some(first), Some(last)) = (first_human, last_human) else {
        return false;
    };
    if !anchor_is_gate_open && !wrote_before_human {
        return false;
    }
    events[first + 1..last].iter().any(|(event, _, file, _)| {
        (event.event == "ARTIFACT_CREATED" || event.event == "ARTIFACT_UPDATED")
            && file
                .as_deref()
                .is_some_and(|file| produces_artifact_file(stage, file, repos))
    })
}

// --- What a gate refuses by name --------------------------------------------------

/// The review findings a gate would record on its decision row (M5): a
/// stage with a reviewer whose ledger holds review rows, or whose review
/// artifact carries a `## Review` section.
///
/// # Errors
///
/// There are findings to record.
pub fn refuse_review_findings(wf: &Workflow, stage: &Stage) -> Result<()> {
    if stage::reviewer(stage).is_none() {
        return Ok(());
    }
    if wf
        .events()
        .iter()
        .any(|row| row.event.starts_with("REVIEW_"))
    {
        bail!(
            "\"{}\" has review rows in its ledger, whose findings the gate would record on its decision: Focus does not read review findings yet.",
            stage.slug
        );
    }
    let Some(record) = wf.record_dir() else {
        return Ok(());
    };
    let review_artifact = stage.raw.get("review_artifact").and_then(Value::as_str);
    if let Some(artifact) = review_artifact {
        let path = record
            .join(&stage.phase)
            .join(&stage.slug)
            .join(artifact_filename(artifact));
        // As upstream reads the section: a heading in a code block does not
        // open it, and an empty section is no section.
        let has_review_section = std::fs::read_to_string(&path).ok().is_some_and(|text| {
            focus_markdown::visible_lines(&text, focus_markdown::Visible::default())
                .map(|lines| {
                    !focus_graph::guard_memory::section_body(&lines, "## Review").is_empty()
                })
                .unwrap_or(false)
        });
        if has_review_section {
            bail!(
                "{} carries a `## Review` section, whose findings the gate would record on its decision: Focus does not read review findings yet.",
                path.display()
            );
        }
    }
    Ok(())
}

/// `withdrawProtectedQuestions(pd, "*")`: the protected questions kept for
/// every session are removed when a gate opens.
///
/// # Errors
///
/// The directory sits behind a link, or a file cannot be removed.
pub fn withdraw_protected_questions(wf: &Workflow, _held: &WorkspaceLock) -> Result<()> {
    let dir = wf
        .layout
        .workspace_root()
        .join(SESSIONS_DIR)
        .join("plan-approval");
    links::refuse_link_on_the_way(&wf.layout.project, &dir)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => bail!("{}: {err}", dir.display()),
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("protected-question-")
            && name.ends_with(".json")
            && let Err(err) = std::fs::remove_file(entry.path())
            && err.kind() != std::io::ErrorKind::NotFound
        {
            bail!("{}: {err}", entry.path().display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decisions_block_lists_its_ids() {
        assert_eq!(
            parse_decisions_block(
                "decisions:\n  - id: D-1\n    decision: Which db\n    owner: person\n  - id: D-2\n"
            ),
            Some(vec!["D-1".to_owned(), "D-2".to_owned()])
        );
        assert_eq!(
            parse_decisions_block("decisions:\r\n  - id: D-1\r\n"),
            Some(vec!["D-1".to_owned()])
        );
        assert_eq!(parse_decisions_block("items:\n  - id: D-1\n"), None);
        assert_eq!(parse_decisions_block("decisions:\n"), None);
        assert_eq!(
            parse_decisions_block("decisions:\n  - id: D-1\n    note: x\n"),
            None
        );
        assert_eq!(parse_decisions_block(""), None);
    }

    #[test]
    fn a_produced_file_is_known_by_its_tail() {
        let stage = &focus_graph::runtime::load_from_value(
            &serde_json::json!([{"slug": "scope-definition", "phase": "ideation", "produces": ["scope-document", "traceability"]}]),
        )[0];
        assert!(produces_artifact_file(
            stage,
            "focus/spaces/default/intents/x/ideation/scope-definition/scope-document.md",
            &[]
        ));
        assert!(produces_artifact_file(
            stage,
            "C:\\p\\ideation\\scope-definition\\traceability.json",
            &[]
        ));
        assert!(!produces_artifact_file(
            stage,
            "focus/x/ideation/other/scope-document.md",
            &[]
        ));
        let codekb = &focus_graph::runtime::load_from_value(
            &serde_json::json!([{"slug": "reverse-engineering", "phase": "inception", "produces": ["architecture"]}]),
        )[0];
        assert!(produces_artifact_file(
            codekb,
            "focus/spaces/default/codekb/app/architecture.md",
            &[]
        ));
        assert!(produces_artifact_file(
            codekb,
            "focus/spaces/default/codekb/app/architecture.md",
            &["app".to_owned()]
        ));
        assert!(!produces_artifact_file(
            codekb,
            "focus/spaces/default/codekb/app/architecture.md",
            &["web".to_owned()]
        ));
        assert!(!produces_artifact_file(
            codekb,
            "focus/spaces/default/codekb/architecture.md",
            &[]
        ));
        assert!(!produces_artifact_file(
            codekb,
            "focus/spaces/default/codekb/a/b/architecture.md",
            &[]
        ));
    }

    #[test]
    fn a_reviewed_artifact_names_its_unit_or_none() {
        let stages = focus_graph::runtime::load_from_value(&serde_json::json!([
            {"slug": "intent-capture", "phase": "ideation", "produces": ["intent-statement", "intent-capture-questions"], "optional_produces": ["stakeholder-map"], "summary_confirmation": "required", "review_artifact": "intent-statement"},
            {"slug": "functional-design", "phase": "construction", "for_each": "unit-of-work", "produces": ["functional-spec", "traceability"]},
        ]));
        let (capture, design) = (&stages[0], &stages[1]);
        // A stage-level artifact, declared or optional: `Some(None)`; a file
        // that is not one: `None`; the summary stage's own questions file is
        // not reviewed input.
        assert_eq!(
            reviewed_artifact_unit(
                capture,
                "/p/ideation/intent-capture/intent-statement.md",
                &[]
            ),
            Some(None)
        );
        assert_eq!(
            reviewed_artifact_unit(
                capture,
                "/p/ideation/intent-capture/stakeholder-map.md",
                &[]
            ),
            Some(None)
        );
        assert_eq!(
            reviewed_artifact_unit(
                capture,
                "/p/ideation/intent-capture/intent-capture-questions.md",
                &[]
            ),
            None
        );
        assert_eq!(
            reviewed_artifact_unit(capture, "/p/ideation/other/intent-statement.md", &[]),
            None
        );
        // A stage that does not run per unit names no unit, whatever the path.
        assert_eq!(
            reviewed_artifact_unit(
                capture,
                "/p/construction/auth/intent-capture/intent-statement.md",
                &[]
            ),
            Some(None)
        );
        // A per-unit path names its unit; an ambiguous one names none.
        assert_eq!(
            reviewed_artifact_unit(
                design,
                "/p/construction/auth/functional-design/functional-spec.md",
                &[]
            ),
            Some(Some("auth".to_owned()))
        );
        assert_eq!(
            reviewed_artifact_unit(
                design,
                "C:\\p\\construction\\auth\\functional-design\\traceability.json",
                &[]
            ),
            Some(Some("auth".to_owned()))
        );
        assert_eq!(
            reviewed_artifact_unit(design, "/p/functional-design/functional-spec.md", &[]),
            Some(None)
        );
        assert_eq!(
            reviewed_artifact_unit(
                design,
                "/p/construction/a/b/functional-design/functional-spec.md",
                &[]
            ),
            Some(None)
        );
        assert_eq!(
            reviewed_artifact_unit(
                design,
                "/p/construction//functional-design/functional-spec.md",
                &[]
            ),
            Some(None)
        );
    }
}

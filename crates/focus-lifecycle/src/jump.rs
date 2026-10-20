//! Jumping between stages: `resolve` says where a jump would land and what
//! it passes over or resets; `execute` moves the work there, with the rows
//! an advance would write at a phase boundary and a `STAGE_JUMPED` that
//! names, for a backward jump, the artifacts and reviews it invalidates.
//!
//! Every read of the plan goes through the effective action: the state's
//! own EXECUTE/SKIP suffix for a stage wins over the scope's grid, when
//! the scope asked about is the state's own.
//!
//! What is not here, and refused by name: `reopen` and `execute --units`,
//! which move units of work one at a time through per-unit stages (M4);
//! the record's active-directive marker and the pending person lines of a
//! chat (the orchestrator's and the human-turn hook's, M4/M5); and the
//! review records a backward jump names (the review brief, M5). The source
//! baseline upstream stamps on the rows is the attestation's (M5) and is
//! not written.
//!
//! Mirrors `aidlc-jump.ts`: `handleResolve`, `handleExecute`, `movedState`,
//! `forwardJumpNotice`, `backwardJumpNotice`, `stageArtifactPaths`,
//! `stageReviewPaths` and `parseFlags`; `stageLabel` in
//! `aidlc-validity.ts`.

use std::path::Path;
use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_graph::runtime::{ScopeDefinition, Stage};
use focus_graph::stage_schema::{PHASES, SLUG_RE};
use focus_intent::plan::{next_in_scope_stage, stages_in_scope};
use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{Result, bail, json, re};
use focus_state::fields::{get_field, remove_field, set_field, set_phase_progress};
use focus_state::progress::{
    CheckboxState, count_checkboxes, parse_checkboxes, parse_stage_suffixes, set_checkbox,
};
use serde_json::{Map, Value, json};

use crate::artifacts::{review_artifact_entries, workspace_artifact_path};
use crate::events::answer_mode_fields;
use crate::stage;
use crate::workflow::Workflow;

/// `parseFlags`: `--name value` pairs; a flag with nothing after it is not
/// one, and a value is whatever follows, another flag included.
#[must_use]
pub fn parse_flags(args: &[String]) -> Vec<(String, String)> {
    let mut flags: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    while at < args.len() {
        if args[at].starts_with("--") && at + 1 < args.len() {
            let name = args[at][2..].to_owned();
            let value = args[at + 1].clone();
            match flags.iter_mut().find(|(known, _)| *known == name) {
                Some(slot) => slot.1 = value,
                None => flags.push((name, value)),
            }
            at += 2;
        } else {
            at += 1;
        }
    }
    flags
}

fn flag<'a>(flags: &'a [(String, String)], name: &str) -> Option<&'a str> {
    flags
        .iter()
        .find(|(known, _)| known == name)
        .map(|(_, value)| value.as_str())
}

/// `stageLabel`: the stage's name, or its slug for a plugin's stage, or
/// nothing when the slug is not one.
#[must_use]
pub fn stage_label(stage: Option<&Stage>, slug: &str) -> Option<String> {
    if let Some(stage) = stage
        && stage.plugin.is_none()
    {
        return Some(stage::name(stage).to_owned());
    }
    let typed = stage.map_or(slug, |stage| stage.slug.as_str());
    re::test(SLUG_RE, "", typed).then(|| typed.to_owned())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Forward,
    Backward,
    Redo,
}

impl Direction {
    fn word(self) -> &'static str {
        match self {
            Self::Forward => "forward",
            Self::Backward => "backward",
            Self::Redo => "redo",
        }
    }

    fn parse(word: &str) -> Option<Self> {
        match word {
            "forward" => Some(Self::Forward),
            "backward" => Some(Self::Backward),
            "redo" => Some(Self::Redo),
            _ => None,
        }
    }
}

/// The scope a jump reads and its effective plan: the state's suffixes when
/// the scope is the state's own, else the scope's grid alone.
struct Plan<'a> {
    scope: String,
    definition: &'a ScopeDefinition,
    suffixes: Vec<(String, &'static str)>,
}

impl Plan<'_> {
    /// `effectiveAction`: the suffix, else the grid's own word.
    fn action(&self, slug: &str) -> Option<String> {
        if let Some((_, action)) = self.suffixes.iter().find(|(known, _)| known == slug) {
            return Some((*action).to_owned());
        }
        self.definition
            .stages
            .get(slug)
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn executes(&self, slug: &str) -> bool {
        self.action(slug).as_deref() == Some("EXECUTE")
    }
}

fn plan<'a>(wf: &'a Workflow, content: &str, asked: Option<&str>) -> Result<Plan<'a>> {
    let own = get_field(content, "Scope").unwrap_or("");
    let scope = asked
        .filter(|scope| !scope.is_empty())
        .or_else(|| (!own.is_empty()).then_some(own))
        .unwrap_or("feature")
        .to_owned();
    let Some(definition) = wf.install.definition(&scope) else {
        bail!("Unknown scope: {scope}");
    };
    let suffixes = if scope == own {
        parse_stage_suffixes(content)
    } else {
        Vec::new()
    };
    Ok(Plan {
        scope,
        definition,
        suffixes,
    })
}

/// `resolveStage`: by slug, else by number.
fn resolve_stage<'a>(wf: &'a Workflow, slug_or_number: &str) -> Option<&'a Stage> {
    wf.install.stage(slug_or_number).or_else(|| {
        wf.install
            .graph
            .iter()
            .find(|stage| stage.number == slug_or_number)
    })
}

fn stage_index(wf: &Workflow, slug: &str) -> Option<usize> {
    wf.install.graph.iter().position(|stage| stage.slug == slug)
}

const PHASE_NUMBERS: [(&str, &str); 5] = [
    ("0", "initialization"),
    ("1", "ideation"),
    ("2", "inception"),
    ("3", "construction"),
    ("4", "operation"),
];

fn refuse_units_flag(flags: &[(String, String)]) -> Result<()> {
    if flag(flags, "units").is_some() {
        bail!(
            "--units moves units of work forward one at a time, which needs the units' lifecycle: Focus does not run units yet."
        );
    }
    Ok(())
}

/// `handleResolve`.
///
/// # Errors
///
/// What upstream refuses, in its words.
pub fn resolve(wf: &Workflow, args: &[String]) -> Result<Value> {
    let allow_skipped = args.iter().any(|arg| arg == "--allow-skipped");
    let kept: Vec<String> = args
        .iter()
        .filter(|arg| *arg != "--allow-skipped")
        .cloned()
        .collect();
    let flags = parse_flags(&kept);
    let content = wf.read_state()?;
    let plan = plan(wf, &content, flag(&flags, "scope"))?;
    let current_slug = get_field(&content, "Current Stage")
        .filter(|slug| !slug.is_empty())
        .unwrap_or("state-init");
    let Some(current) = resolve_stage(wf, current_slug) else {
        bail!("Cannot resolve current stage: {current_slug}");
    };
    let mut target_skipped = false;
    let target: &Stage = if let Some(asked) = flag(&flags, "stage").filter(|s| !s.is_empty()) {
        let Some(target) = resolve_stage(wf, asked) else {
            bail!("Unknown stage: {asked}");
        };
        if plan.action(&target.slug).as_deref() == Some("SKIP") {
            if !allow_skipped {
                bail!(
                    "Stage \"{}\" is skipped for scope \"{}\". Use `next --stage {}`, which handles a stage the plan skips.",
                    target.slug,
                    plan.scope,
                    target.slug
                );
            }
            target_skipped = true;
        }
        target
    } else if let Some(asked) = flag(&flags, "phase").filter(|p| !p.is_empty()) {
        let lower = asked.to_lowercase();
        let canonical = PHASE_NUMBERS
            .iter()
            .find(|(number, _)| *number == lower)
            .map(|(_, phase)| *phase)
            .or_else(|| PHASES.iter().copied().find(|phase| *phase == lower));
        let Some(canonical) = canonical else {
            bail!("Unknown phase: {asked}");
        };
        let found = wf
            .install
            .graph
            .iter()
            .find(|stage| stage.phase == canonical && plan.executes(&stage.slug))
            .or_else(|| first_in_scope_stage_of_phase(wf, canonical, &plan.scope));
        let Some(found) = found else {
            bail!(
                "Phase \"{canonical}\" has no executable stages for scope \"{}\".",
                plan.scope
            );
        };
        found
    } else {
        bail!(
            "Usage: resolve --stage <slug|#> [--allow-skipped] or --phase <name|#> [--scope <scope>]"
        );
    };
    let current_idx = stage_index(wf, &current.slug).unwrap_or(0);
    let target_idx = stage_index(wf, &target.slug).unwrap_or(0);
    let direction = direction_of(current_idx, target_idx);
    let graph = &wf.install.graph;
    let mut affected: Vec<String> = Vec::new();
    match direction {
        Direction::Forward => {
            for stage in &graph[current_idx + 1..target_idx] {
                if plan.executes(&stage.slug) {
                    affected.push(stage.slug.clone());
                }
            }
        }
        Direction::Backward => {
            for stage in &graph[target_idx..] {
                if plan.executes(&stage.slug) {
                    affected.push(stage.slug.clone());
                }
            }
        }
        Direction::Redo => {}
    }
    let mut out = Map::new();
    out.insert("target_slug".to_owned(), json!(target.slug));
    out.insert(
        "target_phase".to_owned(),
        json!(target.phase.to_uppercase()),
    );
    out.insert("target_number".to_owned(), json!(target.number));
    out.insert("target_name".to_owned(), json!(stage::name(target)));
    out.insert("current_slug".to_owned(), json!(current.slug));
    out.insert("current_number".to_owned(), json!(current.number));
    out.insert("direction".to_owned(), json!(direction.word()));
    out.insert("affected_stages".to_owned(), json!(affected));
    if target_skipped {
        out.insert("target_skipped".to_owned(), json!(true));
    }
    out.insert("valid".to_owned(), json!(true));
    Ok(Value::Object(out))
}

fn direction_of(current_idx: usize, target_idx: usize) -> Direction {
    if target_idx > current_idx {
        Direction::Forward
    } else if target_idx < current_idx {
        Direction::Backward
    } else {
        Direction::Redo
    }
}

/// `firstInScopeStageOfPhase`: the first stage of the phase in the scope's
/// own grid column.
fn first_in_scope_stage_of_phase<'a>(
    wf: &'a Workflow,
    phase: &str,
    scope: &str,
) -> Option<&'a Stage> {
    let plan = stages_in_scope(&wf.install.grid, scope, &wf.install.graph);
    let lower = phase.to_lowercase();
    wf.install.graph.iter().find(|stage| {
        stage.phase == lower
            && plan
                .iter()
                .any(|(slug, action)| *slug == stage.slug && *action == "EXECUTE")
    })
}

/// What `movedState` leaves.
struct Moved {
    content: String,
    stages_skipped: Vec<String>,
    stages_reset: Vec<String>,
    crosses_phase_boundary: bool,
    completed_count: usize,
    timestamp: String,
}

/// `movedState`: the stages a jump skips (forward) or resets (backward),
/// the target in progress, and the cursor fields on it.
fn moved_state(
    wf: &Workflow,
    content: &str,
    target: &Stage,
    direction: Direction,
    plan: &Plan<'_>,
) -> Result<Moved> {
    let graph = &wf.install.graph;
    let target_idx = stage_index(wf, &target.slug).unwrap_or(0);
    let boxes = parse_checkboxes(content);
    let state_of = |slug: &str| {
        boxes
            .iter()
            .find(|line| line.slug == slug)
            .map(|line| line.state)
    };
    let mut content = content.to_owned();
    let mut stages_skipped = Vec::new();
    let mut stages_reset = Vec::new();
    let current_slug = get_field(&content, "Current Stage")
        .filter(|slug| !slug.is_empty())
        .unwrap_or("state-init")
        .to_owned();
    const IN_FLIGHT: [CheckboxState; 4] = [
        CheckboxState::Pending,
        CheckboxState::InProgress,
        CheckboxState::AwaitingApproval,
        CheckboxState::Revising,
    ];
    match direction {
        Direction::Forward => {
            let current_idx = stage_index(wf, &current_slug).map_or(0, |idx| idx + 1);
            for stage in graph
                .iter()
                .skip(current_idx)
                .take(target_idx.saturating_sub(current_idx))
            {
                if !plan.executes(&stage.slug) {
                    continue;
                }
                if state_of(&stage.slug).is_some_and(|state| IN_FLIGHT.contains(&state)) {
                    content = set_checkbox(&content, &stage.slug, CheckboxState::Skipped);
                    stages_skipped.push(stage.slug.clone());
                }
            }
            if current_slug != target.slug
                && let Some(state) = state_of(&current_slug)
                && state != CheckboxState::Pending
                && IN_FLIGHT.contains(&state)
            {
                content = set_checkbox(&content, &current_slug, CheckboxState::Skipped);
                stages_skipped.push(current_slug.clone());
            }
        }
        Direction::Backward => {
            const RESETTABLE: [CheckboxState; 5] = [
                CheckboxState::Completed,
                CheckboxState::InProgress,
                CheckboxState::AwaitingApproval,
                CheckboxState::Revising,
                CheckboxState::Skipped,
            ];
            for stage in &graph[target_idx..] {
                if !plan.executes(&stage.slug) {
                    continue;
                }
                if state_of(&stage.slug).is_some_and(|state| RESETTABLE.contains(&state)) {
                    content = set_checkbox(&content, &stage.slug, CheckboxState::Pending);
                    stages_reset.push(stage.slug.clone());
                }
            }
        }
        Direction::Redo => {
            content = set_checkbox(&content, &target.slug, CheckboxState::Pending);
            stages_reset.push(target.slug.clone());
        }
    }
    content = set_checkbox(&content, &target.slug, CheckboxState::InProgress);
    let current_stage = wf.install.stage(&current_slug);
    let crosses_phase_boundary = current_stage.is_some_and(|current| current.phase != target.phase);
    let next_after = next_in_scope_stage(&target.slug, plan.definition, graph, Some(&content));
    let timestamp = iso_timestamp(SystemTime::now());
    content = set_field(&content, "Lifecycle Phase", &target.phase.to_uppercase());
    content = set_field(&content, "Current Stage", &target.slug);
    for field in [
        "Active Unit",
        "Unit Stage",
        "Unit State",
        "Unit Pause Reason",
        "Unit Next Action",
    ] {
        content = remove_field(&content, field);
    }
    content = set_field(
        &content,
        "Next Stage",
        next_after.map_or("none", |stage| stage.slug.as_str()),
    );
    content = set_field(&content, "Active Agent", stage::lead_agent(target));
    content = set_field(&content, "Status", "Running");
    content = set_field(&content, "Last Updated", &timestamp);
    content = set_field(&content, "In Progress", &target.slug);
    content = set_field(
        &content,
        "Next Action",
        &format!("Execute {}", stage::name(target)),
    );
    let completed_count = count_checkboxes(&content, CheckboxState::Completed);
    content = set_field(&content, "Completed", &completed_count.to_string());
    if crosses_phase_boundary && let Some(current) = current_stage {
        let phase_idx = |phase: &str| PHASES.iter().position(|known| *known == phase);
        if direction == Direction::Forward {
            content = set_phase_progress(&content, &current.phase, "Verified");
            if let (Some(from), Some(to)) = (phase_idx(&current.phase), phase_idx(&target.phase)) {
                for phase in PHASES.iter().take(to).skip(from + 1) {
                    content = set_phase_progress(&content, phase, "Skipped");
                }
            }
        } else if let Some(to) = phase_idx(&target.phase) {
            for phase in PHASES.iter().skip(to + 1) {
                let has_execute = graph
                    .iter()
                    .any(|stage| stage.phase == *phase && plan.executes(&stage.slug));
                if has_execute {
                    content = set_phase_progress(&content, phase, "Pending");
                }
            }
        }
        content = set_phase_progress(&content, &target.phase, "Active");
    }
    let after = parse_checkboxes(&content);
    let mut last_completed = "state-init".to_owned();
    for stage in graph[..target_idx].iter().rev() {
        if after
            .iter()
            .any(|line| line.slug == stage.slug && line.state == CheckboxState::Completed)
        {
            last_completed = stage.slug.clone();
            break;
        }
    }
    content = set_field(&content, "Last Completed Stage", &last_completed);
    Ok(Moved {
        content,
        stages_skipped,
        stages_reset,
        crosses_phase_boundary,
        completed_count,
        timestamp,
    })
}

fn sorted_unique(mut paths: Vec<String>) -> Vec<String> {
    paths.sort_by(|a, b| cmp_utf16(a, b));
    paths.dedup();
    paths
}

/// `stageArtifactPaths`.
fn stage_artifact_paths(wf: &Workflow, stage: &Stage, content: &str) -> Vec<String> {
    let entries = review_artifact_entries(wf, stage, Some(content)).unwrap_or_default();
    sorted_unique(
        entries
            .iter()
            .map(|entry| workspace_artifact_path(&wf.layout.project, entry))
            .collect(),
    )
}

/// `existingStageArtifactPaths`.
fn existing_stage_artifact_paths(wf: &Workflow, stage: &Stage, content: &str) -> Vec<String> {
    stage_artifact_paths(wf, stage, content)
        .into_iter()
        .filter(|path| resolve_project_path(&wf.layout.project, path).exists())
        .collect()
}

fn resolve_project_path(project: &Path, path: &str) -> std::path::PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        project.join(candidate)
    }
}

/// `stageReviewPaths`: the reviews a backward jump invalidates, named
/// `<artifact>#Review`. The record-backed reviews are the review brief's
/// (M5): with a reviewer and review rows on the ledger, the jump is refused
/// by name; what remains is the embedded `## Review` section of a declared
/// artifact, which the brief seeds from too.
fn stage_review_paths(wf: &Workflow, stage: &Stage, content: &str) -> Result<Vec<String>> {
    if stage::reviewer(stage).is_some()
        && wf
            .events()
            .iter()
            .any(|row| row.event.starts_with("REVIEW_"))
    {
        bail!(
            "A backward jump names the reviews it invalidates, and \"{}\" has review rows on the ledger that the review brief would read: Focus does not read review records yet.",
            stage.slug
        );
    }
    let mut paths = Vec::new();
    for path in existing_stage_artifact_paths(wf, stage, content) {
        let full = resolve_project_path(&wf.layout.project, &path);
        let has_section = std::fs::read_to_string(&full).ok().is_some_and(|text| {
            focus_markdown::visible_lines(&text, focus_markdown::Visible::default())
                .map(|lines| {
                    !focus_graph::guard_memory::section_body(&lines, "## Review").is_empty()
                })
                .unwrap_or(false)
        });
        if has_section {
            paths.push(format!("{path}#Review"));
        }
    }
    Ok(sorted_unique(paths))
}

fn list_names(names: &[String]) -> String {
    match names.len() {
        0 => String::new(),
        1 => names[0].clone(),
        n => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
    }
}

/// `forwardJumpNotice`.
fn forward_jump_notice(
    wf: &Workflow,
    target: &Stage,
    skipped: &[&Stage],
    came_from: &str,
) -> String {
    let names: Vec<String> = skipped
        .iter()
        .filter_map(|node| stage_label(Some(node), &node.slug))
        .collect();
    let list = list_names(&names);
    let from = stage_label(wf.install.stage(came_from), came_from)
        .unwrap_or_else(|| "where you were".to_owned());
    format!(
        "Moved to {}{}. You can go back to {from} any time.",
        stage_label(Some(target), &target.slug).unwrap_or_else(|| "that stage".to_owned()),
        if list.is_empty() {
            String::new()
        } else {
            format!("; skipped {list}")
        }
    )
}

/// `backwardJumpNotice`.
fn backward_jump_notice(target: &Stage, from: &Stage) -> String {
    format!(
        "Moved back to {}. You can return to {} any time.",
        stage_label(Some(target), &target.slug).unwrap_or_else(|| "that stage".to_owned()),
        stage_label(Some(from), &from.slug).unwrap_or_else(|| "where you were".to_owned())
    )
}

fn refuse_directive_marker(wf: &Workflow) -> Result<()> {
    let marker = wf
        .selection
        .engine_dir(&wf.layout)
        .join("active-directive.json");
    if marker.exists() {
        bail!(
            "{} is the orchestrator's directive marker, which a jump reads for the step the person was on: Focus does not read it yet.",
            marker.display()
        );
    }
    Ok(())
}

/// `handleExecute`.
///
/// # Errors
///
/// What upstream refuses, in its words; what Focus refuses by name.
pub fn execute(wf: &Workflow, args: &[String]) -> Result<Value> {
    let flags = parse_flags(args);
    let held = wf.lock("jumping between stages")?;
    let content = wf.read_state()?;
    let Some(target_slug) = flag(&flags, "target").filter(|slug| !slug.is_empty()) else {
        bail!(
            "Usage: execute --target <slug> --direction <forward|backward|redo> [--units <unit[,unit...]> [--stages <slug[,slug...]>]] [--scope <scope>]"
        );
    };
    let Some(direction) = flag(&flags, "direction").and_then(Direction::parse) else {
        bail!(
            "Invalid direction: {}. Valid: forward, backward, redo",
            flag(&flags, "direction").unwrap_or("undefined")
        );
    };
    let plan = plan(wf, &content, flag(&flags, "scope"))?;
    let target = wf.stage(target_slug)?;
    if plan.action(target_slug).as_deref() == Some("SKIP") {
        bail!(
            "Stage \"{target_slug}\" is skipped for scope \"{}\". Use `next --stage {target_slug}`, which handles a stage the plan skips.",
            plan.scope
        );
    }
    refuse_units_flag(&flags)?;
    refuse_directive_marker(wf)?;
    let graph = &wf.install.graph;
    let current_slug = get_field(&content, "Current Stage")
        .filter(|slug| !slug.is_empty())
        .unwrap_or("state-init")
        .to_owned();
    // Where the person was: the active unit's own step, else the current stage.
    let unit_stage = get_field(&content, "Unit Stage").map(trim).unwrap_or("");
    let came_from = if graph.iter().any(|node| node.slug == unit_stage) {
        unit_stage.to_owned()
    } else {
        current_slug.clone()
    };
    let backward = direction == Direction::Backward;
    let moved = moved_state(wf, &content, target, direction, &plan)?;
    let changed_upstream = if backward {
        stage_artifact_paths(wf, target, &content)
    } else {
        Vec::new()
    };
    let downstream: Vec<&Stage> = if backward {
        moved
            .stages_reset
            .iter()
            .filter(|slug| *slug != target_slug)
            .filter_map(|slug| wf.install.stage(slug))
            .collect()
    } else {
        Vec::new()
    };
    let invalidated_artifacts = sorted_unique(
        downstream
            .iter()
            .flat_map(|stage| existing_stage_artifact_paths(wf, stage, &content))
            .collect(),
    );
    let mut invalidated_reviews = Vec::new();
    for stage in &downstream {
        invalidated_reviews.extend(stage_review_paths(wf, stage, &content)?);
    }
    let invalidated_reviews = sorted_unique(invalidated_reviews);
    // A backward jump's way back rides the chat when one is bound: the
    // pending person lines are the human-turn hook's record (M4).
    let session = wf
        .selection
        .session_id
        .as_deref()
        .or(wf.invoker.session_override.as_deref());
    if backward && session.is_some() {
        bail!(
            "A backward jump keeps its way back for the chat's next turn (the pending person lines of session {}), which is the human-turn hook's record: Focus does not write it yet.",
            session.unwrap_or_default()
        );
    }
    let word = direction.word();
    for skipped in &moved.stages_skipped {
        wf.emit(
            &held,
            "STAGE_SKIPPED",
            &[
                ("Stage", skipped.clone()),
                (
                    "Reason",
                    format!("Skipped by jump to {target_slug} ({word})"),
                ),
                ("Skip Kind", "jump".to_owned()),
            ],
        )?;
    }
    let current_stage = wf.install.stage(&current_slug);
    if moved.crosses_phase_boundary
        && let Some(current) = current_stage
    {
        wf.emit(
            &held,
            "PHASE_COMPLETED",
            &[
                ("From phase", current.phase.clone()),
                ("To phase", target.phase.clone()),
                ("Stages completed", moved.completed_count.to_string()),
                ("Details", format!("Phase boundary crossed via {word} jump")),
            ],
        )?;
        wf.emit(
            &held,
            "PHASE_VERIFIED",
            &[
                (
                    "Phase boundary",
                    format!("{} \u{2192} {}", current.phase, target.phase),
                ),
                ("Details", "Traceability verification on jump".to_owned()),
            ],
        )?;
        wf.emit(
            &held,
            "PHASE_STARTED",
            &[
                ("Phase", target.phase.clone()),
                ("Scope", plan.scope.clone()),
            ],
        )?;
    }
    let mut jumped = vec![
        ("Direction", word.to_uppercase()),
        ("Source", current_slug.clone()),
        ("Target", target_slug.to_owned()),
        ("Scope", plan.scope.clone()),
        (
            "Details",
            format!(
                "{} jump from {current_slug} to {target_slug} ({}). Scope: {}.",
                word.to_uppercase(),
                target.number,
                plan.scope
            ),
        ),
    ];
    if backward {
        jumped.push((
            "Changed Upstream Artifacts",
            json::stringify(&json!(changed_upstream)),
        ));
        jumped.push((
            "Invalidated Downstream Artifacts",
            json::stringify(&json!(invalidated_artifacts)),
        ));
        jumped.push((
            "Invalidated Downstream Reviews",
            json::stringify(&json!(invalidated_reviews)),
        ));
    }
    wf.emit(&held, "STAGE_JUMPED", &jumped)?;
    let mut started = vec![
        ("Stage", target_slug.to_owned()),
        ("Agent", stage::lead_agent(target).to_owned()),
    ];
    started.extend(answer_mode_fields(&wf.events()));
    wf.emit(&held, "STAGE_STARTED", &started)?;
    wf.write_state(&moved.content)?;
    let notice = match direction {
        Direction::Forward => {
            let skipped: Vec<&Stage> = graph
                .iter()
                .filter(|node| moved.stages_skipped.contains(&node.slug))
                .collect();
            Some(forward_jump_notice(wf, target, &skipped, &came_from))
        }
        Direction::Backward => wf
            .install
            .stage(&came_from)
            .map(|from| backward_jump_notice(target, from)),
        Direction::Redo => None,
    };
    let mut out = Map::new();
    out.insert("direction".to_owned(), json!(word));
    out.insert("target".to_owned(), json!(target_slug));
    out.insert(
        "target_phase".to_owned(),
        json!(target.phase.to_uppercase()),
    );
    if let Some(notice) = notice {
        out.insert("notice".to_owned(), json!(notice));
    }
    out.insert("stages_skipped".to_owned(), json!(moved.stages_skipped));
    out.insert("stages_reset".to_owned(), json!(moved.stages_reset));
    out.insert("state_updated".to_owned(), json!(true));
    out.insert("audit_appended".to_owned(), json!(true));
    out.insert("completed_count".to_owned(), json!(moved.completed_count));
    out.insert("workflow_stopped".to_owned(), json!(false));
    out.insert("timestamp".to_owned(), json!(moved.timestamp));
    Ok(Value::Object(out))
}

/// `handleReopen`: a per-unit stage reopened for named units (M4).
///
/// # Errors
///
/// Always: the units' lifecycle is not ported.
pub fn reopen(wf: &Workflow, args: &[String]) -> Result<Value> {
    let flags = parse_flags(args);
    let _ = wf.read_state()?;
    let target = flag(&flags, "target").filter(|slug| !slug.is_empty());
    let units: Vec<&str> = flag(&flags, "units")
        .unwrap_or("")
        .split(',')
        .map(trim)
        .filter(|unit| !unit.is_empty())
        .collect();
    if target.is_none() || units.is_empty() {
        bail!(
            "Usage: reopen --target <slug> [--stages <slug[,slug...]>] --units <unit[,unit...]> [--via redo|change] [--scope <scope>]"
        );
    }
    bail!(
        "reopen starts a per-unit stage again for the named units ({}), which needs the units' lifecycle: Focus does not run units yet.",
        units.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_read_as_pairs() {
        let args: Vec<String> = ["--stage", "a", "--allow", "--phase", "x", "--last"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let flags = parse_flags(&args);
        assert_eq!(flag(&flags, "stage"), Some("a"));
        assert_eq!(flag(&flags, "allow"), Some("--phase"));
        assert_eq!(flag(&flags, "phase"), None);
        assert_eq!(flag(&flags, "last"), None);
        // The last value of a repeated flag wins.
        let twice: Vec<String> = ["--stage", "a", "--stage", "b"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(flag(&parse_flags(&twice), "stage"), Some("b"));
    }

    #[test]
    fn names_are_listed_as_a_sentence() {
        let names = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(list_names(&names(&[])), "");
        assert_eq!(list_names(&names(&["a"])), "a");
        assert_eq!(list_names(&names(&["a", "b"])), "a and b");
        assert_eq!(list_names(&names(&["a", "b", "c"])), "a, b and c");
    }

    #[test]
    fn a_direction_is_told_from_the_indexes() {
        assert_eq!(direction_of(1, 3), Direction::Forward);
        assert_eq!(direction_of(3, 1), Direction::Backward);
        assert_eq!(direction_of(2, 2), Direction::Redo);
        assert_eq!(Direction::parse("sideways"), None);
    }
}

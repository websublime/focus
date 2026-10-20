//! `focus engine utility status`: the workflow as a person reads it. The
//! project and its scope or plan, the project type and depth, the phase
//! and stage, who is on it, the guard policy and the checks switched off,
//! the ceremonies, the completion count and the phase bars, a finished
//! stage that is behind (the validity receipts against the tree), the last
//! completed and next stages, and the other work open in the space.
//!
//! The unit a team works on, the step the engine last handed out (the
//! directive marker) and the team board are the units' lifecycle and the
//! orchestrator's (M4/M5): with Unit Ownership team, or a marker on the
//! record, the status is refused by name.
//!
//! Mirrors `handleStatus`, `pendingOrganicGate`, `pendingDuration`,
//! `agentDisplayName`, `plainUtc`, `codeScannedClause`, `runningWorkflows`
//! and `declaredProjectType` in `aidlc-utility.ts` and `aidlc-lib.ts`, and
//! `unitProgress` in `aidlc-unit-walk-view.ts`.

use std::path::Path;
use std::time::SystemTime;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_graph::stage_schema::PHASES;
use focus_guards::workflow::Guards;
use focus_intent::plan::stages_in_scope;
use focus_js::date::parse_instant;
use focus_js::order::{cmp_utf16, locale_compare};
use focus_js::text::trim;
use focus_js::{Result, bail, json, re};
use focus_state::fields::get_field;
use focus_state::progress::{CheckboxState, parse_checkboxes, parse_stage_suffixes};
use focus_workspace::names::STATE_FILE;
use focus_workspace::records::{list_intent_dirs, read_intent_registry, row_names_record};
use serde_json::Value;

use crate::ceremonies;
use crate::stage::{self, stage_label};
use crate::validity;
use crate::workflow::Workflow;

/// `pendingOrganicGate`: when the stage's gate was opened and not yet
/// answered, read across shards with same-second ties failing closed.
#[must_use]
pub fn pending_organic_gate(events: &[ShardEvent], stage: &str) -> Option<(String, i64)> {
    const RELEVANT: [&str; 7] = [
        "WORKFLOW_STARTED",
        "STAGE_JUMPED",
        "STAGE_STARTED",
        "STAGE_AWAITING_APPROVAL",
        "GATE_APPROVED",
        "GATE_REJECTED",
        "STAGE_SKIPPED",
    ];
    let mut rows: Vec<&ShardEvent> = events
        .iter()
        .filter(|event| RELEVANT.contains(&event.event.as_str()))
        .collect();
    rows.sort_by(|a, b| {
        cmp_utf16(&a.timestamp, &b.timestamp)
            .then(a.shard_index.cmp(&b.shard_index))
            .then(a.pos.cmp(&b.pos))
    });
    let mut pending: Option<(String, i64)> = None;
    let mut start = 0;
    while start < rows.len() {
        let mut end = start + 1;
        while end < rows.len() && rows[end].timestamp == rows[start].timestamp {
            end += 1;
        }
        // Only same-shard append order is causal: the last effective row in
        // each shard can be globally last, so disagreeing effects fail closed.
        let mut effects: Vec<(String, &'static str)> = Vec::new();
        for event in &rows[start..end] {
            let event_stage = block_field(&event.block, "Stage");
            let boundary = event.event == "WORKFLOW_STARTED"
                || event.event == "STAGE_JUMPED"
                || (event.event == "STAGE_STARTED"
                    && event_stage == Some(stage)
                    && !block_field(&event.block, "Workflow")
                        .is_some_and(|workflow| workflow.starts_with("single-stage:")));
            let effect = if boundary {
                "clear"
            } else if event_stage != Some(stage) {
                continue;
            } else if event.event == "STAGE_AWAITING_APPROVAL" {
                if block_field(&event.block, "Recovered") == Some("true")
                    || block_field(&event.block, "Revalidated") == Some("true")
                {
                    continue;
                }
                "open"
            } else {
                "clear"
            };
            match effects.iter_mut().find(|(shard, _)| *shard == event.shard) {
                Some(slot) => slot.1 = effect,
                None => effects.push((event.shard.clone(), effect)),
            }
        }
        let opens = effects.iter().any(|(_, effect)| *effect == "open");
        let clears = effects.iter().any(|(_, effect)| *effect == "clear");
        if clears {
            pending = None;
        } else if opens {
            let timestamp = rows[start].timestamp.clone();
            pending = parse_instant(&timestamp).map(|ms| (timestamp, ms));
        }
        start = end;
    }
    pending
}

/// `pendingDuration`.
#[must_use]
pub fn pending_duration(age_ms: i64) -> String {
    let words =
        |count: i64, unit: &str| format!("{count} {unit}{}", if count == 1 { "" } else { "s" });
    let minutes = age_ms.max(0) / (60 * 1000);
    if minutes < 60 {
        return words(minutes, "minute");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return words(hours, "hour");
    }
    words(hours / 24, "day")
}

/// `SHIPPED_AGENT_NAMES`: the shipped personas' names; any other slug is "a
/// custom agent", and a stored value that is no slug is not shown.
const SHIPPED_AGENT_NAMES: [(&str, &str); 14] = [
    ("focus-architect-agent", "Architect Agent"),
    ("focus-architecture-reviewer-agent", "Architecture Reviewer"),
    ("focus-aws-platform-agent", "AWS Platform Agent"),
    ("focus-compliance-agent", "Compliance Agent"),
    ("focus-composer-agent", "Composer Agent"),
    ("focus-delivery-agent", "Delivery Agent"),
    ("focus-design-agent", "Design Agent"),
    ("focus-developer-agent", "Developer Agent"),
    ("focus-devsecops-agent", "DevSecOps Agent"),
    ("focus-operations-agent", "Operations Agent"),
    ("focus-pipeline-deploy-agent", "Pipeline & Deploy Agent"),
    ("focus-product-agent", "Product Agent"),
    ("focus-product-lead-agent", "Product Lead"),
    ("focus-quality-agent", "Quality Agent"),
];

/// `agentDisplayName`.
#[must_use]
pub fn agent_display_name(slug: &str) -> Option<String> {
    if let Some((_, name)) = SHIPPED_AGENT_NAMES.iter().find(|(known, _)| *known == slug) {
        return Some((*name).to_owned());
    }
    re::test(r"^[a-z0-9][a-z0-9-]{0,79}$", "", slug).then(|| "a custom agent".to_owned())
}

/// `plainUtc`: the date and the minute, in UTC.
#[must_use]
pub fn plain_utc(timestamp: &str) -> String {
    if re::test(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}", "", timestamp) {
        format!("{} {} UTC", &timestamp[..10], &timestamp[11..16])
    } else {
        timestamp.to_owned()
    }
}

/// `codeScannedClause`: when the existing code was last scanned.
fn code_scanned_clause(wf: &Workflow) -> String {
    let mut unreadable = Vec::new();
    let Ok(events) = wf
        .ledger()
        .read_events(&wf.selector(), &wf.invoker, Some(&mut unreadable))
    else {
        return String::new();
    };
    if !unreadable.is_empty() {
        return String::new();
    }
    let mut stamps: Vec<&str> = events
        .iter()
        .filter(|row| {
            row.event == "STAGE_COMPLETED"
                && block_field(&row.block, "Stage") == Some("reverse-engineering")
        })
        .map(|row| row.timestamp.as_str())
        .filter(|stamp| re::test(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}", "", stamp))
        .collect();
    stamps.sort_by(|a, b| cmp_utf16(a, b));
    stamps
        .last()
        .map(|last| format!(", scanned {}", plain_utc(last)))
        .unwrap_or_default()
}

/// `declaredProjectType`.
#[must_use]
pub fn declared_project_type(value: Option<&str>) -> Option<&'static str> {
    match trim(value?).to_lowercase().as_str() {
        "greenfield" => Some("Greenfield"),
        "brownfield" => Some("Brownfield"),
        _ => None,
    }
}

// --- The unit walk, as a person reads it -------------------------------------

/// `plannedUnits`: the names in the unit DAG's edge block, else in the
/// compiled runtime graph; none when neither reads.
fn planned_units(record: &Path) -> Vec<String> {
    let dag = record
        .join("inception")
        .join("units-generation")
        .join("unit-of-work-dependency.md");
    if let Ok(text) = std::fs::read_to_string(&dag) {
        let block = re::find(
            r"## Machine-Readable Edge Block[\s\S]*?```ya?ml\r?\n([\s\S]*?)```",
            "",
            &text,
        )
        .and_then(|found| re::group(&text, &found, 1).map(str::to_owned))
        .unwrap_or_default();
        let pattern = re::compile(
            r"^\s*-\s+name:\s*([A-Za-z0-9][A-Za-z0-9._-]{0,63})\s*$",
            "gm",
        );
        return pattern
            .find_iter(&block)
            .filter_map(|found| re::group(&block, &found, 1).map(str::to_owned))
            .collect();
    }
    let Ok(text) = std::fs::read_to_string(record.join("runtime-graph.json")) else {
        return Vec::new();
    };
    let Ok(graph) = json::parse(&text) else {
        return Vec::new();
    };
    graph
        .get("bolt_dag")
        .and_then(|dag| dag.get("units"))
        .and_then(Value::as_array)
        .map(|units| {
            units
                .iter()
                .filter_map(|unit| unit.get("name").and_then(Value::as_str))
                .filter(|name| re::test(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$", "", name))
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `unitProgress`: working one unit at a time, the unit the person is on of
/// those planned; `None` when the walk is not unit-by-unit, the DAG cannot
/// be read, or every unit is done.
#[must_use]
pub fn unit_progress(record: &Path, construction_iteration: &str) -> Option<(usize, usize)> {
    if trim(construction_iteration) != "unit-major" {
        return None;
    }
    let units = planned_units(record);
    if units.is_empty() {
        return None;
    }
    let audit_dir = record.join("audit");
    let mut names: Vec<String> = std::fs::read_dir(&audit_dir)
        .ok()?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".md"))
        .collect();
    names.sort_by(|a, b| cmp_utf16(a, b));
    let mut rows: Vec<(String, bool, String)> = Vec::new();
    for name in names {
        let text = std::fs::read_to_string(audit_dir.join(&name)).ok()?;
        for entry in text.split("\n## ") {
            if !re::test(r"^\*\*Checkpoint\*\*: construction-unit$", "m", entry) {
                continue;
            }
            let event = re::find(r"^\*\*Event\*\*:\s*(\S+)", "m", entry)
                .and_then(|found| re::group(entry, &found, 1))
                .unwrap_or("");
            if event != "GATE_APPROVED" && event != "GATE_REJECTED" {
                continue;
            }
            let unit = re::find(r"^\*\*Unit\*\*:\s*(.+)$", "m", entry)
                .and_then(|found| re::group(entry, &found, 1))
                .map(trim)
                .unwrap_or("")
                .to_owned();
            let at = re::find(r"^\*\*Timestamp\*\*:\s*(.+)$", "m", entry)
                .and_then(|found| re::group(entry, &found, 1))
                .map(trim)
                .unwrap_or("")
                .to_owned();
            rows.push((at, event == "GATE_APPROVED", unit));
        }
    }
    rows.sort_by(|a, b| locale_compare(&a.0, &b.0));
    let mut done: Vec<String> = Vec::new();
    for (_, approved, unit) in rows {
        if !units.contains(&unit) {
            continue;
        }
        if approved {
            if !done.contains(&unit) {
                done.push(unit);
            }
        } else {
            done.retain(|known| *known != unit);
        }
    }
    if done.len() >= units.len() {
        return None;
    }
    Some((done.len() + 1, units.len()))
}

// --- The other work open in the space ----------------------------------------

/// `runningWorkflows`, as `(space, dirName)`: every space's intents that
/// neither the registry nor the state file marks completed or archived.
fn running_workflows(wf: &Workflow) -> Vec<(String, String)> {
    let spaces_root = wf.layout.workspace_root().join("spaces");
    let mut spaces: Vec<String> = vec!["default".to_owned()];
    if let Ok(entries) = std::fs::read_dir(&spaces_root) {
        for entry in entries.filter_map(std::result::Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if spaces_root.join(&name).is_dir() && !spaces.contains(&name) {
                spaces.push(name);
            }
        }
    }
    spaces.sort_by(|a, b| cmp_utf16(a, b));
    let mut running = Vec::new();
    for space in spaces {
        let registry = read_intent_registry(&wf.layout, &space);
        let dirs = list_intent_dirs(&wf.layout, &space);
        let mut claimed: Vec<String> = Vec::new();
        let mut candidates: Vec<(Option<String>, Option<String>)> = Vec::new();
        for row in &registry {
            let dir = dirs.iter().find(|dir| row_names_record(row, dir)).cloned();
            if let Some(dir) = &dir {
                claimed.push(dir.clone());
            }
            let status = row
                .get("status")
                .and_then(Value::as_str)
                .map(|status| trim(status).to_lowercase());
            candidates.push((dir, status));
        }
        for dir in &dirs {
            if !claimed.contains(dir) {
                candidates.push((Some(dir.clone()), Some("unknown".to_owned())));
            }
        }
        for (dir, status) in candidates {
            let Some(dir) = dir else {
                continue;
            };
            if matches!(status.as_deref(), Some("complete" | "archived")) {
                continue;
            }
            let path = wf.layout.intents_dir(&space).join(&dir).join(STATE_FILE);
            if path.is_file()
                && let Ok(text) = std::fs::read_to_string(&path)
                && matches!(get_field(&text, "Status"), Some("Completed" | "Archived"))
            {
                continue;
            }
            running.push((space.clone(), dir));
        }
    }
    running
}

fn is_safe_record_name(value: &str) -> bool {
    re::test(r"^[A-Za-z0-9][A-Za-z0-9._-]*$", "", value) && value != "." && value != ".."
}

/// `stageNames`.
fn stage_names(wf: &Workflow, slugs: &[String]) -> String {
    slugs
        .iter()
        .filter_map(|slug| stage_label(wf.install.stage(slug), slug))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The text `handleStatus` prints for a workflow with a state file.
///
/// # Errors
///
/// The state file cannot be read; a team's units or the directive marker
/// (refused by name).
pub fn render(wf: &Workflow, now: SystemTime, by_flags: bool) -> Result<String> {
    let path = wf.state_path();
    let content = wf.read_state()?;
    let graph = &wf.install.graph;
    // `getField(content, name) || "Unknown"`: an empty value is unknown too.
    let field = |name: &str| {
        get_field(&content, name)
            .filter(|value| !value.is_empty())
            .unwrap_or("Unknown")
            .to_owned()
    };
    let project = field("Project");
    let scope = field("Scope");
    let phase = field("Lifecycle Phase");
    let current_stage = field("Current Stage");
    let status = field("Status");
    let agent_slug = get_field(&content, "Active Agent").map(trim).unwrap_or("");
    let agent_name =
        if agent_slug.is_empty() || agent_slug == "None" || agent_slug == "orchestrator" {
            None
        } else {
            agent_display_name(agent_slug)
        };
    let last_stage = wf.install.stage(
        get_field(&content, "Last Completed Stage")
            .map(trim)
            .unwrap_or(""),
    );
    let next_node = wf
        .install
        .stage(get_field(&content, "Next Stage").map(trim).unwrap_or(""));
    let last_name = last_stage
        .filter(|stage| stage.phase != "initialization")
        .and_then(|stage| stage_label(Some(stage), &stage.slug));
    let next_name = next_node.and_then(|stage| stage_label(Some(stage), &stage.slug));
    let agent_line = agent_name.map_or(String::new(), |name| format!("Active Agent:   {name}\n"));
    let last_line = last_name.map_or(String::new(), |name| format!("Last Completed: {name}\n"));
    let next_line = next_name.map_or(String::new(), |name| format!("Next Stage:     {name}\n"));
    let plan = get_field(&content, "Plan");
    // A plan composed for this work runs on a stock scope the person never
    // chose, so a setting that came from that scope reads as the plan's.
    let shown_source = |source: &str| -> String {
        if plan.is_some() && source == format!("scope {}", trim(&scope).to_lowercase()) {
            "the approved plan".to_owned()
        } else {
            source.to_owned()
        }
    };
    let mut fences_off_line = String::new();
    let guard_policy_display = match Guards::read(&wf.layout, &wf.selector(), &wf.invoker)
        .map(|guards| guards.with_state(&content))
        .and_then(|guards| guards.policy(false).map(|policy| (guards, policy)))
    {
        Ok((guards, policy)) => {
            let display = focus_guards::policy::format_guard_policy(
                policy.value,
                &shown_source(&policy.source),
            );
            let mut off_by_source: Vec<(String, Vec<String>)> = Vec::new();
            for fence in guards.fences(&policy) {
                if fence.value != focus_guards::fences::Setting::Off
                    || fence.source.starts_with("guard policy ")
                {
                    continue;
                }
                let source = fence.source_label();
                let name = if fence.fence == focus_guards::fences::Fence::PlanApproval {
                    "plan re-approval".to_owned()
                } else {
                    fence.fence.name().to_owned()
                };
                match off_by_source.iter_mut().find(|(known, _)| *known == source) {
                    Some(slot) => slot.1.push(name),
                    None => off_by_source.push((source, vec![name])),
                }
            }
            if !off_by_source.is_empty() {
                fences_off_line = format!(
                    "Checks off:     {}\n",
                    off_by_source
                        .iter()
                        .map(|(source, names)| format!("{} ({source})", names.join(", ")))
                        .collect::<Vec<_>>()
                        .join("; ")
                );
            }
            display
        }
        Err(err) => format!("unavailable ({})", err.message()),
    };
    let ceremony_display = ceremonies::CEREMONIES
        .iter()
        .map(|(key, label, _)| {
            if *key == "plan_approval" {
                let setting = ceremonies::resolve_plan_approval(wf, &content);
                let shown = ceremonies::PlanApproval {
                    value: setting.value,
                    source: shown_source(&setting.source),
                };
                format!("{label}: {}", ceremonies::format_plan_approval(&shown))
            } else {
                let resolution = ceremonies::resolve(wf, key, &content);
                format!(
                    "{label}: {}",
                    ceremonies::format_ceremony(
                        resolution.value,
                        &shown_source(&resolution.source)
                    )
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let current_entry = graph.iter().find(|stage| stage.slug == current_stage);
    let current_name = match current_entry {
        None => current_stage.clone(),
        Some(entry) => {
            stage_label(Some(entry), &entry.slug).unwrap_or_else(|| "this stage".to_owned())
        }
    };
    let stage_display = current_entry.map_or_else(
        || current_stage.clone(),
        |entry| format!("{current_name} ({})", entry.number),
    );
    let checkboxes = parse_checkboxes(&content);
    let current_box = checkboxes.iter().find(|line| line.slug == current_stage);
    let mut status_line = status.clone();
    match current_box.map(|line| line.state) {
        Some(CheckboxState::AwaitingApproval) => {
            status_line = format!("Awaiting your approval on {current_name}");
            if let Some((timestamp, at_ms)) = pending_organic_gate(&wf.events(), &current_stage) {
                let now_ms = now
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |since| {
                        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
                    });
                status_line.push_str(&format!(
                    " (waiting since {}, about {})",
                    plain_utc(&timestamp),
                    pending_duration(now_ms - at_ms)
                ));
            }
        }
        Some(CheckboxState::Revising) => {
            status_line =
                match get_field(&content, "Revision Count").filter(|count| !count.is_empty()) {
                    Some(count) => format!("Revising {current_name} (revision {count} of 3)"),
                    None => format!("Revising {current_name}"),
                };
        }
        Some(CheckboxState::Completed) if status == "Running" => {
            status_line = format!("{current_name} approved - ready to advance");
        }
        _ => {}
    }
    // Counts against the effective plan: the state's suffixes over the grid.
    let suffixes = parse_stage_suffixes(&content);
    // `stagesInScope`: nothing for a scope the install does not have.
    let in_scope: Vec<String> = if wf.install.definition(&scope).is_some() {
        stages_in_scope(&wf.install.grid, &scope, graph)
    } else {
        Vec::new()
    }
    .into_iter()
    .filter(|(slug, action)| {
        suffixes
            .iter()
            .find(|(known, _)| known == slug)
            .map_or(*action, |(_, suffix)| *suffix)
            == "EXECUTE"
    })
    .map(|(slug, _)| slug)
    .collect();
    let scoped: Vec<_> = if scope != "Unknown" && !in_scope.is_empty() {
        checkboxes
            .iter()
            .filter(|line| in_scope.contains(&line.slug))
            .collect()
    } else {
        checkboxes.iter().collect()
    };
    let total = scoped.len();
    let completed = scoped
        .iter()
        .filter(|line| line.state == CheckboxState::Completed)
        .count();
    let skipped = scoped
        .iter()
        .filter(|line| line.state == CheckboxState::Skipped)
        .count();
    let pct = focus_js::num::to_string(if total > 0 {
        focus_js::num::round(completed as f64 / total as f64 * 100.0)
    } else {
        0.0
    });
    let mut phase_progress = String::new();
    for phase_name in PHASES {
        let phase_slugs: Vec<&str> = graph
            .iter()
            .filter(|stage| stage.phase == phase_name)
            .map(|stage| stage.slug.as_str())
            .collect();
        let phase_boxes: Vec<_> = scoped
            .iter()
            .filter(|line| phase_slugs.contains(&line.slug.as_str()))
            .collect();
        if phase_boxes.is_empty() {
            continue;
        }
        let bar: String = phase_boxes
            .iter()
            .map(|line| match line.state {
                CheckboxState::Completed => '\u{2588}',
                CheckboxState::InProgress
                | CheckboxState::AwaitingApproval
                | CheckboxState::Revising => '\u{2592}',
                CheckboxState::Skipped => 'S',
                CheckboxState::Pending => '\u{2591}',
            })
            .collect();
        let done = phase_boxes
            .iter()
            .filter(|line| line.state == CheckboxState::Completed)
            .count();
        let units = if phase_name == "construction" {
            unit_progress(
                path.parent().unwrap_or(Path::new(".")),
                get_field(&content, "Construction Iteration").unwrap_or(""),
            )
        } else {
            None
        };
        let count = match units {
            Some((current, total)) => format!("Unit {current} of {total}"),
            None => format!("{done}/{}", phase_boxes.len()),
        };
        phase_progress.push_str(&format!(
            "  {:<16} {bar} {count}\n",
            phase_name.to_uppercase()
        ));
    }
    // Only a change the person can act on: a stage whose inputs moved since
    // it was approved, with the redo that refreshes it.
    let mut validity_output = String::new();
    let inspection = validity::inspect(wf, &content, false);
    let directly_stale: Vec<String> = inspection
        .issues
        .iter()
        .filter(|issue| issue.direct)
        .map(|issue| issue.stage.clone())
        .collect();
    let needs_revalidation: Vec<String> = inspection
        .issues
        .iter()
        .filter(|issue| !issue.direct)
        .map(|issue| issue.stage.clone())
        .collect();
    let earliest = directly_stale
        .first()
        .cloned()
        .or_else(|| inspection.issues.first().map(|issue| issue.stage.clone()));
    if let Some(earliest) = earliest
        && let Some(issue) = inspection
            .issues
            .iter()
            .find(|issue| issue.stage == earliest)
        && let Some(name) = stage_label(wf.install.stage(&earliest), &earliest)
    {
        let others: Vec<String> = directly_stale
            .iter()
            .chain(&needs_revalidation)
            .filter(|slug| **slug != earliest)
            .cloned()
            .collect();
        let other_names = stage_names(wf, &others);
        validity_output = format!(
            "{}{}\n",
            validity::stale_stage_note(&name, issue, &content),
            if other_names.is_empty() {
                String::new()
            } else {
                format!(" Also affected: {other_names}.")
            }
        );
    }
    let project_type = declared_project_type(get_field(&content, "Project Type"));
    let project_type_display = match project_type {
        None => String::new(),
        Some(kind) => format!(
            "Project Type:   {}{}{}\n",
            if kind == "Brownfield" {
                "existing code"
            } else {
                "new project"
            },
            if get_field(&content, "Project Type Source") == Some("you") {
                " (you said so)"
            } else {
                ""
            },
            if kind == "Brownfield" {
                code_scanned_clause(wf)
            } else {
                String::new()
            }
        ),
    };
    let depth = get_field(&content, "Depth");
    let test_strategy = get_field(&content, "Test Strategy");
    let scope_depth = wf
        .install
        .definition(&scope)
        .map(|definition| definition.meta.depth.as_str());
    let depth_source = if scope_depth
        .is_some_and(|known| known.to_lowercase() == depth.unwrap_or("").to_lowercase())
    {
        if plan.is_some() {
            "from the approved plan".to_owned()
        } else {
            format!("from scope {scope}")
        }
    } else {
        "set for this piece of work".to_owned()
    };
    let depth_display = match depth {
        None => String::new(),
        Some(depth) => format!(
            "Depth:          {depth} ({depth_source}){}\n",
            match test_strategy {
                Some(strategy) if strategy != depth => format!(", tests: {strategy}"),
                _ => String::new(),
            }
        ),
    };
    // Solo unit-major Construction: the active unit's own step is named too.
    let field_unit = get_field(&content, "Active Unit").map(trim).unwrap_or("");
    let field_stage = wf
        .install
        .stage(get_field(&content, "Unit Stage").map(trim).unwrap_or(""));
    // The step the engine last handed out is read off the directive marker
    // (M5), as is the authority the guards are read with: with one on the
    // record, the status is refused by name, however the record was named.
    let _ = (field_stage.is_none(), by_flags);
    let marker = wf
        .selection
        .engine_dir(&wf.layout)
        .join("active-directive.json");
    if marker.exists() {
        bail!(
            "{} is the orchestrator's directive marker, which the status reads for the step the engine last handed out: Focus does not read it yet.",
            marker.display()
        );
    }
    let current_node = wf.install.stage(&current_stage);
    let current_step = match (field_stage, current_node) {
        (Some(step), Some(current))
            if re::test(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$", "", field_unit)
                && stage::is_per_unit(step)
                && step.slug != current_stage
                && stage::is_per_unit(current) =>
        {
            format!("Current Step:   {} for unit {field_unit}\n", step.slug)
        }
        _ => String::new(),
    };
    let others: Vec<String> = match &wf.selection.intent {
        None => Vec::new(),
        Some(intent) => running_workflows(wf)
            .into_iter()
            .filter(|(space, dir)| {
                *space == wf.selection.space && dir != intent && is_safe_record_name(dir)
            })
            .map(|(_, dir)| dir)
            .collect(),
    };
    let also_open = if others.is_empty() {
        String::new()
    } else {
        format!(
            "Also open:      {} (ask to switch to {})\n",
            others.join(", "),
            if others.len() == 1 { "it" } else { "one" }
        )
    };
    let output = format!(
        "Focus Workflow Status\n==============================\nProject:        {project}\n{}\n{project_type_display}{depth_display}Phase:          {phase}\nCurrent Stage:  {stage_display}\n{current_step}Status:         {status_line}\n{agent_line}Guard Policy:   {guard_policy_display}\n{fences_off_line}{ceremony_display}\nCompletion:     {completed}/{total} stages ({pct}%){}\n\nPhase Progress:\n{phase_progress}\n{}{last_line}{next_line}{also_open}",
        match plan {
            Some(plan) => format!("Plan:           {plan} (this piece of work only)"),
            None => format!("Scope:          {scope}"),
        },
        if skipped > 0 {
            format!(" - {skipped} skipped")
        } else {
            String::new()
        },
        if validity_output.is_empty() {
            String::new()
        } else {
            format!("{validity_output}\n")
        }
    );
    if get_field(&content, "Unit Ownership").map(trim) == Some("team") {
        bail!(
            "Unit Ownership is team, so the status ends with the team board (the units' claims and their work), which Focus does not render yet."
        );
    }
    Ok(output)
}

/// `handleStatus`'s text when there is no workflow.
#[must_use]
pub fn no_workflow_text() -> String {
    "No active Focus workflow found.\n\nTo get started:\n  /focus \"build the auth service\"   Describe what to build (creates the workflow record automatically)\n  /focus <scope>      Start a workflow by scope (e.g., /focus feature)\n  /focus --help       Show all commands and scopes\n".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pending_duration_is_said_in_the_largest_unit() {
        assert_eq!(pending_duration(-5), "0 minutes");
        assert_eq!(pending_duration(60_000), "1 minute");
        assert_eq!(pending_duration(59 * 60_000), "59 minutes");
        assert_eq!(pending_duration(60 * 60_000), "1 hour");
        assert_eq!(pending_duration(47 * 3_600_000 + 1), "47 hours");
        assert_eq!(pending_duration(48 * 3_600_000), "2 days");
        assert_eq!(plain_utc("2026-01-05T10:07:33Z"), "2026-01-05 10:07 UTC");
        assert_eq!(plain_utc("yesterday"), "yesterday");
    }

    #[test]
    fn an_agent_is_named_by_its_persona_or_as_custom() {
        assert_eq!(
            agent_display_name("focus-product-agent").as_deref(),
            Some("Product Agent")
        );
        assert_eq!(
            agent_display_name("my-agent").as_deref(),
            Some("a custom agent")
        );
        assert_eq!(agent_display_name("Not A Slug"), None);
        assert_eq!(
            declared_project_type(Some(" brownfield ")),
            Some("Brownfield")
        );
        assert_eq!(declared_project_type(Some("mixed")), None);
    }
}

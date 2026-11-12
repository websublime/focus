//! The transitions themselves.
//!
//! Each one reads the state under the workspace lock, decides, writes the
//! ledger rows, then the state file (audit first: a row that cannot be
//! written stops the transition before the state changes), and answers
//! with one JSON object for standard output. A transition replayed after
//! a crash recognises what it already did and writes it once.
//!
//! Mirrors `handleAdvance`, `handleFinalize`, `handleCompleteWorkflow`,
//! `handleSkip` and `handleResume` in `aidlc-state.ts`.

use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_evidence::source::snapshot::{self, BASELINE_FIELD};
use focus_evidence::source::walk::Options;
use focus_graph::runtime::{ScopeDefinition, Stage};
use focus_intent::plan::next_in_scope_stage;
use focus_js::text::trim;
use focus_js::{Result, bail, json};
use focus_state::fields::{get_field, set_field, set_phase_progress};
use focus_state::progress::{
    CheckboxState, count_checkboxes, parse_checkboxes, parse_stage_suffixes, set_checkbox,
};
use focus_workspace::WorkspaceLock;
use focus_workspace::names::WORKSPACE_DIR;
use focus_workspace::records::read_intent_registry;
use serde_json::{Map, Value, json};

use crate::admission::{self, Action};
use crate::events::{
    answer_mode_fields, audit_tail_has_fields, current_routed_skip_audit_tail,
    has_stage_audit_event,
};
use crate::registry::update_intent_status;
use crate::stage;
use crate::validity;
use crate::workflow::Workflow;

/// `getSlugState`.
fn slug_state(content: &str, slug: &str) -> Option<CheckboxState> {
    parse_checkboxes(content)
        .into_iter()
        .find(|line| line.slug == slug)
        .map(|line| line.state)
}

/// `validateSlugInState`.
fn validate_slug_in_state(content: &str, slug: &str, allowed: &[CheckboxState]) -> Result<()> {
    let Some(actual) = slug_state(content, slug) else {
        bail!("Stage not found in state file: {slug}");
    };
    if !allowed.contains(&actual) {
        bail!(
            "Stage {slug} is in state '{}' but command requires one of: {}",
            actual.name(),
            allowed
                .iter()
                .map(|state| state.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

/// `relativeMemoryPath`: the stage's diary, relative to the project.
fn relative_memory_path(wf: &Workflow, stage: &Stage) -> String {
    let prefix = wf
        .selection
        .relative_record_dir()
        .unwrap_or_else(|| format!("{WORKSPACE_DIR}/spaces/{}/intents", wf.selection.space));
    format!("{prefix}/{}/{}/memory.md", stage.phase, stage.slug)
}

/// The usage rollup a completion row carries (`stageRollupFields`,
/// `workflowRollupFields`) is read off the usage ledger the harness hooks
/// fold transcripts into. Focus does not read that ledger yet: a ledger
/// that holds usage for this stage (or any usage, for the workflow) is
/// refused by name rather than left off the row.
pub(crate) fn refuse_usage_rollup(wf: &Workflow, slug: Option<&str>) -> Result<()> {
    if focus_workspace::env::var("DISABLE_USAGE_TRACKING").as_deref() == Some("1") {
        return Ok(());
    }
    let path = wf
        .layout
        .workspace_root()
        .join(focus_workspace::names::SESSIONS_DIR)
        .join("usage-ledger.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let Ok(ledger) = json::parse(&text) else {
        return Ok(());
    };
    let Some(workflows) = ledger.get("workflows").and_then(Value::as_object) else {
        return Ok(());
    };
    let has_tokens = |totals: Option<&Value>| {
        totals
            .and_then(|totals| totals.get("tokens"))
            .and_then(Value::as_object)
            .is_some_and(|tokens| {
                tokens
                    .values()
                    .any(|count| count.as_f64().is_some_and(|n| n > 0.0))
            })
    };
    let holds = workflows.values().any(|workflow| match slug {
        Some(slug) => workflow
            .get("byStage")
            .and_then(Value::as_object)
            .is_some_and(|stages| stages.contains_key(slug)),
        None => has_tokens(workflow.get("totals")),
    });
    if holds {
        bail!(
            "{} holds the token usage this completion would roll up onto its row, and Focus does not read the usage ledger yet. Set {}=1 to complete without the rollup.",
            path.display(),
            focus_workspace::env::full_name("DISABLE_USAGE_TRACKING")
        );
    }
    Ok(())
}

/// `STAGE_STARTED`'s fields for `next`: the answer mode reused, and the
/// source baseline of a stage that produces code.
fn stage_started_fields(wf: &Workflow, next: &Stage) -> Result<Vec<(&'static str, String)>> {
    let mut fields = vec![
        ("Stage", next.slug.clone()),
        ("Agent", stage::lead_agent(next).to_owned()),
    ];
    fields.extend(answer_mode_fields(&wf.events()));
    if stage::workspace_requires(next) {
        fields.push((BASELINE_FIELD, source_baseline(wf, &next.slug)?));
    }
    Ok(fields)
}

/// `sourceBaselineAuditFields` for the selected piece of work: the value of
/// a row's `Source Baseline`, with the project's source listing kept
/// beside the record under the stage it is taken for.
///
/// # Errors
///
/// The listing cannot be kept (see `focus_evidence::source::snapshot`).
pub fn source_baseline(wf: &Workflow, stage_slug: &str) -> Result<String> {
    let repos = crate::registry::source_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    snapshot::baseline_field(
        &wf.layout,
        &wf.selection,
        stage_slug,
        &repos,
        &Options::from_env(),
    )
}

fn to_pairs<'a>(fields: &'a [(&'a str, String)]) -> Vec<(&'a str, String)> {
    fields
        .iter()
        .map(|(name, value)| (*name, value.clone()))
        .collect()
}

/// `handleAdvance`: close `completed` and open the next in-scope stage
/// (or `next`, when named).
///
/// # Errors
///
/// Anything upstream refuses, in its words; what Focus refuses by name
/// (see [`crate::admission`]).
pub fn advance(
    wf: &Workflow,
    completed: &str,
    next: Option<&str>,
    operation: &str,
) -> Result<Value> {
    refuse_usage_rollup(wf, Some(completed))?;
    let held = wf.lock("advancing the workflow")?;
    advance_locked(wf, &held, completed, next, operation, None)
}

/// [`advance`] under a lock the caller holds: `approve` delegates here
/// after closing the stage, with the warning its own receipt raised.
///
/// # Errors
///
/// As [`advance`].
pub fn advance_locked(
    wf: &Workflow,
    held: &WorkspaceLock,
    completed: &str,
    next: Option<&str>,
    operation: &str,
    inherited_warning: Option<&str>,
) -> Result<Value> {
    let mut content = wf.read_state()?;
    let completed_stage = wf.stage(completed)?;
    let (scope, definition) = wf.scope_definition(
        &content,
        "Refusing to advance \u{2014} fix the state file first.",
    )?;
    let completed_before = slug_state(&content, completed);
    let current = get_field(&content, "Current Stage").unwrap_or("");
    let matches_current = completed == current;
    let already_marked = completed_before == Some(CheckboxState::Completed);
    let audit = wf.audit_text();
    let already_audited =
        already_marked && has_stage_audit_event(&audit, "STAGE_COMPLETED", completed);
    if !matches_current && !already_marked {
        bail!(
            "Cannot advance \"{completed}\": Current Stage is \"{current}\" and \"{completed}\" is {}. Pass the slug that's actually active, or use 'skip' / 'complete-workflow'.",
            completed_before.map_or("unknown", CheckboxState::name)
        );
    }
    let next_slug: String = match next {
        Some(next) => {
            let action = parse_stage_suffixes(&content)
                .into_iter()
                .find(|(slug, _)| slug == next)
                .map(|(_, action)| action)
                .or_else(|| grid_action(definition, next));
            if action == Some("SKIP") {
                bail!(
                    "Cannot advance to \"{next}\": stage is SKIP for scope \"{scope}\" (or state file). Pick the next EXECUTE stage or use 'skip'."
                );
            }
            next.to_owned()
        }
        None => match next_in_scope_stage(completed, definition, &wf.install.graph, Some(&content))
        {
            Some(next) => next.slug.clone(),
            None => bail!(
                "No next in-scope stage after \"{completed}\" for scope \"{scope}\". Use 'complete-workflow' if this was the final stage."
            ),
        },
    };
    let next_stage = wf.stage(&next_slug)?;
    let next_already_started = matches!(
        slug_state(&content, &next_slug),
        Some(CheckboxState::InProgress | CheckboxState::AwaitingApproval | CheckboxState::Revising)
    );
    if already_marked && already_audited && next_already_started && current == next_slug {
        return Ok(json!({
            "completed": completed,
            "started": next_slug,
            "replay": true,
            "timestamp": iso_timestamp(SystemTime::now()),
        }));
    }
    admission::admit(
        wf,
        &content,
        completed_stage,
        Action::Complete,
        operation,
        false,
    )?;

    let crosses_phase = completed_stage.phase != next_stage.phase;
    content = set_checkbox(&content, completed, CheckboxState::Completed);
    content = set_checkbox(&content, &next_slug, CheckboxState::InProgress);
    let next_after = next_in_scope_stage(&next_slug, definition, &wf.install.graph, Some(&content))
        .map(|stage| stage.slug.clone());
    let timestamp = iso_timestamp(SystemTime::now());
    content = set_field(&content, "Current Stage", &next_stage.slug);
    content = set_field(
        &content,
        "Lifecycle Phase",
        &next_stage.phase.to_uppercase(),
    );
    content = set_field(
        &content,
        "Next Stage",
        next_after.as_deref().unwrap_or("none"),
    );
    content = set_field(&content, "In Progress", &next_stage.slug);
    content = set_field(&content, "Active Agent", stage::lead_agent(next_stage));
    content = set_field(&content, "Status", "Running");
    content = set_field(&content, "Last Updated", &timestamp);
    content = set_field(&content, "Last Completed Stage", completed);
    content = set_field(
        &content,
        "Next Action",
        &format!("Execute {}", stage::name(next_stage)),
    );
    let completed_count = count_checkboxes(&content, CheckboxState::Completed);
    content = set_field(&content, "Completed", &completed_count.to_string());
    if crosses_phase {
        content = set_phase_progress(&content, &completed_stage.phase, "Verified");
        content = set_phase_progress(&content, &next_stage.phase, "Active");
    }
    let emits_completed = !already_marked || !already_audited;
    let validation = if emits_completed {
        validity::audit_fields(wf, completed_stage, &content)
    } else {
        Vec::new()
    };
    let warning = inherited_warning.map(str::to_owned).or_else(|| {
        validation
            .iter()
            .find(|(name, _)| *name == validity::VALIDATION_WARNING_FIELD)
            .map(|(_, value)| value.clone())
    });

    if emits_completed {
        let mut fields = vec![("Stage", completed.to_owned())];
        fields.extend(to_pairs(&validation));
        fields.push((
            "Details",
            format!("Stage {} completed", stage::name(completed_stage)),
        ));
        wf.emit(held, "STAGE_COMPLETED", &fields)?;
    }
    if crosses_phase {
        emit_phase_boundary(
            wf,
            held,
            &completed_stage.phase,
            &next_stage.phase,
            completed_count,
            &scope,
        )?;
    }
    wf.emit(
        held,
        "STAGE_STARTED",
        &stage_started_fields(wf, next_stage)?,
    )?;
    wf.write_state(&content)?;

    let mut out = Map::new();
    out.insert("completed".to_owned(), json!(completed));
    out.insert("started".to_owned(), json!(next_slug));
    out.insert("phase".to_owned(), json!(next_stage.phase.to_uppercase()));
    out.insert("phase_boundary".to_owned(), json!(crosses_phase));
    out.insert("completed_count".to_owned(), json!(completed_count));
    out.insert("next_after".to_owned(), json!(next_after));
    out.insert("already_completed".to_owned(), json!(already_marked));
    out.insert(
        "memory_path".to_owned(),
        json!(relative_memory_path(wf, next_stage)),
    );
    out.insert("timestamp".to_owned(), json!(timestamp));
    if let Some(warning) = warning {
        out.insert("warnings".to_owned(), json!([warning]));
    }
    Ok(Value::Object(out))
}

/// The scope grid's action for a stage, when the grid has a cell.
fn grid_action(definition: &ScopeDefinition, slug: &str) -> Option<&'static str> {
    definition
        .stages
        .get(slug)
        .and_then(Value::as_str)
        .map(|action| {
            if action == "EXECUTE" {
                "EXECUTE"
            } else {
                "SKIP"
            }
        })
}

/// The three rows of a phase boundary.
fn emit_phase_boundary(
    wf: &Workflow,
    held: &WorkspaceLock,
    from: &str,
    to: &str,
    completed_count: usize,
    scope: &str,
) -> Result<()> {
    wf.emit(
        held,
        "PHASE_COMPLETED",
        &[
            ("From phase", from.to_owned()),
            ("To phase", to.to_owned()),
            ("Stages completed", completed_count.to_string()),
        ],
    )?;
    wf.emit(
        held,
        "PHASE_VERIFIED",
        &[("Phase boundary", format!("{from} \u{2192} {to}"))],
    )?;
    wf.emit(
        held,
        "PHASE_STARTED",
        &[("Phase", to.to_owned()), ("Scope", scope.to_owned())],
    )
}

/// `handleFinalize`: close `completed` and move the cursor, without
/// opening the next stage or writing a row.
///
/// # Errors
///
/// As [`advance`].
pub fn finalize(wf: &Workflow, completed: &str, operation: &str) -> Result<Value> {
    let _held = wf.lock("finalizing a stage")?;
    let mut content = wf.read_state()?;
    let completed_stage = wf.stage(completed)?;
    admission::admit(
        wf,
        &content,
        completed_stage,
        Action::Complete,
        operation,
        false,
    )?;
    content = set_checkbox(&content, completed, CheckboxState::Completed);
    let completed_count = count_checkboxes(&content, CheckboxState::Completed);
    content = set_field(&content, "Completed", &completed_count.to_string());
    let (_, definition) = wf.scope_definition(
        &content,
        "Refusing to finalize \u{2014} fix the state file first.",
    )?;
    let next_stage = next_in_scope_stage(completed, definition, &wf.install.graph, Some(&content));
    let next_after = next_stage.and_then(|next| {
        next_in_scope_stage(&next.slug, definition, &wf.install.graph, Some(&content))
    });
    let timestamp = iso_timestamp(SystemTime::now());
    match next_stage {
        Some(next) => {
            content = set_field(&content, "Current Stage", &next.slug);
            content = set_field(
                &content,
                "Next Stage",
                next_after.map_or("none", |stage| stage.slug.as_str()),
            );
            content = set_field(&content, "Lifecycle Phase", &next.phase.to_uppercase());
            content = set_field(&content, "Active Agent", stage::lead_agent(next));
            if completed_stage.phase != next.phase {
                content = set_phase_progress(&content, &completed_stage.phase, "Verified");
                content = set_phase_progress(&content, &next.phase, "Active");
            }
        }
        None => {
            content = set_field(&content, "Current Stage", "none");
            content = set_field(&content, "Next Stage", "none");
            content = set_field(&content, "Status", "Completed");
            content = set_field(&content, "In Progress", "none");
            content = set_phase_progress(&content, &completed_stage.phase, "Verified");
        }
    }
    content = set_field(&content, "Last Completed Stage", completed);
    content = set_field(&content, "Last Updated", &timestamp);
    content = set_field(
        &content,
        "Next Action",
        &next_stage.map_or_else(
            || "Workflow complete".to_owned(),
            |next| format!("Resume from {}", stage::name(next)),
        ),
    );
    wf.write_state(&content)?;
    Ok(json!({
        "completed": completed,
        "completed_count": completed_count,
        "next_stage": next_stage.map_or("none", |next| next.slug.as_str()),
        "phase": next_stage.map_or_else(|| completed_stage.phase.to_uppercase(), |next| next.phase.to_uppercase()),
        "timestamp": timestamp,
    }))
}

/// `updateIntentStatus(..., "complete")` for the selected record: a
/// record with no registry row (the flat legacy layout) is left alone.
fn mark_complete(wf: &Workflow) -> Result<()> {
    if let Some(intent) = wf.selection.intent.as_deref()
        && !read_intent_registry(&wf.layout, &wf.selection.space).is_empty()
    {
        update_intent_status(&wf.layout, &wf.selection.space, intent, "complete")?;
    }
    Ok(())
}

/// `handleCompleteWorkflow`: close `completed` as the last stage.
///
/// # Errors
///
/// As [`advance`].
pub fn complete_workflow(
    wf: &Workflow,
    completed: &str,
    reason: Option<&str>,
    operation: &str,
) -> Result<Value> {
    refuse_usage_rollup(wf, Some(completed))?;
    refuse_usage_rollup(wf, None)?;
    let held = wf.lock("completing the workflow")?;
    complete_workflow_locked(wf, &held, completed, reason, operation, None)
}

/// [`complete_workflow`] under a lock the caller holds.
///
/// # Errors
///
/// As [`advance`].
pub fn complete_workflow_locked(
    wf: &Workflow,
    held: &WorkspaceLock,
    completed: &str,
    reason: Option<&str>,
    operation: &str,
    inherited_warning: Option<&str>,
) -> Result<Value> {
    let mut content = wf.read_state()?;
    let completed_stage = wf.stage(completed)?;
    let already_marked = slug_state(&content, completed) == Some(CheckboxState::Completed);
    let already_audited =
        already_marked && has_stage_audit_event(&wf.audit_text(), "STAGE_COMPLETED", completed);
    admission::admit(
        wf,
        &content,
        completed_stage,
        Action::Complete,
        operation,
        false,
    )?;
    content = set_checkbox(&content, completed, CheckboxState::Completed);
    let completed_count = count_checkboxes(&content, CheckboxState::Completed);
    content = set_field(&content, "Completed", &completed_count.to_string());
    let timestamp = iso_timestamp(SystemTime::now());
    content = set_field(&content, "Status", "Completed");
    content = set_field(&content, "Last Updated", &timestamp);
    content = set_field(&content, "Last Completed Stage", completed);
    content = set_field(&content, "In Progress", "none");
    content = set_field(&content, "Next Stage", "none");
    content = set_field(&content, "Next Action", "Workflow complete");
    content = set_phase_progress(&content, &completed_stage.phase, "Verified");
    let (scope, _) = wf.scope_definition(
        &content,
        "Refusing to complete workflow \u{2014} fix the state file first.",
    )?;
    let emits_completed = !already_marked || !already_audited;
    let validation = if emits_completed {
        validity::audit_fields(wf, completed_stage, &content)
    } else {
        Vec::new()
    };
    let warning = inherited_warning.map(str::to_owned).or_else(|| {
        validation
            .iter()
            .find(|(name, _)| *name == validity::VALIDATION_WARNING_FIELD)
            .map(|(_, value)| value.clone())
    });
    if emits_completed {
        let mut fields = vec![("Stage", completed.to_owned())];
        fields.extend(to_pairs(&validation));
        fields.push((
            "Details",
            format!("Final stage {} completed", stage::name(completed_stage)),
        ));
        wf.emit(held, "STAGE_COMPLETED", &fields)?;
    }
    wf.emit(
        held,
        "PHASE_COMPLETED",
        &[
            ("From phase", completed_stage.phase.clone()),
            ("To phase", "(end)".to_owned()),
            ("Stages completed", completed_count.to_string()),
        ],
    )?;
    wf.emit(
        held,
        "PHASE_VERIFIED",
        &[(
            "Phase boundary",
            format!("{} \u{2192} end", completed_stage.phase),
        )],
    )?;
    let mut fields = vec![
        ("Scope", scope.clone()),
        (
            "Details",
            format!("Scope: {scope}, {completed_count} stages completed"),
        ),
    ];
    if let Some(reason) = reason {
        fields.push(("Reason", reason.to_owned()));
    }
    wf.emit(held, "WORKFLOW_COMPLETED", &fields)?;
    wf.write_state(&content)?;
    mark_complete(wf)?;
    let mut out = Map::new();
    out.insert("completed".to_owned(), json!(completed));
    out.insert("completed_count".to_owned(), json!(completed_count));
    out.insert("status".to_owned(), json!("Completed"));
    out.insert("reason".to_owned(), json!(reason));
    out.insert("timestamp".to_owned(), json!(timestamp));
    if let Some(warning) = warning {
        out.insert("warnings".to_owned(), json!([warning]));
    }
    Ok(Value::Object(out))
}

/// `handleSkip` without `--unit`: mark `slug` skipped; with `route`, move
/// on from it as `advance` would, writing once what a crashed attempt
/// already wrote.
///
/// # Errors
///
/// Anything upstream refuses, in its words.
pub fn skip(wf: &Workflow, slug: &str, reason: Option<&str>, route: bool) -> Result<Value> {
    let reason = reason.map(trim).filter(|reason| !reason.is_empty());
    if route {
        refuse_usage_rollup(wf, None)?;
    }
    let held = wf.lock("skipping a stage")?;
    let mut content = wf.read_state()?;
    let stage = wf.stage(slug)?;
    if !route {
        validate_slug_in_state(
            &content,
            slug,
            &[
                CheckboxState::Pending,
                CheckboxState::InProgress,
                CheckboxState::Revising,
            ],
        )?;
        content = set_checkbox(&content, slug, CheckboxState::Skipped);
        let timestamp = iso_timestamp(SystemTime::now());
        content = set_field(&content, "Last Updated", &timestamp);
        let mut fields = vec![("Stage", slug.to_owned())];
        if let Some(reason) = reason {
            fields.push(("Reason", reason.to_owned()));
        }
        wf.emit(&held, "STAGE_SKIPPED", &fields)?;
        wf.write_state(&content)?;
        return Ok(json!({ "slug": slug, "new_state": "skipped", "timestamp": timestamp }));
    }
    let Some(reason) = reason else {
        bail!("focus engine state skip --route requires a nonblank --reason <text>.");
    };
    let plan_skips = parse_stage_suffixes(&content)
        .iter()
        .any(|(known, action)| known == slug && *action == "SKIP");
    let mut allowed = vec![
        CheckboxState::InProgress,
        CheckboxState::Revising,
        CheckboxState::Skipped,
    ];
    if plan_skips {
        allowed.push(CheckboxState::AwaitingApproval);
    }
    validate_slug_in_state(&content, slug, &allowed)?;
    let current = get_field(&content, "Current Stage").unwrap_or("");
    if current != slug {
        bail!("Cannot route skipped stage \"{slug}\": Current Stage is \"{current}\".");
    }
    let (scope, definition) = wf.scope_definition(
        &content,
        "Refusing to route skip - fix the state file first.",
    )?;
    let was_skipped = slug_state(&content, slug) == Some(CheckboxState::Skipped);
    let tail = if was_skipped {
        current_routed_skip_audit_tail(&wf.audit_text(), slug)
    } else {
        None
    };
    let skip_audited = tail.is_some();
    content = set_checkbox(&content, slug, CheckboxState::Skipped);
    let timestamp = iso_timestamp(SystemTime::now());
    let next_stage = next_in_scope_stage(slug, definition, &wf.install.graph, Some(&content));
    let crosses_phase = next_stage.is_some_and(|next| next.phase != stage.phase);
    let boundary_target = next_stage.map_or("(end)", |next| next.phase.as_str());
    let boundary = format!(
        "{} \u{2192} {}",
        stage.phase,
        next_stage.map_or("end", |next| next.phase.as_str())
    );
    let already = |event: &str, fields: &[(&str, &str)]| -> bool {
        tail.as_deref()
            .is_some_and(|tail| audit_tail_has_fields(tail, event, fields))
    };
    let phase_completed_audited = already(
        "PHASE_COMPLETED",
        &[("From phase", &stage.phase), ("To phase", boundary_target)],
    );
    let phase_verified_audited = already("PHASE_VERIFIED", &[("Phase boundary", &boundary)]);
    let phase_started_audited = next_stage.is_some_and(|next| {
        already(
            "PHASE_STARTED",
            &[("Phase", &next.phase), ("Scope", &scope)],
        )
    });
    let stage_started_audited =
        next_stage.is_some_and(|next| already("STAGE_STARTED", &[("Stage", &next.slug)]));
    let workflow_completed_audited = next_stage.is_none()
        && already(
            "WORKFLOW_COMPLETED",
            &[
                ("Scope", &scope),
                (
                    "Details",
                    &format!("Scope: {scope}, final stage {slug} skipped"),
                ),
            ],
        );

    match next_stage {
        Some(next) => {
            content = set_checkbox(&content, &next.slug, CheckboxState::InProgress);
            let next_after =
                next_in_scope_stage(&next.slug, definition, &wf.install.graph, Some(&content));
            content = set_field(&content, "Current Stage", &next.slug);
            content = set_field(&content, "Lifecycle Phase", &next.phase.to_uppercase());
            content = set_field(
                &content,
                "Next Stage",
                next_after.map_or("none", |after| after.slug.as_str()),
            );
            content = set_field(&content, "In Progress", &next.slug);
            content = set_field(&content, "Active Agent", stage::lead_agent(next));
            content = set_field(&content, "Status", "Running");
            content = set_field(
                &content,
                "Next Action",
                &format!("Execute {}", stage::name(next)),
            );
            if crosses_phase {
                content = set_phase_progress(&content, &stage.phase, "Verified");
                content = set_phase_progress(&content, &next.phase, "Active");
            }
        }
        None => {
            content = set_field(&content, "Status", "Completed");
            content = set_field(&content, "In Progress", "none");
            content = set_field(&content, "Next Stage", "none");
            content = set_field(&content, "Next Action", "Workflow complete");
            content = set_phase_progress(&content, &stage.phase, "Verified");
        }
    }
    let completed_count = count_checkboxes(&content, CheckboxState::Completed);
    content = set_field(&content, "Completed", &completed_count.to_string());
    content = set_field(&content, "Last Updated", &timestamp);

    if !skip_audited {
        wf.emit(
            &held,
            "STAGE_SKIPPED",
            &[
                ("Stage", slug.to_owned()),
                ("Reason", reason.to_owned()),
                ("Skip Kind", "conditional-runtime".to_owned()),
            ],
        )?;
    }
    match next_stage {
        Some(next) => {
            if crosses_phase {
                if !phase_completed_audited {
                    wf.emit(
                        &held,
                        "PHASE_COMPLETED",
                        &[
                            ("From phase", stage.phase.clone()),
                            ("To phase", next.phase.clone()),
                            ("Stages completed", completed_count.to_string()),
                        ],
                    )?;
                }
                if !phase_verified_audited {
                    wf.emit(
                        &held,
                        "PHASE_VERIFIED",
                        &[("Phase boundary", boundary.clone())],
                    )?;
                }
                if !phase_started_audited {
                    wf.emit(
                        &held,
                        "PHASE_STARTED",
                        &[("Phase", next.phase.clone()), ("Scope", scope.clone())],
                    )?;
                }
            }
            if !stage_started_audited {
                wf.emit(&held, "STAGE_STARTED", &stage_started_fields(wf, next)?)?;
            }
        }
        None => {
            if !phase_completed_audited {
                wf.emit(
                    &held,
                    "PHASE_COMPLETED",
                    &[
                        ("From phase", stage.phase.clone()),
                        ("To phase", "(end)".to_owned()),
                        ("Stages completed", completed_count.to_string()),
                    ],
                )?;
            }
            if !phase_verified_audited {
                wf.emit(
                    &held,
                    "PHASE_VERIFIED",
                    &[("Phase boundary", boundary.clone())],
                )?;
            }
            if !workflow_completed_audited {
                wf.emit(
                    &held,
                    "WORKFLOW_COMPLETED",
                    &[
                        ("Scope", scope.clone()),
                        (
                            "Details",
                            format!("Scope: {scope}, final stage {slug} skipped"),
                        ),
                        ("Reason", reason.to_owned()),
                    ],
                )?;
            }
        }
    }
    wf.write_state(&content)?;
    if next_stage.is_none() {
        mark_complete(wf)?;
    }
    Ok(json!({
        "slug": slug,
        "new_state": "skipped",
        "started": next_stage.map(|next| next.slug.clone()),
        "workflow_completed": next_stage.is_none(),
        "recovered": was_skipped || skip_audited,
        "timestamp": timestamp,
    }))
}

/// `handleResume`: where the work stands, read-only.
///
/// # Errors
///
/// The state file cannot be read.
pub fn resume(wf: &Workflow) -> Result<Value> {
    let content = wf.read_state()?;
    let field = |name: &str, fallback: &str| -> String {
        get_field(&content, name)
            .filter(|value| !value.is_empty())
            .unwrap_or(fallback)
            .to_owned()
    };
    let current = field("Current Stage", "unknown");
    let gate_state = slug_state(&content, &current).map_or("unknown", CheckboxState::name);
    let audit = wf.audit_text();
    let mut compaction_pending = false;
    if !audit.is_empty() {
        let lines: Vec<&str> = audit.split('\n').collect();
        let tail = lines[lines.len().saturating_sub(400)..].join("\n");
        if let Some(at) = tail.rfind("**Event**: SESSION_COMPACTED") {
            let after = &tail[at..];
            let activity = focus_js::re::test(
                r"\*\*Event\*\*: (STAGE_STARTED|STAGE_COMPLETED|GATE_APPROVED|SESSION_RESUMED|RECOVERY_COMPLETED)",
                "",
                after,
            );
            compaction_pending = !activity;
        }
    }
    let mut out = Map::new();
    out.insert("resumed".to_owned(), json!(true));
    out.insert("current_stage".to_owned(), json!(current));
    out.insert(
        "phase".to_owned(),
        json!(field("Lifecycle Phase", "unknown")),
    );
    out.insert("status".to_owned(), json!(field("Status", "unknown")));
    out.insert("scope".to_owned(), json!(field("Scope", "unknown")));
    out.insert(
        "active_agent".to_owned(),
        json!(field("Active Agent", "unknown")),
    );
    out.insert("next_stage".to_owned(), json!(field("Next Stage", "none")));
    out.insert("gate_state".to_owned(), json!(gate_state));
    out.insert("compaction_pending".to_owned(), json!(compaction_pending));
    if let Some(unit) = get_field(&content, "Active Unit").filter(|unit| !unit.is_empty()) {
        out.insert("active_unit".to_owned(), json!(unit));
        out.insert(
            "unit_state".to_owned(),
            json!(get_field(&content, "Unit State").unwrap_or("in-progress")),
        );
        if let Some(reason) = get_field(&content, "Unit Pause Reason") {
            out.insert("unit_pause_reason".to_owned(), json!(reason));
        }
        if let Some(action) = get_field(&content, "Unit Next Action") {
            out.insert("unit_next_action".to_owned(), json!(action));
        }
    }
    Ok(Value::Object(out))
}

//! SessionStart: record how a conversation began (`SESSION_STARTED`,
//! `SESSION_RESUMED`), bind it to the record it works on, and hand the
//! model the workflow's context on a resume. Mirrors
//! `aidlc-session-start.ts`.
//!
//! Session events are hook-owned because only the host knows when a
//! conversation begins. With no state file the hook emits no workflow event
//! or context, but still records the host's session identity (the current
//! session, the process map, the binding) so the first record created later
//! in the turn can bind to it.

use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_graph::stage_schema::PHASES;
use focus_harness::Harness;
use focus_intent::record::ensure_active_space_cursor;
use focus_js::{EngineError, json as js_json, re};
use focus_state::fields::get_field;
use focus_workspace::Layout;
use focus_workspace::active_memory::{self, ACTIVE_MEMORY_DIR};
use focus_workspace::names::{STATE_FILE, UNIT_SCOPE_FILE};
use focus_workspace::records::{
    Invoker, Selection, Selector, read_active_intent_cursor, read_session_binding,
    resolve_selection, valid_session_id,
};
use serde_json::{Value, json};

use super::{append, is_refusal, open, refused};
use crate::context::{Participation, participation};
use crate::health::{hooks_health_dir, record_drop, write_status_file};
use crate::registry::{
    active_intent_uuid, find_intent_by_uuid, intent_uuid_of, session_intent_uuid,
};
use crate::rules_held::record_rules_load;
use crate::sessions::{
    clear_plan_approval_bypass, clear_rebind_offer, clear_session_intent_uuid,
    is_bindable_record_name, is_safe_record_name, read_rebind_offer, record_bypass,
    write_current_session_id, write_current_transcript_path, write_rebind_offer,
    write_selection_notice, write_session_binding, write_session_intent_uuid,
};
use crate::switches::switches_off_lines;
use crate::{Outcome, payload};

const HOOK: &str = "session-start";
/// What the host keeps of the hook's context: text past this goes by name.
const HANDOFF_MAX_BYTES: usize = 8 * 1024;
/// `UNIT_NAME_REGEX`.
const UNIT_NAME: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]*$";

/// `entrySkillInvocation`: the harness's own entry skill.
const ENTRY_SKILL: &str = "/focus";

/// `hookContextLine`: the hook's context as the host reads it.
fn context_line(context: &str) -> String {
    format!(
        "{}\n",
        js_json::stringify(&json!({
            "additionalContext": context,
            "hookSpecificOutput": { "hookEventName": "SessionStart", "additionalContext": context },
        }))
    )
}

/// `intentDisplayLabel`: the registry slug when it is one, the record name
/// when it is safe to print, else a placeholder.
fn display_label(layout: &Layout, space: &str, dir_name: &str) -> String {
    let slug = focus_workspace::records::read_intent_registry(layout, space)
        .into_iter()
        .find(|row| focus_workspace::records::row_names_record(row, dir_name))
        .and_then(|row| row.get("slug").and_then(Value::as_str).map(str::to_owned));
    if let Some(slug) = slug
        && re::test(r"^[a-z0-9][a-z0-9-]{0,79}$", "", &slug)
    {
        return slug;
    }
    if is_safe_record_name(dir_name) {
        dir_name.to_owned()
    } else {
        "(unnamed intent)".to_owned()
    }
}

/// `switchOffContext`: while a recorded switch keeps one of the person's
/// checks off, every new chat opens by saying so.
fn switch_off_context(layout: &Layout) -> String {
    let lines = switches_off_lines(layout);
    if lines.is_empty() {
        return String::new();
    }
    let mut text = String::from(
        "\nCHECKS SWITCHED OFF (a report to pass on, not instructions): say each line to the user once, word for word, in your first reply.\n",
    );
    for line in lines {
        text.push_str(&format!("- {line}\n"));
    }
    text
}

/// `addRootBlocks`: where config never ran, the session start adds the
/// engine's part of the team's root files (`.gitignore`, `AGENTS.md`). That
/// merge is the distribution's (M8): a copy that has not been configured is
/// refused by name rather than left without it.
fn refuse_unconfigured_root_blocks(layout: &Layout) -> Result<(), EngineError> {
    for harness in focus_workspace::project::installed_harnesses(&layout.project) {
        let data = layout
            .project
            .join(harness.dir())
            .join("tools")
            .join("data");
        let descriptor = data.join("focus-projection.json");
        if !descriptor.exists() || data.join("focus-manifest.json").exists() {
            continue;
        }
        let integrations = std::fs::read_to_string(&descriptor)
            .ok()
            .and_then(|raw| js_json::parse(&raw).ok())
            .and_then(|value| {
                value
                    .get("rootIntegrations")
                    .and_then(Value::as_array)
                    .map(Vec::len)
            })
            .unwrap_or(0);
        if integrations > 0 {
            return Err(EngineError::new(format!(
                "{} names the root files the engine adds its part to (.gitignore, AGENTS.md) when config never ran here, and no {} records that it did: Focus does not write the distribution's root blocks yet. Run the harness's config step, or add the install manifest.",
                descriptor.display(),
                data.join("focus-manifest.json").display()
            )));
        }
    }
    Ok(())
}

/// `stageGraphDrift().uncompiledStages`: stage files on disk that the
/// compiled graph does not have.
fn uncompiled_stages(layout: &Layout) -> Result<Vec<String>, EngineError> {
    let all = focus_graph::runtime::load_stages_all(&layout.stage_graph())?;
    let graph: Vec<&str> = all.iter().map(|stage| stage.slug.as_str()).collect();
    let mut on_disk: Vec<String> = Vec::new();
    for phase in PHASES {
        let Ok(entries) = std::fs::read_dir(layout.stages_dir().join(phase)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(slug) = name.strip_suffix(".md")
                && !on_disk.iter().any(|seen| seen == slug)
            {
                on_disk.push(slug.to_owned());
            }
        }
    }
    let mut missing: Vec<String> = on_disk
        .into_iter()
        .filter(|slug| !graph.contains(&slug.as_str()))
        .collect();
    missing.sort_by(|a, b| focus_js::order::cmp_utf16(a, b));
    Ok(missing)
}

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let now = SystemTime::now();
    // The payload, read before the workflow-state gate: a fresh session
    // commonly starts before the first record is created.
    let mut source = "startup".to_owned();
    let mut rebind_check_only = false;
    let mut session_id = String::new();
    let mut transcript_path = String::new();
    if !input.is_empty() {
        match js_json::parse(input) {
            Ok(Value::Object(raw)) => {
                source = match raw.get("source") {
                    Some(value) if payload::truthy(Some(value)) => payload::js_string(value),
                    _ => "unknown".to_owned(),
                };
                if let Some(id) = raw.get("session_id").and_then(Value::as_str) {
                    session_id = valid_session_id(Some(id)).unwrap_or("").to_owned();
                }
                if let Some(path) = raw.get("transcript_path").and_then(Value::as_str) {
                    transcript_path = path.to_owned();
                }
                rebind_check_only = raw.get("rebind_check") == Some(&Value::Bool(true));
            }
            Ok(_) => source = "unknown".to_owned(),
            Err(_) => source = "malformed".to_owned(),
        }
    }
    write_current_transcript_path(layout, &session_id, &transcript_path);
    // Record the live conversation on every fire, including a pre-workflow
    // start: a creation reads this marker and binds an unstamped session.
    if !session_id.is_empty() {
        write_current_session_id(layout, &session_id);
        // `humanPresenceGuardDisabled` reads the settings files; one it
        // refuses throws past the plan-approval bookkeeping too, and the
        // bypass bookkeeping never breaks a session start.
        match focus_workspace::settings::Places::of(&layout.project).flag(
            &focus_workspace::env::full_name("SKIP_HUMAN_PRESENCE_GUARD"),
            &|name| std::env::var(name).ok(),
        ) {
            Err(_) => {}
            Ok(disabled) => {
                let presence = if disabled.as_deref() == Some("1") {
                    record_bypass(layout, "presence", &session_id, now)
                } else {
                    Ok(())
                };
                if presence.is_ok() {
                    if focus_workspace::env::is_on("DISABLE_PLAN_APPROVAL_GUARD") {
                        let _ = record_bypass(layout, "plan-approval", &session_id, now);
                    } else {
                        clear_plan_approval_bypass(layout, &session_id);
                    }
                }
            }
        }
        focus_workspace::ancestry::write_ancestry(layout, &session_id);
    }
    // One session-local workflow target before any state read: an existing
    // binding wins; a first-seen session inherits the shared cursors and
    // records that fallback, an intent-less cold workspace included.
    let pre_binding = (!session_id.is_empty())
        .then(|| read_session_binding(layout, &session_id))
        .flatten();
    let pre_stamp = (!session_id.is_empty())
        .then(|| session_intent_uuid(layout, &session_id))
        .flatten();
    if !session_id.is_empty()
        && (source == "startup"
            || source == "clear"
            || (read_rebind_offer(layout, &session_id).is_some() && pre_binding.is_none()))
    {
        clear_rebind_offer(layout, &session_id);
    }
    let stamped_target = if source == "resume" && pre_binding.is_none() {
        pre_stamp
            .as_deref()
            .and_then(|uuid| find_intent_by_uuid(layout, uuid))
    } else {
        None
    };
    let invoker = Invoker::from_env();
    let resolved = match &stamped_target {
        Some(found) => Selection {
            space: found.space.clone(),
            intent: Some(found.dir_name.clone()),
            session_id: Some(session_id.clone()),
            binding: None,
        },
        None => {
            let selector = Selector {
                session_id: (!session_id.is_empty()).then(|| session_id.clone()),
                ..Selector::default()
            };
            match resolve_selection(layout, &selector, &invoker) {
                Ok(selection) => selection,
                Err(err) => return refused(&err),
            }
        }
    };
    // Resolving a record is not joining it. A resumed session's own stamp
    // does join it; a record name the binding cannot carry does not.
    let joins_by_stamp = stamped_target
        .as_ref()
        .is_some_and(|found| is_bindable_record_name(&found.dir_name));
    let joined = joins_by_stamp
        || (stamped_target.is_none()
            && match participation(layout, &resolved) {
                Ok(part) => part == Participation::Participant,
                Err(err) => return refused(&err),
            });
    let selection = if joined {
        resolved.clone()
    } else {
        Selection {
            intent: None,
            binding: None,
            ..resolved.clone()
        }
    };
    // The record a previously bound conversation can rejoin explicitly.
    let rejoin_record = (!joined
        && resolved.intent.is_some()
        && pre_binding.as_ref().and_then(|b| b.intent.as_deref()) == resolved.intent.as_deref())
    .then(|| resolved.clone());
    // `bindingSource`.
    let binding_source: Option<String> = if !joined {
        Some(if resolved.intent.is_none() {
            pre_binding
                .as_ref()
                .and_then(|b| b.source.clone())
                .unwrap_or_else(|| "none".to_owned())
        } else {
            "unjoined".to_owned()
        })
    } else if joins_by_stamp {
        Some("stamp".to_owned())
    } else if pre_binding
        .as_ref()
        .is_some_and(|b| b.space == selection.space && b.intent == selection.intent)
    {
        pre_binding.as_ref().and_then(|b| b.source.clone())
    } else if read_active_intent_cursor(layout, &selection.space).as_deref()
        == selection.intent.as_deref()
    {
        Some("cursor".to_owned())
    } else {
        // A Unit claimed on this machine names its space and intent (M4's
        // units); Focus does not read the claim yet.
        let stamp = layout.workspace_root().join(UNIT_SCOPE_FILE);
        if stamp.exists() {
            return refused(&EngineError::new(format!(
                "{} claims a unit of work for this checkout, which would say how this conversation joined its record: Focus does not read unit claims yet.",
                stamp.display()
            )));
        }
        Some("worktree".to_owned())
    };
    // Persist the selection before any early return.
    if !session_id.is_empty() {
        write_session_binding(
            layout,
            &session_id,
            &selection.space,
            selection.intent.as_deref(),
            binding_source.as_deref(),
            now,
        );
    }
    // A clone's missing gitignored cursor, the copy the harness includes
    // read, and the team's root files, before the no-workflow early exit.
    ensure_active_space_cursor(layout);
    if let Err(err) = refuse_unconfigured_root_blocks(layout) {
        return refused(&err);
    }
    let mut include_repointed = false;
    let mut copy_behind = active_memory::drift(layout, &selection.space);
    if active_memory::refresh(layout, &selection.space) {
        include_repointed = true;
    } else {
        copy_behind.clear();
    }
    // Claude Code and Copilot read their imports before this hook runs: when
    // the copy was written just now, this chat is told, with the text when
    // it fits, else by name.
    let mut memory_handoff = String::new();
    if !copy_behind.is_empty() && matches!(layout.harness, Harness::Claude | Harness::Copilot) {
        let written = format!(
            "The method files this chat's instructions import ({ACTIVE_MEMORY_DIR}/) were written for space `{}` just now, after the instructions were read",
            selection.space
        );
        memory_handoff = match active_memory::inline_memory_files(
            layout,
            &selection.space,
            Some(&copy_behind),
        ) {
            Some(inline) if inline.bytes <= HANDOFF_MAX_BYTES => {
                format!(
                    "\n\n{written}. This is their text now:\n\n{}",
                    inline.parts.join("\n")
                )
            }
            _ => format!(
                "\n\n{written}: {}. Read them from that folder before acting on AI-DLC work or on a question about the team's practices.",
                copy_behind.join(", ")
            ),
        };
    }
    // What the host loaded into this chat, so `next` can tell whether the
    // chat already holds a stage's rule text.
    if !session_id.is_empty() && !rebind_check_only {
        record_rules_load(
            layout,
            &session_id,
            &source,
            &selection.space,
            include_repointed,
            now,
        );
    }
    let state_file = selection.docs_root(layout).join(STATE_FILE);
    // No workflow joined: only the session identity recorded above.
    if !state_file.exists() {
        if session_id.is_empty() {
            return Outcome::exit(0);
        }
        let mut rejoin = String::new();
        let signature = rejoin_record
            .as_ref()
            .and_then(|record| {
                record
                    .intent
                    .as_ref()
                    .map(|intent| format!("rejoin:{}/{intent}", record.space))
            })
            .unwrap_or_default();
        let offer_now = !signature.is_empty()
            && (!rebind_check_only
                || read_rebind_offer(layout, &session_id).as_deref() != Some(signature.as_str()));
        if let Some(record) = &rejoin_record
            && let Some(intent) = &record.intent
            && is_safe_record_name(intent)
            && offer_now
        {
            if rebind_check_only {
                write_rebind_offer(layout, &session_id, &signature);
            }
            let slug = display_label(layout, &record.space, intent);
            let command = if record.space == layout.active_space() {
                format!("`{ENTRY_SKILL} intent {intent}`")
            } else {
                format!(
                    "`{ENTRY_SKILL} space {}`, then `{ENTRY_SKILL} intent {intent}`",
                    record.space
                )
            };
            rejoin = format!(
                "\nINTENT REBIND OFFER: This conversation was working {slug}, but it has not joined that workflow on this machine. Rejoin {slug}? [Y/n] - on Yes, run {command}; on No, continue without a workflow."
            );
            if rebind_check_only {
                write_selection_notice(
                    layout,
                    &session_id,
                    &format!(
                        "This chat was working on {slug}, which it has not joined on this machine, so it carries on without a workflow. To pick {slug} up again, run {command}."
                    ),
                );
            }
        }
        let switches = if rebind_check_only {
            String::new()
        } else {
            switch_off_context(layout)
        };
        return Outcome {
            code: 0,
            stdout: context_line(&format!(
                "FOCUS Runtime Session: {session_id}\nUse this exact value for any Plan Approval --session argument in this conversation. It goes only on a command that asks for --session, never on next.{rejoin}{switches}{memory_handoff}"
            )),
            stderr: String::new(),
        };
    }
    write_status_file(
        &hooks_health_dir(layout, &selection),
        "session-start.last",
        &iso_timestamp(now),
        false,
    );
    // The session event. compact / unknown: no emission (compact is the
    // PreCompact hook's).
    let event = if rebind_check_only {
        None
    } else {
        match source.as_str() {
            "startup" | "clear" | "malformed" => Some("SESSION_STARTED"),
            "resume" => Some("SESSION_RESUMED"),
            _ => None,
        }
    };
    let wf = match open(layout, &selection, &invoker) {
        Ok(wf) => wf,
        Err(err) => return refused(&err),
    };
    if let Some(event) = event {
        let mut fields: Vec<(&str, String)> = vec![("Source", source.clone())];
        if !session_id.is_empty() {
            fields.push(("Session", session_id.clone()));
        }
        if let Err(err) = append(&wf, event, &fields) {
            if is_refusal(&err) {
                return refused(&err);
            }
            record_drop(layout, &selection, HOOK, &err.to_string());
        }
        // The commit-provenance anchor sweep: an opt-in walk over recent
        // commits that writes SOURCE_COMMITTED rows (the attestation, M5).
        if focus_workspace::env::is_on("SESSION_ANCHOR") {
            return refused(&EngineError::new(format!(
                "{}=1 asks the session start to anchor recent commits in the audit trail, which reads the attestation's receipts: Focus does not run the anchor sweep yet. Unset it.",
                focus_workspace::env::full_name("SESSION_ANCHOR")
            )));
        }
    }
    // Resume rebind: a conversation works one record; the cursor is shared.
    // On a start the working record's uuid is stamped for this session; on
    // a resume a stamp or binding that differs from the live cursor earns a
    // rebind offer, made once.
    let active_space = layout.active_space();
    let live_dir = match focus_workspace::records::active_intent(layout, &active_space) {
        Ok(dir) => dir,
        Err(err) => return refused(&err),
    };
    let live_uuid = active_intent_uuid(layout, Some(&active_space));
    let selected_uuid = selection
        .intent
        .as_deref()
        .and_then(|intent| intent_uuid_of(layout, &selection.space, intent))
        .filter(|uuid| !uuid.is_empty());
    let mut rebind_offer = String::new();
    if !session_id.is_empty() {
        let stamped_uuid = pre_stamp.clone();
        if event == Some("SESSION_STARTED") {
            if let Some(uuid) = &selected_uuid {
                write_session_intent_uuid(layout, &session_id, uuid);
            }
        } else if source == "resume" {
            let owned_uuid = if pre_binding.is_some() {
                selected_uuid.clone()
            } else {
                stamped_uuid.clone()
            };
            if let Some(owned) = owned_uuid
                .as_deref()
                .filter(|owned| Some(*owned) != live_uuid.as_deref())
            {
                let was = find_intent_by_uuid(layout, owned)
                    .filter(|was| is_safe_record_name(&was.dir_name));
                if let Some(was) = was {
                    let signature = format!(
                        "{}/{}->{active_space}/{}",
                        was.space,
                        was.dir_name,
                        live_dir.as_deref().unwrap_or("(none)")
                    );
                    let already_offered = read_rebind_offer(layout, &session_id).as_deref()
                        == Some(signature.as_str());
                    let live_slug = live_uuid
                        .as_deref()
                        .and_then(|uuid| find_intent_by_uuid(layout, uuid))
                        .map_or_else(
                            || "(none)".to_owned(),
                            |live| display_label(layout, &live.space, &live.dir_name),
                        );
                    let was_label = display_label(layout, &was.space, &was.dir_name);
                    let switch_instruction = if was.space == active_space {
                        format!("run `{ENTRY_SKILL} intent {}`", was.dir_name)
                    } else {
                        format!(
                            "first run `{ENTRY_SKILL} space {}`; after it completes, run `{ENTRY_SKILL} intent {}`",
                            was.space, was.dir_name
                        )
                    };
                    if !already_offered {
                        rebind_offer = format!(
                            "INTENT REBIND OFFER: This conversation is bound to {was_label}, but the shared cursor names {live_slug}. Move the shared cursor back to {was_label}? [Y/n] - on Yes, {switch_instruction}; on No, keep working {was_label} through this session binding. This changes only machine-local navigation.\n"
                        );
                        write_rebind_offer(layout, &session_id, &signature);
                        if rebind_check_only {
                            write_selection_notice(
                                layout,
                                &session_id,
                                &format!(
                                    "Another chat selected {live_slug}; this chat stays on {was_label}. To make {was_label} the selected work again, {switch_instruction}."
                                ),
                            );
                        }
                    }
                }
            } else {
                clear_rebind_offer(layout, &session_id);
            }
            // A binding owns attribution; without one the legacy stamp
            // follows the live cursor after the offer.
            match (
                &pre_binding,
                &selected_uuid,
                &stamped_uuid,
                &stamped_target,
                &live_uuid,
            ) {
                (Some(_), Some(uuid), _, _, _) => {
                    write_session_intent_uuid(layout, &session_id, uuid)
                }
                (Some(_), None, Some(_), _, _) => clear_session_intent_uuid(layout, &session_id),
                (None, Some(uuid), _, Some(_), _) => {
                    write_session_intent_uuid(layout, &session_id, uuid)
                }
                (None, _, _, _, Some(live)) => write_session_intent_uuid(layout, &session_id, live),
                (None, _, Some(_), _, None) => clear_session_intent_uuid(layout, &session_id),
                _ => {}
            }
        } else if stamped_uuid.is_none()
            && let Some(uuid) = &selected_uuid
        {
            write_session_intent_uuid(layout, &session_id, uuid);
        }
    }
    // Cursor's prompt hook cannot add context: the probe's offer reaches the
    // person as the line written above, on the next directive.
    if rebind_check_only {
        if !rebind_offer.is_empty() {
            match (&pre_binding, &selected_uuid, &live_uuid) {
                (Some(_), Some(uuid), _) | (None, _, Some(uuid)) => {
                    write_session_intent_uuid(layout, &session_id, uuid)
                }
                _ => {}
            }
            return Outcome {
                code: 0,
                stdout: context_line(&format!(
                    "FOCUS Runtime Session: {session_id}\n{rebind_offer}{memory_handoff}"
                )),
                stderr: String::new(),
            };
        }
        return Outcome::exit(0);
    }
    // The context the model is handed, read off the state file.
    let content = match wf.read_state() {
        Ok(content) => content,
        Err(err) => return refused(&err),
    };
    let field =
        |name: &str, fallback: &str| get_field(&content, name).unwrap_or(fallback).to_owned();
    let phase = field("Lifecycle Phase", "unknown");
    let stage = field("Current Stage", "unknown");
    let status = field("Status", "unknown");
    let last = field("Last Completed Stage", "none");
    let next = field("Next Action", "resume current stage");
    let agent = field("Active Agent", "unknown");
    let scope = field("Scope", "unknown");
    // A per-unit stage stopped mid-unit: name the unit, its state and, for
    // a paused unit, the reason and next action.
    let active_unit = get_field(&content, "Active Unit");
    let per_unit = |slug: &str| {
        wf.install
            .stage(slug)
            .is_some_and(focus_lifecycle::stage::is_per_unit)
    };
    let unit_stage_slug = get_field(&content, "Unit Stage")
        .map(focus_js::text::trim)
        .unwrap_or("");
    let unit_stage = per_unit(unit_stage_slug).then(|| unit_stage_slug.to_owned());
    let step_unit = active_unit
        .map(focus_js::text::trim)
        .filter(|unit| re::test(UNIT_NAME, "", unit));
    let in_unit_stages = per_unit(&stage);
    let later_unit_stage = match (&step_unit, &unit_stage) {
        (Some(_), Some(unit_stage)) if in_unit_stages && *unit_stage != stage => {
            Some(unit_stage.clone())
        }
        _ => None,
    };
    let unit_by_unit = get_field(&content, "Construction Iteration").map(focus_js::text::trim)
        == Some("unit-major")
        && get_field(&content, "Unit Ownership").map(focus_js::text::trim) != Some("team");
    let unit_line = match active_unit {
        Some(unit) => {
            let mut line = format!(
                "Active Unit: {unit}{} ({}",
                later_unit_stage
                    .as_ref()
                    .map_or_else(String::new, |later| format!(" on {later}")),
                get_field(&content, "Unit State").unwrap_or("in-progress")
            );
            if let Some(reason) = get_field(&content, "Unit Pause Reason") {
                line.push_str(&format!("; reason: {reason}"));
            }
            if let Some(action) = get_field(&content, "Unit Next Action") {
                line.push_str(&format!("; next: {action}"));
            }
            line.push_str(")\n");
            if let Some(later) = &later_unit_stage
                && unit_by_unit
            {
                line.push_str(&format!(
                    "Current Step: {later} for unit {}. Construction runs one unit at a time, so Current Stage stays {stage} until every unit is done.\n",
                    step_unit.unwrap_or("")
                ));
            }
            line
        }
        None => String::new(),
    };
    let recovery = if selection.engine_dir(layout).join("recovery.md").exists() {
        "NOTE: A compaction recovery breadcrumb exists at .focus-engine/recovery.md - check if state was preserved correctly.\n"
    } else {
        ""
    };
    // Stage-graph drift: a stage file on disk the compiled graph lacks is
    // never executed; said once at session start, never blocking it.
    let drift_note = match uncompiled_stages(layout) {
        Ok(uncompiled) if !uncompiled.is_empty() => format!(
            "NOTE: {} stage file(s) on disk are not in the compiled stage graph and will NOT execute: {}. Run `focus engine graph compile` to include them, then start a fresh workflow (an in-flight workflow keeps its original stage set).\n",
            uncompiled.len(),
            uncompiled.join(", ")
        ),
        _ => String::new(),
    };
    // None of Focus's harnesses hides the Stop note from the person.
    let say_the_line = ", and say nothing about it.";
    let switches = switch_off_context(layout);
    let context = format!(
        "FOCUS WORKFLOW ACTIVE\n{rebind_offer}Scope: {scope}\nRuntime Session: {}\nLifecycle Phase: {phase}\nCurrent Stage: {stage}\nStatus: {status}\nActive Agent: {agent}\nLast Completed: {last}\nNext Action: {next}\n{unit_line}{recovery}{drift_note}{}A BARE {ENTRY_SKILL} re-entry carries on with this work, the same as {ENTRY_SKILL} --resume: send the first `next` as `next --resume` and continue directly, with no resume menu. Then follow the recovery protocol's Session resume, including its one SAY line. When the person asks to redo, jump, or start fresh (at an approval gate too, where it is that request and not the gate's answer), report it with `report --result resumed --choice <redo|jump|fresh>` (add `--target <stage slug>` for the stage they named, and `--unit <unit>` or `--every-unit` when they named a Unit or said every Unit) and follow the print it returns. Check the active intent's {STATE_FILE} for full context.\n\nFORWARDING-LOOP DISCIPLINE (non-negotiable — the engine owns ALL routing):\n- The engine route (`focus engine orchestrate`) is the ONLY authority on the next move. You run it, you do EXACTLY what its one directive says, and you report stage-work outcomes. Repeat only when the directive calls for continuation; a terminal directive or required human wait ends the turn. You never re-derive routing yourself.\n- STEP 1: YOUR VERY FIRST ACTION: take everything the user typed after `{ENTRY_SKILL}` and append it to the first `next` call UNCHANGED. The flags ARE the user's intent; dropping them sends the workflow to the wrong place. `{ENTRY_SKILL} --phase ideation` -> you MUST run `next --phase ideation`, never bare `next`. `{ENTRY_SKILL} --stage X` -> `next --stage X`. `{ENTRY_SKILL}` alone -> `next --resume` (this work is active). Before running that first `next`, verify: if the user's message contained `--phase`/`--stage`/`--scope`/`--depth`/freeform text, it MUST appear on your `next` command; a bare `next` when the user gave arguments is a bug.\n- When a directive is `{{kind:\"print\"}}` whose message names a command to run (e.g. `focus engine jump execute ...`, a scope/config change, or `init`): that named command is your IMMEDIATE next tool call. Run THAT EXACT command FIRST. Do NOT run `next` again, do NOT read more files, do NOT plan a stage — until the named command has run. Re-running the engine before it is a protocol violation that silently skips the move.\n- After the named command, obey the message's ending. If it says \"then stop\", print the command's output and END THE TURN: no `next`, `report`, or stage work. In particular, `{ENTRY_SKILL} space default` and other terminal workspace navigation stop even when the destination has an unfinished intent. Selecting it does not request resuming it. Continue only when the directive explicitly says to continue.\n- If you end a turn while this work still needs you, Focus answers with one line, \"Focus is carrying on with <stage>.\" It is from Focus, not the person: never record it as their answer{say_the_line} Follow the focus skill's \"When Focus carries on by itself\" steps; in short: if you just asked the person a question you have not recorded, record it with `log decision` and end the turn without asking it again or saying anything else; if you were doing the work of a `run-stage` you still hold, finish its steps and run the `report` built from it (its stage, plus `--unit` in team-owned Unit work); otherwise `continue` with the rules receipt you hold, or run `next`, and follow the step it returns.",
        if session_id.is_empty() {
            "(unavailable)"
        } else {
            session_id.as_str()
        },
        switches.trim_start()
    );
    Outcome {
        code: 0,
        stdout: context_line(&format!("{context}{memory_handoff}")),
        stderr: String::new(),
    }
}

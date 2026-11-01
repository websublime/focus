//! PostToolUse (Bash): recompile the runtime graph after every
//! transition-class audit emit. Filters cheaply on the command, then reads
//! the last three audit blocks of the record this conversation joined for a
//! transition event, and runs `focus engine runtime compile` when one is
//! there. On the way, two things that need no workflow state: a creation
//! observed in the call's output binds the invoking session to the record
//! it created, and the engine's own error directive would be relayed to the
//! person. Mirrors `aidlc-rebuild-stage-graph.ts`.

use std::process::{Command, Stdio};
use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_guards::engine_command::{
    CompileCommand, classify_runtime_compile_command, literal_orchestrate_verb,
    shell_tool_response_text,
};
use focus_intent::record::{consume_creation_receipt, write_session_binding};
use focus_js::{EngineError, re};
use focus_workspace::Layout;
use focus_workspace::records::{
    Invoker, Selection, Selector, read_session_binding, resolve_selection, valid_session_id,
};
use serde_json::{Value, json};

use super::{open, refused};
use crate::context::{self, Participation, participation};
use crate::health::{cursor_selection, debug, hooks_health_dir, record_drop, write_status_file};
use crate::registry::{intent_uuid_of, session_intent_uuid};
use crate::sessions::{
    clear_session_intent_uuid, write_session_intent_handoff, write_session_intent_uuid,
};
use crate::{Outcome, payload};

const HOOK: &str = "rebuild-stage-graph";

/// The space with no record, for a debug line written before any record is
/// known.
fn bare(layout: &Layout) -> Selection {
    Selection {
        space: layout.active_space(),
        intent: None,
        session_id: None,
        binding: None,
    }
}

/// The events whose row in the audit tail means the graph is behind.
const TRANSITION: &str = r"^\*\*Event\*\*:[ \t]*(GATE_APPROVED|STAGE_STARTED|STAGE_AWAITING_APPROVAL|AUDIT_MERGED|UNIT_MERGED|WORKFLOW_COMPLETED)[ \t]*$";

/// `bindCreatedIntentToInvokingSession`: intent-create runs before a
/// workflow exists, so SessionStart cannot stamp that conversation yet;
/// PostToolUse is the first boundary that carries both the host's session
/// and the creation's result. A second creation moves binding and
/// attribution to the new record; the handoff receipt keeps the prior uuid
/// for the Stop hook.
fn bind_created_intent(
    layout: &Layout,
    parsed: &serde_json::Map<String, Value>,
    command: &str,
    ide_mode: bool,
) {
    let Some(session_id) = parsed
        .get("session_id")
        .and_then(Value::as_str)
        .and_then(|id| valid_session_id(Some(id)))
    else {
        return;
    };
    if !ide_mode && !re::test(r"(?:intent-create|intent\s+create)", "", command) {
        return;
    }
    let response = match parsed.get("tool_response") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => focus_js::json::stringify(&json!("")),
        Some(other) => focus_js::json::stringify(other),
    };
    let Some(found) = re::find(
        r"(?:Intent created:|Migrated flat workspace into intent:)\s*([A-Za-z0-9._-]+)\s+\(space:\s*([A-Za-z0-9._-]+)\)",
        "",
        &response,
    ) else {
        return;
    };
    let dir_name = re::group(&response, &found, 1).unwrap_or("").to_owned();
    let space = re::group(&response, &found, 2).unwrap_or("").to_owned();
    let created_uuid = intent_uuid_of(layout, &space, &dir_name);
    let existing_uuid = session_intent_uuid(layout, session_id);
    debug(
        layout,
        &cursor_selection(layout).unwrap_or_else(|| bare(layout)),
        HOOK,
        "session-bind",
        &[
            ("sessionId", json!(session_id)),
            ("dirName", json!(dir_name)),
            ("space", json!(space)),
            (
                "resolvedUuid",
                json!(created_uuid.clone().unwrap_or_default()),
            ),
            (
                "existingUuid",
                json!(existing_uuid.clone().unwrap_or_default()),
            ),
        ],
    );
    let Some(created_uuid) = created_uuid.filter(|uuid| !uuid.is_empty()) else {
        return;
    };
    // The response text alone proves nothing; the creation receipt intent
    // create left on this machine does, once. A binding intent create
    // already wrote for this record keeps its source.
    let source = if consume_creation_receipt(layout, &space, &dir_name) {
        "create"
    } else {
        "observed-create"
    };
    let existing = read_session_binding(layout, session_id);
    // Unproven text cannot move a session that takes part in another
    // record, by any evidence participation accepts.
    if source == "observed-create"
        && let Some(bound) = &existing
        && let Some(bound_intent) = &bound.intent
        && (bound.space != space || *bound_intent != dir_name)
    {
        let theirs = Selection {
            space: bound.space.clone(),
            intent: Some(bound_intent.clone()),
            session_id: Some(session_id.to_owned()),
            binding: existing.clone(),
        };
        if participation(layout, &theirs).ok() == Some(Participation::Participant) {
            return;
        }
    }
    let unchanged = existing.as_ref().is_some_and(|bound| {
        bound.space == space
            && bound.intent.as_deref() == Some(dir_name.as_str())
            && bound.source.is_some()
    });
    if !unchanged {
        write_session_binding(
            layout,
            session_id,
            &space,
            &dir_name,
            source,
            SystemTime::now(),
        );
    }
    if let Some(previous) = &existing_uuid
        && *previous != created_uuid
    {
        write_session_intent_handoff(
            layout,
            session_id,
            previous,
            &created_uuid,
            false,
            SystemTime::now(),
        );
    }
    // A stamp joins the session on resume, so only a session this creation
    // joined is stamped; an observed creation clears the older stamp instead.
    let bound = read_session_binding(layout, session_id);
    if bound.as_ref().is_some_and(|bound| {
        bound.intent.as_deref() == Some(dir_name.as_str())
            && bound
                .source
                .as_deref()
                .is_some_and(context::is_trusted_binding_source)
    }) {
        write_session_intent_uuid(layout, session_id, &created_uuid);
    } else {
        clear_session_intent_uuid(layout, session_id);
    }
}

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let Some(parsed) = payload::parse(input) else {
        return Outcome::exit(0);
    };
    let tool_input = parsed.get("tool_input").and_then(Value::as_object);
    let ide_mode = payload::text(tool_input, "source") == "ide-audit-sync";
    // `tool_input?.command ?? ""`: text, or a value upstream falls over on
    // (its debug line slices the command before anything is decided).
    let command: String = match tool_input.and_then(|input| input.get("command")) {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => {
            return refused(&EngineError::new(format!(
                "{HOOK}: the payload's command is not text ({}).",
                focus_js::json::stringify(other)
            )));
        }
    };
    // The engine's error directive, relayed to the person through the
    // harness's own channel: the directive is the orchestrator's (M5).
    if shell_tool_response_text(parsed.get("tool_response"))
        .is_some_and(|text| text.contains("\"kind\":\"error\""))
        && literal_orchestrate_verb(&command).is_some()
    {
        return refused(&EngineError::new(format!(
            "{HOOK}: this call ran the engine's orchestrator and its output names an error directive, which the hook would relay to the person: Focus does not read engine directives yet."
        )));
    }
    bind_created_intent(layout, &parsed, &command, ide_mode);
    debug(
        layout,
        &cursor_selection(layout).unwrap_or_else(|| bare(layout)),
        HOOK,
        "command-gate",
        &[
            ("ideAuditMode", json!(ide_mode)),
            (
                "command",
                json!(command.chars().take(120).collect::<String>()),
            ),
        ],
    );
    if !ide_mode {
        match classify_runtime_compile_command(&command) {
            CompileCommand::Reject | CompileCommand::Pass => return Outcome::exit(0),
            CompileCommand::Fire => {}
        }
    }
    // The record this conversation joined, resolved once with its session;
    // then its shards, not this process's: the helpers take the resolved
    // space and record, and a conversation bound to no record leaves them
    // to the cursor (`readAllAuditShards(projectDir, intent, space)`).
    let session = payload::session_id(Some(&parsed));
    let named = Selector {
        session_id: session
            .as_deref()
            .and_then(|id| valid_session_id(Some(id)))
            .map(str::to_owned),
        ..Selector::default()
    };
    let invoker = Invoker::from_env();
    let joined = match resolve_selection(layout, &named, &invoker) {
        Ok(selection) => selection,
        Err(err) if super::is_refusal(&err) => return refused(&err),
        Err(_) => return Outcome::exit(0),
    };
    if joined.intent.is_some() {
        match participation(layout, &joined) {
            Ok(Participation::Participant) => {}
            Ok(_) => return Outcome::exit(0),
            Err(err) => return refused(&err),
        }
    }
    let explicit = Selector {
        space: Some(joined.space.clone()),
        intent: joined.intent.clone(),
        session_id: None,
    };
    // With the record named, the session is only what the helpers hand
    // down (upstream walks the process tree for it and finds nothing to
    // change); with none, the cursor decides, as upstream's helpers do.
    let invoker = match (&joined.intent, &named.session_id) {
        (Some(_), Some(session)) => context::pinned(session),
        _ => invoker,
    };
    let selection = match resolve_selection(layout, &explicit, &invoker) {
        Ok(selection) => selection,
        Err(err) if super::is_refusal(&err) => return refused(&err),
        Err(_) => return Outcome::exit(0),
    };
    let selection = &selection;
    let wf = match open(layout, selection, &invoker) {
        Ok(wf) => wf,
        Err(err) => return refused(&err),
    };
    let audit = wf.audit_text().replace("\r\n", "\n");
    if audit.is_empty() {
        return Outcome::exit(0);
    }
    let health_dir = hooks_health_dir(layout, selection);
    write_status_file(
        &health_dir,
        "rebuild-stage-graph.last",
        &iso_timestamp(SystemTime::now()),
        false,
    );
    // The last three blocks: one approve writes up to three rows in one call.
    let blocks: Vec<&str> = audit.split("\n---\n").collect();
    let last3 = &blocks[blocks.len().saturating_sub(3)..];
    let has_transition = last3.iter().any(|block| re::test(TRANSITION, "m", block));
    let noop = || {
        if ide_mode {
            write_status_file(
                &health_dir,
                "rebuild-stage-graph.noop",
                &iso_timestamp(SystemTime::now()),
                false,
            );
        }
        Outcome::exit(0)
    };
    if !has_transition {
        return noop();
    }
    // In IDE mode the command filter is skipped, so the transition sits in
    // the tail across every later call: a graph at least as new as the
    // newest shard was compiled from this tail already.
    if ide_mode
        && let Ok(graph) = std::fs::metadata(selection.docs_root(layout).join("runtime-graph.json"))
        && let Ok(graph_mtime) = graph.modified()
    {
        let newest = wf
            .ledger()
            .shards(&wf.selector(), &wf.invoker, None)
            .unwrap_or_default()
            .iter()
            .filter_map(|shard| std::fs::metadata(shard).ok()?.modified().ok())
            .max();
        if newest.is_none_or(|newest| graph_mtime >= newest) {
            return noop();
        }
    }
    // The compile, as its own process: the hook waits, and a failure is a
    // drop, never a block on the parent call.
    let drop_to = cursor_selection(layout);
    match compile(layout, session.as_deref()) {
        Ok(None) => {}
        Ok(Some(reason)) | Err(reason) => {
            if let Some(selection) = &drop_to {
                record_drop(layout, selection, HOOK, &reason);
            }
        }
    }
    Outcome::exit(0)
}

/// Run `focus engine runtime compile` for the project, with the payload's
/// session handed down. `Ok(None)` when it succeeded; `Ok(Some(reason))`
/// when it exited otherwise; `Err` when it could not be run.
fn compile(layout: &Layout, session: Option<&str>) -> Result<Option<String>, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let mut command = Command::new(exe);
    command
        .args(["engine", "runtime", "compile", "--project-dir"])
        .arg(&layout.project)
        .args(["--harness", layout.harness.name()])
        .current_dir(&layout.project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(session) = session.and_then(|id| valid_session_id(Some(id))) {
        command.env("FOCUS_SESSION_OVERRIDE", session);
        command.env("FOCUS_SESSION_OVERRIDE_SOURCE", "payload");
    }
    let output = command.output().map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(None);
    }
    let status = output
        .status
        .code()
        .map_or_else(|| "null".to_owned(), |code| code.to_string());
    Ok(Some(format!(
        "exit {status}: {}",
        String::from_utf8_lossy(&output.stderr)
    )))
}

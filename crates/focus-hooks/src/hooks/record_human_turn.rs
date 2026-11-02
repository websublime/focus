//! UserPromptSubmit (and the picker's PostToolUse): record a `HUMAN_TURN`
//! row, the human-presence gate's signal. On every real human prompt the
//! hook appends the row to the active record's shard; the gates refuse
//! unless a turn was recorded since the last resolution. The hook records
//! presence and order; it does not authenticate who launched the
//! dispatcher. Mirrors `aidlc-record-human-turn.ts`.
//!
//! The same seam keeps the message record (`messages`), the words the
//! person typed in this chat (the gate-words record a stage gate reads),
//! the `human-turn` marker the Stop hook compares against the engine's
//! advance, and the rows for a question box that came back empty or with
//! its picks. `FOCUS_UNATTENDED=1` withholds the authority-bearing row and
//! the records while retaining the marker. Fail-open throughout: a failure
//! to mint never blocks the person's turn.
//!
//! What this slice leaves for later, refused by name while its evidence is
//! there: a typed guard switch to apply (the config setter's), a `/focus`
//! line with words after the entry (the orchestrator's parser reads its
//! flags), the plan approval challenge and protected questions, the
//! directive marker's asks, the break-glass phrase, a review awaiting its
//! verdict (the receipts, M5), and the lines queued for the person.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use focus_audit::read::block_field;
use focus_audit::time::iso_timestamp;
use focus_guards::typed_switch::{
    entry_words, is_command_prompt, is_typed_guard_switch_prompt, is_typed_guard_switch_question,
};
use focus_guards::workflow::Guards;
use focus_js::text::{len_utf16, slice_utf16, trim};
use focus_js::{EngineError, json as js_json, re};
use focus_lifecycle::reply::strip_recommended_decorator;
use focus_lifecycle::workflow::Workflow;
use focus_state::fields::get_field;
use focus_workspace::names::{ENGINE_DIR, SESSIONS_DIR};
use focus_workspace::records::{Selection, Selector, valid_session_id};
use focus_workspace::{Layout, links, observer};
use serde_json::{Map, Value, json};

use super::{is_refusal, open, refused};
use crate::health::{hooks_health_dir, write_status_file};
use crate::messages::{
    MESSAGE_PICKER_MAX_CHARS, MESSAGE_TEXT_MAX_CHARS, Message, MessageRoute, save_message,
};
use crate::turn::{
    TurnOrigin, carries_several_picks, claude_transcript_origin, empty_picker_result,
    extract_option_labels, extract_question_text, extract_response_text, host_envelope_turn_text,
    host_template_origin, picked_labels, picker_replies, supplied_turn_origin,
};
use crate::{Outcome, context, payload};

/// `COMMAND_TURN_REPLY`, `QUESTION_TURN_REPLY`: the `Reply` marks of a turn
/// that was only a command, or only a question about a switch.
const COMMAND_TURN_REPLY: &str = "command";
const QUESTION_TURN_REPLY: &str = "question";

/// `QUESTION_UNANSWERED_NOTICE`: what the agent does after an empty box.
const QUESTION_UNANSWERED_NOTICE: &str = "The question box closed with no answer, so nothing was answered or recorded. Ask the same question again in your reply as numbered options, not in the question box, and end the turn; never pick an answer for the person.";

/// `PLAN_APPROVAL_OVERRIDE_PHRASE_RE`: the break-glass line.
const PLAN_APPROVAL_OVERRIDE_PHRASE: &str = r"^override plan approval:\s*(\S.*)$";

/// `PAST_REVIEW_REQUEST`: rows after a review request that say the work
/// went past it.
const PAST_REVIEW_REQUEST: [&str; 9] = [
    "DECISION_RECORDED",
    "QUESTION_ANSWERED",
    "CHECKPOINT_VERIFICATION_RECORDED",
    "STAGE_AWAITING_APPROVAL",
    "GATE_APPROVED",
    "GATE_REJECTED",
    "STAGE_COMPLETED",
    "STAGE_JUMPED",
    "WORKFLOW_STARTED",
];

/// `GATE_WORDS_MAX_MESSAGES`, `GATE_WORDS_MAX_CHARS`: the gate-words record's
/// bounds.
const GATE_WORDS_MAX_MESSAGES: usize = 8;
const GATE_WORDS_MAX_CHARS: usize = 8000;

/// The picker a reply came through: the question and labels the harness
/// reports it under.
struct PickerQuestion {
    options: Option<Vec<String>>,
    several_picks: bool,
}

/// What the payload said, read once.
struct Turn {
    session: String,
    prompt_submitted: bool,
    typed_prompt: String,
    human_response_text: String,
    question_text: Option<String>,
    picker: Option<PickerQuestion>,
    picker_unanswered: bool,
    picked: Vec<String>,
    origin: TurnOrigin,
}

fn read_turn(parsed: Option<&Map<String, Value>>) -> Turn {
    let mut turn = Turn {
        session: String::new(),
        prompt_submitted: false,
        typed_prompt: String::new(),
        human_response_text: String::new(),
        question_text: None,
        picker: None,
        picker_unanswered: false,
        picked: Vec::new(),
        origin: TurnOrigin::Person { recorded: false },
    };
    // Presence still records without identity on a legacy payload.
    let Some(parsed) = parsed else {
        return turn;
    };
    if let Some(Value::String(session)) = parsed.get("session_id") {
        turn.session = valid_session_id(Some(trim(session)))
            .unwrap_or("")
            .to_owned();
    }
    let tool_input = parsed.get("tool_input").or_else(|| parsed.get("toolInput"));
    let tool_response = parsed
        .get("tool_response")
        .or_else(|| parsed.get("toolResponse"));
    turn.question_text = extract_question_text(tool_input);
    // A host that wraps the person's turn in its own context hands over the
    // whole envelope as the prompt; only the person's turn is read.
    let own_turn = |value: Option<&Value>| -> Option<Value> {
        match value {
            Some(Value::String(text)) => {
                Some(Value::String(host_envelope_turn_text(text).to_owned()))
            }
            other => other.cloned(),
        }
    };
    let prompt = own_turn(parsed.get("prompt"));
    let user_prompt = own_turn(parsed.get("user_prompt"));
    let message = own_turn(parsed.get("message"));
    for candidate in [
        prompt.as_ref(),
        user_prompt.as_ref(),
        message.as_ref(),
        parsed.get("tool_response"),
        parsed.get("toolResponse"),
    ] {
        let extracted = extract_response_text(candidate);
        if !extracted.is_empty() {
            turn.human_response_text = extracted;
            break;
        }
    }
    if parsed.get("hook_event_name") == Some(&Value::String("UserPromptSubmit".into()))
        && !parsed.get("tool_name").is_some_and(Value::is_string)
    {
        turn.prompt_submitted = true;
        turn.typed_prompt = [prompt.as_ref(), user_prompt.as_ref(), message.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|text| !trim(text).is_empty())
            .unwrap_or("")
            .to_owned();
    } else if tool_response.is_some() {
        turn.picker_unanswered = empty_picker_result(tool_response);
        turn.picked = if turn.picker_unanswered {
            Vec::new()
        } else {
            picked_labels(tool_response)
        };
        turn.picker = Some(PickerQuestion {
            options: extract_option_labels(tool_input),
            several_picks: carries_several_picks(tool_input, tool_response),
        });
    }
    if let Some(supplied) = supplied_turn_origin(parsed.get("origin")) {
        turn.origin = supplied;
    } else if turn.prompt_submitted
        && !turn.typed_prompt.is_empty()
        && parsed.get("transcript_path").is_some_and(Value::is_string)
    {
        // Claude Code hands its transcript to the hook: its row for this
        // prompt names who started the turn. With no row, only the whole
        // notice sentence counts as the host's.
        turn.origin = claude_transcript_origin(
            parsed.get("transcript_path"),
            parsed.get("prompt_id"),
            &turn.typed_prompt,
        )
        .or_else(|| host_template_origin(&turn.typed_prompt))
        .unwrap_or(TurnOrigin::Person { recorded: false });
    }
    turn
}

/// `hookContextLine("UserPromptSubmit", …)`.
fn context_line(context: &str) -> String {
    format!(
        "{}\n",
        js_json::stringify(&json!({
            "additionalContext": context,
            "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": context },
        }))
    )
}

fn sessions_dir(layout: &Layout) -> PathBuf {
    layout.workspace_root().join(SESSIONS_DIR)
}

/// `runtimeSessionSegment`.
fn session_segment(session: &str) -> String {
    focus_intent::create::runtime_session_segment(session)
}

/// What this slice does not read yet, said by name before anything is
/// written: the records of the plan approval question, the directive
/// marker's asks, the lines queued for the person.
fn refuse_later_records(
    layout: &Layout,
    selection: Option<&Selection>,
    session: &str,
) -> Result<(), EngineError> {
    let sessions = sessions_dir(layout);
    if !session.is_empty() {
        let person_lines = sessions.join(format!("{session}.person-lines"));
        if person_lines.exists() {
            return Err(EngineError::new(format!(
                "{} holds the lines queued for the person to hear on the engine's next step, which this turn would carry forward: Focus does not keep person lines yet.",
                person_lines.display()
            )));
        }
        let segment = session_segment(session);
        if !segment.is_empty() {
            let challenge = sessions
                .join("plan-approval")
                .join(format!("challenge-{segment}.json"));
            if challenge.exists() {
                return Err(EngineError::new(format!(
                    "{} is the plan approval challenge this chat must answer, which this turn would answer: Focus does not read plan approval challenges yet.",
                    challenge.display()
                )));
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(sessions.join("plan-approval")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with("protected-question-") && name.ends_with(".json") {
                return Err(EngineError::new(format!(
                    "{} is a protected question a chat asked (a checkpoint, a verification command, a policy), which this turn may answer: Focus does not read protected questions yet.",
                    entry.path().display()
                )));
            }
        }
    }
    if let Some(selection) = selection {
        let marker = selection.engine_dir(layout).join("active-directive.json");
        if marker.symlink_metadata().is_ok() {
            return Err(EngineError::new(format!(
                "{} is the orchestrator's directive marker, whose asks (the plan approval question, a guard-recovery ask) this turn would answer: Focus does not read it yet.",
                marker.display()
            )));
        }
    }
    Ok(())
}

/// `stoppedReviewNote`'s question: a review asked for in this workflow has
/// no verdict. Whether it still waits is the receipts' answer (M5), so a
/// turn that would carry the note is refused by name.
fn refuse_open_review(
    layout: &Layout,
    wf: &Workflow,
    selection: &Selection,
) -> Result<(), EngineError> {
    let mut open: Vec<(String, String)> = Vec::new();
    for row in focus_lifecycle::gate::sort_attempt_events(&wf.events()) {
        if PAST_REVIEW_REQUEST.contains(&row.event.as_str()) {
            open.clear();
            continue;
        }
        if row.event != "REVIEW_REQUESTED" && row.event != "REVIEW_COMPLETED" {
            continue;
        }
        if block_field(&row.block, "Workflow").is_some_and(|w| w.starts_with("single-stage:")) {
            continue;
        }
        let Some(stage) = block_field(&row.block, "Stage") else {
            continue;
        };
        let unit = block_field(&row.block, "Unit").unwrap_or("");
        let key = format!(
            "{stage}\0{unit}\0{}",
            block_field(&row.block, "Iteration").unwrap_or("")
        );
        if row.event == "REVIEW_REQUESTED" {
            if !open.iter().any(|(k, _)| *k == key) {
                open.push((key, stage.to_owned()));
            }
        } else {
            open.retain(|(k, _)| *k != key);
        }
    }
    if open.is_empty() {
        return Ok(());
    }
    let Ok(state) = wf.read_state() else {
        return Ok(());
    };
    let Ok(stages) = focus_graph::runtime::load_stages_all(&layout.stage_graph()) else {
        return Ok(());
    };
    for (_, slug) in open {
        let Some(stage) = stages.iter().find(|stage| stage.slug == slug) else {
            continue;
        };
        if focus_lifecycle::stage::reviewer(stage).is_none() {
            continue;
        }
        let class = focus_lifecycle::admission::resolve_review_class(wf, stage, &state);
        if class == "none" {
            continue;
        }
        return Err(EngineError::new(format!(
            "the ledger of {} holds a review request for \"{slug}\" with no verdict: whether it still awaits one is the review receipt scan's answer, and Focus does not weigh review receipts yet. Record the review's outcome, or run the step that finishes it.",
            selection.docs_root(layout).display()
        )));
    }
    Ok(())
}

/// `normalizeRetiredGuardPolicyField`: rename an active intent's valid
/// retired policy field without changing its stored value, source label,
/// effective policy or audit history. Answers the value kept.
fn normalize_retired_guard_policy_field(layout: &Layout, wf: &Workflow) -> Option<String> {
    let content = wf.read_state().ok()?;
    if get_field(&content, focus_guards::policy::GUARD_POLICY_FIELD).is_some() {
        return None;
    }
    let retired = get_field(&content, focus_guards::policy::CHANGE_CONTROL_FIELD)?;
    let parsed = focus_guards::policy::parse_state_line(Some(retired))?;
    let value = match parsed.value {
        focus_guards::GuardPolicy::Relaxed => "relaxed",
        focus_guards::GuardPolicy::Off => "off",
        focus_guards::GuardPolicy::Strict => return None,
    };
    let _held = wf.lock("renaming the retired Change Control field").ok()?;
    let read = || Guards::read(layout, &Selector::default(), &wf.invoker).ok();
    let before = read()?.with_state(&content).policy(true).ok()?.value;
    let updated = focus_guards::policy::set_guard_policy_line(&content, retired);
    let after = read()?.with_state(&updated).policy(true).ok()?.value;
    if before != after {
        return None;
    }
    wf.write_state(&updated).ok()?;
    Some(value.to_owned())
}

/// `markHumanTurn`: the `human-turn` marker under the record's engine
/// directory, dated now; nothing without a created workflow.
fn mark_human_turn(layout: &Layout, selection: &Selection) {
    let record_root = selection.docs_root(layout);
    if !record_root
        .join(focus_workspace::names::STATE_FILE)
        .exists()
    {
        return;
    }
    let relative = Path::new(ENGINE_DIR).join("human-turn");
    let Ok(anchor) = std::fs::canonicalize(&record_root) else {
        return;
    };
    if links::refuse_link_in_chain(&anchor, &relative).is_err() {
        return;
    }
    let target = anchor.join(&relative);
    if let Some(parent) = target.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return;
    }
    if links::refuse_link_in_chain(&anchor, &relative).is_err() {
        return;
    }
    let _ = std::fs::write(target, format!("{}\n", iso_timestamp(SystemTime::now())));
}

/// `recordGateWords`: keep one message the person typed in this chat, and
/// answer the offset it was kept under (`None` when nothing was kept).
/// Called with the audit lock held, after the `HUMAN_TURN` row.
fn record_gate_words(
    layout: &Layout,
    wf: &Workflow,
    selection: &Selection,
    session: &str,
    text: &str,
) -> Result<Option<u64>, EngineError> {
    let words = trim(text);
    let segment = session_segment(session);
    if words.is_empty() || segment.is_empty() {
        return Ok(None);
    }
    let shard_path = wf.ledger().audit_file(&wf.selector(), &wf.invoker)?;
    let offset = std::fs::metadata(&shard_path)
        .map_err(|err| EngineError::new(format!("{}: {err}", shard_path.display())))?
        .len();
    let shard = shard_path
        .strip_prefix(&layout.project)
        .unwrap_or(&shard_path)
        .to_string_lossy()
        .replace('\\', "/");
    let dir = selection.engine_dir(layout).join("gate-words");
    let path = dir.join(format!("{segment}.json"));
    let prior = std::fs::read(&path)
        .ok()
        .and_then(|bytes| js_json::parse(&String::from_utf8_lossy(&bytes)).ok())
        .filter(|record| record.get("shard").and_then(Value::as_str) == Some(shard.as_str()));
    let mut dropped: u64 = prior
        .as_ref()
        .and_then(|record| record.get("dropped"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut messages: Vec<Value> = prior
        .as_ref()
        .and_then(|record| record.get("messages"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let over = len_utf16(words) > GATE_WORDS_MAX_CHARS;
    if over {
        dropped = offset;
    } else {
        messages.push(json!({ "offset": offset, "text": words }));
    }
    while messages.len() > GATE_WORDS_MAX_MESSAGES {
        let shifted = messages.remove(0);
        dropped = dropped.max(shifted.get("offset").and_then(Value::as_u64).unwrap_or(0));
    }
    let record = json!({
        "version": 1,
        "session": session,
        "shard": shard,
        "dropped": dropped,
        "messages": messages,
    });
    let rel = path
        .strip_prefix(&layout.project)
        .map_err(|_| EngineError::new("the gate-words record is outside the project"))?
        .to_path_buf();
    links::refuse_link_in_chain(&layout.project, &rel)?;
    std::fs::create_dir_all(&dir)
        .map_err(|err| EngineError::new(format!("{}: {err}", dir.display())))?;
    observer::refuse_write(observer::WRITE_FILE_ATOMIC)?;
    focus_workspace::atomic::write_atomic(
        &path,
        format!("{}\n", js_json::stringify(&record)).as_bytes(),
    )
    .map_err(|err| EngineError::new(format!("{}: {err}", path.display())))?;
    Ok(if over { None } else { Some(offset) })
}

/// `pickerFreeText`: the words a person typed into a single-choice picker's
/// free-text field. A pick of one of the offered labels is the conductor's
/// wording, not theirs.
fn picker_free_text(text: &str, picker: Option<&PickerQuestion>) -> String {
    let Some(picker) = picker else {
        return String::new();
    };
    let Some(options) = &picker.options else {
        return String::new();
    };
    if text.is_empty() || picker.several_picks {
        return String::new();
    }
    let typed = focus_js::text::to_lower_case(&strip_recommended_decorator(text));
    if options
        .iter()
        .any(|label| focus_js::text::to_lower_case(&strip_recommended_decorator(label)) == typed)
    {
        String::new()
    } else {
        text.to_owned()
    }
}

/// `pickedGateLabel`: a stage-gate choice picked in the picker is the
/// person's exact pick, kept like a typed one. Only a picker that offers
/// the gate's own two choices is a gate picker.
fn picked_gate_label(text: &str, picker: Option<&PickerQuestion>) -> String {
    let Some(picker) = picker else {
        return String::new();
    };
    let Some(options) = &picker.options else {
        return String::new();
    };
    if text.is_empty() || picker.several_picks {
        return String::new();
    }
    let offered: Vec<String> = options
        .iter()
        .map(|option| focus_js::text::to_lower_case(trim(&strip_recommended_decorator(option))))
        .collect();
    if !offered.iter().any(|o| o == "approve") || !offered.iter().any(|o| o == "request changes") {
        return String::new();
    }
    let label = trim(&strip_recommended_decorator(text)).to_owned();
    if ["approve", "request changes", "accept as-is"]
        .contains(&focus_js::text::to_lower_case(&label).as_str())
    {
        label
    } else {
        String::new()
    }
}

fn exit_with_notes(code: i32, notes: &[String], stdout: String) -> Outcome {
    let mut out = stdout;
    if !notes.is_empty() {
        out.push_str(&context_line(&notes.join("\n")));
    }
    Outcome {
        code,
        stdout: out,
        stderr: String::new(),
    }
}

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let parsed = payload::parse(input);
    let turn = read_turn(parsed.as_ref());
    let mut notes: Vec<String> = Vec::new();
    // A new prompt starts a new turn: a one-shot stop left from an earlier
    // turn is spent here, before any early return.
    if turn.prompt_submitted && !turn.session.is_empty() {
        let _ = std::fs::remove_file(
            sessions_dir(layout).join(format!("{}.handoff.json", turn.session)),
        );
    }
    // A conversation that has not joined the selected workflow is not a
    // human at its gates: it mints nothing there and its typed switches do
    // not reach it.
    let workflow = match context::enter(layout, Some(&turn.session)) {
        Ok(workflow) => workflow,
        Err(err) => return refused(&err),
    };
    if workflow.stands_outside() {
        if !turn.typed_prompt.is_empty()
            && is_typed_guard_switch_prompt(&turn.typed_prompt)
            && workflow
                .selection
                .as_ref()
                .is_some_and(|selection| selection.intent.is_some())
        {
            notes.push("FOCUS Guard Policy: the typed switch was not applied because this conversation has not joined the selected workflow; select its intent with the intent command first.".to_owned());
        }
        return exit_with_notes(0, &notes, String::new());
    }
    let selection = workflow.selection.clone();
    let wf = selection
        .as_ref()
        .filter(|selection| selection.intent.is_some())
        .and_then(|selection| open(layout, selection, &workflow.invoker).ok());
    let state_exists = wf.as_ref().is_some_and(|wf| wf.read_state().is_ok());
    // A field-only rename preserves the stored and effective value, so it
    // carries no switch authority.
    if turn.prompt_submitted
        && !turn.session.is_empty()
        && let Some(wf) = &wf
        && let Some(value) = normalize_retired_guard_policy_field(layout, wf)
    {
        notes.push(format!(
            "FOCUS Guard Policy migration: kept {value} and renamed the active intent's retired Change Control field to Guard Policy."
        ));
    }
    // The heartbeat says the host ran this hook. Before any workflow it
    // lands where doctor and `next` look then.
    let health_dir = match &selection {
        Some(selection) if selection.intent.is_some() => hooks_health_dir(layout, selection),
        Some(selection) => layout
            .intents_dir(&selection.space)
            .join(ENGINE_DIR)
            .join("hooks-health"),
        None => layout
            .intents_dir(&layout.active_space())
            .join(ENGINE_DIR)
            .join("hooks-health"),
    };
    let now = || iso_timestamp(SystemTime::now());
    if selection
        .as_ref()
        .is_none_or(|selection| selection.intent.is_none())
    {
        write_status_file(&health_dir, "record-human-turn.last", &now(), false);
    }
    let mint_allowed = focus_workspace::env::var("UNATTENDED").as_deref() != Some("1");
    if !mint_allowed
        && !turn.typed_prompt.is_empty()
        && is_typed_guard_switch_prompt(&turn.typed_prompt)
    {
        notes.push("FOCUS Guard Policy: the typed switch was not applied because FOCUS_UNATTENDED=1 withholds human authority on this driver; run it from an attended session.".to_owned());
    }
    // A turn the host made: none of the person's authority rides on it. The
    // record keeps that it happened and why, with no text.
    if let TurnOrigin::Host { reason, source } = &turn.origin {
        if state_exists && let Some(wf) = &wf {
            write_status_file(&health_dir, "record-human-turn.last", &now(), false);
            let mut fields: Vec<(&str, String)> = Vec::new();
            if !turn.session.is_empty() {
                fields.push(("Session", turn.session.clone()));
            }
            fields.push(("Origin", "host".to_owned()));
            fields.push(("Reason", reason.clone()));
            fields.push(("Source", source.name().to_owned()));
            if let Err(err) = super::append(wf, "HOST_TURN", &fields)
                && is_refusal(&err)
            {
                return refused(&err);
            }
        }
        return exit_with_notes(0, &notes, String::new());
    }
    // What this slice does not do yet is said by name before anything is
    // written, so a refused turn leaves no half record.
    if let Err(err) = refuse_later_records(layout, selection.as_ref(), &turn.session) {
        return refused(&err);
    }
    if mint_allowed && !turn.session.is_empty() && !turn.typed_prompt.is_empty() {
        let request =
            focus_guards::typed_switch::parse_typed_guard_switch_request(&turn.typed_prompt, false);
        if !request.switches.is_empty() || request.error.is_some() {
            return refused(&EngineError::new(
                "the prompt is a typed guard switch, which the hook applies to the piece of work this chat has open through the config setter: Focus does not apply typed guard switches yet. Type the setting with `focus engine config set` instead.",
            ));
        }
    }
    if mint_allowed && !turn.typed_prompt.is_empty() {
        if let Some(words) = entry_words(&turn.typed_prompt)
            && !words.is_empty()
        {
            return refused(&EngineError::new(
                "the prompt is a `/focus` line with words after the entry, which the orchestrator's parser reads for its flags and the person's words: Focus does not read `/focus` lines typed in chat yet.",
            ));
        }
        if re::test(PLAN_APPROVAL_OVERRIDE_PHRASE, "i", trim(&turn.typed_prompt)) {
            return refused(&EngineError::new(
                "the prompt is the plan approval override phrase, which the hook records as a break-glass request for the answer to pair with: Focus does not record plan approval overrides yet.",
            ));
        }
    }
    if mint_allowed
        && turn.prompt_submitted
        && !trim(&turn.typed_prompt).is_empty()
        && let (Some(wf), Some(selection)) = (&wf, &selection)
        && state_exists
        && let Err(err) = refuse_open_review(layout, wf, selection)
    {
        return refused(&err);
    }
    // The message record: proof that a person said this in this chat, and
    // exactly what they said. Written before the state-file gate so a
    // message before any work exists is kept.
    let mut message_id: Option<String> = None;
    if mint_allowed && !turn.picker_unanswered && (turn.prompt_submitted || turn.picker.is_some()) {
        let (words, settings, route) = if turn.prompt_submitted {
            // A plain prompt is words; a bare entry is a command with none.
            // (A line with words after the entry was refused above.)
            match entry_words(&turn.typed_prompt) {
                Some(_) => (None, Vec::new(), MessageRoute::default()),
                None => (
                    Some(trim(&turn.typed_prompt).to_owned()).filter(|w| !w.is_empty()),
                    Vec::new(),
                    MessageRoute::default(),
                ),
            }
        } else {
            (
                Some(trim(&turn.human_response_text).to_owned()).filter(|w| !w.is_empty()),
                Vec::new(),
                MessageRoute::default(),
            )
        };
        let full = if turn.prompt_submitted {
            turn.typed_prompt.as_str()
        } else {
            turn.human_response_text.as_str()
        };
        let (picker, picker_cut) = if turn.prompt_submitted {
            (None, false)
        } else {
            let (entries, cut) = picker_replies(
                parsed.as_ref().unwrap_or(&Map::new()),
                Some(MESSAGE_PICKER_MAX_CHARS),
            );
            (
                Some(
                    entries
                        .into_iter()
                        .map(|entry| (entry.question, entry.reply))
                        .collect::<Vec<_>>(),
                ),
                cut,
            )
        };
        let cut_words = words
            .as_deref()
            .map(|w| len_utf16(w) > MESSAGE_TEXT_MAX_CHARS)
            .unwrap_or(false);
        let words = words.map(|w| {
            if len_utf16(&w) > MESSAGE_TEXT_MAX_CHARS {
                slice_utf16(&w, 0, MESSAGE_TEXT_MAX_CHARS)
            } else {
                w
            }
        });
        let cut = len_utf16(full) > MESSAGE_TEXT_MAX_CHARS || cut_words || picker_cut;
        let message = Message {
            session: (!turn.session.is_empty()).then(|| turn.session.clone()),
            at: now(),
            source: if turn.prompt_submitted {
                "prompt"
            } else {
                "picker"
            },
            text: slice_utf16(full, 0, MESSAGE_TEXT_MAX_CHARS),
            cut,
            picker,
            words,
            settings,
            route,
            applied: Vec::new(),
        };
        match save_message(layout, &message) {
            Ok(id) => message_id = Some(id),
            Err(err) if is_refusal(&err) => return refused(&err),
            Err(_) => {}
        }
    }
    let (Some(wf), Some(selection)) = (&wf, &selection) else {
        return exit_with_notes(0, &notes, String::new());
    };
    if !state_exists {
        return exit_with_notes(0, &notes, String::new());
    }
    // Inside a workflow the heartbeat lands in the record, before the mint
    // branch and apart from it.
    write_status_file(&health_dir, "record-human-turn.last", &now(), false);
    if turn.picker_unanswered {
        // No turn and no answer: the row spends any earlier turn.
        let fields: Vec<(&str, String)> = if turn.session.is_empty() {
            Vec::new()
        } else {
            vec![("Session", turn.session.clone())]
        };
        if let Err(err) = super::append(wf, "QUESTION_UNANSWERED", &fields)
            && is_refusal(&err)
        {
            return refused(&err);
        }
        let stdout = format!(
            "{}\n",
            js_json::stringify(&json!({
                "hookSpecificOutput": { "hookEventName": "PostToolUse", "additionalContext": QUESTION_UNANSWERED_NOTICE },
            }))
        );
        return exit_with_notes(0, &notes, stdout);
    }
    if mint_allowed {
        let typed = !turn.typed_prompt.is_empty();
        // A typed guard switch or break-glass request is an instruction to
        // the framework, not an answer; a question about a switch is for the
        // agent. (Both the switch and the phrase were refused above.)
        let switch_question = typed && is_typed_guard_switch_question(&turn.typed_prompt);
        // Words typed after the entry would be a reply; a line with them was
        // refused above, so the entry reply is never there.
        let not_a_reply = typed && (is_command_prompt(&turn.typed_prompt) || switch_question);
        let outcome = (|| -> Result<(), EngineError> {
            let held = wf.lock("recording the human turn")?;
            let mut fields: Vec<(&str, String)> = Vec::new();
            if !turn.session.is_empty() {
                fields.push(("Session", turn.session.clone()));
            }
            fields.push(("Origin", "person".to_owned()));
            if turn.prompt_submitted {
                fields.push(("Source", "typed".to_owned()));
            } else if turn.picker.is_some() {
                fields.push(("Source", "picker".to_owned()));
            }
            if switch_question {
                fields.push(("Reply", QUESTION_TURN_REPLY.to_owned()));
            } else if not_a_reply {
                fields.push(("Reply", COMMAND_TURN_REPLY.to_owned()));
            }
            if !turn.picked.is_empty() {
                fields.push(("Picked", js_json::stringify(&json!(turn.picked))));
            }
            if let Some(id) = &message_id {
                fields.push(("Message Id", id.clone()));
            }
            wf.emit_plain(&held, "HUMAN_TURN", &fields)?;
            // Keep what the person typed in this chat, so a decision at a
            // stage gate records their own words beside the conductor's
            // reading. Never blocks the turn.
            let typed_words = if typed {
                if not_a_reply {
                    String::new()
                } else {
                    turn.typed_prompt.clone()
                }
            } else {
                let free = picker_free_text(&turn.human_response_text, turn.picker.as_ref());
                if free.is_empty() {
                    picked_gate_label(&turn.human_response_text, turn.picker.as_ref())
                } else {
                    free
                }
            };
            if !turn.session.is_empty() && !typed_words.is_empty() {
                match record_gate_words(layout, wf, selection, &turn.session, &typed_words) {
                    Err(err) if is_refusal(&err) => return Err(err),
                    _ => {}
                }
            }
            // What the question box carried back goes on the record,
            // question by question. It decides nothing and spends no turn.
            if turn.picker.is_some() && !not_a_reply {
                let (entries, _) = picker_replies(parsed.as_ref().unwrap_or(&Map::new()), None);
                for entry in entries {
                    let Some(reply) = entry.reply else {
                        continue;
                    };
                    let mut fields: Vec<(&str, String)> = Vec::new();
                    if !turn.session.is_empty() {
                        fields.push(("Session", turn.session.clone()));
                    }
                    fields.push(("Question", entry.question));
                    fields.push(("Reply", reply));
                    wf.emit_plain(&held, "QUESTION_REPLIED", &fields)?;
                }
            }
            Ok(())
        })();
        if let Err(err) = outcome
            && is_refusal(&err)
        {
            return refused(&err);
        }
    }
    mark_human_turn(layout, selection);
    exit_with_notes(0, &notes, String::new())
}

//! SubagentStop: emit `SUBAGENT_COMPLETED` when a subagent finishes.
//! Mirrors `aidlc-log-subagent.ts`.

use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_state::fields::get_field;
use focus_workspace::Layout;
use focus_workspace::records::{Invoker, Selector, resolve_selection, valid_session_id};
use serde_json::Value;

use super::{append, dropped, is_refusal, open, refused};
use crate::context::{Participation, participation, pinned};
use crate::engine::refuse_subagent_ledger;
use crate::health::{hooks_health_dir, write_status_file};
use crate::{Outcome, payload};

pub fn run(input: &str, layout: &Layout) -> Outcome {
    // The payload is read before workflow resolution: completion removes
    // only the finishing session's in-flight entry, even when that session
    // no longer has a running workflow to audit.
    let Some(parsed) = payload::parse(input) else {
        return Outcome::exit(0);
    };
    let raw_session = parsed.get("session_id");
    let session_id: Option<String> = raw_session
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .and_then(|id| valid_session_id(Some(id)))
        .map(str::to_owned);
    // `completeSubagentInflight`: a ledger of subagents in flight is M5's.
    if let Err(err) = refuse_subagent_ledger(layout) {
        return refused(&err);
    }
    // The payload's session is what every path helper resolves with from
    // here on (upstream threads the resolved space and intent explicitly).
    let invoker = session_id.as_deref().map_or_else(Invoker::from_env, pinned);
    let selector = Selector {
        session_id: session_id.clone(),
        ..Selector::default()
    };
    let selection = match resolve_selection(layout, &selector, &invoker) {
        Ok(selection) => selection,
        Err(err) if is_refusal(&err) => return refused(&err),
        Err(_) => return Outcome::exit(0),
    };
    let wf = match open(layout, &selection, &invoker) {
        Ok(wf) => wf,
        Err(err) => return refused(&err),
    };
    let Ok(state) = wf.read_state() else {
        return Outcome::exit(0);
    };
    if get_field(&state, "Status") != Some("Running") {
        return Outcome::exit(0);
    }
    // A conversation that has not joined this workflow records nothing in it.
    if selection.intent.is_some() {
        match participation(layout, &selection) {
            Ok(Participation::Participant) => {}
            Ok(_) => return Outcome::exit(0),
            Err(err) => return refused(&err),
        }
    }
    write_status_file(
        &hooks_health_dir(layout, &selection),
        "log-subagent.last",
        &iso_timestamp(SystemTime::now()),
        false,
    );
    let agent_type = match parsed.get("agent_type").and_then(Value::as_str) {
        Some(text) if !focus_js::text::trim(text).is_empty() => text.to_owned(),
        _ => "unknown".to_owned(),
    };
    // `agent_id ?? ""`, written when truthy: so a number or an array is
    // written as the runtime prints it, and 0 or false is left out.
    let agent_id = parsed
        .get("agent_id")
        .filter(|value| payload::truthy(Some(value)))
        .map(payload::js_string);
    // `(message ?? "").slice(0, 200)`: text or an array (which slices to
    // its first items, then prints as text); anything else upstream falls
    // over on, and Focus refuses by name. The slice counts UTF-16 units; a
    // pair it splits is written as the replacement character, as the
    // runtime writes a lone surrogate.
    // Written when the sliced value is truthy: an array always is, even
    // empty; text is when it has characters.
    let message = match parsed.get("last_assistant_message") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(String::from_utf16_lossy(
            &text.encode_utf16().take(200).collect::<Vec<u16>>(),
        ))
        .filter(|text| !text.is_empty()),
        Some(Value::Array(items)) => Some(payload::js_string(&Value::Array(
            items.iter().take(200).cloned().collect(),
        ))),
        Some(other) => {
            return refused(&focus_js::EngineError::new(format!(
                "log-subagent: the payload's last_assistant_message is not text ({}).",
                focus_js::json::stringify(other)
            )));
        }
    };
    let mut fields: Vec<(&str, String)> = vec![("Agent Type", agent_type)];
    if let Some(agent_id) = agent_id {
        fields.push(("Agent ID", agent_id));
    }
    if let Some(message) = message {
        fields.push(("Message", message));
    }
    if let Err(err) = append(&wf, "SUBAGENT_COMPLETED", &fields)
        && let Some(outcome) = dropped(layout, &selection, "log-subagent", &err)
    {
        return outcome;
    }
    Outcome::exit(0)
}

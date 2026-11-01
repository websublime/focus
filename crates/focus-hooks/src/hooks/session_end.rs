//! SessionEnd: emit `SESSION_ENDED` when a conversation ends. The workflow
//! lifecycle is independent of the session's: ending a session does not
//! complete the workflow; this is observability only. Mirrors
//! `aidlc-session-end.ts`.

use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_workspace::Layout;
use focus_workspace::names::STATE_FILE;
use focus_workspace::records::{
    Invoker, Selection, Selector, read_session_binding, resolve_selection, valid_session_id,
};

use super::{append, dropped, is_refusal, open, refused};
use crate::context::{Participation, participation, pinned};
use crate::health::{hooks_health_dir, record_drop, write_status_file};
use crate::registry::{active_intent_uuid, find_intent_by_uuid, session_intent_uuid};
use crate::{Outcome, payload};

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let parsed = payload::parse(input);
    let mut reason = "unknown".to_owned();
    let mut session_id = String::new();
    if let Some(map) = &parsed {
        if payload::truthy(map.get("reason")) {
            reason = payload::js_string(map.get("reason").expect("a truthy value"));
        }
        if let Some(raw) = map.get("session_id").and_then(serde_json::Value::as_str) {
            session_id = valid_session_id(Some(raw)).unwrap_or("").to_owned();
        }
    }

    let mut intent: Option<String> = None;
    let mut space: Option<String> = None;
    // The payload's session is what every path helper resolves with
    // (upstream threads the resolved space and intent explicitly).
    let invoker = if session_id.is_empty() {
        Invoker::from_env()
    } else {
        pinned(&session_id)
    };
    // One identity per session: a binding that names a record decides where
    // the end goes, and a binding to no record means an older stamp is not
    // read either.
    let binding = if session_id.is_empty() {
        None
    } else {
        read_session_binding(layout, &session_id)
    };
    if let Some(bound) = &binding {
        if let Some(bound_intent) = &bound.intent {
            intent = Some(bound_intent.clone());
            space = Some(bound.space.clone());
        } else {
            // No record takes the end of a session bound to none; only a
            // flat workspace's root workflow can. The pinned session makes
            // every path helper below read this binding rather than the
            // shared cursor.
            space = Some(bound.space.clone());
        }
    } else if !session_id.is_empty() {
        if let Some(stamped) = session_intent_uuid(layout, &session_id) {
            match find_intent_by_uuid(layout, &stamped) {
                None => {
                    // The drop goes where the cursor points (`hooksHealthDir`
                    // with no record named); a cursor that cannot be read
                    // loses the drop, as upstream's `recordHookDrop` swallows
                    // the throw.
                    if let Ok(selection) =
                        resolve_selection(layout, &Selector::default(), &Invoker::default())
                    {
                        record_drop(
                            layout,
                            &selection,
                            "session-end",
                            &format!(
                                "session {session_id} is stamped to unknown intent {stamped}; refusing active-cursor fallback"
                            ),
                        );
                    }
                    return Outcome::exit(0);
                }
                Some(found) => {
                    intent = Some(found.dir_name);
                    space = Some(found.space);
                }
            }
        } else if active_intent_uuid(layout, None).is_some() {
            // A UUID-backed workflow has exact per-session ownership; missing
            // identity fails closed.
            return Outcome::exit(0);
        }
    }

    // A conversation that has not joined the workflow does not end a session
    // in it.
    let ended = match (&intent, &space) {
        (Some(intent), Some(space)) => Selection {
            space: space.clone(),
            intent: Some(intent.clone()),
            session_id: (!session_id.is_empty()).then(|| session_id.clone()),
            binding: binding.clone(),
        },
        _ => {
            let selector = Selector {
                session_id: (!session_id.is_empty()).then(|| session_id.clone()),
                ..Selector::default()
            };
            match resolve_selection(layout, &selector, &invoker) {
                Ok(selection) => selection,
                Err(err) if is_refusal(&err) => return refused(&err),
                Err(_) => return Outcome::exit(0),
            }
        }
    };
    // A binding that records staying out of a workflow outweighs a stamp.
    if ended.binding.as_ref().and_then(|b| b.source.as_deref()) == Some("unjoined") {
        return Outcome::exit(0);
    }
    if ended.intent.is_some() {
        match participation(layout, &ended) {
            Ok(Participation::Participant) => {}
            Ok(_) => return Outcome::exit(0),
            Err(err) => return refused(&err),
        }
    }

    // The record the end goes to: the session-owned intent, else the
    // resolved one.
    let target = match (&intent, &space) {
        (Some(intent), Some(space)) => Selection {
            space: space.clone(),
            intent: Some(intent.clone()),
            session_id: None,
            binding: None,
        },
        _ => {
            let selector = Selector {
                space: space.clone(),
                ..Selector::default()
            };
            match resolve_selection(layout, &selector, &invoker) {
                Ok(selection) => selection,
                Err(err) if is_refusal(&err) => return refused(&err),
                Err(_) => return Outcome::exit(0),
            }
        }
    };
    if !target.docs_root(layout).join(STATE_FILE).exists() {
        return Outcome::exit(0);
    }
    // Health heartbeat follows the same session-owned intent as the audit
    // event.
    write_status_file(
        &hooks_health_dir(layout, &target),
        "session-end.last",
        &iso_timestamp(SystemTime::now()),
        false,
    );
    let wf = match open(layout, &target, &invoker) {
        Ok(wf) => wf,
        Err(err) => return refused(&err),
    };
    if let Err(err) = append(&wf, "SESSION_ENDED", &[("Reason", reason)])
        && let Some(outcome) = dropped(layout, &target, "session-end", &err)
    {
        return outcome;
    }
    Outcome::exit(0)
}

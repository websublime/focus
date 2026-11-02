//! This crate's share of the oracle corpus (`tools/oracle/driver.ts` asks
//! upstream the same cases): the readers of a prompt-submit payload that
//! `aidlc-lib.ts` exports, so they can be compared one call at a time.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::turn::{
    TurnOrigin, claude_transcript_origin, empty_picker_result, host_envelope_turn_text,
    host_template_origin, supplied_turn_origin,
};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 5] = [
    "host_envelope",
    "host_template",
    "supplied_origin",
    "picker_empty",
    "transcript_origin",
];

static TREES: AtomicU64 = AtomicU64::new(0);

fn origin_json(origin: Option<TurnOrigin>) -> Value {
    match origin {
        None => Value::Null,
        Some(TurnOrigin::Person { recorded }) => {
            if recorded {
                json!({ "kind": "person", "source": "record" })
            } else {
                json!({ "kind": "person" })
            }
        }
        Some(TurnOrigin::Host { reason, source }) => {
            json!({ "kind": "host", "reason": reason, "source": source.name() })
        }
    }
}

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    match case.get("kind").and_then(Value::as_str).unwrap_or_default() {
        "host_envelope" => json!(host_envelope_turn_text(text("prompt"))),
        "host_template" => origin_json(host_template_origin(text("prompt"))),
        "supplied_origin" => origin_json(supplied_turn_origin(case.get("origin"))),
        "picker_empty" => json!(empty_picker_result(case.get("response"))),
        "transcript_origin" => {
            // The transcript the case describes, written under a scratch
            // root of this process's own; `{P}` in the path names it.
            let root = std::env::temp_dir().join(format!(
                "focus-oracle-transcript-{}-{}",
                std::process::id(),
                TREES.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("a scratch root");
            let rel = text("file");
            let path: PathBuf = root.join(rel);
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Some(rows) = case.get("rows").and_then(Value::as_str) {
                std::fs::write(&path, rows).expect("the transcript");
            }
            let transcript = case.get("transcript_path").map(|value| match value {
                Value::String(given) => {
                    Value::String(given.replace("{P}", &root.to_string_lossy()))
                }
                other => other.clone(),
            });
            let answer = origin_json(claude_transcript_origin(
                transcript.as_ref(),
                case.get("prompt_id"),
                text("prompt"),
            ));
            let _ = std::fs::remove_dir_all(&root);
            answer
        }
        other => json!({ "unsupported": other }),
    }
}

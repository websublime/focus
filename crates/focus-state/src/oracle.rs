//! The oracle protocol for the state file's readers (see `focus-graph`'s
//! `oracle` module for the protocol).

use serde_json::{Value, json};

use crate::progress::{parse_checkboxes, parse_stage_suffixes};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 1] = ["progress"];

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    match text("kind") {
        "progress" => {
            let boxes: Vec<Value> = parse_checkboxes(text("content"))
                .into_iter()
                .map(|line| json!({ "slug": line.slug, "state": line.state.name(), "suffix": line.suffix }))
                .collect();
            let actions: Vec<Value> = parse_stage_suffixes(text("content"))
                .into_iter()
                .map(|(slug, action)| json!([slug, action]))
                .collect();
            json!({ "checkboxes": boxes, "suffixes": actions })
        }
        other => json!({ "unsupported": other }),
    }
}

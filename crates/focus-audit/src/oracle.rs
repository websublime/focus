//! The oracle protocol for the ledger's functions.
//!
//! Each case names a `kind` and its inputs; [`run_case`] answers it in the
//! shape `tools/oracle/driver.ts` produces from upstream's own code, and the
//! two answers are compared as text (see `focus-graph`'s `oracle` module for
//! the protocol).
//!
//! An entry case never touches the disk here. Upstream can only be asked to
//! render an entry by having it append one to a shard, so the driver does
//! that in a scratch project and reads the shard back; this side builds the
//! same bytes from the title and the block. A value names the scratch project
//! as `{P}`, which each side replaces with a project location of its own and
//! puts back in its answer.

use std::path::PathBuf;

use serde_json::{Map, Value, json};

use crate::entry::{Entry, Fields, SHARD_TITLE, render_block, validate};
use crate::read;

/// The kinds of case this crate answers.
pub const KINDS: [&str; 6] = [
    "audit_entry",
    "audit_find",
    "audit_field",
    "bullet_field",
    "unsafe_line",
    "field_args",
];

const PROJECT: &str = "{P}";

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    match text("kind") {
        "audit_entry" => entry(case, text("eventType")),
        "audit_find" => {
            let slug = case.get("slug").and_then(Value::as_str);
            let found: Vec<Value> = read::find_all_events(text("audit"), text("event"), slug)
                .into_iter()
                .map(|found| json!({ "timestamp": found.timestamp, "block": found.block }))
                .collect();
            json!({ "found": found })
        }
        "audit_field" => json!({ "value": read::block_field(text("block"), text("name")) }),
        "bullet_field" => json!({ "value": read::bullet_field(text("content"), text("field")) }),
        "unsafe_line" => json!({ "unsafe": read::has_unsafe_single_line_character(text("value")) }),
        "field_args" => {
            let args: Vec<String> = strings(case.get("args"));
            let mut fields = Map::new();
            for (key, value) in read::parse_field_args(&args).entries() {
                fields.insert(key.to_owned(), Value::String(value.to_owned()));
            }
            json!({ "fields": fields })
        }
        other => json!({ "unsupported": other }),
    }
}

fn strings(list: Option<&Value>) -> Vec<String> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn entry(case: &Value, event_type: &str) -> Value {
    // Never created: only its spelling matters to a redaction.
    let project: PathBuf = std::env::temp_dir().join("focus-oracle-project");
    let here = project.to_string_lossy().into_owned();
    let mut fields = Fields::new();
    for pair in case
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let pair = strings(Some(pair));
        if let [key, value] = pair.as_slice() {
            fields.set(key.as_str(), value.replace(PROJECT, &here));
        }
    }
    let entry = Entry::new(event_type, fields);
    match validate(&entry) {
        Err(err) => json!({ "error": err.message().replace(&here, PROJECT) }),
        Ok(()) => {
            let shard = format!("{SHARD_TITLE}{}", render_block(&entry, "<ts>", &project));
            json!({ "shard": shard.replace(&here, PROJECT) })
        }
    }
}

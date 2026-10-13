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
//!
//! A `ledger` case does touch the disk: it describes a project as a set of
//! files, and each side builds it in a scratch directory and reads it as a
//! clone would. Two things in the answer are the machine's and not the
//! engine's, the host name and a freshly minted token; both sides write them
//! `{H}` and `{T}`. A case can also name the machine, as `{H}`, in a path or
//! in the clone file, which is how "a shard under this machine's current
//! name" is asked without knowing the name.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use focus_harness::Harness;
use focus_workspace::Layout;
use focus_workspace::names::CLONE_ID_FILE;
use focus_workspace::oracle::{MACHINE, in_scratch_tree_on, relative};
use focus_workspace::records::{Invoker, Selector};
use serde_json::{Map, Value, json};

use crate::entry::{Entry, Fields, SHARD_TITLE, render_block, validate};
use crate::ledger::{Ledger, Machine};
use crate::memory::parse_memory_headings;
use crate::read;
use crate::shards::{ShardText, copied_blocks, parse_shard_events, parse_shard_notes};
use crate::units::parse_bolt_dag;

/// The kinds of case this crate answers.
pub const KINDS: [&str; 10] = [
    "audit_entry",
    "audit_find",
    "audit_field",
    "bullet_field",
    "unsafe_line",
    "field_args",
    "shard_read",
    "ledger",
    "memory_counts",
    "bolt_dag",
];

/// The machine a `ledger` case is answered as. Neither value is one a case
/// uses, so wherever one shows in an answer it came from the machine.
const ORACLE_HOST: &str = "the-oracle-s-machine";
const ORACLE_TOKEN: &str = "0racle0token";
const MINTED: &str = "{T}";

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
        "shard_read" => shard_read(case),
        "ledger" => in_scratch_tree_on(case, Some(ORACLE_HOST), |root| ledger(root, case)),
        "memory_counts" => {
            let counts = parse_memory_headings(text("raw"));
            json!({
                "interpretations": counts.interpretations,
                "deviations": counts.deviations,
                "tradeoffs": counts.tradeoffs,
                "open_questions": counts.open_questions,
                "total": counts.total(),
            })
        }
        "bolt_dag" => match parse_bolt_dag(text("body")) {
            Ok(dag) => {
                let units: Vec<Value> = dag
                    .units
                    .into_iter()
                    .map(|unit| {
                        let mut row = Map::new();
                        row.insert("name".to_owned(), json!(unit.name));
                        row.insert("depends_on".to_owned(), json!(unit.depends_on));
                        if let Some(kind) = unit.kind {
                            row.insert("kind".to_owned(), json!(kind));
                        }
                        Value::Object(row)
                    })
                    .collect();
                json!({ "ok": true, "units": units, "batches": dag.batches })
            }
            Err(unreadable) => {
                json!({ "ok": false, "reason": unreadable.reason, "detail": unreadable.detail })
            }
        },
        other => json!({ "unsupported": other }),
    }
}

fn shard_read(case: &Value) -> Value {
    let shards: Vec<ShardText> = case
        .get("shards")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|shard| ShardText {
            shard: shard["shard"].as_str().unwrap_or_default().to_owned(),
            content: shard["content"].as_str().unwrap_or_default().to_owned(),
        })
        .collect();
    let own = case.get("own").and_then(Value::as_str).map(str::to_owned);
    let skip: Vec<BTreeSet<usize>> = copied_blocks(&shards, || own);
    let mut events = Vec::new();
    let mut notes = Vec::new();
    for (index, (shard, copied)) in shards.iter().zip(&skip).enumerate() {
        for row in parse_shard_events(&shard.content, &shard.shard, index, copied) {
            events.push(json!({
                "event": row.event,
                "timestamp": row.timestamp,
                "pos": row.pos,
                "shardIndex": row.shard_index,
                "block": row.block,
            }));
        }
        for note in parse_shard_notes(&shard.content, &shard.shard, index, copied) {
            notes.push(json!({
                "heading": note.heading,
                "text": note.text,
                "timestamp": note.timestamp,
                "pos": note.pos,
                "shardIndex": note.shard_index,
            }));
        }
    }
    json!({ "skip": skip, "events": events, "notes": notes })
}

fn ledger(root: &Path, case: &Value) -> Value {
    let layout = Layout::new(root, Harness::Claude);
    let chosen = case.get("selector");
    let pick = |key: &str| {
        chosen
            .and_then(|selector| selector.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let selector = Selector {
        space: pick("space"),
        intent: pick("intent"),
        session_id: None,
    };
    let invoker = Invoker::default();
    let machine = Machine::fixed(ORACLE_HOST, ORACLE_TOKEN, std::time::SystemTime::UNIX_EPOCH);
    let ledger = Ledger::new(&layout, machine);
    let named = |text: &str| {
        text.replace(ORACLE_HOST, MACHINE)
            .replace(ORACLE_TOKEN, MINTED)
    };
    let in_tree = |path: &Path| named(&relative(root, path));
    let clone_id = || {
        std::fs::read(layout.workspace_root().join(CLONE_ID_FILE))
            .ok()
            .map(|bytes| named(&String::from_utf8_lossy(&bytes)))
    };

    let answer = (|| -> focus_js::Result<Value> {
        let mut audit_file = None;
        if case.get("mintFirst") == Some(&Value::Bool(true)) {
            audit_file = Some(ledger.audit_file(&selector, &invoker)?);
        }
        let shards = ledger.shards(&selector, &invoker, None)?;
        let mut unreadable = Vec::new();
        let events = ledger.read_events(&selector, &invoker, Some(&mut unreadable))?;
        let all = ledger.read_all(&selector, &invoker)?;
        let reader_left = clone_id();
        let audit_file = match audit_file {
            Some(path) => path,
            None => ledger.audit_file(&selector, &invoker)?,
        };
        let mut answer = Map::new();
        answer.insert("auditFile".to_owned(), json!(in_tree(&audit_file)));
        answer.insert("cloneId".to_owned(), json!(clone_id()));
        answer.insert("readerLeft".to_owned(), json!(reader_left));
        answer.insert(
            "shards".to_owned(),
            json!(
                shards
                    .iter()
                    .map(|shard| in_tree(shard))
                    .collect::<Vec<_>>()
            ),
        );
        answer.insert(
            "unreadable".to_owned(),
            json!(
                unreadable
                    .iter()
                    .map(|path| in_tree(path))
                    .collect::<Vec<_>>()
            ),
        );
        answer.insert(
            "events".to_owned(),
            json!(
                events
                    .iter()
                    .map(|row| json!([
                        in_tree(Path::new(&row.shard)),
                        row.shard_index,
                        row.pos,
                        row.event,
                        row.timestamp
                    ]))
                    .collect::<Vec<_>>()
            ),
        );
        answer.insert("all".to_owned(), json!(all));
        Ok(Value::Object(answer))
    })();
    answer.unwrap_or_else(
        |err| json!({ "error": err.message().replace(&root.display().to_string(), "{P}") }),
    )
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

//! `focus engine audit <verb>`.
//!
//! The ledger's command: the diagnostic way in, and the way to read a
//! record's history. Every event that carries authority has one owning
//! emitter inside the engine, and this command refuses to write any of them,
//! so an agent cannot mint a receipt for something that did not happen.
//!
//! Every answer is one line of JSON on standard output; every refusal is
//! `{"error": "..."}` on standard error with exit code 1. Upstream prints
//! most refusals that way and lets the rest reach its runtime, which prints a
//! source excerpt and a stack trace around the same sentence; here they all
//! take the one shape.
//!
//! Mirrors `main`, `handleAppend`, `handleAppendBatch`, `handleAppendRaw` and
//! `handleHistory` in `aidlc-audit.ts`. `audit-fork` and `audit-merge` belong
//! to bolts and worktrees, which are not ported: both are refused by name.

use std::path::Path;

use focus_audit::events::{PROTECTED_FROM_THE_COMMAND, RESERVED_FOR_OWNERS};
use focus_audit::history::{Filter, history_json};
use focus_audit::ledger::{Ledger, Machine};
use focus_audit::read::parse_field_args;
use focus_audit::{Entry, Fields, Note, validate};
use focus_harness::Harness;
use focus_js::json;
use focus_workspace::records::{Invoker, Selector};
use focus_workspace::{Layout, WorkspaceLock, env, project};
use serde_json::{Map, Value, json};

use crate::Failure;
use crate::args::Args;

const HELP: &str = "\
Usage: focus engine audit <verb> [--project-dir <dir>]

The audit ledger of the active record. Every answer is one line of JSON.

  append <event-type> [--field key=value ...]
                               Append one diagnostic event
  append-batch <entries-json>  Append several events as one write; the
                               argument is a JSON array of
                               {\"eventType\": \"...\", \"fields\": {\"key\": \"value\"}}
  append-raw <heading> <body>  Append a free-form note; \\n in the body is a
                               line break
  history [--stage <slug>] [--event <TYPE>]... [--limit <n>]
                               The record's events and notes, oldest first

Events that carry authority (gate resolutions, approvals, a person's turn,
unit and bolt lifecycle, provenance) are written only by the engine's own
tools and hooks, and are refused here.";

const USAGE: &str = "Usage: focus engine audit <append|append-batch|append-raw|history> [args...]";

const VERBS: &str = "append, append-batch, append-raw, history";

pub fn run(args: &mut Args) -> Result<(), Failure> {
    if matches!(args.peek(), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    match answer(args) {
        Ok(answer) => {
            println!("{}", json::stringify(&answer));
            Ok(())
        }
        Err(message) => {
            eprintln!("{}", json::stringify(&json!({ "error": message })));
            Err(Failure::Reported)
        }
    }
}

type Answer = Result<Value, String>;

fn answer(args: &mut Args) -> Answer {
    let project_dir = args.value("--project-dir")?;
    let harness = args.value("--harness")?;
    let Some(verb) = args.next_positional() else {
        return Err(USAGE.to_owned());
    };
    let layout = || -> Result<Layout, String> {
        let project = project::resolve_project_dir(project_dir.as_deref().map(Path::new))
            .map_err(|err| err.to_string())?;
        // The ledger lives in the workspace, which every harness installed in
        // a project shares: no path here depends on the harness, so a project
        // with several, or none yet, is not asked which one is meant.
        let harness = if harness.is_some() || env::var("HARNESS").is_some_and(|h| !h.is_empty()) {
            project::resolve_harness(&project, harness.as_deref()).map_err(|err| err.to_string())?
        } else {
            project::installed_harnesses(&project)
                .first()
                .copied()
                .unwrap_or(Harness::Claude)
        };
        Ok(Layout::new(project, harness))
    };
    match verb.as_str() {
        "append" => append(args, &layout),
        "append-batch" => append_batch(args, &layout),
        "append-raw" => append_raw(args, &layout),
        "history" => history(args, &layout),
        "audit-fork" | "audit-merge" => Err(format!(
            "{verb} forks or merges a bolt's ledger, and Focus does not work with bolts and worktrees yet."
        )),
        other => Err(format!("Unknown subcommand: {other}. Expected: {VERBS}")),
    }
}

fn refuse_reserved(event_type: &str) -> Result<(), String> {
    if RESERVED_FOR_OWNERS.contains(&event_type) {
        return Err(format!(
            "{event_type} is reserved for its owning hook/tool and cannot be appended through the public audit CLI."
        ));
    }
    Ok(())
}

fn refuse_protected(event_type: &str) -> Result<(), String> {
    if PROTECTED_FROM_THE_COMMAND.contains(&event_type) && !env::is_on("ALLOW_DIRECT_AUDIT_EVENTS")
    {
        return Err(format!(
            "Direct emission of {event_type} is blocked: it is an authority-bearing receipt owned by its emitting tool or hook (gate resolutions and approvals come from `focus engine orchestrate report`, interview answers and reviews from `focus engine log`, human presence from the prompt-submit hook). The audit CLI appends diagnostic events only."
        ));
    }
    Ok(())
}

/// Validate, then append under the workspace lock. Validation comes first so
/// that a refused entry leaves nothing in the project, not even a lock file.
fn write_entries(layout: &Layout, entries: &[Entry]) -> Result<Vec<String>, String> {
    for entry in entries {
        validate(entry).map_err(|err| err.to_string())?;
    }
    let lock = WorkspaceLock::acquire(layout, "appending to the audit ledger")
        .map_err(|err| err.to_string())?;
    Ledger::new(layout, Machine::this())
        .append(&lock, entries, &Selector::default(), &Invoker::from_env())
        .map_err(|err| err.to_string())
}

fn append(args: &mut Args, layout: &dyn Fn() -> Result<Layout, String>) -> Answer {
    let event_type = args.next_positional().filter(|name| !name.is_empty());
    let Some(event_type) = event_type else {
        return Err(
            "Usage: focus engine audit append <event-type> [--field key=value ...]".to_owned(),
        );
    };
    refuse_reserved(&event_type)?;
    let rest = args.take_rest();
    // Whatever follows the event type is fields, each written in full: a
    // field this command dropped would be missing from the ledger with
    // nothing to say so.
    let mut index = 0;
    while index < rest.len() {
        if rest[index] != "--field" {
            return Err(format!("Unexpected argument: {}", rest[index]));
        }
        match rest.get(index + 1).and_then(|pair| pair.split_once('=')) {
            Some((key, _)) if !key.is_empty() => {}
            _ => {
                return Err(format!(
                    "--field expects key=value, and got: {}",
                    rest.get(index + 1).map_or("nothing", String::as_str)
                ));
            }
        }
        index += 2;
    }
    let fields = parse_field_args(&rest);
    refuse_protected(&event_type)?;
    let entry = Entry::new(event_type, fields);
    let timestamps = write_entries(&layout()?, std::slice::from_ref(&entry))?;
    let mut answer = Map::new();
    answer.insert("appended".to_owned(), json!(true));
    answer.insert("event".to_owned(), json!(entry.event_type));
    answer.insert("timestamp".to_owned(), json!(timestamps.first()));
    Ok(Value::Object(answer))
}

fn append_batch(args: &mut Args, layout: &dyn Fn() -> Result<Layout, String>) -> Answer {
    let raw = args.next_positional().filter(|raw| !raw.is_empty());
    let Some(raw) = raw else {
        return Err("Usage: focus engine audit append-batch <entries-json>".to_owned());
    };
    args.finish()?;
    let Ok(parsed) = json::parse(&raw) else {
        return Err("append-batch entries must be valid JSON".to_owned());
    };
    let rows = match parsed.as_array() {
        Some(rows) if !rows.is_empty() => rows,
        _ => return Err("append-batch entries must be a non-empty JSON array".to_owned()),
    };
    // A reserved type is refused wherever it stands in the batch, before the
    // shape of any other row is looked at.
    for row in rows {
        if let Some(event_type) = row.get("eventType").and_then(Value::as_str) {
            refuse_reserved(event_type)?;
        }
    }
    let mut entries = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let shape = row.as_object().and_then(|row| {
            Some((
                row.get("eventType")?.as_str()?,
                row.get("fields")?.as_object()?,
            ))
        });
        let Some((event_type, given)) = shape else {
            return Err(format!(
                "append-batch entry {index} must contain string eventType and object fields"
            ));
        };
        if given.values().any(|value| !value.is_string()) {
            return Err(format!(
                "append-batch entry {index} field values must be strings"
            ));
        }
        let fields: Fields = given
            .iter()
            .filter_map(|(key, value)| Some((key.as_str(), value.as_str()?)))
            .collect();
        entries.push(Entry::new(event_type, fields));
    }
    for entry in &entries {
        refuse_protected(&entry.event_type)?;
    }
    let timestamps = write_entries(&layout()?, &entries)?;
    let mut answer = Map::new();
    answer.insert("appended".to_owned(), json!(true));
    answer.insert(
        "events".to_owned(),
        json!(
            entries
                .iter()
                .map(|entry| entry.event_type.as_str())
                .collect::<Vec<_>>()
        ),
    );
    answer.insert("timestamps".to_owned(), json!(timestamps));
    Ok(Value::Object(answer))
}

fn append_raw(args: &mut Args, layout: &dyn Fn() -> Result<Layout, String>) -> Answer {
    let heading = args.next_positional().filter(|text| !text.is_empty());
    let body = args.next_positional().filter(|text| !text.is_empty());
    let (Some(heading), Some(body)) = (heading, body) else {
        return Err("Usage: focus engine audit append-raw <heading> <body>".to_owned());
    };
    args.finish()?;
    let layout = layout()?;
    let note = Note::check(&heading, &body, &layout.project).map_err(|err| err.to_string())?;
    let lock = WorkspaceLock::acquire(&layout, "appending to the audit ledger")
        .map_err(|err| err.to_string())?;
    let timestamp = Ledger::new(&layout, Machine::this())
        .append_note(&lock, &note, &Invoker::from_env())
        .map_err(|err| err.to_string())?;
    let mut answer = Map::new();
    answer.insert("appended".to_owned(), json!(true));
    answer.insert("heading".to_owned(), json!(heading));
    answer.insert("timestamp".to_owned(), json!(timestamp));
    Ok(Value::Object(answer))
}

/// JavaScript's largest safe integer, the most `--limit` accepts.
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

fn history(args: &mut Args, layout: &dyn Fn() -> Result<Layout, String>) -> Answer {
    let rest = args.take_rest();
    let mut filter = Filter::default();
    for pair in rest.chunks(2) {
        let flag = pair[0].as_str();
        if !["--stage", "--event", "--limit"].contains(&flag) {
            return Err(format!("Unknown history argument: {flag}"));
        }
        let value = match pair.get(1) {
            Some(value) if !value.is_empty() && !value.starts_with("--") => value,
            _ => return Err(format!("{flag} expects a value.")),
        };
        match flag {
            "--stage" => filter.stage = Some(value.clone()),
            "--event" => {
                if !filter.events.contains(value) {
                    filter.events.push(value.clone());
                }
            }
            _ => {
                let digits = value.bytes().all(|byte| byte.is_ascii_digit());
                let limit = value
                    .parse::<u64>()
                    .ok()
                    .filter(|limit| digits && !value.starts_with('0') && *limit <= MAX_SAFE_INTEGER)
                    .and_then(|limit| usize::try_from(limit).ok());
                let Some(limit) = limit else {
                    return Err("--limit must be a positive integer.".to_owned());
                };
                filter.limit = Some(limit);
            }
        }
    }
    let layout = layout()?;
    let rows = Ledger::new(&layout, Machine::this())
        .active_history(&Invoker::from_env())
        .map_err(|err| err.to_string())?;
    Ok(history_json(&rows, &filter))
}

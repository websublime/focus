//! The message store (`aidlc-message-store.ts`): one record per message
//! the person sends, written once by the human-turn hook as the message
//! arrives. It proves that a real person said this in this chat and
//! exactly what they said; which question a message answers is never
//! written here. Each record is its own file under
//! `focus/.focus-sessions/messages/<id>.json`, written through no symlink;
//! the store keeps a day's worth and at most `MESSAGE_MAX_RECORDS`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use focus_js::date::parse_instant;
use focus_js::{Result, bail, json};
use focus_workspace::Layout;
use focus_workspace::links::refuse_link_in_chain;
use focus_workspace::names::SESSIONS_DIR;
use serde_json::{Map, Value, json};

use crate::engine::write_record_file_no_follow;

/// `MESSAGE_TEXT_MAX_CHARS`: the text and the words are cut here.
pub const MESSAGE_TEXT_MAX_CHARS: usize = 8000;
/// `MESSAGE_PICKER_MAX_CHARS`: a question box's question or reply is kept to
/// this many characters.
pub const MESSAGE_PICKER_MAX_CHARS: usize = 2000;
pub const MESSAGE_MAX_RECORDS: usize = 200;
/// A ceiling against a corrupt or foreign file, not a size the hook can
/// reach.
const MESSAGE_MAX_BYTES: usize = 256 * 1024;
const MESSAGE_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// A scope typed with the request, and the request's own flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MessageRoute {
    pub scope: Option<String>,
    pub new_intent: bool,
    pub skip: Vec<String>,
    pub add: Vec<String>,
    pub project_type: Option<String>,
}

/// One message as the store keeps it, before its id is minted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The chat the message came through.
    pub session: Option<String>,
    pub at: String,
    /// `prompt` or `picker` (`terminal` is a command typed at the terminal).
    pub source: &'static str,
    /// As the host delivered it, cut at `MESSAGE_TEXT_MAX_CHARS`.
    pub text: String,
    pub cut: bool,
    /// What a question box carried back, or `None` for a typed prompt.
    pub picker: Option<Vec<(String, Option<String>)>>,
    /// The person's own words: the text after the entry word and after any
    /// flags. `None` for a command with no words.
    pub words: Option<String>,
    /// The settings typed as flags, as the config setter names them.
    pub settings: Vec<(String, String)>,
    pub route: MessageRoute,
    /// The lines the hook's typed-switch step produced.
    pub applied: Vec<String>,
}

/// `isMessageId`: eight lowercase hex characters.
#[must_use]
pub fn is_message_id(id: &str) -> bool {
    id.len() == 8
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn messages_rel(id: Option<&str>) -> PathBuf {
    let dir = Path::new(focus_workspace::names::WORKSPACE_DIR)
        .join(SESSIONS_DIR)
        .join("messages");
    match id {
        Some(id) => dir.join(format!("{id}.json")),
        None => dir,
    }
}

/// `recordFileTargetOrThrow`: the file's path under the project, through no
/// link.
fn target(layout: &Layout, rel: &Path) -> Result<PathBuf> {
    refuse_link_in_chain(&layout.project, rel)?;
    Ok(layout.project.join(rel))
}

fn record_ids(layout: &Layout) -> Vec<String> {
    let Ok(dir) = target(layout, &messages_rel(None)) else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json")?;
            is_message_id(id).then(|| id.to_owned())
        })
        .collect();
    // `readdirSync` order is the file system's; the callers sort or take a
    // maximum, so any order serves.
    ids.sort();
    ids
}

/// `mintMessageId`: eight hex characters no record holds yet.
fn mint_id(layout: &Layout) -> Result<String> {
    loop {
        let mut bytes = [0u8; 4];
        if getrandom::fill(&mut bytes).is_err() {
            bail!("the system's random source is unavailable");
        }
        let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        if !target(layout, &messages_rel(Some(&id)))?.exists() {
            return Ok(id);
        }
    }
}

/// The record's JSON, with the keys in upstream's order.
fn serialise(id: &str, message: &Message) -> Value {
    let mut out = Map::new();
    out.insert("session".into(), json!(message.session));
    out.insert("at".into(), json!(message.at));
    out.insert("source".into(), json!(message.source));
    out.insert("text".into(), json!(message.text));
    if message.cut {
        out.insert("cut".into(), json!(true));
    }
    out.insert(
        "picker".into(),
        match &message.picker {
            None => Value::Null,
            Some(entries) => json!(
                entries
                    .iter()
                    .map(|(question, reply)| json!({"question": question, "reply": reply}))
                    .collect::<Vec<_>>()
            ),
        },
    );
    out.insert("words".into(), json!(message.words));
    out.insert(
        "settings".into(),
        json!(
            message
                .settings
                .iter()
                .map(|(key, value)| json!({"key": key, "value": value}))
                .collect::<Vec<_>>()
        ),
    );
    out.insert(
        "route".into(),
        json!({
            "scope": message.route.scope,
            "newIntent": message.route.new_intent,
            "skip": message.route.skip,
            "add": message.route.add,
            "projectType": message.route.project_type,
        }),
    );
    out.insert("applied".into(), json!(message.applied));
    out.insert("id".into(), json!(id));
    Value::Object(out)
}

/// `saveMessage`: store one message and return its fresh id. Older records
/// are pruned after the write, so a store failure never costs the message
/// just received.
///
/// # Errors
///
/// The record cannot be written, or would be over the size the reader
/// accepts.
pub fn save_message(layout: &Layout, message: &Message) -> Result<String> {
    let id = mint_id(layout)?;
    let serialised = format!("{}\n", json::stringify(&serialise(&id, message)));
    if serialised.len() > MESSAGE_MAX_BYTES {
        bail!("message record over the size the reader accepts");
    }
    write_record_file_no_follow(
        &layout.project,
        &messages_rel(Some(&id)).to_string_lossy(),
        serialised.as_bytes(),
    )?;
    prune_expired(layout);
    Ok(id)
}

/// The `at` of a record, when the file parses as one of ours.
fn record_at(layout: &Layout, id: &str) -> Option<SystemTime> {
    let path = target(layout, &messages_rel(Some(id))).ok()?;
    let bytes = focus_workspace::files::read_regular(&path, "message").ok()?;
    if bytes.len() > MESSAGE_MAX_BYTES {
        return None;
    }
    let value = json::parse(&String::from_utf8_lossy(&bytes)).ok()?;
    let record = value.as_object()?;
    if record.get("id").and_then(Value::as_str) != Some(id) {
        return None;
    }
    let at = record.get("at").and_then(Value::as_str)?;
    let millis = parse_instant(at)?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_millis(u64::try_from(millis).ok()?))
}

/// A message read back: what a reader of the store acts on. The store
/// proves that a person said this in this chat; which question it answers
/// is never written in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub id: String,
    /// The chat the message came through; `None` for the terminal.
    pub session: Option<String>,
    pub at: String,
    pub text: String,
    /// The person's own words, or `None` for a command with none.
    pub words: Option<String>,
}

fn is_text_list(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_array)
        .is_some_and(|list| list.iter().all(Value::is_string))
}

fn is_null_or_text(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Null | Value::String(_)))
}

/// `parseMessage`: a record is one of ours when every field is there with
/// the kind the writer gives it.
fn parse_stored(id: &str, raw: &Value) -> Option<Stored> {
    let message = raw.as_object()?;
    let text_of = |key: &str| message.get(key).and_then(Value::as_str);
    let pairs = |value: Option<&Value>, first: &str, second: &str| {
        value.and_then(Value::as_array).is_some_and(|list| {
            list.iter().all(|entry| {
                entry.is_object() && entry[first].is_string() && entry[second].is_string()
            })
        })
    };
    let picker = match message.get("picker") {
        Some(Value::Null) => true,
        Some(Value::Array(entries)) => entries.iter().all(|entry| {
            entry.is_object()
                && entry["question"].is_string()
                && is_null_or_text(entry.get("reply"))
        }),
        _ => false,
    };
    let route = message.get("route")?.as_object()?;
    let project_type = match route.get("projectType") {
        Some(Value::Null) => true,
        Some(Value::String(kind)) => kind == "greenfield" || kind == "brownfield",
        _ => false,
    };
    let ours = text_of("id") == Some(id)
        && is_null_or_text(message.get("session"))
        && text_of("at").is_some_and(|at| parse_instant(at).is_some())
        && matches!(text_of("source"), Some("prompt" | "picker" | "terminal"))
        && text_of("text").is_some()
        && picker
        && is_null_or_text(message.get("words"))
        && pairs(message.get("settings"), "key", "value")
        && is_null_or_text(route.get("scope"))
        && route.get("newIntent").is_some_and(Value::is_boolean)
        && is_text_list(route.get("skip"))
        && is_text_list(route.get("add"))
        && project_type
        && is_text_list(message.get("applied"))
        && matches!(message.get("cut"), None | Some(Value::Bool(true)));
    ours.then(|| Stored {
        id: id.to_owned(),
        session: text_of("session").map(str::to_owned),
        at: text_of("at").unwrap_or_default().to_owned(),
        text: text_of("text").unwrap_or_default().to_owned(),
        words: text_of("words").map(str::to_owned),
    })
}

/// `readMessage`: the message behind `id`; `None` when it is missing,
/// redirected, unreadable, or not a record of ours.
#[must_use]
pub fn read_message(layout: &Layout, id: &str) -> Option<Stored> {
    if !is_message_id(id) {
        return None;
    }
    let path = target(layout, &messages_rel(Some(id))).ok()?;
    let bytes = focus_workspace::files::read_regular(&path, "message").ok()?;
    if bytes.len() > MESSAGE_MAX_BYTES {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    parse_stored(id, &json::parse(&text).ok()?)
}

/// `listMessages`: every readable record of the project, oldest first
/// (ties by id).
#[must_use]
pub fn list_messages(layout: &Layout) -> Vec<Stored> {
    let mut messages: Vec<Stored> = record_ids(layout)
        .iter()
        .filter_map(|id| read_message(layout, id))
        .collect();
    messages.sort_by(|a, b| focus_js::order::cmp_utf16(&a.at, &b.at).then_with(|| a.id.cmp(&b.id)));
    messages
}

fn remove_record(layout: &Layout, id: &str) {
    if let Ok(path) = target(layout, &messages_rel(Some(id)))
        && path
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_file())
    {
        let _ = std::fs::remove_file(path);
    }
}

/// `pruneExpiredMessages`: remove records older than a day, then the oldest
/// beyond `MESSAGE_MAX_RECORDS`. A file this process may not remove is left
/// for the operating system's permissions to decide.
pub fn prune_expired(layout: &Layout) {
    let now = SystemTime::now();
    let cutoff = now
        .checked_sub(MESSAGE_RETENTION)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut kept: Vec<(String, SystemTime)> = Vec::new();
    for id in record_ids(layout) {
        let Ok(path) = target(layout, &messages_rel(Some(&id))) else {
            continue;
        };
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if !meta.file_type().is_file() {
            continue;
        }
        let when = record_at(layout, &id)
            .or_else(|| meta.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if when < cutoff {
            remove_record(layout, &id);
        } else {
            kept.push((id, when));
        }
    }
    kept.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let over = kept.len().saturating_sub(MESSAGE_MAX_RECORDS);
    for (id, _) in kept.into_iter().take(over) {
        remove_record(layout, &id);
    }
}

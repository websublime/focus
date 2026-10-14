//! Listing a minted record: the registry row, the cursors, the session.
//!
//! A record is built whole before it is listed (`intents.json` is the commit
//! point of creation) and then selected: the space's `active-intent` cursor
//! points at it, and the creating session is bound to it when one is known.
//! The cursors and the binding are per-user state, written best-effort;
//! the registry is written atomically, after the observer barrier.
//!
//! Mirrors `appendIntentToRegistry`, `setActiveIntentCursor`,
//! `ensureActiveSpaceCursor`, `selectIntentForSession`, `writeSessionBinding`
//! and `leaveCreationReceipt` in `aidlc-lib.ts`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_js::{Result, json};
use focus_workspace::atomic::write_atomic;
use focus_workspace::names::{
    ACTIVE_INTENT_POINTER, ACTIVE_SPACE_POINTER, ENGINE_DIR, SESSIONS_DIR,
};
use focus_workspace::records::valid_session_id;
use focus_workspace::{Layout, links, observer, project};
use serde_json::{Map, Value, json};

/// The one-shot receipt a creation leaves for a host that learns which
/// session created the record only afterwards.
pub const CREATION_RECEIPT_FILE: &str = "creation-receipt";

/// A record just minted: its directory exists and nothing else does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minted {
    pub uuid: String,
    pub slug: String,
    pub dir_name: String,
    pub record_dir: PathBuf,
    pub space: String,
}

/// The registry row of a record, as `intents.json` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub uuid: String,
    pub slug: String,
    pub dir_name: String,
    pub scope: Option<String>,
    /// Recorded only when not empty.
    pub repos: Vec<String>,
    pub status: String,
    /// The engine question the record answered, when one did.
    pub request: Option<String>,
}

impl Row {
    fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("uuid".to_owned(), json!(self.uuid));
        out.insert("slug".to_owned(), json!(self.slug));
        out.insert("dirName".to_owned(), json!(self.dir_name));
        if let Some(scope) = &self.scope {
            out.insert("scope".to_owned(), json!(scope));
        }
        if !self.repos.is_empty() {
            out.insert("repos".to_owned(), json!(self.repos));
        }
        out.insert("status".to_owned(), json!(self.status));
        if let Some(request) = &self.request {
            out.insert("request".to_owned(), json!(request));
        }
        Value::Object(out)
    }
}

/// `appendIntentToRegistry`: add `row` to the space's `intents.json`. A
/// registry that is absent or not a JSON list starts over as a fresh list.
///
/// # Errors
///
/// The process is an observer, the registry sits behind a link, or the file
/// cannot be written.
pub fn append_to_registry(layout: &Layout, space: &str, row: &Row) -> Result<()> {
    observer::refuse_write(observer::WRITE_FILE_ATOMIC)?;
    let path = layout.intents_dir(space).join("intents.json");
    let mut list: Vec<Value> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| json::parse(&String::from_utf8_lossy(&bytes)).ok())
        .and_then(|parsed| parsed.as_array().cloned())
        .unwrap_or_default();
    list.push(row.to_json());
    links::refuse_link_on_the_way(&layout.project, &path)?;
    std::fs::create_dir_all(path.parent().unwrap_or(&path))
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))?;
    write_atomic(
        &path,
        format!("{}\n", json::stringify_pretty(&Value::Array(list))).as_bytes(),
    )
    .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))?;
    Ok(())
}

/// `ensureActiveSpaceCursor`: write `focus/active-space` naming the active
/// space when there is no cursor yet. An existing cursor wins, and a
/// workspace that cannot be written keeps none; both are fine.
pub fn ensure_active_space_cursor(layout: &Layout) {
    let space = layout.active_space();
    let root = layout.workspace_root();
    let cursor = root.join(ACTIVE_SPACE_POINTER);
    let _ = std::fs::create_dir_all(&root);
    if let Ok(mut file) = OpenOptions::new().write(true).create_new(true).open(cursor) {
        let _ = writeln!(file, "{space}");
    }
}

/// `setActiveIntentCursor`: point the space's `active-intent` cursor at
/// `dir_name`. Best effort: the cursor is per-user state.
pub fn set_active_intent_cursor(layout: &Layout, dir_name: &str, space: &str) {
    ensure_active_space_cursor(layout);
    let dir = layout.intents_dir(space);
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(dir.join(ACTIVE_INTENT_POINTER), format!("{dir_name}\n"));
    }
}

/// `writeSessionBinding`: bind `session_id` to the record, so that the
/// conversation that created it stays on it. Best effort, and nothing for a
/// name that cannot be bound.
pub fn write_session_binding(
    layout: &Layout,
    session_id: &str,
    space: &str,
    intent: &str,
    source: &str,
    at: SystemTime,
) {
    let Some(id) = valid_session_id(Some(session_id)) else {
        return;
    };
    if !project::is_space_name(space) || !is_bindable_record_name(intent) {
        return;
    }
    let dir = layout.workspace_root().join(SESSIONS_DIR);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let binding = json!({
        "space": space,
        "intent": intent,
        "boundAt": iso_timestamp(at),
        "source": source,
    });
    let _ = std::fs::write(
        dir.join(format!("{id}.binding.json")),
        format!("{}\n", json::stringify(&binding)),
    );
}

/// `isBindableIntentRecordName`: a record name a binding may carry.
fn is_bindable_record_name(name: &str) -> bool {
    !name.is_empty()
        && focus_js::text::trim(name) == name
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !(cfg!(windows) && name.contains('\\'))
        && name.chars().all(|ch| {
            let code = u32::from(ch);
            code >= 0x20 && code != 0x7f && !(0x80..=0x9f).contains(&code)
        })
}

/// `leaveCreationReceipt`: one receipt per creation, never replaced, and
/// never written through a link.
pub fn leave_creation_receipt(record_dir: &Path, uuid: &str) {
    let relative = Path::new(ENGINE_DIR).join(CREATION_RECEIPT_FILE);
    let target = record_dir.join(&relative);
    if links::refuse_link_in_chain(record_dir, &relative).is_err() || target.exists() {
        return;
    }
    if std::fs::create_dir_all(record_dir.join(ENGINE_DIR)).is_err() {
        return;
    }
    let _ = std::fs::write(target, format!("{uuid}\n"));
}

/// `selectIntentForSession`: the cursor, then the binding when a session is
/// known; whether one was bound.
pub fn select_for_session(
    layout: &Layout,
    dir_name: &str,
    space: &str,
    session_id: Option<&str>,
    at: SystemTime,
) -> bool {
    set_active_intent_cursor(layout, dir_name, space);
    let Some(session) = session_id.and_then(|id| valid_session_id(Some(id))) else {
        return false;
    };
    write_session_binding(layout, session, space, dir_name, "create", at);
    true
}

/// `registerIntentRecord`: list the record, then select it; a creation that
/// bound no session leaves the receipt for the host to pick up.
///
/// # Errors
///
/// The registry cannot be written.
pub fn register(
    layout: &Layout,
    minted: &Minted,
    scope: &str,
    repos: &[String],
    session_id: Option<&str>,
    request: Option<&str>,
    at: SystemTime,
) -> Result<()> {
    append_to_registry(
        layout,
        &minted.space,
        &Row {
            uuid: minted.uuid.clone(),
            slug: minted.slug.clone(),
            dir_name: minted.dir_name.clone(),
            scope: Some(scope.to_owned()),
            repos: repos.to_vec(),
            status: "in-flight".to_owned(),
            request: request.map(str::to_owned),
        },
    )?;
    if select_for_session(layout, &minted.dir_name, &minted.space, session_id, at) {
        return Ok(());
    }
    leave_creation_receipt(&minted.record_dir, &minted.uuid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    fn layout(name: &str) -> Layout {
        let root = std::env::temp_dir().join(format!("focus-record-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Layout::new(root, Harness::Claude)
    }

    fn minted(layout: &Layout) -> Minted {
        let record_dir = layout.intents_dir("default").join("261014-add-login");
        std::fs::create_dir_all(&record_dir).unwrap();
        Minted {
            uuid: "01a137c7-7d33-7bb4-836e-74d3c49202d6".to_owned(),
            slug: "add-login".to_owned(),
            dir_name: "261014-add-login".to_owned(),
            record_dir,
            space: "default".to_owned(),
        }
    }

    #[test]
    fn a_record_is_listed_then_selected() {
        let layout = layout("list");
        let minted = minted(&layout);
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_792_540_800);
        register(&layout, &minted, "feature", &[], None, None, at).unwrap();
        let registry =
            std::fs::read_to_string(layout.intents_dir("default").join("intents.json")).unwrap();
        assert_eq!(
            registry,
            "[\n  {\n    \"uuid\": \"01a137c7-7d33-7bb4-836e-74d3c49202d6\",\n    \"slug\": \"add-login\",\n    \"dirName\": \"261014-add-login\",\n    \"scope\": \"feature\",\n    \"status\": \"in-flight\"\n  }\n]\n"
        );
        assert_eq!(
            std::fs::read_to_string(layout.intents_dir("default").join(ACTIVE_INTENT_POINTER))
                .unwrap(),
            "261014-add-login\n"
        );
        assert_eq!(
            std::fs::read_to_string(layout.workspace_root().join(ACTIVE_SPACE_POINTER)).unwrap(),
            "default\n"
        );
        // No session: the receipt is left, once.
        let receipt = minted
            .record_dir
            .join(ENGINE_DIR)
            .join(CREATION_RECEIPT_FILE);
        assert_eq!(
            std::fs::read_to_string(&receipt).unwrap(),
            "01a137c7-7d33-7bb4-836e-74d3c49202d6\n"
        );
        // A second row joins the first; a malformed registry starts over.
        let second = Minted {
            dir_name: "261014-x".to_owned(),
            slug: "x".to_owned(),
            ..minted.clone()
        };
        register(
            &layout,
            &second,
            "bugfix",
            &["api".to_owned(), "web".to_owned()],
            Some("sess-1"),
            Some("q1"),
            at,
        )
        .unwrap();
        let registry =
            std::fs::read_to_string(layout.intents_dir("default").join("intents.json")).unwrap();
        assert!(registry.contains("\"dirName\": \"261014-x\",\n    \"scope\": \"bugfix\",\n    \"repos\": [\n      \"api\",\n      \"web\"\n    ],\n    \"status\": \"in-flight\",\n    \"request\": \"q1\"\n"), "{registry}");
        assert!(registry.starts_with("[\n  {\n    \"uuid\""));
        let binding = std::fs::read_to_string(
            layout
                .workspace_root()
                .join(SESSIONS_DIR)
                .join("sess-1.binding.json"),
        )
        .unwrap();
        assert_eq!(
            binding,
            "{\"space\":\"default\",\"intent\":\"261014-x\",\"boundAt\":\"2026-10-21T00:00:00Z\",\"source\":\"create\"}\n"
        );
        std::fs::write(
            layout.intents_dir("default").join("intents.json"),
            "{not a list",
        )
        .unwrap();
        register(&layout, &minted, "feature", &[], None, None, at).unwrap();
        let registry =
            std::fs::read_to_string(layout.intents_dir("default").join("intents.json")).unwrap();
        assert_eq!(registry.matches("\"uuid\"").count(), 1);
        let _ = std::fs::remove_dir_all(&layout.project);
    }

    #[test]
    fn an_existing_space_cursor_wins() {
        let layout = layout("cursor");
        std::fs::create_dir_all(layout.space_dir("payments")).unwrap();
        std::fs::write(
            layout.workspace_root().join(ACTIVE_SPACE_POINTER),
            "payments\n",
        )
        .unwrap();
        ensure_active_space_cursor(&layout);
        assert_eq!(
            std::fs::read_to_string(layout.workspace_root().join(ACTIVE_SPACE_POINTER)).unwrap(),
            "payments\n"
        );
        // A cursor naming no space stays as it is: an existing cursor wins,
        // whatever it says.
        std::fs::write(layout.workspace_root().join(ACTIVE_SPACE_POINTER), "nope\n").unwrap();
        ensure_active_space_cursor(&layout);
        assert_eq!(
            std::fs::read_to_string(layout.workspace_root().join(ACTIVE_SPACE_POINTER)).unwrap(),
            "nope\n"
        );
        assert!(!is_bindable_record_name("a/b"));
        assert!(!is_bindable_record_name(" a"));
        assert!(!is_bindable_record_name("a\u{85}"));
        assert!(is_bindable_record_name("261014-add-login"));
        let _ = std::fs::remove_dir_all(&layout.project);
    }
}

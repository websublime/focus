//! Which record a command acts on.
//!
//! Work is filed by space and by intent: `focus/spaces/<space>/intents/<record>/`
//! holds one intent's state file, its audit shards and its artifacts. Almost
//! every command names neither, so the engine has to settle both, the same way
//! every time, before it reads or writes anything. In order:
//!
//! 1. what the command said (`--space`, `--intent`);
//! 2. what the calling conversation is bound to, when a session is known and
//!    has a binding;
//! 3. the cursors this checkout keeps (`focus/active-space`, and in the space
//!    `active-intent`);
//! 4. the only live record of the space, when there is exactly one.
//!
//! With none of those, no intent resolves and the answer is the space alone.
//! That is not an error here: what to do about it belongs to the command.
//!
//! Mirrors `resolveWorkflowSelection` (L6841), `activeIntent` (L3150),
//! `readActiveIntentCursor`, `listIntentDirs`, `recordDirMatches` (L4141),
//! `isArchivedIntent`, `readIntentRegistry` (L4200), `readSessionBinding`
//! (L5980) and `recordDir` / `relativeRecordDir` in `aidlc-lib.ts`.
//!
//! Two things upstream resolves here are not ported, and both are refused
//! rather than skipped: a delegated worktree (worktrees come later), and
//! finding the calling conversation by walking the process tree, which only
//! matters once session hooks have recorded which process is which.

use std::path::{Component, MAIN_SEPARATOR, Path, PathBuf};

use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{EngineError, Result, bail, json, num, re};
use serde_json::Value;

use crate::names::{
    ACTIVE_INTENT_POINTER, ENGINE_DIR, SESSIONS_DIR, STATE_FILE, WORKSPACE_DIR, WORKTREE_META,
};
use crate::project::{Layout, is_space_name};

/// What a command said about where to act.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector {
    pub space: Option<String>,
    pub intent: Option<String>,
    /// The session the command named, if any.
    pub session_id: Option<String>,
}

/// Who is calling, as far as the environment says: the session a hook passed
/// down (`FOCUS_SESSION_OVERRIDE`), and whether it came from the hook's own
/// payload (`FOCUS_SESSION_OVERRIDE_SOURCE=payload`) rather than from a
/// variable someone exported.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Invoker {
    pub session_override: Option<String>,
    pub from_payload: bool,
}

impl Invoker {
    /// Read the two variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            session_override: crate::env::var("SESSION_OVERRIDE"),
            from_payload: crate::env::var("SESSION_OVERRIDE_SOURCE").as_deref() == Some("payload"),
        }
    }
}

/// What a session is pinned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub space: String,
    pub intent: Option<String>,
    pub bound_at: String,
    /// How the binding's intent was chosen; absent on older bindings and on
    /// any source this engine does not know.
    pub source: Option<String>,
}

/// The settled answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub space: String,
    /// The record directory's name. `None` when no intent resolves. The empty
    /// name is the space's own record root, as a command may name it.
    pub intent: Option<String>,
    pub session_id: Option<String>,
    pub binding: Option<Binding>,
}

const BINDING_SOURCES: [&str; 14] = [
    "create",
    "observed-create",
    "migration",
    "switch",
    "archive",
    "space-switch-cursor",
    "space-switch-lone",
    "space-switch-none",
    "worktree",
    "unit-claim",
    "cursor",
    "stamp",
    "unjoined",
    "none",
];

/// A registry row the engine cannot read past. Upstream fails on these with
/// whatever its runtime says about the value; Focus says which file.
fn malformed(layout: &Layout, space: &str) -> EngineError {
    EngineError::new(format!(
        "{} has a row that is not an intent: every row needs a text `status`, and a row matched by its slug needs a text `uuid`. Fix the row or remove it.",
        registry_path(layout, space).display()
    ))
}

fn registry_path(layout: &Layout, space: &str) -> PathBuf {
    layout.intents_dir(space).join("intents.json")
}

/// The rows of a space's `intents.json`; empty when the file is missing, is
/// not JSON, or is not a list.
#[must_use]
pub fn read_intent_registry(layout: &Layout, space: &str) -> Vec<Value> {
    let Ok(bytes) = std::fs::read(registry_path(layout, space)) else {
        return Vec::new();
    };
    match json::parse(&String::from_utf8_lossy(&bytes)) {
        Ok(Value::Array(rows)) => rows,
        _ => Vec::new(),
    }
}

/// What JavaScript makes of a JSON value inside a template string.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => num::to_string(number.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// Whether record directory `dir_name` belongs to registry row `row`: by the
/// directory name the row stores, or, for rows written before names were
/// stored, by `<slug>-<hex>` where the hex is the tail of the row's uuid.
///
/// `Err(())` where upstream's untyped read would fail: a row that is `null`,
/// or one matched by slug whose slug or uuid cannot be read as text.
fn record_dir_matches(row: &Value, dir_name: &str) -> std::result::Result<bool, ()> {
    if row.is_null() {
        return Err(());
    }
    let stored = row.get("dirName");
    if is_truthy(stored) {
        return Ok(stored.and_then(Value::as_str) == Some(dir_name));
    }
    let slug = row.get("slug");
    if !dir_name.starts_with(&format!("{}-", js_string(slug))) {
        return Ok(false);
    }
    // `slug.length + 1` characters are dropped; a value with no length drops none.
    let skip = match slug {
        None | Some(Value::Null) => return Err(()),
        Some(Value::String(text)) => text.encode_utf16().count() + 1,
        Some(Value::Array(items)) => items.len() + 1,
        Some(_) => 0,
    };
    let units: Vec<u16> = dir_name.encode_utf16().collect();
    let suffix = String::from_utf16_lossy(units.get(skip..).unwrap_or_default());
    if !re::test("^[0-9a-f]+$", "", &suffix) {
        return Ok(false);
    }
    let Some(Value::String(uuid)) = row.get("uuid") else {
        return Err(());
    };
    let hex: Vec<u16> = uuid.replace('-', "").encode_utf16().collect();
    let tail = &hex[hex.len().saturating_sub(suffix.len())..];
    Ok(String::from_utf16_lossy(tail) == suffix)
}

/// `recordDirMatches` for a reader that takes an unreadable row as no
/// match: whether record directory `dir_name` belongs to registry row `row`.
#[must_use]
pub fn row_names_record(row: &Value, dir_name: &str) -> bool {
    record_dir_matches(row, dir_name) == Ok(true)
}

/// Whether a row's status is `archived`, whatever its case or padding.
fn is_archived(row: &Value) -> std::result::Result<bool, ()> {
    match row.get("status") {
        Some(Value::String(status)) => Ok(trim(status).to_lowercase() == "archived"),
        _ => Err(()),
    }
}

/// The record directories of a space, sorted: every entry of its intents
/// directory that holds a state file. The registry is not consulted, so a
/// record nobody registered still counts.
#[must_use]
pub fn list_intent_dirs(layout: &Layout, space: &str) -> Vec<String> {
    let dir = layout.intents_dir(space);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut records: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| dir.join(name).join(STATE_FILE).exists())
        .collect();
    records.sort_by(|a, b| cmp_utf16(a, b));
    records
}

/// `path.join(base, tail)`: put the two together, then settle `.` and `..` by
/// reading the path, never the disk. Unlike [`Path::join`], a `tail` that
/// starts at the root does not replace `base`. A cursor file is free text, so
/// what it names is joined exactly as upstream joins it.
fn join_as_written(base: &Path, tail: &str) -> PathBuf {
    let joined = format!("{}{MAIN_SEPARATOR}{tail}", base.display());
    let mut settled = PathBuf::new();
    for component in Path::new(&joined).components() {
        match component {
            Component::ParentDir => {
                settled.pop();
            }
            Component::CurDir => {}
            other => settled.push(other),
        }
    }
    settled
}

/// The record the space's `active-intent` cursor names, when it names a real
/// one. The cursor is written only by this checkout's own create and switch,
/// so unlike a lone committed record it is evidence of a choice made here.
#[must_use]
pub fn read_active_intent_cursor(layout: &Layout, space: &str) -> Option<String> {
    let dir = layout.intents_dir(space);
    let bytes = std::fs::read(dir.join(ACTIVE_INTENT_POINTER)).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let name = trim(&text);
    let names_a_record = !name.is_empty() && join_as_written(&dir, name).join(STATE_FILE).exists();
    names_a_record.then(|| name.to_owned())
}

/// The active intent of a space: the cursor's record, else the only record
/// that is not archived. `None` with no record, and with several and no
/// cursor: a path helper cannot guess which one was meant.
///
/// # Errors
///
/// The registry has a row that cannot be read (see [`record_dir_matches`]).
pub fn active_intent(layout: &Layout, space: &str) -> Result<Option<String>> {
    if let Some(cursor) = read_active_intent_cursor(layout, space) {
        return Ok(Some(cursor));
    }
    // An archived record never comes back on its own: a space whose only
    // record was archived reads as having no active intent.
    let registry = read_intent_registry(layout, space);
    let mut live: Vec<String> = Vec::new();
    for dir_name in list_intent_dirs(layout, space) {
        let mut row = None;
        for candidate in &registry {
            if record_dir_matches(candidate, &dir_name).map_err(|()| malformed(layout, space))? {
                row = Some(candidate);
                break;
            }
        }
        let archived = match row {
            Some(row) => is_archived(row).map_err(|()| malformed(layout, space))?,
            None => false,
        };
        if !archived {
            live.push(dir_name);
        }
    }
    Ok(if live.len() == 1 { live.pop() } else { None })
}

/// A session id in the one shape the engine files sessions under: letters,
/// digits, `.`, `_` and `-`, not starting or ending with `-`, at most 180
/// characters, and not `.` or `..`.
#[must_use]
pub fn valid_session_id(session_id: Option<&str>) -> Option<&str> {
    let raw = session_id.filter(|raw| !raw.is_empty())?;
    let safe = re::test(r"^[A-Za-z0-9._-]+$", "", raw)
        && !raw.starts_with('-')
        && !raw.ends_with('-')
        && raw.len() <= 180
        && raw != "."
        && raw != "..";
    safe.then_some(raw)
}

/// A record directory name a session may be bound to: one path segment, with
/// no control character and no surrounding white space.
fn is_bindable_record_name(name: &str) -> bool {
    !name.is_empty()
        && trim(name) == name
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !(cfg!(windows) && name.contains('\\'))
        && name
            .chars()
            .all(|c| c >= '\u{20}' && c != '\u{7f}' && !('\u{80}'..='\u{9f}').contains(&c))
}

/// The binding a session holds; `None` when it has none, or when what is on
/// disk is malformed or names a record that no longer exists. A bad binding
/// is no binding: the cursors remain the fallback.
#[must_use]
pub fn read_session_binding(layout: &Layout, session_id: &str) -> Option<Binding> {
    let id = valid_session_id(Some(session_id))?;
    let path = layout
        .workspace_root()
        .join(SESSIONS_DIR)
        .join(format!("{id}.binding.json"));
    let bytes = std::fs::read(path).ok()?;
    let parsed = json::parse(&String::from_utf8_lossy(&bytes)).ok()?;
    let space = parsed
        .get("space")?
        .as_str()
        .filter(|space| is_space_name(space))?;
    let intent = match parsed.get("intent")? {
        Value::Null => None,
        Value::String(name) if is_bindable_record_name(name) => Some(name.clone()),
        _ => return None,
    };
    let bound_at = parsed
        .get("boundAt")?
        .as_str()
        .filter(|at| !at.is_empty())?;
    if let Some(name) = &intent
        && !layout
            .intents_dir(space)
            .join(name)
            .join(STATE_FILE)
            .exists()
    {
        return None;
    }
    let source = parsed
        .get("source")
        .and_then(Value::as_str)
        .filter(|source| BINDING_SOURCES.contains(source))
        .map(str::to_owned);
    Some(Binding {
        space: space.to_owned(),
        intent,
        bound_at: bound_at.to_owned(),
        source,
    })
}

/// A selector becomes one path segment, so it must not be a path: `..` in
/// `--space` or `--intent` would otherwise reach outside the project. A
/// backslash separates only on Windows; elsewhere it is a character a
/// record's name may carry.
fn is_path(value: &str) -> bool {
    value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\0')
        || (cfg!(windows) && value.contains('\\'))
}

/// Why this project's selection cannot be settled yet, when the reason is
/// something Focus has not ported: a delegated worktree, or sessions recorded
/// by process with no session named. Upstream settles both. A caller that
/// can do without the selection may carry on where this answers.
#[must_use]
pub fn not_ported(layout: &Layout, selector: &Selector, invoker: &Invoker) -> Option<EngineError> {
    let worktree_meta = layout.project.join(WORKTREE_META);
    if worktree_meta.exists() {
        return Some(EngineError::new(format!(
            "{} marks this checkout as a worktree an engine created for one unit of work. Focus does not work inside delegated worktrees yet. Run the command from the main checkout.",
            worktree_meta.display()
        )));
    }
    let named = valid_session_id(selector.session_id.as_deref());
    let handed_down = valid_session_id(invoker.session_override.as_deref());
    let pid_map = layout.workspace_root().join(SESSIONS_DIR).join("pids");
    if named.is_none() && !(handed_down.is_some() && invoker.from_payload) && pid_map.exists() {
        // Upstream would walk the process tree to find which recorded
        // conversation is calling, and refuse an exported override that
        // disagrees with it. Guessing here could act on another
        // conversation's intent.
        return Some(EngineError::new(format!(
            "{} records which process belongs to which conversation, and Focus cannot yet tell which conversation is calling. Name the session, or pass --space and --intent.",
            pid_map.display()
        )));
    }
    None
}

/// Settle the space and the intent a command acts on.
///
/// # Errors
///
/// A selector is a path; the project is a delegated worktree, or has sessions
/// recorded by process and the caller named none (neither is ported); or the
/// registry has a row that cannot be read.
pub fn resolve_selection(
    layout: &Layout,
    selector: &Selector,
    invoker: &Invoker,
) -> Result<Selection> {
    if let Some(space) = selector.space.as_deref().filter(|space| is_path(space)) {
        bail!("\"{space}\" is not a space name: it is a path.");
    }
    if let Some(intent) = selector.intent.as_deref().filter(|intent| is_path(intent)) {
        bail!("\"{intent}\" is not an intent name: it is a path.");
    }
    if let Some(refusal) = not_ported(layout, selector, invoker) {
        return Err(refusal);
    }
    let named = valid_session_id(selector.session_id.as_deref());
    let handed_down = valid_session_id(invoker.session_override.as_deref());
    let session_id = named.or(handed_down);
    let binding = session_id.and_then(|id| read_session_binding(layout, id));

    let space = match (&selector.space, &binding) {
        (Some(space), _) => space.clone(),
        (None, Some(binding)) => binding.space.clone(),
        (None, None) => layout.active_space(),
    };
    let intent = match (&selector.intent, &binding) {
        (Some(intent), _) => Some(intent.clone()),
        (None, Some(binding)) if binding.space == space => binding.intent.clone(),
        _ => active_intent(layout, &space)?,
    };
    Ok(Selection {
        space,
        intent,
        session_id: session_id.map(str::to_owned),
        binding,
    })
}

impl Selection {
    /// The record directory, `focus/spaces/<space>/intents/<record>`; `None`
    /// when no intent resolved.
    #[must_use]
    pub fn record_dir(&self, layout: &Layout) -> Option<PathBuf> {
        self.intent
            .as_ref()
            .map(|intent| join_as_written(&layout.intents_dir(&self.space), intent))
    }

    /// Where the record's files are: the record directory, or the space's
    /// `intents` directory while no intent resolves (`docsRoot`).
    #[must_use]
    pub fn docs_root(&self, layout: &Layout) -> PathBuf {
        self.record_dir(layout)
            .unwrap_or_else(|| layout.intents_dir(&self.space))
    }

    /// The engine's own directory under [`Self::docs_root`] (`engineDir`).
    #[must_use]
    pub fn engine_dir(&self, layout: &Layout) -> PathBuf {
        self.docs_root(layout).join(ENGINE_DIR)
    }

    /// The record directory as text relative to the project, with forward
    /// slashes on every platform: the form written into what agents read.
    #[must_use]
    pub fn relative_record_dir(&self) -> Option<String> {
        self.intent
            .as_ref()
            .map(|intent| format!("{WORKSPACE_DIR}/spaces/{}/intents/{intent}", self.space))
    }
}

#[cfg(test)]
mod tests {
    use focus_harness::Harness;

    use super::*;

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("focus-records-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(Layout::new(root, Harness::Claude))
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.project.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn record(&self, space: &str, name: &str) {
            self.write(
                &format!("focus/spaces/{space}/intents/{name}/focus-state.md"),
                "state",
            );
        }

        fn select(&self, selector: &Selector) -> Result<Selection> {
            resolve_selection(&self.0, selector, &Invoker::default())
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    #[test]
    fn a_lone_record_is_the_intent_and_two_need_a_cursor() {
        let tree = Tree::new("lone");
        assert_eq!(tree.select(&Selector::default()).unwrap().intent, None);

        tree.record("default", "add-login-1a2b3c4d");
        let lone = tree.select(&Selector::default()).unwrap();
        assert_eq!(lone.space, "default");
        assert_eq!(lone.intent.as_deref(), Some("add-login-1a2b3c4d"));
        assert_eq!(
            lone.relative_record_dir().as_deref(),
            Some("focus/spaces/default/intents/add-login-1a2b3c4d")
        );

        tree.record("default", "fix-logout-9f8e7d6c");
        assert_eq!(tree.select(&Selector::default()).unwrap().intent, None);
        tree.write(
            "focus/spaces/default/intents/active-intent",
            " fix-logout-9f8e7d6c\n",
        );
        assert_eq!(
            tree.select(&Selector::default()).unwrap().intent.as_deref(),
            Some("fix-logout-9f8e7d6c")
        );
    }

    #[test]
    fn an_archived_record_does_not_come_back_on_its_own() {
        let tree = Tree::new("archived");
        tree.record("default", "add-login-1a2b3c4d");
        tree.write(
            "focus/spaces/default/intents/intents.json",
            r#"[{"uuid":"00000000-0000-0000-0000-00001a2b3c4d","slug":"add-login","status":" Archived "}]"#,
        );
        assert_eq!(tree.select(&Selector::default()).unwrap().intent, None);
        // The cursor still names it, so the engine can explain it.
        tree.write(
            "focus/spaces/default/intents/active-intent",
            "add-login-1a2b3c4d",
        );
        assert!(tree.select(&Selector::default()).unwrap().intent.is_some());
    }

    #[test]
    fn a_selector_that_is_a_path_is_refused() {
        let tree = Tree::new("paths");
        let space = Selector {
            space: Some("../x".to_owned()),
            ..Selector::default()
        };
        assert_eq!(
            tree.select(&space).unwrap_err().message(),
            "\"../x\" is not a space name: it is a path."
        );
        let intent = Selector {
            intent: Some("a/b".to_owned()),
            ..Selector::default()
        };
        assert_eq!(
            tree.select(&intent).unwrap_err().message(),
            "\"a/b\" is not an intent name: it is a path."
        );
    }

    #[test]
    fn a_session_binding_outranks_the_cursor_and_a_selector_outranks_both() {
        let tree = Tree::new("binding");
        tree.record("default", "add-login-1a2b3c4d");
        tree.record("payments", "refunds-0000aaaa");
        tree.write(
            "focus/spaces/default/intents/active-intent",
            "add-login-1a2b3c4d",
        );
        tree.write(
            "focus/.focus-sessions/s1.binding.json",
            r#"{"space":"payments","intent":"refunds-0000aaaa","boundAt":"t","source":"switch"}"#,
        );
        let bound = Selector {
            session_id: Some("s1".to_owned()),
            ..Selector::default()
        };
        let selection = tree.select(&bound).unwrap();
        assert_eq!(
            (selection.space.as_str(), selection.intent.as_deref()),
            ("payments", Some("refunds-0000aaaa"))
        );
        assert_eq!(selection.binding.unwrap().source.as_deref(), Some("switch"));

        // Another space named outright: the binding's intent does not follow.
        let elsewhere = Selector {
            space: Some("default".to_owned()),
            ..bound.clone()
        };
        assert_eq!(
            tree.select(&elsewhere).unwrap().intent.as_deref(),
            Some("add-login-1a2b3c4d")
        );

        // The same session handed down by a hook resolves the same way.
        let invoker = Invoker {
            session_override: Some("s1".to_owned()),
            from_payload: true,
        };
        let handed = resolve_selection(&tree.0, &Selector::default(), &invoker).unwrap();
        assert_eq!(handed.space, "payments");
    }

    /// Deliberate deviation. Upstream walks the process tree to find the
    /// calling conversation, and resolves a delegated worktree to its parent's
    /// intent. Neither is ported, and both are refused rather than skipped.
    #[test]
    fn unported_ways_of_naming_the_caller_are_refused() {
        let tree = Tree::new("unported");
        tree.write("focus/.focus-sessions/pids/123", "{}");
        let refused = tree.select(&Selector::default()).unwrap_err();
        assert!(
            refused
                .message()
                .contains("cannot yet tell which conversation is calling"),
            "{refused}"
        );
        // Naming the session is enough to go on.
        let named = Selector {
            session_id: Some("s1".to_owned()),
            ..Selector::default()
        };
        assert!(tree.select(&named).is_ok());

        let tree = Tree::new("worktree");
        tree.write(".focus/worktree-meta.json", "{}");
        let refused = tree.select(&Selector::default()).unwrap_err();
        assert!(
            refused
                .message()
                .contains("does not work inside delegated worktrees yet"),
            "{refused}"
        );
    }
}

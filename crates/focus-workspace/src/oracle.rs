//! The oracle protocol for record resolution.
//!
//! A `selection` case describes a project as a set of files, a selector and
//! what the environment says about the caller. Each side builds the project
//! in a scratch directory of its own, resolves, and answers with paths
//! relative to it (see `focus-graph`'s `oracle` module for the protocol).
//!
//! Where upstream fails on a registry row with whatever its runtime says
//! about the value, its driver answers `<runtime>`; so does this side, for
//! the refusal Focus words itself. The corpus then pins when the failure
//! happens, which is the part that is the engine's.

use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};

use focus_harness::Harness;
use serde_json::{Map, Value, json};

use crate::project::Layout;
use crate::records::{Invoker, Selector, list_intent_dirs, resolve_selection};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 1] = ["selection"];

/// How a case names the machine answering it, in a path or a clone file.
pub const MACHINE: &str = "{H}";

static TREES: AtomicU64 = AtomicU64::new(0);

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    match case.get("kind").and_then(Value::as_str).unwrap_or_default() {
        "selection" => selection(case),
        other => json!({ "unsupported": other }),
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn selection(case: &Value) -> Value {
    in_scratch_tree(case, |root| resolve(root, case))
}

/// Build the project a case describes in a scratch directory of its own, ask
/// `answer` about it, and remove it. `{H}` in a file's path stands for the
/// name of the machine answering, which `host` gives; a case that needs none
/// passes `None` and the paths are used as written.
pub fn in_scratch_tree(case: &Value, answer: impl FnOnce(&Path) -> Value) -> Value {
    in_scratch_tree_on(case, None, answer)
}

/// [`in_scratch_tree`] for a case whose paths may name the machine.
pub fn in_scratch_tree_on(
    case: &Value,
    host: Option<&str>,
    answer: impl FnOnce(&Path) -> Value,
) -> Value {
    let root = std::env::temp_dir().join(format!(
        "focus-oracle-{}-{}",
        std::process::id(),
        TREES.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let answer = match build(&root, case, host) {
        Ok(()) => answer(&root),
        Err(err) => json!({ "error": format!("could not build the case's tree: {err}") }),
    };
    let _ = std::fs::remove_dir_all(&root);
    answer
}

fn build(root: &Path, case: &Value, host: Option<&str>) -> std::io::Result<()> {
    let named = |rel: &str| match host {
        Some(host) => root.join(rel.replace(MACHINE, host)),
        None => root.join(rel),
    };
    std::fs::create_dir_all(root)?;
    for (rel, content) in case
        .get("files")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let path = named(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = content.as_str().unwrap_or_default();
        match host {
            Some(host) => std::fs::write(path, content.replace(MACHINE, host))?,
            None => std::fs::write(path, content)?,
        }
    }
    // Written last, so that building the tree does not disturb them.
    for (rel, seconds) in case
        .get("mtimes")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let at = std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(seconds.as_u64().unwrap_or_default());
        std::fs::OpenOptions::new()
            .write(true)
            .open(named(rel))?
            .set_modified(at)?;
    }
    Ok(())
}

fn resolve(root: &Path, case: &Value) -> Value {
    let layout = Layout::new(root, Harness::Claude);
    let chosen = case.get("selector");
    let selector = Selector {
        space: text(chosen.and_then(|s| s.get("space"))),
        intent: text(chosen.and_then(|s| s.get("intent"))),
        session_id: text(chosen.and_then(|s| s.get("sessionId"))),
    };
    let handed = case.get("override");
    let invoker = Invoker {
        session_override: text(handed.and_then(|o| o.get("id"))),
        from_payload: handed.and_then(|o| o.get("payload")) == Some(&Value::Bool(true)),
    };
    let selection = match resolve_selection(&layout, &selector, &invoker) {
        Ok(selection) => selection,
        Err(err) if err.message().contains("has a row that is not an intent") => {
            return json!({ "error": "<runtime>" });
        }
        Err(err) => {
            return json!({ "error": err.message().replace(&root.display().to_string(), "{P}") });
        }
    };

    let mut answer = Map::new();
    answer.insert("space".to_owned(), json!(selection.space));
    answer.insert("intent".to_owned(), json!(selection.intent));
    answer.insert("sessionId".to_owned(), json!(selection.session_id));
    answer.insert(
        "binding".to_owned(),
        match &selection.binding {
            None => Value::Null,
            Some(binding) => {
                let mut held = Map::new();
                held.insert("space".to_owned(), json!(binding.space));
                held.insert("intent".to_owned(), json!(binding.intent));
                held.insert("boundAt".to_owned(), json!(binding.bound_at));
                if let Some(source) = &binding.source {
                    held.insert("source".to_owned(), json!(source));
                }
                Value::Object(held)
            }
        },
    );
    // Upstream's path helpers take no session, so the driver can only ask
    // them when the case names none.
    if selector.session_id.is_none() {
        let record = selection
            .record_dir(&layout)
            .map(|dir| relative(root, &dir));
        answer.insert("recordDir".to_owned(), json!(record));
        answer.insert(
            "relativeRecordDir".to_owned(),
            json!(selection.relative_record_dir()),
        );
    }
    answer.insert(
        "intents".to_owned(),
        json!(list_intent_dirs(&layout, &selection.space)),
    );
    Value::Object(answer)
}

/// `path` relative to `root`, with forward slashes; a path outside `root`
/// climbs with `..`, as `path.relative` writes it.
#[must_use]
pub fn relative(root: &Path, path: &Path) -> String {
    let (root, path): (Vec<Component>, Vec<Component>) =
        (root.components().collect(), path.components().collect());
    let shared = root.iter().zip(&path).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = vec!["..".to_owned(); root.len() - shared];
    parts.extend(
        path[shared..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

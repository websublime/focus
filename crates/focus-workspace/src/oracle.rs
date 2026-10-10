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
use crate::settings::{Layer, Places, is_kill_switch_source, normalize_settings};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 3] = ["selection", "settings_file", "settings_layers"];

/// How a case names the machine answering it, in a path or a clone file.
pub const MACHINE: &str = "{H}";

static TREES: AtomicU64 = AtomicU64::new(0);

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    match case.get("kind").and_then(Value::as_str).unwrap_or_default() {
        "selection" => selection(case),
        "settings_file" => settings_file(case),
        "settings_layers" => in_scratch_tree(case, |root| settings_layers(root, case)),
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

/// Where a settings case's machine file is, as an answer names it.
const MACHINE_FILE: &str = "{M}/focus.settings.json";

fn layer_named(name: &str) -> Layer {
    match name {
        "machine" => Layer::Machine,
        "local" => Layer::Local,
        _ => Layer::Project,
    }
}

fn settings_file(case: &Value) -> Value {
    let raw = case["raw"].as_str().unwrap_or_default();
    let Ok(parsed) = focus_js::json::parse(raw) else {
        return json!({ "error": "<json>" });
    };
    let layer = layer_named(case["layer"].as_str().unwrap_or_default());
    match normalize_settings(&parsed, layer, "{F}", MACHINE_FILE) {
        Ok(value) => json!({ "value": value }),
        Err(err) => json!({ "error": err.message() }),
    }
}

fn settings_layers(root: &Path, case: &Value) -> Value {
    let (project, machine) = (root.join("project"), root.join("machine"));
    let _ = std::fs::create_dir_all(&project);
    let _ = std::fs::create_dir_all(&machine);
    let places = Places::with_install_root(&project, &machine);
    // The runtime's words for a file that is not JSON are its own.
    let named = |text: &str| {
        let text = match text.find(": invalid JSON: ") {
            Some(at) => format!("{}<runtime>", &text[..at + ": invalid JSON: ".len()]),
            None => text.to_owned(),
        };
        // A file is named with the platform's separator; the answer uses `/`.
        let separator = std::path::MAIN_SEPARATOR;
        let (machine, project) = (machine.display().to_string(), project.display().to_string());
        text.replace(&format!("{machine}{separator}"), "{M}/")
            .replace(&format!("{project}{separator}"), "{P}/")
            .replace(&machine, "{M}")
            .replace(&project, "{P}")
    };
    let given = case.get("env").and_then(Value::as_object);
    let env = |name: &str| -> Option<String> { given?.get(name)?.as_str().map(str::to_owned) };

    let mut answer = Map::new();
    match places.resolve() {
        Ok(resolved) => {
            let mut sources = Map::new();
            for (path, layer) in &resolved.sources {
                sources.insert(path.clone(), json!(layer.name()));
            }
            answer.insert("value".to_owned(), resolved.value);
            answer.insert("sources".to_owned(), Value::Object(sources));
            answer.insert("present".to_owned(), json!(resolved.present));
        }
        Err(err) => {
            answer.insert("error".to_owned(), json!(named(err.message())));
        }
    }
    let (mut flags, mut switches, mut recorded) = (Map::new(), Map::new(), Map::new());
    for name in case["ask"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
    {
        flags.insert(
            name.to_owned(),
            match places.flag(name, &env) {
                Ok(value) => json!(value),
                Err(_) => json!({ "error": true }),
            },
        );
        switches.insert(
            name.to_owned(),
            json!(named(&places.kill_switch_source(name, &env))),
        );
        recorded.insert(
            name.to_owned(),
            json!(places.bypass_recorded_in(name).map(Layer::target)),
        );
    }
    answer.insert("flags".to_owned(), Value::Object(flags));
    answer.insert("switches".to_owned(), Value::Object(switches));
    answer.insert("recorded".to_owned(), Value::Object(recorded));
    answer.insert(
        "isSwitch".to_owned(),
        json!(
            case["probe"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(is_kill_switch_source)
                .collect::<Vec<_>>()
        ),
    );
    Value::Object(answer)
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

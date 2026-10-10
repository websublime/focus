//! Settings: what a machine, a project and one person's clone have recorded.
//!
//! Three files hold them, each optional, each the same shape:
//!
//! - the **machine**'s, `focus.settings.json` in the install root;
//! - the **project**'s, `focus.settings.json` at the project root, committed
//!   and shared with the team;
//! - the **local** one, `focus.settings.local.json` at the project root, one
//!   person's and never committed.
//!
//! A nearer file wins value by value: local over project over machine. The
//! one exception is the list of recorded switches (`flags.bypasses`), each of
//! which turns one check off. That list is the union of all three files: a
//! nearer file adds switches and never silently turns back on a check
//! another file switched off.
//!
//! A file is read whole or not at all. An unknown key, a value of the wrong
//! kind, or a machine-only key in a project file is an error naming the file,
//! so a typo cannot pass for a setting that is merely absent.
//!
//! Mirrors `aidlc-settings.ts`, the install root of `aidlc-install-paths.ts`,
//! and `resolveProjectFlag`, `killSwitchSource` and `isKillSwitchSource` in
//! `aidlc-lib.ts` (L699 to L742).

use std::path::{Path, PathBuf};

use focus_js::order::{cmp_utf16, js_key_order};
use focus_js::text::trim;
use focus_js::{EngineError, Result, bail, json, num, re};
use serde_json::{Map, Value, json};

use crate::names::{ENV_PREFIX, LOCAL_SETTINGS_FILE, SETTINGS_FILE, WORKSPACE_DIR};

/// The switches a settings file can record, each named after the environment
/// variable that does the same for one process. Every one turns a check or a
/// ceremony off.
pub const RECORDABLE_BYPASSES: [&str; 12] = [
    "FOCUS_SKIP_ARTIFACT_GUARD",
    "FOCUS_SKIP_REVISION_BACKSTOP",
    "FOCUS_SKIP_SUMMARY_CONFIRMATION_GUARD",
    "FOCUS_SKIP_HUMAN_PRESENCE_GUARD",
    "FOCUS_DISABLE_ENSEMBLE_EVIDENCE",
    "FOCUS_DISABLE_PLAN_APPROVAL_GUARD",
    "FOCUS_DISABLE_REVIEWER_SCOPE_HOOK",
    "FOCUS_DISABLE_REVIEW_FREEZE_HOOK",
    "FOCUS_DISABLE_USAGE_TRACKING",
    "FOCUS_DISABLE_SENSORS",
    "FOCUS_DISABLE_LEARNINGS",
    "FOCUS_DISABLE_SUMMARY_CONFIRMATION",
];

/// The variables that have a field of their own under `flags`.
const FLAG_FIELDS: [(&str, &str); 5] = [
    ("FOCUS_DEFAULT_SCOPE", "defaultScope"),
    ("FOCUS_USE_SWARM", "swarm"),
    ("FOCUS_HOOK_DEBUG", "hookDebug"),
    ("FOCUS_SENSOR_TIMEOUT_MS", "sensorTimeoutMs"),
    ("FOCUS_QUESTION_RETENTION_DAYS", "questionRetentionDays"),
];

const MACHINE_KEYS: [&str; 4] = ["update-check", "offline", "release-base-url", "ca-bundle"];
const TOP_LEVEL_KEYS: [&str; 8] = [
    "$schema",
    "schemaVersion",
    "models",
    "flags",
    "update-check",
    "offline",
    "release-base-url",
    "ca-bundle",
];
const FLAG_KEYS: [&str; 7] = [
    "schemaVersion",
    "defaultScope",
    "swarm",
    "hookDebug",
    "sensorTimeoutMs",
    "questionRetentionDays",
    "bypasses",
];
const MODEL_KEYS: [&str; 5] = ["schemaVersion", "preset", "groups", "agents", "profiles"];
const MODEL_GROUPS: [&str; 3] = ["deciding", "reviewing", "writing-up"];
const MODEL_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const MODEL_PRESETS: [&str; 3] = ["thorough", "balanced", "minimal"];
const MODEL_HARNESSES: [&str; 7] = [
    "claude", "codex", "copilot", "cursor", "kiro", "kiro-ide", "opencode",
];
const PROFILE_NAME: &str = "^[a-z0-9][a-z0-9-]*$";

/// Which of the three files a value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Machine,
    Project,
    Local,
}

impl Layer {
    /// Nearest last: the order values are merged in.
    pub const ALL: [Self; 3] = [Self::Machine, Self::Project, Self::Local];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Machine => "machine",
            Self::Project => "project",
            Self::Local => "local",
        }
    }

    /// What the `config` command calls the file: `global`, `project`, `local`.
    #[must_use]
    pub fn target(self) -> &'static str {
        match self {
            Self::Machine => "global",
            other => other.name(),
        }
    }
}

fn record(value: &Value) -> Option<&Map<String, Value>> {
    value.as_object()
}

/// `value === 1`, however the number was written (`1`, `1.0`, `1e0`).
fn is_one(value: Option<&Value>) -> bool {
    value.and_then(Value::as_f64) == Some(1.0)
}

fn unknown_keys(map: &Map<String, Value>, known: &[&str]) -> Vec<String> {
    map.keys()
        .filter(|key| !known.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// A JSON number that is a positive integer, as `Number.isInteger` and `> 0`
/// see it.
fn positive_integer(value: &Value) -> bool {
    value
        .as_f64()
        .is_some_and(|number| num::is_integer(number) && number > 0.0)
}

/// The `flags` section, checked and in its written order; `None` when the
/// file has none. `at` names the section in a refusal.
///
/// # Errors
///
/// The section is not an object, its `schemaVersion` is not 1, it has a key
/// that is not a flag, or a flag has a value of the wrong kind.
pub fn normalize_flags(value: Option<&Value>, at: &str) -> Result<Option<Value>> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let Some(given) = record(value) else {
        bail!("{at} must be an object");
    };
    if !is_one(given.get("schemaVersion")) {
        bail!("{at}.schemaVersion must be 1");
    }
    let unknown = unknown_keys(given, &FLAG_KEYS);
    if !unknown.is_empty() {
        bail!("{at} has unknown key(s): {}", unknown.join(", "));
    }
    let mut out = Map::new();
    out.insert("schemaVersion".to_owned(), json!(1));
    if let Some(scope) = given.get("defaultScope") {
        match scope.as_str() {
            Some(name) if re::test("^[a-z][a-z0-9-]*$", "", name) => {
                out.insert("defaultScope".to_owned(), json!(name));
            }
            _ => bail!("{at}.defaultScope must be a scope name"),
        }
    }
    for key in ["swarm", "hookDebug"] {
        match given.get(key) {
            None => {}
            Some(Value::Bool(on)) => {
                out.insert(key.to_owned(), json!(on));
            }
            Some(_) => bail!("{at}.{key} must be true or false"),
        }
    }
    for key in ["sensorTimeoutMs", "questionRetentionDays"] {
        match given.get(key) {
            None => {}
            Some(number) if positive_integer(number) => {
                out.insert(key.to_owned(), number.clone());
            }
            Some(_) => bail!("{at}.{key} must be a positive integer"),
        }
    }
    if let Some(bypasses) = given.get("bypasses") {
        let names: Option<Vec<&str>> = bypasses.as_array().and_then(|items| {
            items
                .iter()
                .map(|item| {
                    item.as_str()
                        .filter(|name| RECORDABLE_BYPASSES.contains(name))
                })
                .collect()
        });
        let Some(mut names) = names else {
            bail!(
                "{at}.bypasses must contain only {}",
                RECORDABLE_BYPASSES.join(", ")
            );
        };
        names.sort_by(|a, b| cmp_utf16(a, b));
        names.dedup();
        out.insert("bypasses".to_owned(), json!(names));
    }
    Ok(Some(Value::Object(out)))
}

fn harness_models(value: &Value, at: &str) -> Result<Map<String, Value>> {
    let Some(given) = record(value) else {
        bail!("{at} must be an object keyed by harness");
    };
    let mut out = Map::new();
    for (harness, model) in given {
        if !MODEL_HARNESSES.contains(&harness.as_str()) {
            bail!(
                "{at} has unknown harness {}",
                json::stringify(&json!(harness))
            );
        }
        match model.as_str().map(trim) {
            Some(model) if !model.is_empty() => {
                out.insert(harness.clone(), json!(model));
            }
            _ => bail!("{at}.{harness} must be a non-empty model ID"),
        }
    }
    Ok(out)
}

fn model_groups(value: &Value, at: &str) -> Result<Map<String, Value>> {
    let Some(given) = record(value) else {
        bail!("{at} must be an object");
    };
    let mut groups = Map::new();
    for (name, policy) in given {
        if !MODEL_GROUPS.contains(&name.as_str()) {
            bail!("{at} has unknown group {}", json::stringify(&json!(name)));
        }
        let effort = record(policy)
            .filter(|policy| policy.keys().all(|key| key == "effort"))
            .and_then(|policy| policy.get("effort"))
            .and_then(Value::as_str)
            .filter(|effort| MODEL_EFFORTS.contains(effort));
        let Some(effort) = effort else {
            bail!("{at}.{name} must contain one valid effort");
        };
        groups.insert(name.clone(), json!({ "effort": effort }));
    }
    Ok(groups)
}

/// The `models` section, checked; `None` when the file has none.
///
/// # Errors
///
/// The section, a group, an agent or a profile is not what the schema allows.
pub fn normalize_models(value: Option<&Value>, at: &str) -> Result<Option<Value>> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    let Some(given) = record(value) else {
        bail!("{at} must be an object");
    };
    if !is_one(given.get("schemaVersion")) {
        bail!("{at}.schemaVersion must be 1");
    }
    let unknown = unknown_keys(given, &MODEL_KEYS);
    if !unknown.is_empty() {
        bail!("{at} has unknown key(s): {}", unknown.join(", "));
    }
    let mut out = Map::new();
    out.insert("schemaVersion".to_owned(), json!(1));
    if let Some(preset) = given.get("preset") {
        match preset.as_str() {
            Some(name) if MODEL_PRESETS.contains(&name) => {
                out.insert("preset".to_owned(), json!(name));
            }
            _ => bail!("{at}.preset must be one of {}", MODEL_PRESETS.join(", ")),
        }
    }
    if let Some(groups) = given.get("groups") {
        out.insert(
            "groups".to_owned(),
            Value::Object(model_groups(groups, &format!("{at}.groups"))?),
        );
    }
    let mut agents = Map::new();
    if let Some(given_agents) = given.get("agents") {
        let Some(given_agents) = record(given_agents) else {
            bail!("{at}.agents must be an object");
        };
        for (name, raw) in given_agents {
            let Some(raw) = record(raw).filter(|_| re::test(PROFILE_NAME, "", name)) else {
                bail!(
                    "{at}.agents has invalid agent {}",
                    json::stringify(&json!(name))
                );
            };
            let unknown = unknown_keys(raw, &["effort", "model"]);
            if !unknown.is_empty() {
                bail!(
                    "{at}.agents.{name} has unknown key(s): {}",
                    unknown.join(", ")
                );
            }
            let effort = match raw.get("effort") {
                None => None,
                Some(effort) => match effort.as_str() {
                    Some(effort) if MODEL_EFFORTS.contains(&effort) => Some(effort),
                    _ => bail!("{at}.agents.{name}.effort must be valid"),
                },
            };
            let model = match raw.get("model") {
                None => None,
                Some(model) => Some(harness_models(model, &format!("{at}.agents.{name}.model"))?),
            }
            .filter(|model| !model.is_empty());
            if effort.is_some() || model.is_some() {
                let mut agent = Map::new();
                if let Some(effort) = effort {
                    agent.insert("effort".to_owned(), json!(effort));
                }
                if let Some(model) = model {
                    agent.insert("model".to_owned(), Value::Object(model));
                }
                agents.insert(name.clone(), Value::Object(agent));
            }
        }
    }
    if !agents.is_empty() {
        out.insert("agents".to_owned(), Value::Object(js_key_order(agents)));
    }
    if let Some(given_profiles) = given.get("profiles") {
        let Some(given_profiles) = record(given_profiles) else {
            bail!("{at}.profiles must be an object");
        };
        let mut profiles = Map::new();
        for (name, raw) in given_profiles {
            let groups = record(raw)
                .filter(|raw| {
                    re::test(PROFILE_NAME, "", name) && raw.keys().all(|key| key == "groups")
                })
                .and_then(|raw| raw.get("groups"));
            let Some(groups) = groups else {
                bail!("{at}.profiles.{name} must contain only groups");
            };
            let groups = model_groups(groups, &format!("{at}.profiles.{name}.groups"))?;
            profiles.insert(name.clone(), json!({ "groups": groups }));
        }
        if !profiles.is_empty() {
            out.insert("profiles".to_owned(), Value::Object(js_key_order(profiles)));
        }
    }
    Ok(Some(Value::Object(out)))
}

/// Whether `value` is a release location Focus accepts: `https://host`, with
/// an optional port and path, or `http://` to this machine only. No
/// credentials, query or fragment.
///
/// Upstream asks its runtime's URL parser, which accepts many spellings of
/// one location. This reads one spelling: lower-case scheme, two slashes, a
/// host of letters, digits, dots and hyphens. A value written any other way
/// is refused, with upstream's sentence.
fn valid_release_url(value: &str) -> bool {
    let Some(found) = re::find(
        r"^(https?)://([A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?)(?::([0-9]{1,5}))?(/[A-Za-z0-9._~!$&'()*+,;=:@%/-]*)?$",
        "",
        value,
    ) else {
        return false;
    };
    let part = |index: usize| re::group(value, &found, index).unwrap_or_default();
    let (scheme, host, port) = (part(1), part(2).to_ascii_lowercase(), part(3));
    if !port.is_empty() && port.parse::<u32>().is_ok_and(|port| port > 65_535) {
        return false;
    }
    // A host whose last label is a number is an address, and has to be one.
    let labels: Vec<&str> = host.split('.').collect();
    let numeric = |label: &&str| !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit());
    if labels.last().is_some_and(numeric) {
        let address = labels.len() == 4
            && labels.iter().all(|label| {
                numeric(label) && label.len() <= 3 && label.parse::<u16>().is_ok_and(|n| n <= 255)
            })
            && labels
                .iter()
                .all(|label| label.len() == 1 || !label.starts_with('0'));
        if !address {
            return false;
        }
    }
    if labels.iter().any(|label| label.is_empty()) {
        return false;
    }
    scheme == "https" || host == "127.0.0.1" || host == "localhost"
}

/// Whether Node's `path.isAbsolute` says so on this platform.
fn is_absolute(path: &str) -> bool {
    if cfg!(windows) {
        let bytes = path.as_bytes();
        let separator = |byte: u8| byte == b'/' || byte == b'\\';
        match bytes {
            [first, ..] if separator(*first) => true,
            [drive, b':', third, ..] => drive.is_ascii_alphabetic() && separator(*third),
            _ => false,
        }
    } else {
        path.starts_with('/')
    }
}

/// One settings file, checked: only known keys, each of the right kind, in a
/// fixed order. `path` names the file in a refusal; `machine_file` is where a
/// machine-only key belongs.
///
/// # Errors
///
/// The value is not an object, has an unknown key or a value of the wrong
/// kind, or holds a machine-only key in a project or local file.
pub fn normalize_settings(
    value: &Value,
    layer: Layer,
    path: &str,
    machine_file: &str,
) -> Result<Value> {
    let Some(given) = record(value) else {
        bail!("{path}: settings must be a JSON object");
    };
    let unknown = unknown_keys(given, &TOP_LEVEL_KEYS);
    if !unknown.is_empty() {
        bail!(
            "{path}: settings contain unknown key(s): {}",
            unknown.join(", ")
        );
    }
    if !is_one(given.get("schemaVersion")) {
        bail!("{path}: settings.schemaVersion must be 1");
    }
    let schema = match given.get("$schema") {
        None => None,
        Some(Value::String(schema)) if !trim(schema).is_empty() => Some(schema),
        Some(_) => bail!("{path}: settings.$schema must be a non-empty string"),
    };
    let machine_only: Vec<&str> = MACHINE_KEYS
        .into_iter()
        .filter(|key| given.contains_key(*key))
        .collect();
    if layer != Layer::Machine && !machine_only.is_empty() {
        bail!(
            "{path}: {} {} machine-only; write {machine_file} instead",
            machine_only.join(", "),
            if machine_only.len() == 1 { "is" } else { "are" }
        );
    }
    for key in ["update-check", "offline"] {
        if given.get(key).is_some_and(|value| !value.is_boolean()) {
            bail!("{path}: settings.{key} must be true or false");
        }
    }
    let release = match given.get("release-base-url") {
        None => None,
        Some(Value::String(url)) if valid_release_url(url) => Some(url.trim_end_matches('/')),
        Some(_) => bail!(
            "{path}: settings.release-base-url must be HTTPS without credentials, query, or fragment"
        ),
    };
    let bundle = match given.get("ca-bundle") {
        None => None,
        Some(Value::String(bundle)) if is_absolute(bundle) => Some(bundle),
        Some(_) => bail!("{path}: settings.ca-bundle must be an absolute path"),
    };
    let models = normalize_models(given.get("models"), &format!("{path}: models"))?;
    let flags = normalize_flags(given.get("flags"), &format!("{path}: flags"))?;

    let mut out = Map::new();
    if let Some(schema) = schema {
        out.insert("$schema".to_owned(), json!(schema));
    }
    out.insert("schemaVersion".to_owned(), json!(1));
    if let Some(models) = models {
        out.insert("models".to_owned(), models);
    }
    if let Some(flags) = flags {
        out.insert("flags".to_owned(), flags);
    }
    for key in ["update-check", "offline"] {
        if let Some(on) = given.get(key) {
            out.insert(key.to_owned(), on.clone());
        }
    }
    if let Some(release) = release {
        out.insert("release-base-url".to_owned(), json!(release));
    }
    if let Some(bundle) = bundle {
        out.insert("ca-bundle".to_owned(), json!(bundle));
    }
    Ok(Value::Object(out))
}

/// Where Focus keeps what belongs to the machine and not to a project:
/// `FOCUS_INSTALL_ROOT`; else `%LOCALAPPDATA%\focus` on Windows; else
/// `$XDG_DATA_HOME/focus`, or `~/.local/share/focus`. Links in the part of
/// the path that exists are resolved, so two spellings of one directory are
/// one install root.
#[must_use]
pub fn install_root() -> PathBuf {
    let named = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let home = || std::env::home_dir().unwrap_or_default();
    let lexical = match named(&format!("{ENV_PREFIX}INSTALL_ROOT"))
        .map(|root| trim(&root).to_owned())
        .filter(|root| !root.is_empty())
    {
        Some(explicit) => {
            std::path::absolute(&explicit).unwrap_or_else(|_| PathBuf::from(explicit))
        }
        None if cfg!(windows) => named("LOCALAPPDATA")
            .map_or_else(|| home().join("AppData").join("Local"), PathBuf::from)
            .join(WORKSPACE_DIR),
        None => named("XDG_DATA_HOME")
            .map_or_else(|| home().join(".local").join("share"), PathBuf::from)
            .join(WORKSPACE_DIR),
    };
    canonical_where_it_exists(&lexical)
}

/// `path` with the longest part of it that exists resolved, and the rest
/// appended as written.
fn canonical_where_it_exists(path: &Path) -> PathBuf {
    let mut cursor = path.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    while !cursor.exists() {
        let Some(name) = cursor.file_name().map(std::ffi::OsStr::to_owned) else {
            break;
        };
        rest.push(name);
        if !cursor.pop() {
            break;
        }
    }
    let base = std::fs::canonicalize(&cursor).map_or(cursor, |real| without_verbatim_prefix(&real));
    rest.into_iter()
        .rev()
        .fold(base, |path, name| path.join(name))
}

/// Windows spells a canonical path `\\?\C:\...`; a path a person reads is
/// spelled `C:\...`.
fn without_verbatim_prefix(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{share}"))
    } else if let Some(plain) = text.strip_prefix(r"\\?\") {
        PathBuf::from(plain)
    } else {
        path.to_path_buf()
    }
}

/// Where the three files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    pub machine: PathBuf,
    pub project: PathBuf,
    pub local: PathBuf,
}

impl Places {
    /// The files for `project`, with the machine's in [`install_root`].
    #[must_use]
    pub fn of(project: &Path) -> Self {
        Self::with_install_root(project, &install_root())
    }

    /// The files for `project`, with the machine's in `install_root`.
    #[must_use]
    pub fn with_install_root(project: &Path, install_root: &Path) -> Self {
        Self {
            machine: install_root.join(SETTINGS_FILE),
            project: project.join(SETTINGS_FILE),
            local: project.join(LOCAL_SETTINGS_FILE),
        }
    }

    #[must_use]
    pub fn path(&self, layer: Layer) -> &Path {
        match layer {
            Layer::Machine => &self.machine,
            Layer::Project => &self.project,
            Layer::Local => &self.local,
        }
    }

    /// One file, checked; `None` when it is not there.
    ///
    /// # Errors
    ///
    /// The path is not a regular file, the file is not JSON, or its content
    /// is refused (see [`normalize_settings`]).
    pub fn read(&self, layer: Layer) -> Result<Option<Value>> {
        let path = self.path(layer);
        let shown = path.display();
        let meta = match std::fs::metadata(path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => bail!("{shown}: settings file cannot be read: {error}"),
        };
        if !meta.is_file() {
            bail!("{shown}: settings path is not a regular file");
        }
        let text = std::fs::read(path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|error| {
                EngineError::new(format!("{shown}: settings file cannot be read: {error}"))
            })?;
        let parsed = json::parse(&text)
            .map_err(|error| EngineError::new(format!("{shown}: invalid JSON: {error}")))?;
        normalize_settings(
            &parsed,
            layer,
            &shown.to_string(),
            &self.machine.display().to_string(),
        )
        .map(Some)
    }

    /// The three files as one.
    ///
    /// # Errors
    ///
    /// A file that is there cannot be read or is refused.
    pub fn resolve(&self) -> Result<Resolved> {
        let layers = [
            self.read(Layer::Machine)?,
            self.read(Layer::Project)?,
            self.read(Layer::Local)?,
        ];
        resolve_layers(&layers, &self.machine.display().to_string())
    }

    /// The nearest file that records the switch `name`: this clone's, then
    /// the project's, then the machine's. That is the file to change to turn
    /// the check back on. A file that cannot be read records nothing.
    #[must_use]
    pub fn bypass_recorded_in(&self, name: &str) -> Option<Layer> {
        [Layer::Local, Layer::Project, Layer::Machine]
            .into_iter()
            .find(|layer| {
                self.read(*layer)
                    .ok()
                    .flatten()
                    .is_some_and(|file| bypasses_of(&file).contains(&name))
            })
    }

    /// The value of the variable `name` for this project: what the
    /// environment says when it says anything, even an empty value, and
    /// otherwise what the settings record. A recorded switch reads `1`; a
    /// recorded `true` reads `1` and a recorded `false` reads empty.
    ///
    /// # Errors
    ///
    /// The environment does not set `name` and a settings file is refused.
    pub fn flag(&self, name: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Option<String>> {
        if let Some(set) = env(name) {
            return Ok(Some(set));
        }
        let resolved = self.resolve()?;
        let Some(flags) = resolved.flags() else {
            return Ok(None);
        };
        if RECORDABLE_BYPASSES.contains(&name) {
            return Ok(resolved.bypasses().contains(&name).then(|| "1".to_owned()));
        }
        let field = FLAG_FIELDS
            .iter()
            .find(|(variable, _)| *variable == name)
            .and_then(|(_, field)| flags.get(*field));
        Ok(match field {
            Some(Value::Bool(on)) => Some(if *on { "1" } else { "" }.to_owned()),
            Some(Value::Number(number)) => number.as_f64().map(num::to_string),
            Some(Value::String(text)) => Some(text.clone()),
            _ => None,
        })
    }

    /// How a switch that keeps a check off is named to a person: `env NAME`
    /// when the environment sets it (it goes away only by starting without
    /// it), or `NAME in <file>` when a settings file records it.
    #[must_use]
    pub fn kill_switch_source(&self, name: &str, env: &dyn Fn(&str) -> Option<String>) -> String {
        if env(name).is_some() {
            return format!("env {name}");
        }
        match self.bypass_recorded_in(name) {
            None => format!("{name} in the Focus settings"),
            Some(Layer::Machine) => format!("{name} in {}", self.machine.display()),
            Some(layer) => format!(
                "{name} in {}",
                self.path(layer)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
            ),
        }
    }
}

/// Whether `source` is one [`Places::kill_switch_source`] wrote: a switch,
/// not the work's own setting.
#[must_use]
pub fn is_kill_switch_source(source: &str) -> bool {
    source.starts_with("env ") || re::test("^FOCUS_[A-Z0-9_]+ in ", "", source)
}

fn bypasses_of(file: &Value) -> Vec<&str> {
    file.get("flags")
        .and_then(|flags| flags.get("bypasses"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// The three files merged, with where each value came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved {
    /// The merged settings, in the shape of one file.
    pub value: Value,
    /// Each value's path (`flags.swarm`, `flags.bypasses.FOCUS_DISABLE_SENSORS`)
    /// and the nearest file that sets it, in the order they were merged.
    pub sources: Vec<(String, Layer)>,
    /// Which of the machine, project and local files exist.
    pub present: [bool; 3],
}

impl Resolved {
    #[must_use]
    pub fn flags(&self) -> Option<&Map<String, Value>> {
        self.value.get("flags").and_then(Value::as_object)
    }

    #[must_use]
    pub fn models(&self) -> Option<&Map<String, Value>> {
        self.value.get("models").and_then(Value::as_object)
    }

    /// Every switch any of the three files records.
    #[must_use]
    pub fn bypasses(&self) -> Vec<&str> {
        bypasses_of(&self.value)
    }

    /// The file a value came from; `None` when no file sets it.
    #[must_use]
    pub fn source(&self, path: &str) -> Option<Layer> {
        self.sources
            .iter()
            .find(|(set, _)| set == path)
            .map(|(_, layer)| *layer)
    }
}

fn set_source(sources: &mut Vec<(String, Layer)>, path: String, layer: Layer) {
    match sources.iter_mut().find(|(set, _)| *set == path) {
        Some(slot) => slot.1 = layer,
        None => sources.push((path, layer)),
    }
}

/// Copy every value of `source` over `target`, object by object, noting the
/// layer each one came from. Anything that is not an object is one value: a
/// list replaces a list.
fn merge_values(
    target: &mut Map<String, Value>,
    source: &Map<String, Value>,
    layer: Layer,
    sources: &mut Vec<(String, Layer)>,
    prefix: &str,
) {
    for (key, value) in source {
        if key == "$schema" || (key == "schemaVersion" && prefix.is_empty()) {
            continue;
        }
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        if let Some(nested_source) = value.as_object() {
            if !target.get(key).is_some_and(Value::is_object) {
                target.insert(key.clone(), Value::Object(Map::new()));
            }
            if let Some(Value::Object(nested)) = target.get_mut(key) {
                merge_values(nested, nested_source, layer, sources, &path);
                // An object that grew a key reads in JavaScript's order.
                let ordered = js_key_order(std::mem::take(nested));
                *nested = ordered;
            }
        } else {
            target.insert(key.clone(), value.clone());
            set_source(sources, path, layer);
        }
    }
}

/// Merge the machine, project and local files, each already checked or
/// absent.
///
/// # Errors
///
/// The merged settings are refused, which no three valid files produce.
pub fn resolve_layers(layers: &[Option<Value>; 3], machine_file: &str) -> Result<Resolved> {
    let mut merged = Map::new();
    merged.insert("schemaVersion".to_owned(), json!(1));
    let mut sources = Vec::new();
    for (layer, file) in Layer::ALL.into_iter().zip(layers) {
        if let Some(file) = file.as_ref().and_then(Value::as_object) {
            merge_values(&mut merged, file, layer, &mut sources, "");
        }
    }
    // A switch is on while any file records it. Each one's source is the
    // nearest file that does.
    let mut bypasses: Vec<&str> = Vec::new();
    for (layer, file) in Layer::ALL.into_iter().zip(layers) {
        for name in file.as_ref().map(bypasses_of).unwrap_or_default() {
            if !bypasses.contains(&name) {
                bypasses.push(name);
            }
            set_source(&mut sources, format!("flags.bypasses.{name}"), layer);
        }
    }
    if !bypasses.is_empty()
        && let Some(Value::Object(flags)) = merged.get_mut("flags")
    {
        flags.insert("bypasses".to_owned(), json!(bypasses));
    }
    let value = normalize_settings(
        &Value::Object(merged),
        Layer::Machine,
        "<resolved focus settings>",
        machine_file,
    )?;
    Ok(Resolved {
        value,
        sources,
        present: [
            layers[0].is_some(),
            layers[1].is_some(),
            layers[2].is_some(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("focus-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("project")).unwrap();
        std::fs::create_dir_all(dir.join("machine")).unwrap();
        dir
    }

    fn places(dir: &Path) -> Places {
        Places::with_install_root(&dir.join("project"), &dir.join("machine"))
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn a_nearer_file_wins_value_by_value_and_switches_add_up() {
        let dir = scratch("layers");
        let places = places(&dir);
        std::fs::write(
            &places.machine,
            r#"{"schemaVersion":1,"offline":true,"flags":{"schemaVersion":1,"swarm":true,"sensorTimeoutMs":900,"bypasses":["FOCUS_DISABLE_SENSORS"]}}"#,
        )
        .unwrap();
        std::fs::write(
            &places.project,
            r#"{"schemaVersion":1,"flags":{"schemaVersion":1,"swarm":false,"defaultScope":"feature","bypasses":["FOCUS_DISABLE_LEARNINGS"]}}"#,
        )
        .unwrap();
        std::fs::write(
            &places.local,
            r#"{"schemaVersion":1,"flags":{"schemaVersion":1,"bypasses":["FOCUS_DISABLE_SENSORS"]}}"#,
        )
        .unwrap();
        let resolved = places.resolve().unwrap();
        assert_eq!(
            json::stringify(&resolved.value),
            r#"{"schemaVersion":1,"flags":{"schemaVersion":1,"defaultScope":"feature","swarm":false,"sensorTimeoutMs":900,"bypasses":["FOCUS_DISABLE_LEARNINGS","FOCUS_DISABLE_SENSORS"]},"offline":true}"#
        );
        assert_eq!(resolved.source("flags.swarm"), Some(Layer::Project));
        assert_eq!(
            resolved.source("flags.sensorTimeoutMs"),
            Some(Layer::Machine)
        );
        assert_eq!(
            resolved.source("flags.bypasses.FOCUS_DISABLE_SENSORS"),
            Some(Layer::Local)
        );
        assert_eq!(resolved.source("flags.hookDebug"), None);
        assert_eq!(resolved.present, [true, true, true]);

        assert_eq!(
            places.bypass_recorded_in("FOCUS_DISABLE_SENSORS"),
            Some(Layer::Local)
        );
        assert_eq!(
            places.bypass_recorded_in("FOCUS_DISABLE_LEARNINGS"),
            Some(Layer::Project)
        );
        assert_eq!(places.bypass_recorded_in("FOCUS_SKIP_ARTIFACT_GUARD"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_environment_answers_first_even_with_an_empty_value() {
        let dir = scratch("flag");
        let places = places(&dir);
        std::fs::write(
            &places.project,
            r#"{"schemaVersion":1,"flags":{"schemaVersion":1,"swarm":true,"hookDebug":false,"sensorTimeoutMs":1500,"defaultScope":"bugfix","bypasses":["FOCUS_DISABLE_SENSORS"]}}"#,
        )
        .unwrap();
        let flag = |name: &str| places.flag(name, &no_env).unwrap();
        assert_eq!(flag("FOCUS_DISABLE_SENSORS").as_deref(), Some("1"));
        assert_eq!(flag("FOCUS_DISABLE_LEARNINGS"), None);
        assert_eq!(flag("FOCUS_USE_SWARM").as_deref(), Some("1"));
        assert_eq!(flag("FOCUS_HOOK_DEBUG").as_deref(), Some(""));
        assert_eq!(flag("FOCUS_SENSOR_TIMEOUT_MS").as_deref(), Some("1500"));
        assert_eq!(flag("FOCUS_DEFAULT_SCOPE").as_deref(), Some("bugfix"));
        assert_eq!(flag("FOCUS_SOMETHING_ELSE"), None);

        // Set to anything, the environment is the answer: this is how one
        // process turns a recorded switch back on.
        let cleared = |name: &str| (name == "FOCUS_DISABLE_SENSORS").then(String::new);
        assert_eq!(
            places
                .flag("FOCUS_DISABLE_SENSORS", &cleared)
                .unwrap()
                .as_deref(),
            Some("")
        );
        assert_eq!(
            places.kill_switch_source("FOCUS_DISABLE_SENSORS", &cleared),
            "env FOCUS_DISABLE_SENSORS"
        );
        assert_eq!(
            places.kill_switch_source("FOCUS_DISABLE_SENSORS", &no_env),
            "FOCUS_DISABLE_SENSORS in focus.settings.json"
        );
        assert_eq!(
            places.kill_switch_source("FOCUS_DISABLE_LEARNINGS", &no_env),
            "FOCUS_DISABLE_LEARNINGS in the Focus settings"
        );
        assert!(is_kill_switch_source("env X"));
        assert!(is_kill_switch_source(
            "FOCUS_DISABLE_SENSORS in focus.settings.json"
        ));
        assert!(!is_kill_switch_source("scope feature"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_is_read_whole_or_refused_by_name() {
        let dir = scratch("refused");
        let places = places(&dir);
        let project = places.project.display().to_string();
        for (text, said) in [
            ("{ nope", format!("{project}: invalid JSON: ")),
            ("[]", format!("{project}: settings must be a JSON object")),
            (
                r#"{"schemaVersion":1,"flag":{}}"#,
                format!("{project}: settings contain unknown key(s): flag"),
            ),
            (
                r#"{"schemaVersion":2}"#,
                format!("{project}: settings.schemaVersion must be 1"),
            ),
            (
                r#"{"schemaVersion":1,"offline":true,"update-check":false}"#,
                format!(
                    "{project}: update-check, offline are machine-only; write {} instead",
                    places.machine.display()
                ),
            ),
            (
                r#"{"schemaVersion":1,"flags":{"schemaVersion":1,"bypasses":["FOCUS_DISABLE_COLLABORATORS"]}}"#,
                format!("{project}: flags.bypasses must contain only FOCUS_SKIP_ARTIFACT_GUARD, "),
            ),
        ] {
            std::fs::write(&places.project, text).unwrap();
            let refused = places.resolve().unwrap_err();
            assert!(refused.message().starts_with(&said), "{text}: {refused}");
        }
        // No file at all is no settings, not an error.
        std::fs::remove_file(&places.project).unwrap();
        let empty = places.resolve().unwrap();
        assert_eq!(json::stringify(&empty.value), r#"{"schemaVersion":1}"#);
        assert_eq!(empty.present, [false, false, false]);
        assert_eq!(places.flag("FOCUS_DISABLE_SENSORS", &no_env).unwrap(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Deliberate deviation. Upstream validates `release-base-url` with its
    /// runtime's URL parser, which accepts many spellings of one location.
    /// Focus reads one spelling and refuses the rest, in upstream's words.
    #[test]
    fn a_release_location_is_read_in_one_spelling() {
        for accepted in [
            "https://example.com",
            "https://releases.example.com:8443/focus/",
            "https://example.com/a/b%20c",
            "http://localhost:8080",
            "http://127.0.0.1/x",
            "https://10.0.0.1",
        ] {
            assert!(valid_release_url(accepted), "{accepted}");
        }
        for refused in [
            // Refused by upstream too.
            "http://example.com",
            "https://user@example.com",
            "https://user:secret@example.com",
            "https://example.com/?q=1",
            "https://example.com/#top",
            "ftp://example.com",
            "example.com",
            "https://",
            "https://example.com:99999",
            "https://1.2.3.4.5",
            "https://256.1.1.1",
            // Accepted by upstream's parser, refused here.
            "HTTPS://example.com",
            "https:/example.com",
            " https://example.com ",
            "https://example.com?",
            "https://exa mple.com",
            "https://example.com/a b",
            "https://[::1]",
            "https://1.2.3",
        ] {
            assert!(!valid_release_url(refused), "{refused}");
        }
    }
}

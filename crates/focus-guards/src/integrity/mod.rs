//! Runtime integrity: defense in depth for harness-owned runtime records
//! and hooks, not a sandbox. Hooks and tool calls run as the same user, so
//! an agent with unrestricted execution can always find a path around a
//! lexical check. The outer boundary is the harness's permission model and
//! the person's review of what the agent runs. This check runs before fence
//! decisions; no Guard Policy word, lowered fence, or presence bypass turns
//! it off.
//!
//! A tool call is refused when it would invoke a hook, choose a session,
//! set a bypass, edit a runtime record, write the audit trail, or replace an
//! installed enforcement file — directly, through a shell command, through
//! a script it writes or runs, or through an interpreter it feeds.
//!
//! Ported from `core/hooks/runtime-integrity.ts`, and held to its answers
//! by the oracle corpus (`runtime_integrity` cases).

mod paths;
mod shell;
mod source;

use focus_js::re;
use serde_json::{Map, Value};

use crate::shell::{CommandShell, Host, shell_write_targets, write_targets};
use shell::{Language, PROSE_EXTENSION, Walk};
use source::protected_content;

const MAX_EXECUTION_DEPTH: usize = 32;
const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;

/// What a refused tool call was about to touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Violation {
    /// A runtime record, a hook, a bypass, or an installed enforcement file.
    Runtime,
    /// A script that imports a hook or runs one.
    Content,
    /// The audit trail.
    Audit,
}

impl Violation {
    /// The name upstream's `runtimeIntegrityViolationKind` answers.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Runtime => "runtime",
            Self::Content => "content",
            Self::Audit => "audit",
        }
    }

    /// What the hook prints on stderr before exiting 2
    /// (`refuseRuntimeIntegrityViolation`).
    #[must_use]
    pub fn refusal(self) -> String {
        match self {
            Self::Audit => format!("{AUDIT_TRAIL_REFUSAL}\n"),
            Self::Content => format!(
                "{RUNTIME_INTEGRITY_REFUSAL} (Scripts the agent writes or runs may not import these hooks either.)\n"
            ),
            Self::Runtime => format!("{RUNTIME_INTEGRITY_REFUSAL}\n"),
        }
    }
}

pub const RUNTIME_INTEGRITY_REFUSAL: &str = "FOCUS runtime records and hooks belong to the harness: hooks write them when the person acts, and the engine reads them. A tool call cannot invoke a hook, choose a session, set a bypass, edit those records, or replace installed enforcement files. Use the engine commands for workflow work. For intentional installed-file maintenance, use the official updater or an external terminal outside the running agent workflow.";

pub const AUDIT_TRAIL_REFUSAL: &str = "The audit trail under focus/spaces/<space>/intents/<intent>/audit/ is written only by the framework's own tools: record the event through the owning engine command (engine log decision and engine log answer for a question and its response, engine audit append-raw for a free-form note, engine orchestrate report for a gate) instead of writing a shard directly.";

/// The host the check runs on: what upstream takes from `process`, its
/// environment and its own location.
#[derive(Debug, Clone)]
pub struct Integrity {
    pub host: Host,
    /// `runtimeProjectDir()`: the hook's own project.
    pub project_dir: String,
    /// `FOCUS_HARNESS_DIR`, when set.
    pub harness_dir: Option<String>,
    /// The engine's own binary, protected as a native hook entrypoint.
    pub executable: Option<String>,
}

impl Integrity {
    /// The host this process runs on.
    #[must_use]
    pub fn current() -> Self {
        let host = Host::current();
        let project_dir = focus_workspace::env::path("RUNTIME_PROJECT_DIR")
            .or_else(|| focus_workspace::env::path("PROJECT_DIR"))
            .or_else(|| std::env::var_os("CLAUDE_PROJECT_DIR").filter(|value| !value.is_empty()))
            .map_or_else(
                || host.cwd.clone(),
                |dir| dir.to_string_lossy().into_owned(),
            );
        Self {
            project_dir,
            harness_dir: focus_workspace::env::var("HARNESS_DIR"),
            executable: std::env::current_exe()
                .ok()
                .map(|exe| exe.to_string_lossy().into_owned()),
            host,
        }
    }

    /// A POSIX host at `cwd` whose project is `project_dir`, for tests and
    /// the corpus.
    #[must_use]
    pub fn posix(cwd: &str, project_dir: &str, home: &str) -> Self {
        Self {
            host: Host::posix(cwd, home),
            project_dir: project_dir.to_owned(),
            harness_dir: None,
            executable: None,
        }
    }
}

/// `executableName`: a program's leaf name, a Windows suffix dropped, in
/// lower case.
fn executable_name(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let leaf = normalized.rsplit('/').next().unwrap_or("");
    re::replace(r"\.(?:exe|cmd|bat)$", "i", leaf, "").to_lowercase()
}

/// `interpreter(name)`.
fn interpreter(name: &str) -> Option<Language> {
    if re::test(r"^(?:sh|bash|zsh)$", "", name) {
        Some(Language::Shell)
    } else if re::test(r"^python(?:\d+(?:\.\d+)*)?$", "", name) {
        Some(Language::Python)
    } else if re::test(r"^(?:bun|node|tsx|deno)$", "", name) {
        Some(Language::JavaScript)
    } else {
        None
    }
}

/// `raw.replace(/\\\r?\n/g, "")`.
fn without_continuations(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\\' {
            if chars.get(index + 1) == Some(&'\n') {
                index += 2;
                continue;
            }
            if chars.get(index + 1) == Some(&'\r') && chars.get(index + 2) == Some(&'\n') {
                index += 3;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn as_str(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

/// `protectedContentWrite`: a write tool whose content, or whose edited
/// file once the edit is applied, is a script that reaches a hook.
fn protected_content_write(
    walk: &mut Walk,
    tool_name: &str,
    tool_input: Option<&Map<String, Value>>,
    cwd: &str,
) -> bool {
    let targets = write_targets(
        &walk.integrity.host,
        tool_name,
        tool_input,
        cwd,
        CommandShell::Posix,
    );
    let mut content_targets: Vec<String> = Vec::new();
    let protected_write = |walk: &mut Walk, value: Option<&Value>, paths: &[String]| -> bool {
        let Some(value) = as_str(value) else {
            return false;
        };
        paths.iter().any(|path| {
            if tool_name != "NotebookEdit" && re::test(PROSE_EXTENSION, "i", path) {
                false
            } else if path.ends_with(".sh") {
                walk.protected_shell(value, 0, false)
            } else {
                protected_content(walk, value, 0, path.ends_with(".py"), None)
            }
        })
    };
    let content = tool_input.and_then(|input| input.get("content"));
    let new_source = tool_input.and_then(|input| input.get("new_source"));
    if protected_write(walk, content, &targets) || protected_write(walk, new_source, &targets) {
        content_targets.extend(targets.iter().cloned());
    }
    // A replacement fragment has no lexical context. Preview the complete
    // file when possible: inserting an example inside a comment is inert,
    // while deleting only comment delimiters can activate code already in
    // the file. MultiEdit replacements are applied in order, without
    // writing anything.
    let mut projected: Vec<(String, String)> = Vec::new();
    let edits: Vec<Option<&Map<String, Value>>> = match tool_input
        .and_then(|input| input.get("edits"))
        .and_then(Value::as_array)
    {
        Some(edits) => edits.iter().map(Value::as_object).collect(),
        None => vec![tool_input],
    };
    for edit in edits.into_iter().flatten() {
        let Some(new_string) = as_str(edit.get("new_string")) else {
            continue;
        };
        let edit_targets = write_targets(
            &walk.integrity.host,
            tool_name,
            Some(edit),
            cwd,
            CommandShell::Posix,
        );
        let candidates = if edit_targets.is_empty() {
            &targets
        } else {
            &edit_targets
        };
        for candidate in candidates {
            let path = walk.integrity.resolve(&[cwd, candidate]);
            let old_string = as_str(edit.get("old_string"));
            let mut current: Option<String> = projected
                .iter()
                .find(|(known, _)| *known == path)
                .map(|(_, text)| text.clone());
            if current.is_none()
                && old_string.is_some()
                && let Ok(meta) = std::fs::metadata(&path)
                && meta.is_file()
                && meta.len() <= MAX_SCRIPT_BYTES
                && let Ok(bytes) = std::fs::read(&path)
            {
                current = Some(String::from_utf8_lossy(&bytes).into_owned());
            }
            match (current, old_string) {
                (Some(current), Some(old_string))
                    if (!old_string.is_empty() || current.is_empty())
                        && current.contains(old_string) =>
                {
                    let replaced = if edit.get("replace_all") == Some(&Value::Bool(true)) {
                        // `current.split(old).join(new)`: an empty `old`
                        // splits an empty string into nothing.
                        if old_string.is_empty() {
                            String::new()
                        } else {
                            current.replace(old_string, new_string)
                        }
                    } else {
                        current.replacen(old_string, new_string, 1)
                    };
                    match projected.iter_mut().find(|(known, _)| *known == path) {
                        Some((_, text)) => *text = replaced,
                        None => projected.push((path.clone(), replaced)),
                    }
                }
                _ => {
                    let value = Value::String(new_string.to_owned());
                    if protected_write(walk, Some(&value), std::slice::from_ref(&path)) {
                        content_targets.push(path.clone());
                    }
                }
            }
        }
    }
    for (path, content) in &projected {
        let value = Value::String(content.clone());
        if protected_write(walk, Some(&value), std::slice::from_ref(path)) {
            content_targets.push(path.clone());
        }
    }
    if content_targets.is_empty() {
        return false;
    }
    // The authored repository must be able to develop and document its
    // hooks.
    content_targets
        .iter()
        .any(|path| !walk.integrity.is_authored_development_path(path, cwd))
}

/// `runtimeIntegrityViolation`: the refusal class for a hook's tool call,
/// or `None` when the call is allowed. `input` is the hook payload
/// (`tool_name`, `tool_input`, `cwd`, and `focus_shell` when the adapter
/// marks the command as PowerShell).
#[must_use]
pub fn violation(integrity: &Integrity, input: &Value) -> Option<Violation> {
    let tool_name = as_str(input.get("tool_name")).unwrap_or("");
    let tool_input = input.get("tool_input").and_then(Value::as_object);
    let cwd = as_str(input.get("cwd")).map_or_else(|| integrity.host.cwd.clone(), str::to_owned);
    let mut walk = Walk {
        integrity,
        cwd: cwd.clone(),
        audit_trail_matched: false,
    };
    if tool_name == "Bash" {
        let raw = as_str(tool_input.and_then(|input| input.get("command")))?;
        // The shell removes a backslash-newline continuation before it parses
        // anything, so `au\<newline>dit` is `audit` to it and must be to us.
        let command = without_continuations(raw);
        // A command the adapter marks as PowerShell (outside the agent's
        // input) is also read as PowerShell; the POSIX reading below stays,
        // so the mark can only add targets.
        let power_shell_write = as_str(input.get("focus_shell")) == Some("powershell")
            && shell_write_targets(&integrity.host, raw, &cwd, None, CommandShell::PowerShell)
                .iter()
                .any(|path| walk.protected_write_target(path));
        if !power_shell_write && !walk.protected_shell(&command, 0, false) {
            return None;
        }
        return Some(if walk.audit_trail_matched {
            Violation::Audit
        } else {
            Violation::Runtime
        });
    }
    if !["Write", "Edit", "MultiEdit", "NotebookEdit"].contains(&tool_name) {
        return None;
    }
    let mut targets = write_targets(
        &integrity.host,
        tool_name,
        tool_input,
        &cwd,
        CommandShell::Posix,
    );
    if let Some(edits) = tool_input
        .and_then(|input| input.get("edits"))
        .and_then(Value::as_array)
    {
        for edit in edits {
            if let Some(edit) = edit.as_object() {
                targets.extend(write_targets(
                    &integrity.host,
                    tool_name,
                    Some(edit),
                    &cwd,
                    CommandShell::Posix,
                ));
            }
        }
    }
    if targets.iter().any(|path| {
        integrity.protected_runtime_path(path, &cwd)
            || integrity.protected_installed_path(path, &cwd, false)
    }) {
        return Some(Violation::Runtime);
    }
    if targets
        .iter()
        .any(|path| integrity.protected_audit_trail_path(path, &cwd))
    {
        return Some(Violation::Audit);
    }
    protected_content_write(&mut walk, tool_name, tool_input, &cwd).then_some(Violation::Content)
}

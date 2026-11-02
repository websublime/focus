//! The oracle protocol for the guards (see `focus-graph`'s `oracle` module
//! for the protocol).
//!
//! `guard_policy` cases resolve a policy and every fence from a state text,
//! a scope default and memory declarations; `guards_line` cases parse and
//! write the state lines the guards read; `shell_invocations`,
//! `shell_targets` and `write_targets` cases read a command line as the
//! shell reader does.

use std::path::Path;

use focus_graph::guard_memory::Declaration;
use serde_json::{Map, Value, json};

use crate::authority::Authority;
use crate::authority::decide_fence;
use crate::engine_command;
use crate::fences::{
    Fence, FenceResolution, format_guards_line, parse_guards_line, resolve_fences,
    set_guards_off_line, set_guards_on_line,
};
use crate::integrity::{self, Integrity};
use crate::policy::{
    self, GuardPolicy, Resolution, format_guard_policy, parse_state_line, set_guard_policy_line,
    source_label,
};
use crate::reviewer_scope;
use crate::shell::{self, CommandShell, Host};
use crate::state_transition;

/// The kinds of case this crate answers.
pub const KINDS: [&str; 10] = [
    "guard_policy",
    "guards_line",
    "engine_command",
    "shell_invocations",
    "shell_targets",
    "write_targets",
    "runtime_integrity",
    "reviewer_scope",
    "reviewer_dispatch",
    "state_transition",
];

fn resolution_json(resolution: &Resolution) -> Value {
    let mut out = Map::new();
    out.insert("value".to_owned(), json!(resolution.value.as_str()));
    out.insert("source".to_owned(), json!(resolution.source));
    out.insert(
        "scopeDefault".to_owned(),
        json!(resolution.scope_default.as_str()),
    );
    out.insert(
        "intent".to_owned(),
        resolution.intent.as_ref().map_or(
            Value::Null,
            |line| json!({ "value": line.value.as_str(), "source": line.source }),
        ),
    );
    out.insert(
        "stateValue".to_owned(),
        json!(resolution.state_value.as_str()),
    );
    out.insert(
        "rawStateValue".to_owned(),
        json!(resolution.raw_state_value),
    );
    out.insert("stateField".to_owned(), json!(resolution.state_field));
    if let Some((guard_policy, change_control)) = &resolution.conflict {
        out.insert(
            "conflict".to_owned(),
            json!({ "guardPolicy": guard_policy, "changeControl": change_control }),
        );
    }
    out.insert(
        "memoryStrict".to_owned(),
        resolution
            .memory_strict
            .as_ref()
            .map_or(Value::Null, |declaration| {
                json!({
                    "layer": declaration.layer,
                    "path": declaration.path.to_string_lossy().replace('\\', "/"),
                    "heading": declaration.heading,
                    "value": declaration.value.as_str(),
                })
            }),
    );
    Value::Object(out)
}

fn fences_json(fences: &[FenceResolution]) -> Value {
    fences
        .iter()
        .map(|resolved| {
            json!({
                "fence": resolved.fence.name(),
                "value": resolved.value.as_str(),
                "source": resolved.source,
                "label": resolved.source_label(),
                "formatted": resolved.format(),
            })
        })
        .collect()
}

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    match text("kind") {
        "guard_policy" => {
            let state = text("state");
            // The scope files' defaults, as `scopeGuardPolicyDefault` reads
            // them: by the scope's name lowercased, `off` for a file that
            // declares none, `strict` for a scope with no file or no scope.
            let scope_default = match focus_state::get_field(state, "Scope") {
                None => GuardPolicy::Strict,
                Some(scope) => {
                    let wanted = focus_js::text::trim(scope).to_lowercase();
                    let declared = case["scopes"]
                        .as_object()
                        .and_then(|scopes| scopes.get(&wanted))
                        .map(|policy| policy.as_str().and_then(GuardPolicy::parse));
                    policy::scope_default(declared)
                }
            };
            // The memory files are written and read as a project's are, so
            // that what a file may declare is decided by the reader.
            let root = std::env::temp_dir().join(format!(
                "focus-oracle-guards-{}-{}",
                std::process::id(),
                text("id")
            ));
            let memory_dir = root.join("focus/spaces/default/memory");
            let _ = std::fs::create_dir_all(&memory_dir);
            for declared in case["memory"].as_array().into_iter().flatten() {
                let layer = declared["layer"].as_str().unwrap_or_default();
                let _ = std::fs::write(
                    memory_dir.join(format!("{layer}.md")),
                    format!(
                        "# {layer}\n\n{}\n\nMode: {}\n",
                        declared["heading"].as_str().unwrap_or_default(),
                        declared["value"].as_str().unwrap_or_default()
                    ),
                );
            }
            let named = |path: &Path| {
                path.to_string_lossy()
                    .replace(&*root.to_string_lossy(), "{P}")
                    .replace('\\', "/")
            };
            let declarations = || -> focus_js::Result<Vec<Declaration>> {
                let read = focus_graph::guard_memory::declarations(&memory_dir)
                    .map_err(|err| focus_js::EngineError::new(named(Path::new(err.message()))))?;
                Ok(read
                    .into_iter()
                    .map(|declaration| Declaration {
                        path: Path::new(&named(&declaration.path)).to_path_buf(),
                        ..declaration
                    })
                    .collect())
            };
            let tolerate = case["tolerate"].as_bool().unwrap_or(false);
            let state_path = Path::new("{P}/focus/spaces/default/intents/focus-state.md");
            let resolved =
                policy::resolve(state, state_path, scope_default, declarations, tolerate);
            // `guardPolicyAcceptsChanges` resolves again without tolerance,
            // and a policy that cannot be read counts as strict.
            let accepts = policy::resolve(state, state_path, scope_default, declarations, false)
                .is_ok_and(|strictly| strictly.value != GuardPolicy::Strict);
            let _ = std::fs::remove_dir_all(&root);
            let resolved = match resolved {
                Ok(resolved) => resolved,
                Err(err) => return json!({ "error": err.message() }),
            };
            let env: Vec<&str> = case["env"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            let flag = |name: &str| env.contains(&name).then(|| "1".to_owned());
            let source = |name: &str| format!("env {name}");
            let fences = resolve_fences(&resolved, state, &flag, &source);
            let unattended = env.contains(&"FOCUS_UNATTENDED");
            let decisions: Vec<Value> = fences
                .iter()
                .map(|fence| {
                    let decided =
                        decide_fence(&resolved, fence, Authority::without_markers(unattended));
                    json!({
                        "decision": decided.decision.as_str(),
                        "authority": {
                            "covered": decided.authority.covered.as_str(),
                            "actor": decided.authority.actor.as_str(),
                            "unattended": decided.authority.unattended,
                        },
                        "policy": decided.policy.as_str(),
                        "fenceSetting": decided.fence_setting.as_str(),
                        "source": decided.source,
                        "speaks": decided.speaks(),
                        "line": decided.stood_aside_line(Some("focus-state set"), true),
                    })
                })
                .collect();
            json!({
                "policy": resolution_json(&resolved),
                "accepts": accepts,
                "fences": fences_json(&fences),
                "decisions": decisions,
            })
        }
        "guards_line" => {
            let fences: Vec<Fence> = case["fences"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(Fence::parse)
                .collect();
            let parsed = parse_state_line(case["line"].as_str());
            json!({
                "parsedGuards": parse_guards_line(case["line"].as_str())
                    .iter()
                    .map(|fence| fence.name())
                    .collect::<Vec<_>>(),
                "parsedPolicy": parsed.as_ref().map(|line| json!({ "value": line.value.as_str(), "source": line.source })),
                "formattedGuards": format_guards_line(&fences),
                "formattedPolicy": parsed.as_ref().map(|line| format_guard_policy(line.value, &line.source)),
                "label": source_label(text("line")),
                "guardsOff": set_guards_off_line(text("content"), &fences),
                "guardsOn": set_guards_on_line(text("content"), &fences),
                "policyLine": set_guard_policy_line(text("content"), text("line")),
            })
        }
        "shell_invocations" => {
            let command = text("command");
            json!({
                "details": shell::shell_command_invocation_details(command)
                    .iter()
                    .map(shell::ShellInvocationDetails::to_json)
                    .collect::<Vec<Value>>(),
                "invocations": shell::shell_command_invocations(command)
                    .iter()
                    .map(shell::ShellInvocation::to_json)
                    .collect::<Vec<Value>>(),
                "altersResolution": shell::shell_command_alters_executable_resolution(command),
            })
        }
        "engine_command" => {
            let command = text("command");
            let literal = engine_command::parse_literal_shell_invocation(command).map(|literal| {
                json!({
                    "directory": literal.directory,
                    "command": literal.command,
                    "argv": literal.argv,
                    "rawWords": literal.raw_words,
                })
            });
            json!({
                "classify": engine_command::classify_runtime_compile_command(command).name(),
                "literal": literal,
                "verb": engine_command::literal_orchestrate_verb(command),
            })
        }
        "shell_targets" | "write_targets" => shell_case(case),
        "runtime_integrity" => integrity_case(case),
        "reviewer_scope" => reviewer_scope_case(case),
        "state_transition" => {
            let command = case
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            json!({
                "direct": state_transition::direct_state_transition(command),
                "boundary": state_transition::is_lifecycle_boundary_command(command),
                "delegated": state_transition::delegated_lifecycle_command(command),
            })
        }
        "reviewer_dispatch" => {
            let record = reviewer_scope::parse_dispatch_record(
                case.get("raw").and_then(Value::as_str).unwrap_or_default(),
            );
            json!({
                "record": record.map(|record| json!({
                    "reviewer": record.reviewer, "stage": record.stage, "unit": record.unit, "exempt": record.exempt,
                })),
            })
        }
        other => json!({ "unsupported": other }),
    }
}

/// A `reviewer_scope` case: the matcher's verdict over one tool call, in
/// the shape upstream's object takes (`target` only on a block, `defaulted`
/// only when marked), and the refusal it would print.
fn reviewer_scope_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    let exempt: Vec<String> = case["exempt"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    let context = reviewer_scope::ScopeContext {
        record_root: case["context"]["record_root"].as_str().map(str::to_owned),
        cwd: case["context"]["cwd"].as_str().map(str::to_owned),
    };
    let verdict = reviewer_scope::evaluate_reviewer_scope(
        text("tool"),
        case.get("input").and_then(Value::as_object),
        text("unit"),
        &exempt,
        &context,
    );
    let mut out = Map::new();
    out.insert("block".to_owned(), json!(verdict.block));
    if let Some(target) = &verdict.target {
        out.insert("target".to_owned(), json!(target));
    }
    if verdict.defaulted {
        out.insert("defaulted".to_owned(), json!(true));
    }
    let reason = verdict.block.then(|| {
        reviewer_scope::block_reason(
            verdict.target.as_deref().unwrap_or(""),
            text("unit"),
            verdict.defaulted,
        )
    });
    json!({ "verdict": Value::Object(out), "reason": reason })
}

/// The scratch root a shell case's `{P}` stands for, spelled as upstream's
/// driver spells it rather than taken from the host's temporary directory:
/// two levels under `/`, so a `..` that climbs out of it lands in the same
/// place, and named after the case, so a `cp` into its parent names the
/// same child.
fn shell_root(id: &str) -> String {
    format!("/tmp/focus-oracle-shell-{id}")
}

/// A `shell_targets` or `write_targets` case: the command read from
/// `{P}/w`, with the directories the case lists created under it (where the
/// host can make a `/tmp`), and the root written back as `{P}`.
fn shell_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    let root = shell_root(text("id"));
    let cwd = text("cwd").replace("{P}", &root);
    if cfg!(unix) {
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::create_dir_all(&cwd);
        for dir in case["dirs"].as_array().into_iter().flatten() {
            let _ = std::fs::create_dir_all(format!("{cwd}/{}", dir.as_str().unwrap_or_default()));
        }
    }
    let host = Host::posix(&root, text("home"));
    let shell = if text("shell") == "powershell" {
        CommandShell::PowerShell
    } else {
        CommandShell::Posix
    };
    let say = |path: &String| path.replace(&root, "{P}");
    let out = if text("kind") == "shell_targets" {
        let command = text("command");
        let mut raw = Vec::new();
        let targets = shell::shell_write_targets(&host, command, &cwd, Some(&mut raw), shell);
        let roots = shell::shell_directory_roots(&host, command, &cwd, shell);
        json!({
            "targets": targets.iter().map(say).collect::<Vec<String>>(),
            "raw": raw.iter().map(say).collect::<Vec<String>>(),
            "roots": roots.iter().map(say).collect::<Vec<String>>(),
            "changes": shell::shell_directory_changes(command)
                .iter()
                .map(shell::DirectoryChange::to_json)
                .collect::<Vec<Value>>(),
        })
    } else {
        let targets =
            shell::write_targets(&host, text("tool"), case["input"].as_object(), &cwd, shell);
        json!({ "targets": targets.iter().map(say).collect::<Vec<String>>() })
    };
    if cfg!(unix) {
        let _ = std::fs::remove_dir_all(&root);
    }
    out
}

/// A `runtime_integrity` case: a project tree laid out under a scratch root
/// spelled as upstream's driver spells its own, the hook payload read from
/// it, and the refusal class and text answered with the root written back
/// as `{P}`.
fn integrity_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    let root = format!("/tmp/focus-oracle-integrity-{}", text("id"));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    for (rel, content) in case["files"].as_object().into_iter().flatten() {
        let path = format!("{root}/{rel}");
        match content.as_str() {
            None => {
                let _ = std::fs::create_dir_all(&path);
            }
            Some(_) if rel.ends_with('/') => {
                let _ = std::fs::create_dir_all(&path);
            }
            Some(text) => {
                if let Some(parent) = std::path::Path::new(&path).parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // A file too large to read: a hook require, then a comment
                // padded to the size, so that reading it would refuse.
                let bytes: Vec<u8> = match text
                    .strip_prefix("@bytes:")
                    .and_then(|n| n.parse::<usize>().ok())
                {
                    Some(size) => {
                        let mut bytes = b"require(\"hooks/focus-x\")\n//".to_vec();
                        bytes.resize(size, b'x');
                        bytes
                    }
                    None => text.as_bytes().to_vec(),
                };
                let _ = std::fs::write(&path, bytes);
            }
        }
    }
    for link in case["links"].as_array().into_iter().flatten() {
        if let (Some(rel), Some(target)) = (link[0].as_str(), link[1].as_str()) {
            let path = format!("{root}/{rel}");
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            #[cfg(unix)]
            let _ = std::os::unix::fs::symlink(target, &path);
            #[cfg(not(unix))]
            let _ = (target, path);
        }
    }
    #[cfg(unix)]
    for rel in case["exec"].as_array().into_iter().flatten() {
        use std::os::unix::fs::PermissionsExt;
        let path = format!("{root}/{}", rel.as_str().unwrap_or_default());
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    let project = focus_js::path::join(focus_js::path::Flavour::Posix, &[&root, text("project")]);
    let mut host = Integrity::posix(&root, &project, "/home/me");
    host.harness_dir = case["env"]["FOCUS_HARNESS_DIR"].as_str().map(str::to_owned);
    let mut input = case["input"].clone();
    if let Some(cwd) = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(|cwd| cwd.replace("{P}", &root))
    {
        input["cwd"] = Value::String(cwd);
    }
    let violation = integrity::violation(&host, &input);
    let _ = std::fs::remove_dir_all(&root);
    json!({
        "kind": violation.map(integrity::Violation::name),
        "refused": violation.is_some(),
        "stderr": violation.map_or_else(String::new, |violation| violation.refusal().replace(&root, "{P}")),
    })
}

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
use crate::dispatcher::resolve_engine_action;
use crate::engine_command;
use crate::fences::{
    Fence, FenceResolution, format_guards_line, parse_guards_line, resolve_fences,
    set_guards_off_line, set_guards_on_line,
};
use crate::guard_operation::{self, RemedyWordingContext, RenderMode, RenderShell};
use crate::integrity::{self, Integrity};
use crate::plan_approval::{self, HandoffDefect, UnitEvidence};
use crate::plan_wait;
use crate::policy::{
    self, GuardPolicy, Resolution, format_guard_policy, parse_state_line, set_guard_policy_line,
    source_label,
};
use crate::posture;
use crate::reviewer_scope;
use crate::shell::{self, CommandShell, Host};
use crate::state_transition;
use crate::typed_switch::{self, TypedSwitchRequest};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 25] = [
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
    "typed_switch",
    "kiro_args",
    "guard_operation",
    "plan_dispatch",
    "plan_reason",
    "contract_markers",
    "questions_file",
    "review_appendix",
    "engine_route",
    "every_helper",
    "plan_prerequisite",
    "engine_directed",
    "read_only_diagnostic",
    "plain_powershell",
    "dynamic_evaluation",
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
        "typed_switch" => {
            let prompt = case
                .get("prompt")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let words_answer = case.get("words_answer") == Some(&Value::Bool(true));
            typed_switch_json(&typed_switch::parse_typed_guard_switch_request(
                prompt,
                words_answer,
            ))
        }
        "kiro_args" => json!(typed_switch::split_kiro_command_args(
            case.get("raw").and_then(Value::as_str).unwrap_or_default()
        )),
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
        "guard_operation" => guard_operation_case(case),
        "plan_dispatch" => {
            let units: Vec<UnitEvidence> = case["units"]
                .as_array()
                .into_iter()
                .flatten()
                .map(evidence_of)
                .collect();
            verdict_json(&plan_approval::evaluate_plan_approval_dispatch(
                text("tool"),
                text("subagent"),
                text("prompt"),
                text("current_stage"),
                &units,
            ))
        }
        "plan_reason" => plan_reason_case(case),
        "contract_markers" => json!(posture::prompt_testing_contract_markers(text("text"))),
        "questions_file" => {
            let body = text("body");
            let latest = posture::latest_plan_approval(body);
            json!({
                "approved": posture::questions_file_approved(body),
                "pending": posture::questions_file_has_pending_plan_approval(body),
                "fingerprint": latest.fingerprint,
                "planned_source": latest.planned_source,
            })
        }
        "review_appendix" => json!(posture::plan_review_appendix(text("plan"))),
        "engine_route" => {
            match resolve_engine_action(&strings_of(&case["args"]), ORACLE_PROJECT, ORACLE_PROJECT)
            {
                Some(found) => json!({"tool": found.tool, "args": found.args}),
                None => Value::Null,
            }
        }
        "every_helper" => json!(plan_wait::every_helper_may_run(
            &strings_of(&case["args"]),
            ".claude",
            ORACLE_PROJECT,
            ORACLE_PROJECT,
        )),
        "plan_prerequisite" => {
            let spoke = case["spoke"].as_bool().unwrap_or(false);
            json!(plan_wait::is_plan_approval_prerequisite(
                &strings_of(&case["args"]),
                case["gate_held"].as_bool().unwrap_or(false),
                &mut || spoke,
            ))
        }
        "engine_directed" => {
            let asked = case["asked"].as_bool().unwrap_or(false);
            let args = strings_of(&case["args"]);
            json!({
                "directed": plan_wait::engine_directed_while_plan_waits(&args, &mut || asked),
                "repo": plan_wait::intent_repo_change_admitted(&args, &mut || asked),
            })
        }
        "read_only_diagnostic" => {
            let args = strings_of(&case["args"]);
            json!({
                "diagnostic": plan_wait::is_read_only_diagnostic(&args),
                "tool": plan_wait::is_read_only_tool_diagnostic(text("stem"), &args),
            })
        }
        "plain_powershell" => json!({
            "plain": plan_wait::plain_power_shell(text("command"))
                .map(|plain| json!({"commands": plain.commands, "rendering": plain.rendering})),
            "reading": plan_wait::power_shell_reading(text("command")),
        }),
        "dynamic_evaluation" => json!(plan_wait::shell_uses_dynamic_evaluation(text("command"))),
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

/// The request as upstream's object serialises: the six fields always,
/// then each optional field only when it is set.
fn typed_switch_json(request: &TypedSwitchRequest) -> Value {
    let mut out = serde_json::Map::new();
    out.insert(
        "switches".into(),
        json!(
            request
                .switches
                .iter()
                .map(|s| json!({"key": s.key, "value": s.value}))
                .collect::<Vec<_>>()
        ),
    );
    out.insert(
        "settings".into(),
        json!(
            request
                .settings
                .iter()
                .map(|s| json!({"key": s.key, "value": s.value}))
                .collect::<Vec<_>>()
        ),
    );
    out.insert("space".into(), json!(request.space));
    out.insert("intent".into(), json!(request.intent));
    out.insert("scope".into(), json!(request.scope));
    out.insert("error".into(), json!(request.error));
    if request.new_work_plan_approval_off {
        out.insert("newWorkPlanApprovalOff".into(), json!(true));
    }
    if let Some(policy) = &request.new_work_guard_policy {
        out.insert("newWorkGuardPolicy".into(), json!(policy));
    }
    if !request.new_work_ceremonies.is_empty() {
        let mut ceremonies = serde_json::Map::new();
        for (key, value) in &request.new_work_ceremonies {
            ceremonies.insert(key.clone(), json!(value));
        }
        out.insert("newWorkCeremonies".into(), Value::Object(ceremonies));
    }
    if !request.new_work_fences_off.is_empty() {
        out.insert(
            "newWorkFencesOff".into(),
            json!(
                request
                    .new_work_fences_off
                    .iter()
                    .map(|f| f.name())
                    .collect::<Vec<_>>()
            ),
        );
    }
    if request.asked {
        out.insert("asked".into(), json!(true));
    }
    if let Some(words) = &request.words {
        out.insert("words".into(), json!(words));
    }
    if let Some(unread) = &request.unread {
        out.insert("unread".into(), json!(unread));
    }
    Value::Object(out)
}

/// The project the dispatcher is asked from in a case: upstream's driver
/// pins the same directory in its environment.
const ORACLE_PROJECT: &str = "/oracle/project";

fn strings_of(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// A case's per-target evidence, as the hook would gather it.
fn evidence_of(value: &Value) -> UnitEvidence {
    let flag = |key: &str| value[key].as_bool().unwrap_or(false);
    UnitEvidence {
        unit: value["unit"].as_str().map(str::to_owned),
        plan_exists: flag("planExists"),
        instructions_exist: flag("instructionsExist"),
        approved: flag("approved"),
        contract_valid: flag("contractValid"),
        fingerprint_valid: flag("fingerprintValid"),
        receipt_valid: flag("receiptValid"),
        contract_hash: value["contractHash"].as_str().map(str::to_owned),
        reason: value["reason"].as_str().map(str::to_owned),
        review_appendix: value["reviewAppendix"].as_str().map(str::to_owned),
    }
}

/// A verdict with upstream's keys, the optional ones only when set.
fn verdict_json(verdict: &plan_approval::PlanApprovalVerdict) -> Value {
    let mut out = Map::new();
    out.insert("block".into(), json!(verdict.block));
    out.insert("mentioned".into(), json!(verdict.mentioned));
    if verdict.appendix_in_brief {
        out.insert("appendixInBrief".into(), json!(true));
    }
    if let Some(handoff) = verdict.handoff {
        out.insert("handoff".into(), json!(handoff.name()));
    }
    Value::Object(out)
}

/// A `plan_reason` case: one of the guard's refusals, named by `op`.
fn plan_reason_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    let mentioned = strings_of(&case["mentioned"]);
    let targets: Option<Vec<Option<String>>> = case["targets"].as_array().map(|targets| {
        targets
            .iter()
            .map(|target| target.as_str().map(str::to_owned))
            .collect()
    });
    let detail = case["detail"].as_str();
    match text("op") {
        "appendix" => json!(plan_approval::appendix_block_reason(&mentioned)),
        "block" => json!(plan_approval::block_reason(
            &mentioned,
            detail,
            targets.as_deref()
        )),
        "handoff" => match HandoffDefect::parse(text("cause")) {
            Some(cause) => json!(plan_approval::handoff_block_reason(
                &mentioned,
                cause,
                targets.as_deref()
            )),
            None => Value::Null,
        },
        "mutation" => json!(plan_approval::mutation_block_reason(
            case["unit"].as_str(),
            case["opaque"].as_bool().unwrap_or(false),
            detail
        )),
        "receipt" => {
            let evidence: Vec<UnitEvidence> = case["evidence"]
                .as_array()
                .into_iter()
                .flatten()
                .map(evidence_of)
                .collect();
            json!(plan_approval::receipt_detail(&evidence, &mentioned))
        }
        other => json!({ "unsupported": other }),
    }
}

/// A `guard_operation` case: one of the module's readers, named by `op`.
fn guard_operation_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    let operation = || {
        case.get("operation")
            .and_then(guard_operation::parse_operation)
    };
    let mode = if text("mode") == "source" {
        RenderMode::Source
    } else {
        RenderMode::Native
    };
    let shell = if text("shell") == "powershell" {
        RenderShell::PowerShell
    } else {
        RenderShell::Posix
    };
    match text("op") {
        "parse" => json!(operation().is_some()),
        "invocation" => operation().map_or(Value::Null, |operation| {
            let invocation = guard_operation::invocation(&operation);
            let mut out = Map::new();
            out.insert("route".into(), json!(invocation.route));
            out.insert("args".into(), json!(invocation.args));
            if let Some(source) = invocation.source {
                out.insert(
                    "source".into(),
                    json!({"route": source.route, "args": source.args}),
                );
            }
            Value::Object(out)
        }),
        "render" => operation().map_or(Value::Null, |operation| {
            match guard_operation::render(&operation, mode, text("harness"), shell) {
                Ok(command) => json!(command),
                Err(error) => json!({ "error": error }),
            }
        }),
        "matches_command" => json!(operation().is_some_and(|operation| {
            guard_operation::matches_command(&operation, text("command"))
        })),
        "matches_remedy" => json!(operation().is_some_and(|operation| {
            guard_operation::matches_remedy(
                &operation,
                text("remedy"),
                text("stage"),
                case["unit"].as_str(),
            )
        })),
        "same" => json!(guard_operation::same(case.get("left"), case.get("right"))),
        "continuation" => json!(
            guard_operation::parse_guard_restart_continuation_command(
                text("command"),
                case["harness"].as_str()
            )
            .map(|found| json!({
                "operation": {"kind": "restart-stage", "stage": found.stage},
                "direction": found.direction.name(),
                "scope": found.scope,
            }))
        ),
        "engine_invocation" => json!(guard_operation::is_guard_recovery_engine_invocation(
            &strings_of(&case["args"])
        )),
        "matches_engine_args" => json!(operation().is_some_and(|operation| {
            guard_operation::matches_engine_args(&operation, &strings_of(&case["args"]))
        })),
        "wording" => {
            let context = RemedyWordingContext {
                code: text("code"),
                stage: text("stage"),
                target: text("target"),
                unit: case["unit"].as_str(),
                fence: case["fence"].as_str().and_then(Fence::parse),
                source_unbindable: case["source_unbindable"].as_bool().unwrap_or(false),
                every_unit: case["every_unit"].as_bool().unwrap_or(false),
            };
            match guard_operation::remedy_wording(text("remedy"), &context) {
                Some((label, description)) => json!({"label": label, "description": description}),
                None => Value::Null,
            }
        }
        other => json!({ "unsupported": other }),
    }
}

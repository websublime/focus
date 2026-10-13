//! The oracle protocol for the guards (see `focus-graph`'s `oracle` module
//! for the protocol).
//!
//! `guard_policy` cases resolve a policy and every fence from a state text,
//! a scope default and memory declarations; `guards_line` cases parse and
//! write the state lines the guards read.

use std::path::Path;

use focus_graph::guard_memory::Declaration;
use serde_json::{Map, Value, json};

use crate::authority::Authority;
use crate::authority::decide_fence;
use crate::fences::{
    Fence, FenceResolution, format_guards_line, parse_guards_line, resolve_fences,
    set_guards_off_line, set_guards_on_line,
};
use crate::policy::{
    self, GuardPolicy, Resolution, format_guard_policy, parse_state_line, set_guard_policy_line,
    source_label,
};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 2] = ["guard_policy", "guards_line"];

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
        other => json!({ "unsupported": other }),
    }
}

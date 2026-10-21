//! The scope-owned ceremonies as a person reads them: each one's setting
//! and where it came from (the machine switch, the piece of work's own
//! line, the scope, the default), and the plan-approval stop, which a
//! strict memory policy keeps on and a machine switch lowers only when the
//! harness launched with it.
//!
//! Mirrors `resolveCeremony`, `parseCeremonyStateLine`, `formatCeremony`,
//! `changeControlSourceFromLabel` in `aidlc-lib.ts`, and
//! `resolvePlanApprovalSetting`, `planApprovalEnv`,
//! `formatPlanApprovalSetting` in `aidlc-guard-switch.ts`.

use focus_graph::scopes::GuardPolicy;
use focus_guards::policy::source_label;
use focus_guards::workflow::Guards;
use focus_js::date::parse_instant;
use focus_js::re;
use focus_js::text::trim;
use focus_state::fields::get_field;
use focus_workspace::names::SESSIONS_DIR;
use focus_workspace::records::{Selector, valid_session_id};
use focus_workspace::settings::Places;
use focus_workspace::{env, terminal};

use crate::workflow::Workflow;

/// `CEREMONY_KEYS`, with the state field and the kill switch of each.
pub const CEREMONIES: [(&str, &str, &str); 5] = [
    ("sensors", "Sensors", "DISABLE_SENSORS"),
    ("learnings", "Learnings", "DISABLE_LEARNINGS"),
    (
        "summary_confirmation",
        "Summary Confirmation",
        "DISABLE_SUMMARY_CONFIRMATION",
    ),
    (
        "plan_approval",
        "Plan Approval",
        "DISABLE_PLAN_APPROVAL_GUARD",
    ),
    ("collaborators", "Collaborators", "DISABLE_COLLABORATORS"),
];

/// A ceremony's setting and its source, as `resolveCeremony` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ceremony {
    pub value: &'static str,
    /// `env FOCUS_…`, `FOCUS_… in <file>`, `you`, `command`, `scope classic`
    /// or `default`.
    pub source: String,
}

/// `changeControlSourceFromLabel`.
fn source_from_label(label: &str) -> String {
    if label == "set by you" || label == "you" {
        return "you".to_owned();
    }
    if label == "set by a command" {
        return "command".to_owned();
    }
    if let Some(found) = re::find(r"^from\s+(.+)$", "", label) {
        return trim(re::group(label, &found, 1).unwrap_or("")).to_owned();
    }
    if label.is_empty() {
        "you".to_owned()
    } else {
        label.to_owned()
    }
}

/// `parseCeremonyStateLine`: `on (from scope feature)` as a value and a
/// source.
#[must_use]
pub fn parse_state_line(raw: Option<&str>) -> Option<Ceremony> {
    let raw = trim(raw?);
    if raw.is_empty() {
        return None;
    }
    let found = re::find(r"^(on|off)\b(?:\s*\((.*)\))?\s*$", "i", raw)?;
    let value = if re::group(raw, &found, 1)?.to_lowercase() == "on" {
        "on"
    } else {
        "off"
    };
    let label = re::group(raw, &found, 2).map(trim).unwrap_or("");
    Some(Ceremony {
        value,
        source: source_from_label(label),
    })
}

/// `formatCeremony`.
#[must_use]
pub fn format_ceremony(value: &str, source: &str) -> String {
    format!("{value} ({})", source_label(source))
}

fn places_flag(
    wf: &Workflow,
    switch: &str,
    env_read: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    Places::of(&wf.layout.project)
        .flag(&env::full_name(switch), env_read)
        .ok()
        .flatten()
}

/// `resolveCeremony(key, scope, state)`, reading the kill switch through
/// `env_read`.
fn resolve_with(
    wf: &Workflow,
    key: &str,
    state: &str,
    env_read: &dyn Fn(&str) -> Option<String>,
) -> Ceremony {
    let (_, field, switch) = CEREMONIES
        .iter()
        .find(|(known, _, _)| *known == key)
        .copied()
        .unwrap_or((key, key, key));
    let scope = get_field(state, "Scope")
        .map(|scope| trim(scope).to_lowercase())
        .filter(|scope| !scope.is_empty());
    let declared: Option<&str> = scope.as_deref().and_then(|scope| {
        wf.install.definition(scope).and_then(|definition| {
            definition
                .meta
                .ceremony
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, value)| value.as_str())
        })
    });
    let scope_default = declared.unwrap_or("on");
    let intent = parse_state_line(get_field(state, field));
    if places_flag(wf, switch, env_read).as_deref() == Some("1") {
        return Ceremony {
            value: "off",
            source: Places::of(&wf.layout.project)
                .kill_switch_source(&env::full_name(switch), env_read),
        };
    }
    match intent {
        Some(intent) => intent,
        None => Ceremony {
            value: if scope_default == "off" { "off" } else { "on" },
            source: match (declared, scope) {
                (Some(_), Some(scope)) => format!("scope {scope}"),
                _ => "default".to_owned(),
            },
        },
    }
}

/// `resolveCeremony`.
#[must_use]
pub fn resolve(wf: &Workflow, key: &str, state: &str) -> Ceremony {
    resolve_with(wf, key, state, &|name| std::env::var(name).ok())
}

/// `sessionPlanApprovalBypassRecorded`: the session-start hook recorded the
/// machine switch from its own environment.
fn session_bypass_recorded(wf: &Workflow, session: &str) -> bool {
    let segment = focus_intent::create::runtime_session_segment(session);
    if segment.is_empty() {
        return false;
    }
    let path = wf
        .layout
        .workspace_root()
        .join(SESSIONS_DIR)
        .join("plan-approval")
        .join(format!("plan-approval-bypass-{segment}"));
    // Read without following a link, as upstream reads its runtime files.
    if !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file()) {
        return false;
    }
    std::fs::read(&path)
        .ok()
        .and_then(|bytes| parse_instant(trim(&String::from_utf8_lossy(&bytes))))
        .is_some()
}

/// `planApprovalMachineSwitchTrusted`: the switch counts when the harness
/// launched with it (recorded for the session), or when no harness session
/// was ever recorded in this project.
fn machine_switch_trusted(wf: &Workflow) -> bool {
    if env::var("DISABLE_PLAN_APPROVAL_GUARD").as_deref() != Some("1") {
        return false;
    }
    let session = wf
        .selection
        .session_id
        .as_deref()
        .or(wf.invoker.session_override.as_deref())
        .and_then(|session| valid_session_id(Some(session)).map(|_| session));
    match session {
        Some(session) => session_bypass_recorded(wf, session),
        None => terminal::current_session_id(&wf.layout).is_none(),
    }
}

/// The plan-approval stop and where its setting came from
/// (`PlanApprovalSetting`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanApproval {
    pub value: &'static str,
    pub source: String,
}

/// `resolvePlanApprovalSetting`.
#[must_use]
pub fn resolve_plan_approval(wf: &Workflow, state: &str) -> PlanApproval {
    let switch = env::full_name("DISABLE_PLAN_APPROVAL_GUARD");
    // A command that sets the machine switch for itself is read as unset.
    let trusted = machine_switch_trusted(wf);
    let env_read = move |name: &str| -> Option<String> {
        let value = std::env::var(name).ok()?;
        if name == switch && value == "1" && !trusted {
            return None;
        }
        Some(value)
    };
    let resolution = resolve_with(wf, "plan_approval", state, &env_read);
    let is_kill_switch = resolution.source.starts_with("env ")
        || re::test(r"^FOCUS_[A-Z0-9_]+ in ", "", &resolution.source);
    if is_kill_switch
        && places_flag(wf, "DISABLE_PLAN_APPROVAL_GUARD", &env_read).as_deref() == Some("1")
    {
        return PlanApproval {
            value: "off",
            source: resolution.source,
        };
    }
    let source = if re::test(
        r"^(?:you|command|default|scope [a-z][a-z0-9-]{0,63})$",
        "",
        &resolution.source,
    ) {
        resolution.source.clone()
    } else {
        "this piece of work's settings".to_owned()
    };
    if resolution.value == "off" {
        let strict = Guards::read(&wf.layout, &Selector::default(), &wf.invoker)
            .and_then(|guards| guards.declarations());
        match strict {
            Ok(declarations) => {
                if let Some(strict) = declarations
                    .iter()
                    .find(|declaration| declaration.value == GuardPolicy::Strict)
                {
                    return PlanApproval {
                        value: "on",
                        source: format!("guard policy strict (from {}.md)", strict.layer),
                    };
                }
            }
            Err(_) => {
                return PlanApproval {
                    value: "on",
                    source: "guard policy could not be read".to_owned(),
                };
            }
        }
    }
    PlanApproval {
        value: resolution.value,
        source,
    }
}

/// `formatPlanApprovalSetting`.
#[must_use]
pub fn format_plan_approval(setting: &PlanApproval) -> String {
    if setting.source.starts_with("guard policy") {
        format!("{} ({})", setting.value, setting.source)
    } else {
        format_ceremony(setting.value, &setting.source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_line_reads_as_a_value_and_a_source() {
        let line = parse_state_line(Some("on (from scope feature)")).unwrap();
        assert_eq!((line.value, line.source.as_str()), ("on", "scope feature"));
        let set = parse_state_line(Some("OFF (set by you)")).unwrap();
        assert_eq!((set.value, set.source.as_str()), ("off", "you"));
        let bare = parse_state_line(Some("off")).unwrap();
        assert_eq!((bare.value, bare.source.as_str()), ("off", "you"));
        let command = parse_state_line(Some("off (set by a command)")).unwrap();
        assert_eq!(command.source, "command");
        assert!(parse_state_line(Some("maybe")).is_none());
        assert!(parse_state_line(Some("")).is_none());
        assert!(parse_state_line(None).is_none());
        assert_eq!(format_ceremony("on", "default"), "on (from default)");
        assert_eq!(format_ceremony("off", "you"), "off (set by you)");
    }
}

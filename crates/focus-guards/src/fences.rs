//! The fences, and what each one's setting is for a workflow.
//!
//! A fence is a check that refuses an action nobody directed. There are
//! five. The policy word lowers a fixed set of them; a person can lower a
//! switchable fence for one piece of work (`/focus config set
//! guard.<fence> off`, which writes the `Guards Off` state line) or raise
//! one back above the policy (`Guards On`); and an environment kill switch,
//! set in the shell or recorded in the settings, holds a fence down for the
//! whole machine. Human presence is the key holder, not a fence the policy
//! word touches: only its kill switch lowers it.
//!
//! The ladder, for each fence: the kill switch first, then `Guards Off`
//! unless a memory layer holds strict, then `Guards On`, then the policy
//! word, then on.
//!
//! Mirrors `GUARD_FENCES`, `SWITCHABLE_GUARD_FENCES` and the glosses in
//! `aidlc-guard-fences.ts`, and `GUARD_FENCE_ENV`, `fencesLoweredByPolicy`,
//! `parseGuardFence`, `parseGuardsOffLine`, `formatGuardsOffLine`,
//! `setGuardsOffLine`, `setGuardsOnLine`, `resolveFences`,
//! `fenceSourceLabel` and `formatFence` in `aidlc-lib.ts`.

use focus_js::re;
use focus_js::text::trim;
use focus_state::fields::{get_field, set_field};

use crate::policy::{
    CHANGE_CONTROL_FIELD, GUARD_POLICY_FIELD, GuardPolicy, Resolution, insert_after_anchors,
    source_label,
};

/// The state line naming the fences a person switched off for this work.
pub const GUARDS_OFF_FIELD: &str = "Guards Off";
/// The state line naming the fences a person raised back above the policy.
pub const GUARDS_ON_FIELD: &str = "Guards On";

/// A fence, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fence {
    PlanApproval,
    ReviewFreeze,
    StateTransition,
    ReviewerScope,
    HumanPresence,
}

impl Fence {
    /// The five fences, in upstream's order.
    pub const ALL: [Fence; 5] = [
        Self::PlanApproval,
        Self::ReviewFreeze,
        Self::StateTransition,
        Self::ReviewerScope,
        Self::HumanPresence,
    ];

    /// The fences a person may switch for one piece of work: all but human
    /// presence.
    pub const SWITCHABLE: [Fence; 4] = [
        Self::PlanApproval,
        Self::ReviewFreeze,
        Self::StateTransition,
        Self::ReviewerScope,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PlanApproval => "plan-approval",
            Self::ReviewFreeze => "review-freeze",
            Self::StateTransition => "state-transition",
            Self::ReviewerScope => "reviewer-scope",
            Self::HumanPresence => "human-presence",
        }
    }

    /// `parseGuardFence`: the fence a word names, case and Markdown emphasis
    /// ignored.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let cleaned: String = raw
            .to_lowercase()
            .chars()
            .filter(|c| !matches!(c, '`' | '*' | '_'))
            .collect();
        let word = trim(&cleaned);
        Self::ALL.into_iter().find(|fence| fence.name() == word)
    }

    #[must_use]
    pub fn is_switchable(self) -> bool {
        Self::SWITCHABLE.contains(&self)
    }

    /// The environment variable that is this fence's kill switch, when it
    /// has one (`GUARD_FENCE_ENV`): `FOCUS_…`.
    #[must_use]
    pub const fn kill_switch(self) -> Option<&'static str> {
        match self {
            Self::PlanApproval => Some("FOCUS_DISABLE_PLAN_APPROVAL_GUARD"),
            Self::ReviewFreeze => Some("FOCUS_DISABLE_REVIEW_FREEZE_HOOK"),
            Self::ReviewerScope => Some("FOCUS_DISABLE_REVIEWER_SCOPE_HOOK"),
            Self::HumanPresence => Some("FOCUS_SKIP_HUMAN_PRESENCE_GUARD"),
            Self::StateTransition => None,
        }
    }

    /// `GUARD_FENCE_LABELS`: the fence named for a person.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PlanApproval => "Plan approval (code before an approved plan)",
            Self::ReviewFreeze => "Review freeze (edits after a review receipt)",
            Self::StateTransition => "State transition (direct lifecycle commands)",
            Self::ReviewerScope => "Reviewer read scope (a dispatched reviewer outside its unit)",
            Self::HumanPresence => {
                "Human presence (a real human turn behind approvals and answers)"
            }
        }
    }

    /// `CHECK_GLOSS`: what a switchable fence does for the person, in a few
    /// plain words.
    #[must_use]
    pub const fn gloss(self) -> Option<&'static str> {
        match self {
            Self::PlanApproval => Some("you approve each code plan before it is built"),
            Self::ReviewFreeze => {
                Some("a document stays as it was reviewed until you ask for changes")
            }
            Self::StateTransition => {
                Some("only Focus's own steps move the work from stage to stage")
            }
            Self::ReviewerScope => Some("reviewers read only the work they review"),
            Self::HumanPresence => None,
        }
    }

    /// The config key of a switchable fence: `guard.plan-approval`.
    #[must_use]
    pub fn config_key(self) -> String {
        format!("guard.{}", self.name())
    }

    /// `guardFenceFromConfigKey`: the switchable fence a config key names.
    #[must_use]
    pub fn from_config_key(key: &str) -> Option<Self> {
        let name = key.strip_prefix("guard.")?;
        Self::SWITCHABLE
            .into_iter()
            .find(|fence| fence.name() == name)
    }
}

/// `GUARD_POLICY_GLOSS`: what each policy word means for the person.
#[must_use]
pub const fn policy_gloss(policy: GuardPolicy) -> &'static str {
    match policy {
        GuardPolicy::Strict => "Focus asks you again when something you approved changes",
        GuardPolicy::Relaxed => "Focus carries on with a note when something you approved changes",
        GuardPolicy::Off => "Focus carries on with a note and skips some of its own checks",
    }
}

/// `fencesLoweredByPolicy`: the fences a policy word lowers.
#[must_use]
pub fn lowered_by_policy(policy: GuardPolicy) -> &'static [Fence] {
    match policy {
        GuardPolicy::Off => &[
            Fence::PlanApproval,
            Fence::ReviewFreeze,
            Fence::StateTransition,
            Fence::ReviewerScope,
        ],
        GuardPolicy::Relaxed => &[Fence::PlanApproval, Fence::ReviewFreeze],
        GuardPolicy::Strict => &[],
    }
}

/// `parseGuardsOffLine` (and `parseGuardsOnLine`, which is the same): the
/// switchable fences a `Guards Off` or `Guards On` line names, each once, in
/// the line's order; `none`, an empty or an absent line is no fence. A
/// trailing `(…)` label is ignored.
#[must_use]
pub fn parse_guards_line(raw: Option<&str>) -> Vec<Fence> {
    let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
        return Vec::new();
    };
    let list = re::replace(r"\s*\(.*\)\s*$", "", raw, "");
    let list = trim(&list);
    if list.is_empty() || list.to_lowercase() == "none" {
        return Vec::new();
    }
    let mut fences = Vec::new();
    for part in list.split(',') {
        if let Some(fence) = Fence::parse(part)
            && fence.is_switchable()
            && !fences.contains(&fence)
        {
            fences.push(fence);
        }
    }
    fences
}

/// `formatGuardsOffLine` (and `formatGuardsOnLine`): the line for a set of
/// fences, in canonical order: `plan-approval, review-freeze (set by you)`,
/// or `none`.
#[must_use]
pub fn format_guards_line(fences: &[Fence]) -> String {
    let ordered: Vec<&str> = Fence::SWITCHABLE
        .into_iter()
        .filter(|fence| fences.contains(fence))
        .map(Fence::name)
        .collect();
    if ordered.is_empty() {
        "none".to_owned()
    } else {
        format!("{} (set by you)", ordered.join(", "))
    }
}

/// `setGuardsOffLine`: `content` with its `Guards Off` line set, inserted
/// under the policy line (or the state anchors) when absent.
#[must_use]
pub fn set_guards_off_line(content: &str, fences: &[Fence]) -> String {
    let content = if get_field(content, GUARDS_OFF_FIELD).is_none() {
        insert_after_anchors(
            content,
            &[
                GUARD_POLICY_FIELD,
                CHANGE_CONTROL_FIELD,
                "Review Override",
                "Test Strategy",
                "Scope",
            ],
            GUARDS_OFF_FIELD,
        )
    } else {
        content.to_owned()
    };
    set_field(&content, GUARDS_OFF_FIELD, &format_guards_line(fences))
}

/// `setGuardsOnLine`: `content` with its `Guards On` line set, inserted
/// under `Guards Off` or the same anchors when absent.
#[must_use]
pub fn set_guards_on_line(content: &str, fences: &[Fence]) -> String {
    let content = if get_field(content, GUARDS_ON_FIELD).is_none() {
        insert_after_anchors(
            content,
            &[
                GUARDS_OFF_FIELD,
                GUARD_POLICY_FIELD,
                CHANGE_CONTROL_FIELD,
                "Review Override",
                "Test Strategy",
                "Scope",
            ],
            GUARDS_ON_FIELD,
        )
    } else {
        content.to_owned()
    };
    set_field(&content, GUARDS_ON_FIELD, &format_guards_line(fences))
}

/// Whether a fence is up or down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    On,
    Off,
}

impl Setting {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// One fence's effective setting and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FenceResolution {
    pub fence: Fence,
    pub value: Setting,
    /// `env FOCUS_…`, `FOCUS_… in <file>`, `you`, `guard policy off (from
    /// scope express)`, `guard policy strict (from team.md)` or `default`.
    pub source: String,
}

impl FenceResolution {
    /// `fenceSourceLabel`: the source worded for a person, `set by you` for
    /// `you`.
    #[must_use]
    pub fn source_label(&self) -> String {
        if self.source == "you" {
            "set by you".to_owned()
        } else {
            self.source.clone()
        }
    }

    /// `formatFence`: `on (default)`, `off (set by you)`, `off (env …)`.
    #[must_use]
    pub fn format(&self) -> String {
        format!("{} ({})", self.value.as_str(), self.source_label())
    }
}

/// `resolveFences`: the effective setting of every fence, in [`Fence::ALL`]
/// order. `flag` answers a kill switch's value from the environment or the
/// settings (`resolveProjectFlag`), and `switch_source` names where a switch
/// that is set came from (`killSwitchSource`).
#[must_use]
pub fn resolve_fences(
    policy: &Resolution,
    state: &str,
    flag: &dyn Fn(&str) -> Option<String>,
    switch_source: &dyn Fn(&str) -> String,
) -> Vec<FenceResolution> {
    let per_run_off = parse_guards_line(get_field(state, GUARDS_OFF_FIELD));
    let per_run_on = parse_guards_line(get_field(state, GUARDS_ON_FIELD));
    let by_policy = lowered_by_policy(policy.value);
    let policy_source =
        |word: &str| format!("guard policy {word} ({})", source_label(&policy.source));
    Fence::ALL
        .into_iter()
        .map(|fence| {
            let (value, source) = if let Some(env) = fence.kill_switch()
                && flag(env).as_deref() == Some("1")
            {
                (Setting::Off, switch_source(env))
            } else if policy.memory_strict.is_none()
                && fence.is_switchable()
                && per_run_off.contains(&fence)
            {
                (Setting::Off, "you".to_owned())
            } else if fence.is_switchable() && per_run_on.contains(&fence) {
                (Setting::On, "you".to_owned())
            } else if policy.memory_strict.is_some()
                && fence.is_switchable()
                && per_run_off.contains(&fence)
            {
                (Setting::On, policy_source("strict"))
            } else if by_policy.contains(&fence) {
                (Setting::Off, policy_source(policy.value.as_str()))
            } else {
                (Setting::On, "default".to_owned())
            };
            FenceResolution {
                fence,
                value,
                source,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::policy::{Declaration, resolve};

    #[test]
    fn a_guards_line_names_switchable_fences_once_each() {
        assert_eq!(parse_guards_line(None), Vec::<Fence>::new());
        assert_eq!(parse_guards_line(Some("")), Vec::<Fence>::new());
        assert_eq!(parse_guards_line(Some("None")), Vec::<Fence>::new());
        assert_eq!(
            parse_guards_line(Some(
                " review-freeze , `Plan-Approval`, plan-approval, human-presence, nope (set by you) "
            )),
            [Fence::ReviewFreeze, Fence::PlanApproval]
        );
        assert_eq!(format_guards_line(&[]), "none");
        assert_eq!(
            format_guards_line(&[
                Fence::ReviewerScope,
                Fence::PlanApproval,
                Fence::HumanPresence
            ]),
            "plan-approval, reviewer-scope (set by you)"
        );
        assert_eq!(
            Fence::from_config_key("guard.review-freeze"),
            Some(Fence::ReviewFreeze)
        );
        assert_eq!(Fence::from_config_key("guard.human-presence"), None);
        assert_eq!(Fence::from_config_key("review-freeze"), None);
    }

    #[test]
    fn the_guards_lines_are_inserted_under_the_policy_line() {
        let state = "- **Scope**: feature\n- **Guard Policy**: off (set by you)\n- **A**: 1\n";
        let off = set_guards_off_line(state, &[Fence::ReviewFreeze]);
        assert_eq!(
            off,
            "- **Scope**: feature\n- **Guard Policy**: off (set by you)\n- **Guards Off**: review-freeze (set by you)\n- **A**: 1\n"
        );
        let on = set_guards_on_line(&off, &[]);
        assert!(on.contains(
            "- **Guards Off**: review-freeze (set by you)\n- **Guards On**: none\n- **A**: 1\n"
        ));
        assert_eq!(
            set_guards_off_line("- **A**: 1\n", &[]),
            "- **A**: 1\n- **Guards Off**: none\n"
        );
        assert_eq!(
            set_guards_off_line("- **Scope**: x\n", &[Fence::PlanApproval]),
            "- **Scope**: x\n- **Guards Off**: plan-approval (set by you)\n"
        );
    }

    fn policy(state: &str, memory_strict: bool) -> Resolution {
        let declarations = if memory_strict {
            vec![Declaration {
                layer: "team",
                path: Path::new("/m/team.md").to_path_buf(),
                heading: "## Guard Policy",
                value: GuardPolicy::Strict,
            }]
        } else {
            Vec::new()
        };
        resolve(
            state,
            Path::new("/s.md"),
            GuardPolicy::Off,
            || Ok(declarations),
            true,
        )
        .unwrap()
    }

    #[test]
    fn a_fence_is_settled_by_the_ladder() {
        let setting = |state: &str, memory_strict: bool, env: &[&str]| -> Vec<String> {
            let flag = |name: &str| env.contains(&name).then(|| "1".to_owned());
            let source = |name: &str| format!("env {name}");
            resolve_fences(&policy(state, memory_strict), state, &flag, &source)
                .iter()
                .map(FenceResolution::format)
                .collect()
        };
        // Policy off lowers four; human presence stays up.
        assert_eq!(
            setting("- **Guard Policy**: off (from scope classic)\n", false, &[]),
            [
                "off (guard policy off (from scope classic))",
                "off (guard policy off (from scope classic))",
                "off (guard policy off (from scope classic))",
                "off (guard policy off (from scope classic))",
                "on (default)",
            ]
        );
        assert_eq!(
            setting("- **Guard Policy**: relaxed (set by you)\n", false, &[])[2..],
            ["on (default)", "on (default)", "on (default)"]
        );
        // The person's switches, each way, and the kill switch first.
        let switched = "- **Guard Policy**: relaxed (set by you)\n- **Guards Off**: state-transition (set by you)\n- **Guards On**: plan-approval (set by you)\n";
        assert_eq!(
            setting(switched, false, &["FOCUS_SKIP_HUMAN_PRESENCE_GUARD"]),
            [
                "on (set by you)",
                "off (guard policy relaxed (set by you))",
                "off (set by you)",
                "on (default)",
                "off (env FOCUS_SKIP_HUMAN_PRESENCE_GUARD)",
            ]
        );
        // A kill switch counts only when it reads `1`, and state transition
        // has none.
        let neither = "- **Guard Policy**: strict (set by you)\n";
        let flag_values = |value: &str| -> Vec<String> {
            let flag = |_: &str| Some(value.to_owned());
            let source = |name: &str| format!("env {name}");
            resolve_fences(&policy(neither, false), neither, &flag, &source)
                .iter()
                .map(FenceResolution::format)
                .collect()
        };
        assert_eq!(flag_values("0"), vec!["on (default)"; 5]);
        assert_eq!(flag_values(""), vec!["on (default)"; 5]);
        assert_eq!(flag_values("1")[2], "on (default)");
        assert_eq!(
            flag_values("1")[4],
            "off (env FOCUS_SKIP_HUMAN_PRESENCE_GUARD)"
        );
        assert_eq!(
            parse_guards_line(Some("NONE (set by you)")),
            Vec::<Fence>::new()
        );
        // Memory strict: a switched-off fence is back on, and says why.
        assert_eq!(
            setting(switched, true, &[])[2],
            "on (guard policy strict (from team.md))"
        );
        assert_eq!(setting(switched, true, &[])[0], "on (set by you)");
    }
}

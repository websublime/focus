//! The authority behind an action, and the one decision every guard makes.
//!
//! Every action a guard sees is covered by one of two authorities, or
//! neither: a *grant*, a human message recorded after the engine's last
//! directive, or an *instruction*, the directive the engine has in force.
//! Both are read off markers the hooks and the orchestrator leave in the
//! record's engine directory. Neither lowers a fence: a grant proves a
//! person spoke, not what they asked for, and an instruction covers the work
//! it asks for, not the loop skipping one of its steps. The authority is
//! resolved once and recorded with the decision, so the audit row can name
//! it.
//!
//! The decision itself reads one bit: a fence that is lowered stands aside,
//! one that is up holds.
//!
//! Mirrors `Authority`, `authorityFor`, `decideGuard`, `decideFence`,
//! `guardStoodAsideLine` and `guardStandAsideSpeaks` in `aidlc-lib.ts`.

use crate::fences::{Fence, FenceResolution, Setting};
use crate::policy::{GuardPolicy, Resolution};

/// What covers the action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cover {
    Grant,
    Instruction,
    None,
}

impl Cover {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Grant => "grant",
            Self::Instruction => "instruction",
            Self::None => "none",
        }
    }
}

/// Who is at the keyboard or in the loop: knowledge, never authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Main,
    Subagent,
    Unattended,
}

impl Actor {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Subagent => "subagent",
            Self::Unattended => "unattended",
        }
    }
}

/// How a grant was proven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantSource {
    MarkerSequence,
    TurnMarker,
    DispatchStamp,
}

impl GrantSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MarkerSequence => "marker-sequence",
            Self::TurnMarker => "turn-marker",
            Self::DispatchStamp => "dispatch-stamp",
        }
    }
}

/// The authority covering an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authority {
    pub covered: Cover,
    pub actor: Actor,
    /// The operator declared that no human is watching.
    pub unattended: bool,
    /// Present when `covered` is a grant.
    pub grant: Option<(GrantSource, Option<u64>)>,
    /// Present when the engine has an instruction in force: its stage, its
    /// unit and the directive's kind.
    pub instruction: Option<(String, Option<String>, String)>,
}

impl Authority {
    /// `authorityFor` where the record has no markers to read: nothing
    /// covers the action, and the actor is the main loop, or an unattended
    /// driver when the environment says so (`FOCUS_UNATTENDED=1`).
    #[must_use]
    pub fn without_markers(unattended: bool) -> Self {
        Self {
            covered: Cover::None,
            actor: if unattended {
                Actor::Unattended
            } else {
                Actor::Main
            },
            unattended,
            grant: None,
            instruction: None,
        }
    }

    /// The grant's source for an audit row: its name, or `none`.
    #[must_use]
    pub fn grant_source(&self) -> &'static str {
        self.grant.map_or("none", |(source, _)| source.as_str())
    }
}

/// What a guard does with the action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Nothing to decide; the caller proceeds silently.
    Pass,
    /// The action proceeds, the person gets one line, the ledger one row.
    StandAside,
    /// The fence holds.
    Hold,
}

impl Decision {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::StandAside => "stand-aside",
            Self::Hold => "hold",
        }
    }
}

/// `decideGuard` for a fence: lowered by the policy word or the person's own
/// switch, it stands aside; otherwise it holds. A grant lowers nothing.
#[must_use]
pub fn decide_guard(lowered: bool) -> Decision {
    if lowered {
        Decision::StandAside
    } else {
        Decision::Hold
    }
}

/// The whole decision for one fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FenceDecision {
    pub fence: Fence,
    pub decision: Decision,
    pub authority: Authority,
    pub policy: GuardPolicy,
    pub fence_setting: Setting,
    /// The fence's source, worded for a person (`set by you`).
    pub source: String,
}

/// `decideFence`, from the policy, the fence's resolution and the authority.
#[must_use]
pub fn decide_fence(
    policy: &Resolution,
    fence: &FenceResolution,
    authority: Authority,
) -> FenceDecision {
    FenceDecision {
        fence: fence.fence,
        decision: decide_guard(fence.value == Setting::Off),
        authority,
        policy: policy.value,
        fence_setting: fence.value,
        source: fence.source_label(),
    }
}

impl FenceDecision {
    /// `guardStandAsideSpeaks`: whether a lowered fence says that it stood
    /// aside. Under Guard Policy off it says nothing: off means off, and the
    /// person heard the one line when the policy was set.
    #[must_use]
    pub fn speaks(&self) -> bool {
        self.policy != GuardPolicy::Off
    }

    /// `guardStoodAsideLine`: the one line a person hears when this fence
    /// stood aside. `recorded` false: the audit row could not be written,
    /// and the line says so.
    #[must_use]
    pub fn stood_aside_line(&self, detail: Option<&str>, recorded: bool) -> String {
        stood_aside_line(self.fence, &self.source, detail, recorded)
    }
}

/// `guardStoodAsideLine`.
#[must_use]
pub fn stood_aside_line(
    fence: Fence,
    source: &str,
    detail: Option<&str>,
    recorded: bool,
) -> String {
    let where_ = if recorded {
        "Recorded in the audit trail".to_owned()
    } else {
        "Not recorded in the audit trail, which was busy or could not be written; `focus doctor` lists it".to_owned()
    };
    let detail = detail.map_or_else(|| ".".to_owned(), |detail| format!(": {detail}"));
    format!(
        "Continuing past the {} check because it is off for this piece of work ({source}). {where_}{detail}",
        fence.name()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lowered_fence_stands_aside_and_an_up_one_holds() {
        assert_eq!(decide_guard(true), Decision::StandAside);
        assert_eq!(decide_guard(false), Decision::Hold);
        let main = Authority::without_markers(false);
        assert_eq!((main.covered, main.actor), (Cover::None, Actor::Main));
        assert_eq!(main.grant_source(), "none");
        assert_eq!(Authority::without_markers(true).actor, Actor::Unattended);
        assert_eq!(
            stood_aside_line(
                Fence::StateTransition,
                "set by you",
                Some("focus-state set"),
                true
            ),
            "Continuing past the state-transition check because it is off for this piece of work (set by you). Recorded in the audit trail: focus-state set"
        );
        assert_eq!(
            stood_aside_line(Fence::PlanApproval, "default", None, false),
            "Continuing past the plan-approval check because it is off for this piece of work (default). Not recorded in the audit trail, which was busy or could not be written; `focus doctor` lists it."
        );
    }
}

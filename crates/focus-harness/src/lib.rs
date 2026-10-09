//! Harness descriptors.
//!
//! A harness is a coding assistant the engine runs inside. Upstream supports
//! seven; Focus's first version targets three. Upstream describes part of each
//! harness as data (`harness.json`, projected from the harness manifest), but
//! most behavioural differences are comparisons against a harness name or
//! directory, spread across the engine and the two adapter files. This crate
//! is where those differences are written down once, as data, so engine code
//! asks a question (`hook_events`, `statusline`) instead of matching on a
//! name.
//!
//! Every value here is read off upstream's generated distribution at commit
//! `2a555f7` (`harness.json`, `.claude/settings.json`, `.cursor/hooks.json`,
//! `.github/hooks/focus.json`), not inferred.

use std::fmt;

/// A supported harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Harness {
    Claude,
    Cursor,
    Copilot,
}

/// How a harness process reaches the engine's hook bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEntry {
    /// The host's payload is already in the shape the hook bodies read, so the
    /// host calls them directly: `engine hook <target>`.
    Direct,
    /// The host's payload and decision format differ, so an adapter reshapes
    /// both: `engine adapter <harness> <target>`.
    Adapter,
}

impl Harness {
    /// The harnesses Focus supports, in a stable order.
    pub const ALL: [Harness; 3] = [Harness::Claude, Harness::Cursor, Harness::Copilot];

    /// The identifier used on the command line and in `harness.json` `name`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Cursor => "cursor",
            Harness::Copilot => "copilot",
        }
    }

    /// Resolve a harness from its [`name`](Self::name).
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|h| h.name() == name)
    }

    /// The product name shown to people (`harness.json` `productName`).
    #[must_use]
    pub const fn product_name(self) -> &'static str {
        match self {
            Harness::Claude => "Claude Code",
            Harness::Cursor => "Cursor",
            Harness::Copilot => "GitHub Copilot",
        }
    }

    /// The project-relative directory that holds the engine's files for this
    /// harness; the value upstream substitutes for `{{HARNESS_DIR}}`.
    ///
    /// Copilot's is `.focus`, not `.github`: `.github` belongs to the
    /// repository, so only the files Copilot must discover there (skills,
    /// agents, the hook registration) are emitted into it.
    #[must_use]
    pub const fn dir(self) -> &'static str {
        match self {
            Harness::Claude => ".claude",
            Harness::Cursor => ".cursor",
            Harness::Copilot => ".focus",
        }
    }

    /// The project-relative file where the engine's hooks are registered.
    #[must_use]
    pub const fn hook_registration(self) -> &'static str {
        match self {
            Harness::Claude => ".claude/settings.json",
            Harness::Cursor => ".cursor/hooks.json",
            Harness::Copilot => ".github/hooks/focus.json",
        }
    }

    /// Whether the host calls hook bodies directly or through an adapter.
    #[must_use]
    pub const fn hook_entry(self) -> HookEntry {
        match self {
            Harness::Claude => HookEntry::Direct,
            Harness::Cursor | Harness::Copilot => HookEntry::Adapter,
        }
    }

    /// The host events the engine registers a hook on, spelled as the host
    /// spells them.
    #[must_use]
    pub const fn hook_events(self) -> &'static [&'static str] {
        match self {
            Harness::Claude => &[
                "PreToolUse",
                "UserPromptSubmit",
                "SessionStart",
                "SessionEnd",
                "PostToolUse",
                "PreCompact",
                "SubagentStop",
                "Stop",
            ],
            Harness::Cursor => &[
                "sessionStart",
                "sessionEnd",
                "beforeSubmitPrompt",
                "preToolUse",
                "postToolUse",
                "postToolUseFailure",
                "preCompact",
                "stop",
            ],
            Harness::Copilot => &[
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "PreCompact",
                "SubagentStart",
                "SubagentStop",
                "Stop",
            ],
        }
    }

    /// Whether the harness renders an engine-provided status line.
    #[must_use]
    pub const fn statusline(self) -> bool {
        matches!(self, Harness::Claude)
    }

    /// The largest directive, in UTF-8 bytes, this host shows whole, when the
    /// harness declares one (`harness.json` `directiveMaxBytes`).
    ///
    /// This is not the effective limit. The orchestrator caps every directive
    /// at its own common size (28 KiB upstream) and a declared value can only
    /// lower that cap; `None` means the common cap applies unchanged.
    #[must_use]
    pub const fn declared_directive_max_bytes(self) -> Option<usize> {
        match self {
            Harness::Copilot => Some(19_000),
            Harness::Claude | Harness::Cursor => None,
        }
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for harness in Harness::ALL {
            assert_eq!(Harness::from_name(harness.name()), Some(harness));
        }
        assert_eq!(Harness::from_name("kiro"), None);
    }

    #[test]
    fn only_claude_is_called_directly_and_has_a_statusline() {
        assert_eq!(Harness::Claude.hook_entry(), HookEntry::Direct);
        assert!(Harness::Claude.statusline());
        for harness in [Harness::Cursor, Harness::Copilot] {
            assert_eq!(harness.hook_entry(), HookEntry::Adapter);
            assert!(!harness.statusline());
        }
    }

    #[test]
    fn copilot_has_no_session_end_and_cursor_no_subagent_events() {
        let has = |h: Harness, event: &str| {
            h.hook_events()
                .iter()
                .any(|e| e.eq_ignore_ascii_case(event))
        };
        assert!(has(Harness::Claude, "SessionEnd"));
        assert!(has(Harness::Cursor, "sessionEnd"));
        assert!(!has(Harness::Copilot, "SessionEnd"));
        assert!(has(Harness::Copilot, "SubagentStart"));
        assert!(!has(Harness::Cursor, "subagentStop"));
    }
}

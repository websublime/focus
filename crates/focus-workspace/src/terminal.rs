//! Who is at the terminal this command runs in.
//!
//! A refusal that asks the person for something says where to ask it: in
//! their coding tool's chat, when that tool started this command; at their
//! own terminal, when a person is typing there. Both ends being a terminal
//! says nothing on its own (an IDE runs its agent's commands in one too), so
//! the host's mark on the environment decides.
//!
//! Mirrors `atATerminal`, `agentHostMark`, `agentTerminalHost`,
//! `commandAtPersonsTerminal`, `personAtOwnTerminal` and
//! `readCurrentSessionId` in `aidlc-lib.ts` (L26338 onwards, L7261).

use std::io::IsTerminal;

use crate::names::SESSIONS_DIR;
use crate::records::valid_session_id;
use crate::{Layout, env};

/// The hosts that run their agent's commands in a terminal of their own,
/// by `TERM_PROGRAM`, named the way the person knows them.
const HOST_TERMINALS: [(&str, &str); 3] = [
    ("vscode", "VS Code"),
    ("cursor", "Cursor"),
    ("kiro", "Kiro"),
];

/// The marks a host leaves on the environment of the processes it starts.
const HOST_PREFIXES: [(&str, &str); 8] = [
    ("CLAUDECODE", "Claude Code"),
    ("CLAUDE_CODE_", "Claude Code"),
    ("CODEX_", "Codex"),
    ("CURSOR_", "Cursor"),
    ("KIRO_", "Kiro"),
    ("OPENCODE", "opencode"),
    ("COPILOT_", "Copilot"),
    ("VSCODE_", "VS Code"),
];

/// The step a person at their own terminal takes when the engine cannot see
/// a chat.
pub const OWN_TERMINAL_PRESENCE_STEP: &str = "Focus cannot see a chat in this terminal, so it cannot tell your own commands from an agent's. To drive Focus yourself from this terminal, run your Focus commands with FOCUS_SKIP_HUMAN_PRESENCE_GUARD=1 set; to work in a chat instead, open this folder in your AI tool and type your request there.";

/// Both ends of this command are a terminal (or a test says so).
#[must_use]
pub fn at_a_terminal() -> bool {
    (std::io::stdin().is_terminal() && std::io::stdout().is_terminal())
        || env::var("TEST_CONFIG_TTY").as_deref() == Some("1")
}

/// The host whose mark is on the environment, or none. A tool's settings
/// in a person's own shell (where it keeps its config) say nothing about
/// who is typing, and are passed over.
#[must_use]
pub fn agent_host_mark() -> Option<&'static str> {
    let term = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_lowercase();
    if let Some((_, host)) = HOST_TERMINALS.iter().find(|(name, _)| *name == term) {
        return Some(host);
    }
    for (key, _) in std::env::vars() {
        let upper = key.to_uppercase();
        if [
            "CODEX_HOME",
            "COPILOT_HOME",
            "OPENCODE_CONFIG_DIR",
            "OPENCODE_CONFIG",
        ]
        .contains(&upper.as_str())
        {
            continue;
        }
        if let Some((_, host)) = HOST_PREFIXES
            .iter()
            .find(|(prefix, _)| upper.starts_with(prefix))
        {
            return Some(host);
        }
    }
    None
}

/// The host to name when this command came from a terminal that host runs.
#[must_use]
pub fn agent_terminal_host() -> Option<&'static str> {
    if at_a_terminal() {
        agent_host_mark()
    } else {
        None
    }
}

/// A person's own shell: a terminal, no session handed down, no host's mark.
#[must_use]
pub fn command_at_persons_terminal() -> bool {
    at_a_terminal()
        && valid_session_id(env::var("SESSION_OVERRIDE").as_deref()).is_none()
        && valid_session_id(std::env::var("CODEX_THREAD_ID").ok().as_deref()).is_none()
        && agent_host_mark().is_none()
}

/// `readCurrentSessionId`: the most recently active session, if one was
/// recorded.
#[must_use]
pub fn current_session_id(layout: &Layout) -> Option<String> {
    let raw = std::fs::read(
        layout
            .workspace_root()
            .join(SESSIONS_DIR)
            .join(".current-session"),
    )
    .ok()?;
    let text = focus_js::text::trim(&String::from_utf8_lossy(&raw)).to_owned();
    (!text.is_empty()).then_some(text)
}

/// A person typing at their own terminal in a project where no chat has
/// ever been recorded.
#[must_use]
pub fn person_at_own_terminal(layout: &Layout) -> bool {
    command_at_persons_terminal() && current_session_id(layout).is_none()
}

/// `humanPresenceGuardDisabled`: the human-presence kill switch, from the
/// environment or the settings.
#[must_use]
pub fn human_presence_guard_disabled(layout: &Layout) -> bool {
    crate::settings::Places::of(&layout.project)
        .flag("FOCUS_SKIP_HUMAN_PRESENCE_GUARD", &|name| {
            std::env::var(name).ok()
        })
        .ok()
        .flatten()
        .as_deref()
        == Some("1")
}

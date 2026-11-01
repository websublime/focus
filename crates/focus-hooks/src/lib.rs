//! The bodies of the harness hooks: what the engine runs at each event of a
//! conversation (`focus engine hook <name>`), reading the host's payload on
//! stdin and answering with an exit code, stdout, stderr and audit rows.
//!
//! A hook first asks which workflow it fires in and whether this
//! conversation has joined it ([`context`]); one that only writes into the
//! record stands aside when it has not. Each hook leaves a heartbeat, and
//! records what it had to drop, under the record's hooks-health directory
//! ([`health`]).
//!
//! This crate holds the observers: hooks that record, keep the record in
//! step and recompile what derives from it, and decide nothing. The guards that refuse a tool call are M4's
//! later slices. Ported from `core/hooks/aidlc-*.ts`, and held to upstream's
//! behaviour by the differential.

pub mod context;
pub mod engine;
pub mod health;
pub mod hooks;
pub mod payload;
pub mod registry;
pub mod rules_held;
pub mod sessions;
pub mod status;
pub mod switches;
pub mod trace;

use focus_workspace::Layout;

/// The hooks Focus has, as `engine hook <name>` names them.
pub const HOOKS: [&str; 7] = [
    "session-start",
    "validate-state",
    "session-end",
    "log-subagent",
    "sync-workflow-state",
    "write-audit-log",
    "rebuild-stage-graph",
];

/// Upstream's other hooks, and what each does, for the refusal by name.
pub const NOT_PORTED: [(&str, &str); 9] = [
    (
        "record-human-turn",
        "records the person's turn and the words of a gate",
    ),
    (
        "guard-tool-call",
        "runs the PreToolUse guards in one process",
    ),
    (
        "plan-approval-guard",
        "holds code until its plan is approved",
    ),
    ("review-freeze", "holds a reviewed document still"),
    ("reviewer-scope", "keeps a reviewer to its scope"),
    (
        "state-transition-guard",
        "keeps the state file to the engine's steps",
    ),
    ("continue-workflow", "decides whether a turn may end"),
    ("deliver-stage-rules", "hands a subagent its stage's rules"),
    ("fold-usage", "folds token usage into the ledger"),
];

/// What a hook answered: the exit code, and what it printed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Outcome {
    fn exit(code: i32) -> Self {
        Self {
            code,
            ..Self::default()
        }
    }
}

/// Run the hook `name` on the host's payload `input`, for the project of
/// `layout`.
#[must_use]
pub fn run(name: &str, input: &str, layout: &Layout) -> Outcome {
    match name {
        "session-start" => hooks::session_start::run(input, layout),
        "validate-state" => hooks::validate_state::run(input, layout),
        "session-end" => hooks::session_end::run(input, layout),
        "log-subagent" => hooks::log_subagent::run(input, layout),
        "sync-workflow-state" => hooks::sync_workflow_state::run(input, layout),
        "write-audit-log" => hooks::write_audit_log::run(input, layout),
        "rebuild-stage-graph" => hooks::rebuild_stage_graph::run(input, layout),
        other => {
            let stderr = match NOT_PORTED.iter().find(|(known, _)| *known == other) {
                Some((_, does)) => format!(
                    "focus engine hook {other}: {does}, and Focus does not run that hook yet.\n"
                ),
                None => format!("focus engine hook {other}: not available in this install\n"),
            };
            Outcome {
                code: 1,
                stdout: String::new(),
                stderr,
            }
        }
    }
}

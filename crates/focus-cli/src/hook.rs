//! `focus engine hook <name>`: run one of the harness hooks on the payload
//! the host passes on stdin. The host registers these in its own settings
//! (`.claude/settings.json`), one command per event.
//!
//! Mirrors the `hook` route of `aidlc.ts` (`handleRouteOnly`, `runHook`).

use std::io::{Read, Write};

use focus_hooks::{HOOKS, NOT_PORTED, trace};
use focus_workspace::project;
use focus_workspace::{Layout, env};
use serde_json::json;

use crate::Failure;
use crate::args::Args;

const HELP: &str = "\
Usage: focus engine hook <name> [--project-dir <dir>] [--harness <name>]

Run a harness hook on the payload the host passes on stdin. The project is
FOCUS_PROJECT_DIR, else CLAUDE_PROJECT_DIR, else --project-dir, else the
nearest installed project at or above the current directory.
";

/// `isSafeName`: a hook name as the dispatcher accepts it.
fn is_safe_name(name: &str) -> bool {
    focus_js::re::test(r"^[a-z][a-z0-9-]*$", "", name)
}

pub fn run(args: &mut Args) -> Result<(), Failure> {
    if matches!(args.peek(), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    let project_dir = args.value("--project-dir").map_err(Failure::Message)?;
    let harness = args.value("--harness").map_err(Failure::Message)?;
    let Some(name) = args.next_positional() else {
        return Err(Failure::Message(
            "Usage: focus engine hook <name>. Reads the host's payload on stdin.".to_owned(),
        ));
    };
    if !is_safe_name(&name) {
        return Err(Failure::Message(format!(
            "focus engine hook: \"{name}\" is not a hook name."
        )));
    }
    args.finish().map_err(Failure::Message)?;
    if !HOOKS.contains(&name.as_str()) && focus_hooks::group::members(&name).is_none() {
        let message = match NOT_PORTED.iter().find(|(known, _)| *known == name) {
            Some((_, does)) => {
                format!("focus engine hook {name}: {does}, and Focus does not run that hook yet.")
            }
            None => format!("focus engine hook {name}: not available in this install"),
        };
        return Err(Failure::Message(message));
    }
    // The hook's own project: what the dispatcher or the host says, else
    // what the command said, else discovery.
    let project = match focus_hooks::engine::project_dir_from_env() {
        Some(dir) => project::settled(&std::env::current_dir().unwrap_or_default().join(dir)),
        None => project::resolve_project_dir(project_dir.as_deref().map(std::path::Path::new))
            .map_err(|err| Failure::Message(err.to_string()))?,
    };
    let harness = if harness.is_some() || env::var("HARNESS").is_some_and(|value| !value.is_empty())
    {
        project::resolve_harness(&project, harness.as_deref())
            .map_err(|err| Failure::Message(err.to_string()))?
    } else {
        project::installed_harnesses(&project)
            .first()
            .copied()
            .unwrap_or(focus_harness::Harness::Claude)
    };
    let layout = Layout::new(project, harness);
    let mut input = String::new();
    // A terminal on stdin has no payload coming: the hooks read nothing.
    let _ = std::io::stdin().read_to_string(&mut input);
    trace::trace("hook-run-begin", &[("hook", json!(name))]);
    let outcome = focus_hooks::run(&name, &input, &layout);
    trace::trace("hook-run-end", &[("code", json!(outcome.code))]);
    let _ = std::io::stdout().write_all(outcome.stdout.as_bytes());
    let _ = std::io::stderr().write_all(outcome.stderr.as_bytes());
    // `runHookModule`: Claude Code shows a blocking hook's stderr behind the
    // hook's own command, and a deny decision on stdout with only its
    // reason. So there, before a tool call, a refusal (exit 2 with its reason
    // on stderr) that answered nothing on stdout is also handed to the host
    // as its deny decision, built from that reason. Exit 2 blocks on its
    // own; a payload of another event, or one that names none, gets the
    // plain refusal.
    let before_a_tool_call = focus_js::json::parse(&input).is_ok_and(|payload| {
        payload
            .get("hook_event_name")
            .and_then(|event| event.as_str())
            == Some("PreToolUse")
    }) && layout.harness == focus_harness::Harness::Claude;
    let reason = focus_js::text::trim(&outcome.stderr);
    if before_a_tool_call && outcome.code == 2 && !reason.is_empty() && outcome.stdout.is_empty() {
        let decision = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        });
        let _ = writeln!(
            std::io::stdout(),
            "{}",
            focus_js::json::stringify(&decision)
        );
    }
    if outcome.code == 0 {
        Ok(())
    } else {
        Err(Failure::Code(outcome.code))
    }
}

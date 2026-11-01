//! `focus engine hook <name>`, run as a process on an installed project.
//!
//! What the hooks record is held to upstream's by the recorded differential,
//! which replays on Unix. These tests run everywhere: they cover the
//! command's surface, what each hook leaves behind, when a hook stands
//! aside, and each way Focus deliberately differs.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const RECORD: &str = "focus/spaces/default/intents/260105-add-login";
const SHARD: &str = "focus/spaces/default/intents/260105-add-login/audit/laptop-abc123.md";
const STATE: &str = "# Focus State Tracking\n\n## Project Information\n- **Scope**: feature\n- **Active Agent**: focus-product-agent\n\n## Execution Plan Summary\n- **Total Stages**: 3\n- **Completed**: 0\n- **In Progress**: intent-capture\n\n## Stage Progress\n\n### IDEATION PHASE\n- [-] intent-capture \u{2014} EXECUTE\n- [ ] market-research \u{2014} EXECUTE\n- [ ] feasibility \u{2014} EXECUTE\n\n## Current Status\n- **Lifecycle Phase**: IDEATION\n- **Current Stage**: intent-capture\n- **Next Stage**: market-research\n- **Status**: Running\n- **Last Updated**: 2026-01-05T10:00:00Z\n";
const REGISTRY: &str = "[\n  {\n    \"uuid\": \"01a138fa-d37d-787d-af4a-966efbb0b6fd\",\n    \"slug\": \"add-login\",\n    \"dirName\": \"260105-add-login\",\n    \"scope\": \"feature\",\n    \"status\": \"in-flight\"\n  }\n]\n";
const BINDING: &str = "{\"space\":\"default\",\"intent\":\"260105-add-login\",\"boundAt\":\"2026-01-05T10:00:00Z\",\"source\":\"create\"}";

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|err| panic!("{rel}: {err}"))
}

/// An installed Claude Code project with one created record: its state,
/// its registry row, the cursor, this clone's identity and an audit shard;
/// conversation `s1` is bound to it by its creation.
fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("focus-hook-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    copy_dir(&fixtures.join("installed/claude"), &root);
    write(&root, &format!("{RECORD}/focus-state.md"), STATE);
    write(&root, "focus/spaces/default/intents/intents.json", REGISTRY);
    write(
        &root,
        "focus/spaces/default/intents/active-intent",
        "260105-add-login\n",
    );
    write(&root, "focus/.focus-clone-id", "abc123\nlaptop\n");
    write(&root, "focus/.focus-sessions/s1.binding.json", BINDING);
    write(&root, SHARD, "# Focus Audit Log\n");
    root
}

fn hook(root: &Path, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    command
        .args(["engine", "hook"])
        .args(args)
        .args([
            "--project-dir",
            root.to_str().unwrap(),
            "--harness",
            "claude",
        ])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, _) in std::env::vars() {
        if name.starts_with("FOCUS_") || name == "CLAUDE_PROJECT_DIR" {
            command.env_remove(name);
        }
    }
    for (name, value) in env {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("focus runs");
    // A hook that refuses before reading leaves the pipe closed.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().expect("focus ends")
}

fn quiet(output: &Output) {
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    assert!(output.stderr.is_empty(), "{}", text(&output.stderr));
}

fn refusal(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    text(&output.stderr)
}

fn shard(root: &Path) -> String {
    read(root, SHARD)
}

fn heartbeat(root: &Path, name: &str) -> Option<String> {
    fs::read_to_string(
        root.join(RECORD)
            .join(".focus-engine/hooks-health")
            .join(name),
    )
    .ok()
}

fn payload(fields: &str) -> String {
    format!(
        "{{\"session_id\":\"s1\"{}}}",
        if fields.is_empty() {
            String::new()
        } else {
            format!(",{fields}")
        }
    )
}

#[test]
fn the_command_names_the_hooks_it_has() {
    let root = project("usage");
    let output = hook(&root, &["--help"], "", &[]);
    assert!(output.status.success());
    assert!(text(&output.stdout).starts_with("Usage: focus engine hook <name>"));
    assert_eq!(
        refusal(&hook(&root, &[], "{}", &[])),
        "Usage: focus engine hook <name>. Reads the host's payload on stdin.\n"
    );
    assert_eq!(
        refusal(&hook(&root, &["nope"], "{}", &[])),
        "focus engine hook nope: not available in this install\n"
    );
    assert_eq!(
        refusal(&hook(&root, &["validate-state", "extra"], "{}", &[])),
        "Unexpected argument: extra\n"
    );
    assert_eq!(shard(&root), "# Focus Audit Log\n");
}

/// Deliberate deviation. Upstream's dispatcher reaches the file system for
/// the hook named, and words the refusal of a name it cannot find its own
/// way (`unknown verb '../x' for engine noun 'hook'`, exit 2); Focus refuses
/// a name that is not a hook name before looking.
#[test]
fn a_name_that_is_not_a_hook_name_is_refused_before_looking() {
    let root = project("unsafe");
    for name in ["../x", "Validate-State", "x y", ""] {
        assert_eq!(
            refusal(&hook(&root, &[name], "{}", &[])),
            format!("focus engine hook: \"{name}\" is not a hook name.\n"),
            "{name}"
        );
    }
}

/// Deliberate deviation. Upstream runs its other hooks; Focus refuses each
/// by name, saying what it does, until its slice lands.
#[test]
fn a_hook_not_ported_is_refused_by_name() {
    let root = project("not-ported");
    assert_eq!(
        refusal(&hook(&root, &["record-human-turn"], &payload(""), &[])),
        "focus engine hook record-human-turn: records the person's turn and the words of a gate, and Focus does not run that hook yet.\n"
    );
    assert_eq!(
        refusal(&hook(&root, &["guard-tool-call"], &payload(""), &[])),
        "focus engine hook guard-tool-call: runs the PreToolUse guards in one process, and Focus does not run that hook yet.\n"
    );
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    assert!(heartbeat(&root, "record-human-turn.last").is_none());
}

#[test]
fn a_compaction_leaves_the_breadcrumb_the_heartbeat_and_a_row() {
    let root = project("compaction");
    quiet(&hook(
        &root,
        &["validate-state"],
        &payload("\"hook_event_name\":\"PreCompact\""),
        &[],
    ));
    let crumb = read(&root, &format!("{RECORD}/.focus-engine/recovery.md"));
    assert!(
        crumb.starts_with("# FOCUS Recovery Breadcrumb\n**Last validated**: 20"),
        "{crumb}"
    );
    assert!(crumb.ends_with("**Current stage**: intent-capture\n**State file**: valid (all required sections present)\n"), "{crumb}");
    assert!(heartbeat(&root, "validate-state.last").is_some());
    let rows = shard(&root);
    assert!(rows.contains("**Event**: SESSION_COMPACTED\n**Current Stage**: intent-capture\n**State Validity**: valid\n"), "{rows}");

    // A state file missing a section is warned about, and still recorded.
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &STATE.replace("## Current Status", "## Status"),
    );
    let output = hook(&root, &["validate-state"], &payload(""), &[]);
    assert!(output.status.success());
    assert_eq!(
        text(&output.stderr),
        "WARNING: focus-state.md missing sections: Current Status\n"
    );
    assert!(shard(&root).contains("**State Validity**: invalid\n"));
    assert!(
        read(&root, &format!("{RECORD}/.focus-engine/recovery.md"))
            .contains("INVALID \u{2014} missing sections: Current Status")
    );
}

#[test]
fn a_hook_without_a_payload_or_a_workflow_does_nothing() {
    let root = project("nothing");
    for input in ["", "{nope", "[1]", "null"] {
        quiet(&hook(&root, &["validate-state"], input, &[]));
        quiet(&hook(&root, &["session-end"], input, &[]));
        quiet(&hook(&root, &["log-subagent"], input, &[]));
        quiet(&hook(&root, &["sync-workflow-state"], input, &[]));
        quiet(&hook(&root, &["write-audit-log"], input, &[]));
    }
    // A compaction and a session's end need no payload: the cursor alone
    // makes the conversation a participant, so each of those recorded one
    // (an end with no reason as `unknown`). The others record nothing
    // without the payload's fields.
    quiet(&hook(&root, &["validate-state"], "{}", &[]));
    assert_eq!(shard(&root).matches("SESSION_COMPACTED").count(), 5);
    assert_eq!(
        shard(&root)
            .matches("SESSION_ENDED\n**Reason**: unknown\n")
            .count(),
        4
    );
    assert_eq!(shard(&root).matches("**Event**").count(), 9);
    // Without a state file there is no record to resolve: neither the
    // binding nor the cursor names one, so nothing is recorded, and the
    // heartbeat goes under the space.
    fs::remove_file(root.join(RECORD).join("focus-state.md")).unwrap();
    quiet(&hook(&root, &["validate-state"], &payload(""), &[]));
    quiet(&hook(
        &root,
        &["log-subagent"],
        &payload("\"agent_type\":\"x\""),
        &[],
    ));
    assert_eq!(shard(&root).matches("**Event**").count(), 9);
    assert!(
        root.join("focus/spaces/default/intents/.focus-engine/hooks-health/validate-state.last")
            .exists()
    );
}

#[test]
fn a_conversation_that_has_not_joined_the_record_stands_aside() {
    let root = project("outsider");
    fs::remove_file(root.join("focus/spaces/default/intents/active-intent")).unwrap();
    // Bound to another record, bound as staying out, or bound by a source
    // that is not the conversation's own choice.
    for binding in [
        BINDING.replace("260105-add-login", "other-9f8e7d6c"),
        "{\"space\":\"default\",\"intent\":null,\"boundAt\":\"2026-01-05T10:00:00Z\",\"source\":\"unjoined\"}".to_owned(),
        BINDING.replace("\"create\"", "\"worktree\""),
    ] {
        write(&root, "focus/.focus-sessions/s1.binding.json", &binding);
        quiet(&hook(&root, &["validate-state"], &payload(""), &[]));
        quiet(&hook(&root, &["session-end"], &payload("\"reason\":\"exit\""), &[]));
        quiet(&hook(&root, &["log-subagent"], &payload("\"agent_type\":\"x\""), &[]));
        quiet(&hook(&root, &["sync-workflow-state"], &payload("\"tool_name\":\"TaskUpdate\",\"tool_input\":{\"status\":\"in_progress\",\"activeForm\":\"x [market-research]\"}"), &[]));
        assert_eq!(shard(&root), "# Focus Audit Log\n", "{binding}");
        assert!(!read(&root, &format!("{RECORD}/focus-state.md")).contains("market-research\n- **Next"), "{binding}");
        // Standing aside leaves no heartbeat anywhere: not in the record, not
        // under the space.
        assert!(!root.join(RECORD).join(".focus-engine").exists(), "{binding}");
        assert!(!root.join("focus/spaces/default/intents/.focus-engine").exists(), "{binding}");
    }
    // A binding of a source that is not the conversation's own choice
    // names the record, and the cursor naming another is no evidence for
    // it: the hook acts on neither.
    write(
        &root,
        "focus/.focus-sessions/s1.binding.json",
        &BINDING.replace("\"create\"", "\"worktree\""),
    );
    write(
        &root,
        "focus/spaces/default/intents/active-intent",
        "other-9f8e7d6c\n",
    );
    write(
        &root,
        "focus/spaces/default/intents/other-9f8e7d6c/focus-state.md",
        STATE,
    );
    quiet(&hook(&root, &["validate-state"], &payload(""), &[]));
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    assert!(!root.join(RECORD).join(".focus-engine").exists());
    assert!(
        !root
            .join("focus/spaces/default/intents/other-9f8e7d6c/.focus-engine")
            .exists()
    );
}

#[test]
fn a_session_ends_where_it_is_bound_or_stamped() {
    let root = project("end");
    quiet(&hook(
        &root,
        &["session-end"],
        &payload("\"reason\":\"exit\""),
        &[],
    ));
    quiet(&hook(
        &root,
        &["session-end"],
        &payload("\"reason\":7"),
        &[],
    ));
    quiet(&hook(&root, &["session-end"], &payload(""), &[]));
    let rows = shard(&root);
    assert!(
        rows.contains("**Event**: SESSION_ENDED\n**Reason**: exit\n"),
        "{rows}"
    );
    assert!(rows.contains("**Reason**: 7\n"), "{rows}");
    assert!(rows.contains("**Reason**: unknown\n"), "{rows}");
    assert!(heartbeat(&root, "session-end.last").is_some());

    // Without a binding, the stamp names the record; a stamp to an unknown
    // record is dropped, and a UUID-backed workflow with neither fails
    // closed, so nothing is recorded for a session it cannot place.
    fs::remove_file(root.join("focus/.focus-sessions/s1.binding.json")).unwrap();
    write(
        &root,
        "focus/.focus-sessions/s1",
        "01a138fa-d37d-787d-af4a-966efbb0b6fd\n",
    );
    quiet(&hook(
        &root,
        &["session-end"],
        &payload("\"reason\":\"stamped\""),
        &[],
    ));
    assert!(shard(&root).contains("**Reason**: stamped\n"));
    write(
        &root,
        "focus/.focus-sessions/s1",
        "01a138fa-0000-787d-af4a-966efbb0b6fd\n",
    );
    quiet(&hook(
        &root,
        &["session-end"],
        &payload("\"reason\":\"unknown-stamp\""),
        &[],
    ));
    assert!(!shard(&root).contains("unknown-stamp"));
    let drops = read(
        &root,
        &format!("{RECORD}/.focus-engine/hooks-health/session-end.drops"),
    );
    assert!(drops.ends_with("\tsession s1 is stamped to unknown intent 01a138fa-0000-787d-af4a-966efbb0b6fd; refusing active-cursor fallback\n"), "{drops}");
    fs::remove_file(root.join("focus/.focus-sessions/s1")).unwrap();
    quiet(&hook(
        &root,
        &["session-end"],
        &payload("\"reason\":\"unplaced\""),
        &[],
    ));
    assert!(!shard(&root).contains("unplaced"));
}

#[test]
fn a_finished_subagent_is_recorded_while_the_workflow_runs() {
    let root = project("subagent");
    quiet(&hook(
        &root,
        &["log-subagent"],
        &payload(
            "\"agent_type\":\"focus-product-agent\",\"agent_id\":\"a1\",\"last_assistant_message\":\"Done.\\nAll good.\"",
        ),
        &[],
    ));
    quiet(&hook(
        &root,
        &["log-subagent"],
        &payload("\"agent_type\":\"  \",\"agent_id\":0,\"last_assistant_message\":[]"),
        &[],
    ));
    let rows = shard(&root);
    assert!(rows.contains("**Event**: SUBAGENT_COMPLETED\n**Agent Type**: focus-product-agent\n**Agent ID**: a1\n**Message**: Done.\\nAll good.\n"), "{rows}");
    assert!(
        rows.contains("**Agent Type**: unknown\n**Message**: \n"),
        "{rows}"
    );
    assert!(!rows.contains("**Agent ID**: 0"), "{rows}");
    // The message is cut at 200 UTF-16 units.
    let long = "\u{e9}".repeat(199) + "\u{1f600}x";
    quiet(&hook(
        &root,
        &["log-subagent"],
        &payload(&format!(
            "\"agent_type\":\"x\",\"last_assistant_message\":\"{long}\""
        )),
        &[],
    ));
    assert!(shard(&root).contains(&format!("**Message**: {}\u{fffd}\n", "\u{e9}".repeat(199))));
    // Not while the workflow is parked.
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &STATE.replace("- **Status**: Running", "- **Status**: Completed"),
    );
    quiet(&hook(
        &root,
        &["log-subagent"],
        &payload("\"agent_type\":\"late\""),
        &[],
    ));
    assert!(!shard(&root).contains("late"));
}

#[test]
fn the_task_list_moves_the_stage_forward() {
    let root = project("sync");
    let task = |status: &str, form: &str| {
        payload(&format!(
            "\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"TaskUpdate\",\"tool_input\":{{\"status\":\"{status}\",\"activeForm\":\"{form}\"}}"
        ))
    };
    for input in [
        task("completed", "x [market-research]"),
        task("pending", "x [market-research]"),
        task("in_progress", "Researching"),
        task("in_progress", "x [Market-Research]"),
        task("in_progress", "x [not-a-stage]"),
        task("in_progress", "x [intent-capture]"),
    ] {
        quiet(&hook(&root, &["sync-workflow-state"], &input, &[]));
        assert!(
            read(&root, &format!("{RECORD}/focus-state.md"))
                .contains("- **Current Stage**: intent-capture\n"),
            "{input}"
        );
    }
    // The tool's name is not read: the task's fields are.
    quiet(&hook(
        &root,
        &["sync-workflow-state"],
        &payload(
            "\"tool_name\":\"Write\",\"tool_input\":{\"status\":\"in_progress\",\"activeForm\":\"Researching [market-research]\"}",
        ),
        &[],
    ));
    let state = read(&root, &format!("{RECORD}/focus-state.md"));
    assert!(
        state.contains("- **Current Stage**: market-research\n"),
        "{state}"
    );
    assert!(
        state.contains("- **In Progress**: market-research\n"),
        "{state}"
    );
    assert!(
        state.contains("- [-] market-research \u{2014} EXECUTE\n"),
        "{state}"
    );
    assert!(
        state.contains("- [-] intent-capture \u{2014} EXECUTE\n"),
        "{state}"
    );
    assert!(heartbeat(&root, "sync-workflow-state.last").is_some());
    assert_eq!(shard(&root), "# Focus Audit Log\n");
}

#[test]
fn an_ide_sync_mirrors_the_audit_trail_forward_only() {
    let root = project("ide");
    let ide = payload(
        "\"hook_event_name\":\"PostToolUse\",\"tool_input\":{\"source\":\"ide-audit-sync\"}",
    );
    let started = |stage: &str| {
        format!(
            "\n## Stage Start\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: STAGE_STARTED\n**Stage**: {stage}\n\n---\n"
        )
    };
    // In step: a heartbeat says so and the state stays.
    write(
        &root,
        SHARD,
        &format!("# Focus Audit Log\n{}", started("intent-capture")),
    );
    quiet(&hook(&root, &["sync-workflow-state"], &ide, &[]));
    assert!(heartbeat(&root, "sync-workflow-state.noop").is_some());
    assert!(heartbeat(&root, "sync-workflow-state.last").is_none());
    // Ahead in the trail: the state follows.
    write(
        &root,
        SHARD,
        &format!(
            "# Focus Audit Log\n{}{}",
            started("intent-capture"),
            started("market-research")
        ),
    );
    quiet(&hook(&root, &["sync-workflow-state"], &ide, &[]));
    assert!(
        read(&root, &format!("{RECORD}/focus-state.md"))
            .contains("- **Current Stage**: market-research\n")
    );
    assert!(heartbeat(&root, "sync-workflow-state.last").is_some());
    // Behind in the trail, at a stage the workflow has finished: the state
    // stays where it is. At one still open, the state follows the trail.
    let state = read(&root, &format!("{RECORD}/focus-state.md"));
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &state.replace("- [-] intent-capture", "- [x] intent-capture"),
    );
    write(
        &root,
        SHARD,
        &format!("# Focus Audit Log\n{}", started("intent-capture")),
    );
    quiet(&hook(&root, &["sync-workflow-state"], &ide, &[]));
    assert!(
        read(&root, &format!("{RECORD}/focus-state.md"))
            .contains("- **Current Stage**: market-research\n")
    );
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &state.replace("- [-] intent-capture", "- [S] intent-capture"),
    );
    quiet(&hook(&root, &["sync-workflow-state"], &ide, &[]));
    assert!(
        read(&root, &format!("{RECORD}/focus-state.md"))
            .contains("- **Current Stage**: market-research\n")
    );
    // A workflow that is not running is ahead of its trail by design.
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &state.replace("- **Status**: Running", "- **Status**: Parked"),
    );
    quiet(&hook(&root, &["sync-workflow-state"], &ide, &[]));
    assert!(
        read(&root, &format!("{RECORD}/focus-state.md"))
            .contains("- **Current Stage**: market-research\n")
    );
    write(&root, &format!("{RECORD}/focus-state.md"), &state);
    quiet(&hook(&root, &["sync-workflow-state"], &ide, &[]));
    assert!(
        read(&root, &format!("{RECORD}/focus-state.md"))
            .contains("- **Current Stage**: intent-capture\n")
    );
}

#[test]
fn a_write_into_the_record_is_recorded_as_its_artifact() {
    let root = project("write");
    let doc = format!("{RECORD}/ideation/intent-capture/intent-statement.md");
    write(&root, &doc, "# Intent\n");
    let wrote = |tool: &str, rel: &str| {
        let path = root.join(rel).to_str().unwrap().replace('\\', "\\\\");
        payload(&format!(
            "\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"{tool}\",\"tool_input\":{{\"file_path\":\"{path}\"}}"
        ))
    };
    quiet(&hook(
        &root,
        &["write-audit-log"],
        &wrote("Edit", &doc),
        &[],
    ));
    quiet(&hook(
        &root,
        &["write-audit-log"],
        &wrote("Write", &format!("{RECORD}/missing.md")),
        &[],
    ));
    quiet(&hook(
        &root,
        &["write-audit-log"],
        &wrote("Write", "src/main.ts"),
        &[],
    ));
    quiet(&hook(
        &root,
        &["write-audit-log"],
        &wrote("Write", SHARD),
        &[],
    ));
    quiet(&hook(
        &root,
        &["write-audit-log"],
        &wrote("NotebookEdit", &doc),
        &[],
    ));
    let rows = shard(&root);
    // An Edit is an update; a Write of a file that is new, or that is not
    // there to tell, is a creation; any other tool is read the same way.
    assert_eq!(
        rows.matches("**Event**: ARTIFACT_UPDATED\n**Tool**: Edit\n")
            .count(),
        1,
        "{rows}"
    );
    assert_eq!(
        rows.matches("**Event**: ARTIFACT_CREATED\n").count(),
        2,
        "{rows}"
    );
    assert!(rows.contains("**Tool**: NotebookEdit\n"), "{rows}");
    assert!(
        rows.contains("**Context**: ideation > intent-capture > intent-statement.md\n"),
        "{rows}"
    );
    assert!(rows.contains("**Context**: missing.md\n"), "{rows}");
    // A write outside the record, or into the audit trail itself, is none.
    assert!(!rows.contains("main.ts"), "{rows}");
    assert!(!rows.contains("laptop-abc123"), "{rows}");
    assert!(heartbeat(&root, "write-audit-log.last").is_some());
    // A codekb write carries its own context.
    let kb = "focus/spaces/default/codekb/repo/summary.md";
    write(&root, kb, "x");
    quiet(&hook(&root, &["write-audit-log"], &wrote("Edit", kb), &[]));
    assert!(shard(&root).contains("**Context**: codekb > repo > summary.md\n"));
    // A record whose name merely begins like this one's is another record.
    let sibling = format!("{RECORD}-bis/notes.md");
    write(&root, &sibling, "x");
    quiet(&hook(
        &root,
        &["write-audit-log"],
        &wrote("Edit", &sibling),
        &[],
    ));
    assert!(!shard(&root).contains("notes.md"));
    // A Write of a file that was there before, changed since it was born,
    // is an update.
    let old = format!("{RECORD}/ideation/intent-capture/old.md");
    write(&root, &old, "one");
    std::thread::sleep(std::time::Duration::from_millis(50));
    write(&root, &old, "two");
    quiet(&hook(
        &root,
        &["write-audit-log"],
        &wrote("Write", &old),
        &[],
    ));
    let rows = shard(&root);
    assert!(
        rows.contains("**Event**: ARTIFACT_UPDATED\n**Tool**: Write\n"),
        "{rows}"
    );
}

/// Deliberate deviation. A payload field of a kind the hook does not expect:
/// upstream falls over on it and its dispatcher prints the runtime's
/// TypeError; Focus refuses the field by name. Exit 1 either way, and nothing
/// recorded by either. A falsy value of the wrong kind is empty for both.
#[test]
fn a_payload_field_of_the_wrong_kind_is_refused_by_name() {
    let root = project("kinds");
    assert_eq!(
        refusal(&hook(
            &root,
            &["log-subagent"],
            &payload("\"agent_type\":\"x\",\"last_assistant_message\":{\"a\":1}"),
            &[]
        )),
        "log-subagent: the payload's last_assistant_message is not text ({\"a\":1}).\n"
    );
    assert_eq!(
        refusal(&hook(
            &root,
            &["sync-workflow-state"],
            &payload(
                "\"tool_name\":\"TaskUpdate\",\"tool_input\":{\"status\":\"in_progress\",\"activeForm\":5}"
            ),
            &[]
        )),
        "sync-workflow-state: the payload's activeForm is not text (5).\n"
    );
    assert_eq!(
        refusal(&hook(
            &root,
            &["write-audit-log"],
            &payload("\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":true}"),
            &[]
        )),
        "write-audit-log: the payload's file_path is not text (true).\n"
    );
    quiet(&hook(
        &root,
        &["sync-workflow-state"],
        &payload(
            "\"tool_name\":\"TaskUpdate\",\"tool_input\":{\"status\":\"in_progress\",\"activeForm\":0}",
        ),
        &[],
    ));
    quiet(&hook(
        &root,
        &["write-audit-log"],
        &payload("\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":false}"),
        &[],
    ));
    assert_eq!(shard(&root), "# Focus Audit Log\n");
}

/// Deliberate deviation. A record of a later milestone beside the hook's
/// own (the rules handed to a conversation, the orchestrator's directive
/// marker, a unit claim, the ledger of background subagents, the summary
/// authorizations): upstream reads or clears it and goes on; Focus refuses
/// the hook by name while it is there, and records nothing.
#[test]
fn a_record_of_a_later_milestone_refuses_the_hook_by_name() {
    let root = project("later");
    let engine = format!("{RECORD}/.focus-engine");

    write(
        &root,
        &format!("{engine}/active-directive.json"),
        "{\"stage\":\"intent-capture\"}",
    );
    assert!(
        refusal(&hook(&root, &["validate-state"], &payload(""), &[]))
            .contains("active-directive.json is the orchestrator's directive marker")
    );
    assert!(refusal(&hook(&root, &["sync-workflow-state"], &payload("\"tool_name\":\"TaskUpdate\",\"tool_input\":{\"status\":\"in_progress\",\"activeForm\":\"x [market-research]\"}"), &[]))
        .contains("active-directive.json is the orchestrator's directive marker"));
    fs::remove_file(root.join(&engine).join("active-directive.json")).unwrap();

    write(&root, "focus/.focus-unit-scope.json", "{\"version\":1}");
    assert!(
        refusal(&hook(&root, &["validate-state"], &payload(""), &[]))
            .contains("focus-unit-scope.json")
    );
    assert!(
        refusal(&hook(
            &root,
            &["session-end"],
            &payload("\"reason\":\"exit\""),
            &[]
        ))
        .contains("focus-unit-scope.json")
    );
    fs::remove_file(root.join("focus/.focus-unit-scope.json")).unwrap();

    write(
        &root,
        "focus/.focus-subagent-inflight",
        "{\"version\":1,\"entries\":[]}",
    );
    assert!(
        refusal(&hook(
            &root,
            &["log-subagent"],
            &payload("\"agent_type\":\"x\""),
            &[]
        ))
        .contains("focus-subagent-inflight is the ledger of background subagents in flight")
    );
    assert!(root.join("focus/.focus-subagent-inflight").exists());
    fs::remove_file(root.join("focus/.focus-subagent-inflight")).unwrap();

    let doc = format!("{RECORD}/ideation/intent-capture/intent-statement.md");
    write(&root, &doc, "# Intent\n");
    write(
        &root,
        &format!("{engine}/summary-authorization/x.json"),
        "{}",
    );
    let path = root.join(&doc).to_str().unwrap().replace('\\', "\\\\");
    assert!(
        refusal(&hook(
            &root,
            &["write-audit-log"],
            &payload(&format!(
                "\"tool_name\":\"Edit\",\"tool_input\":{{\"file_path\":\"{path}\"}}"
            )),
            &[]
        ))
        .contains("summary-authorization holds summary authorizations")
    );
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    assert!(
        !read(&root, &format!("{RECORD}/focus-state.md"))
            .contains("Current Stage**: market-research")
    );
}

/// A process map that names no ancestor of the hook is a miss: the cursor
/// decides, as upstream's walk decides when it finds nothing. A map naming
/// the hook's parent (what the session-start hook records for the harness)
/// names the conversation, binding and all.
#[test]
fn the_process_map_names_the_conversation_a_hook_runs_in() {
    let root = project("ancestry");
    write(&root, "focus/.focus-sessions/pids/1.json", "{}\n");
    quiet(&hook(&root, &["validate-state"], "{}", &[]));
    assert_eq!(shard(&root).matches("SESSION_COMPACTED").count(), 1);
    // The cursor elsewhere, and this process (the hook's parent) recorded as
    // the conversation bound to the record: the hook works on the record.
    fs::remove_file(root.join("focus/spaces/default/intents/active-intent")).unwrap();
    quiet(&hook(&root, &["validate-state"], "{}", &[]));
    assert_eq!(shard(&root).matches("SESSION_COMPACTED").count(), 1);
    let layout = focus_workspace::Layout::new(&root, focus_harness::Harness::Claude);
    focus_workspace::ancestry::write_entry(
        &layout,
        std::process::id(),
        "s1",
        std::time::Instant::now() + std::time::Duration::from_secs(10),
        None,
    );
    quiet(&hook(&root, &["validate-state"], "{}", &[]));
    assert_eq!(shard(&root).matches("SESSION_COMPACTED").count(), 2);
    quiet(&hook(
        &root,
        &["log-subagent"],
        "{\"agent_type\":\"mapped\"}",
        &[],
    ));
    assert!(shard(&root).contains("**Agent Type**: mapped\n"));
}

/// Deliberate deviation. Upstream resolves a delegated worktree to the main
/// checkout's intent; Focus refuses it by name. A hook stands outside on any
/// other failure of the resolution, as upstream's does.
#[test]
fn a_resolution_not_ported_refuses_the_hook_by_name() {
    let root = project("resolution");
    write(&root, ".focus/worktree-meta.json", "{}\n");
    assert!(
        refusal(&hook(
            &root,
            &["session-end"],
            &payload("\"reason\":\"worktree\""),
            &[]
        ))
        .contains("worktree-meta.json marks this checkout as a worktree")
    );
    assert!(!shard(&root).contains("**Reason**: worktree"));
}

fn ran(command: &str, out: &str) -> String {
    payload(&format!(
        "\"hook_event_name\":\"PostToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{{\"command\":{}}},\"tool_response\":{{\"stdout\":{},\"stderr\":\"\",\"interrupted\":false}}",
        serde_json::to_string(command).unwrap(),
        serde_json::to_string(out).unwrap()
    ))
}

#[test]
fn a_transition_tool_call_recompiles_the_runtime_graph() {
    let root = project("rebuild");
    let graph = root.join(RECORD).join("runtime-graph.json");
    // A read-only or chatty call, and the runtime tool itself, compile nothing.
    for command in [
        "echo hi",
        "git status",
        "focus intent create --scope feature",
        "bun .claude/tools/focus-log.ts decision x",
        "focus engine runtime compile",
        "bun .claude/tools/focus-runtime.ts compile",
        "focus state approve x && focus engine runtime compile",
    ] {
        quiet(&hook(
            &root,
            &["rebuild-stage-graph"],
            &ran(command, ""),
            &[],
        ));
        assert!(!graph.exists(), "{command}");
    }
    assert!(heartbeat(&root, "rebuild-stage-graph.last").is_none());
    // A transition tool, in any spelling, with a transition row in the tail.
    write(
        &root,
        SHARD,
        "# Focus Audit Log\n\n## Workflow Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: WORKFLOW_STARTED\n**Scope**: feature\n\n---\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: intent-capture\n**Agent**: focus-product-agent\n\n---\n",
    );
    for command in [
        "bun .claude/tools/focus-state.ts approve intent-capture",
        "focus engine state approve intent-capture",
        "focus next --resume && focus report --result done",
        "bun \".claude/tools/focus.ts\" engine jump execute --target x",
        "echo 'focus state approve'",
    ] {
        let _ = fs::remove_file(&graph);
        quiet(&hook(
            &root,
            &["rebuild-stage-graph"],
            &ran(command, ""),
            &[],
        ));
        assert!(graph.exists(), "{command}");
        assert!(
            read(&root, &format!("{RECORD}/runtime-graph.json")).contains("intent-capture"),
            "{command}"
        );
    }
    assert!(heartbeat(&root, "rebuild-stage-graph.last").is_some());
    // No transition among the last three blocks: nothing compiled.
    let chatty =
        "\n## X\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: DECISION_RECORDED\n\n---\n";
    write(
        &root,
        SHARD,
        &format!("{}{chatty}{chatty}{chatty}", read(&root, SHARD)),
    );
    fs::remove_file(&graph).unwrap();
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran("focus state approve x", ""),
        &[],
    ));
    assert!(!graph.exists());
    // One chatty row after it leaves the transition among the last three
    // blocks (the trailing separator closes an empty one); two push it out.
    write(
        &root,
        SHARD,
        "# Focus Audit Log\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: intent-capture\n\n---\n",
    );
    write(&root, SHARD, &format!("{}{chatty}", read(&root, SHARD)));
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran("focus state approve x", ""),
        &[],
    ));
    assert!(graph.exists());
    fs::remove_file(&graph).unwrap();
    write(&root, SHARD, &format!("{}{chatty}", read(&root, SHARD)));
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran("focus state approve x", ""),
        &[],
    ));
    assert!(!graph.exists());
    // An outsider recompiles nothing.
    write(
        &root,
        SHARD,
        "# Focus Audit Log\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: intent-capture\n\n---\n",
    );
    write(
        &root,
        "focus/.focus-sessions/s1.binding.json",
        &BINDING.replace("\"create\"", "\"worktree\""),
    );
    fs::remove_file(root.join("focus/spaces/default/intents/active-intent")).unwrap();
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran("focus state approve x", ""),
        &[],
    ));
    assert!(!graph.exists());
}

#[test]
fn the_ide_mirror_recompiles_once_per_change() {
    let root = project("rebuild-ide");
    let graph = root.join(RECORD).join("runtime-graph.json");
    let ide = payload(
        "\"hook_event_name\":\"PostToolUse\",\"tool_input\":{\"source\":\"ide-audit-sync\"}",
    );
    write(
        &root,
        SHARD,
        "# Focus Audit Log\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: intent-capture\n\n---\n",
    );
    quiet(&hook(&root, &["rebuild-stage-graph"], &ide, &[]));
    assert!(graph.exists());
    assert!(heartbeat(&root, "rebuild-stage-graph.noop").is_none());
    // The tail has not changed since: the graph is at least as new as the
    // shards, so the next call is a no-op the front gate can read.
    let compiled = fs::read_to_string(&graph).unwrap();
    quiet(&hook(&root, &["rebuild-stage-graph"], &ide, &[]));
    assert!(heartbeat(&root, "rebuild-stage-graph.noop").is_some());
    assert_eq!(fs::read_to_string(&graph).unwrap(), compiled);
    // Without a transition in the tail, the no-op mark too.
    fs::remove_file(
        root.join(RECORD)
            .join(".focus-engine/hooks-health/rebuild-stage-graph.noop"),
    )
    .unwrap();
    write(
        &root,
        SHARD,
        "# Focus Audit Log\n\n## X\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: DECISION_RECORDED\n\n---\n",
    );
    quiet(&hook(&root, &["rebuild-stage-graph"], &ide, &[]));
    assert!(heartbeat(&root, "rebuild-stage-graph.noop").is_some());
}

#[test]
fn a_creation_seen_in_the_call_binds_the_session_that_made_it() {
    let root = project("rebuild-bind");
    fs::remove_file(root.join("focus/.focus-sessions/s1.binding.json")).unwrap();
    let made = "Intent created: 260105-add-login (space: default)\nState initialized: feature scope, 32 stages\n";
    let create = "bun .claude/tools/focus-utility.ts intent-create --scope feature";
    let binding = || fs::read_to_string(root.join("focus/.focus-sessions/s1.binding.json")).ok();
    let stamp = || fs::read_to_string(root.join("focus/.focus-sessions/s1")).ok();
    // Seen without the creation receipt: bound as observed, and not stamped.
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran(create, made),
        &[],
    ));
    assert!(
        binding()
            .unwrap()
            .contains("\"source\":\"observed-create\""),
        "{:?}",
        binding()
    );
    assert!(stamp().is_none());
    // With the receipt this machine's intent-create left: proven, stamped,
    // and the receipt spent.
    fs::remove_file(root.join("focus/.focus-sessions/s1.binding.json")).unwrap();
    let receipt = root.join(RECORD).join(".focus-engine/creation-receipt");
    write(
        &root,
        &format!("{RECORD}/.focus-engine/creation-receipt"),
        "01a138fa-d37d-787d-af4a-966efbb0b6fd\n",
    );
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran(create, made),
        &[],
    ));
    assert!(
        binding().unwrap().contains("\"source\":\"create\""),
        "{:?}",
        binding()
    );
    assert_eq!(
        stamp().as_deref(),
        Some("01a138fa-d37d-787d-af4a-966efbb0b6fd\n")
    );
    assert!(!receipt.exists());
    // A second creation in the same session hands the attribution over.
    write(
        &root,
        "focus/.focus-sessions/s1",
        "01a138fa-ffff-787d-af4a-966efbb0b6fd\n",
    );
    write(
        &root,
        &format!("{RECORD}/.focus-engine/creation-receipt"),
        "01a138fa-d37d-787d-af4a-966efbb0b6fd\n",
    );
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran("focus intent create --scope feature", made),
        &[],
    ));
    let handoff = read(&root, "focus/.focus-sessions/s1.handoff.json");
    assert!(handoff.starts_with("{\"fromIntentUuid\":\"01a138fa-ffff-787d-af4a-966efbb0b6fd\",\"toIntentUuid\":\"01a138fa-d37d-787d-af4a-966efbb0b6fd\",\"issuedAtMs\":"), "{handoff}");
    assert_eq!(
        stamp().as_deref(),
        Some("01a138fa-d37d-787d-af4a-966efbb0b6fd\n")
    );
    // Another command, another record, or no session: nothing bound.
    fs::remove_file(root.join("focus/.focus-sessions/s1.binding.json")).unwrap();
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran("focus status", made),
        &[],
    ));
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran(create, "Intent created: 260106-other (space: default)\n"),
        &[],
    ));
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &format!(
            "{{\"tool_name\":\"Bash\",\"tool_input\":{{\"command\":\"{create}\"}},\"tool_response\":\"{}\"}}",
            made.replace('\n', "\\n")
        ),
        &[],
    ));
    assert!(binding().is_none());
}

/// Deliberate deviation. A literal call of the engine's orchestrator whose
/// output names an error directive: upstream validates the directive and
/// relays its message to the person through the harness's own channel;
/// Focus refuses the hook by name, before validating, since the directive is
/// the orchestrator's (M5).
#[test]
fn an_error_directive_in_the_calls_output_is_refused_by_name() {
    let root = project("rebuild-relay");
    let directive = "{\"kind\":\"error\",\"message\":\"No workflow is active.\"}\n";
    for command in [
        "focus next",
        "cd /p && bun .claude/tools/focus.ts engine orchestrate report --result done 2>&1",
        "focus park",
    ] {
        assert!(
            refusal(&hook(
                &root,
                &["rebuild-stage-graph"],
                &ran(command, directive),
                &[]
            ))
            .contains("names an error directive"),
            "{command}"
        );
    }
    // Not a literal call of the orchestrator, or not an error: nothing to relay.
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran("focus next | tee log", directive),
        &[],
    ));
    quiet(&hook(
        &root,
        &["rebuild-stage-graph"],
        &ran("focus next", "{\"kind\":\"print\",\"message\":\"x\"}\n"),
        &[],
    ));
    assert_eq!(
        refusal(&hook(
            &root,
            &["rebuild-stage-graph"],
            &payload("\"tool_name\":\"Bash\",\"tool_input\":{\"command\":5}"),
            &[]
        )),
        "rebuild-stage-graph: the payload's command is not text (5).\n"
    );
}

#[test]
fn the_project_comes_from_the_host_before_the_flag() {
    let root = project("host");
    let other = project("host-other");
    let env = [("CLAUDE_PROJECT_DIR", other.to_str().unwrap())];
    quiet(&hook(&root, &["validate-state"], &payload(""), &env));
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    assert!(shard(&other).contains("SESSION_COMPACTED"));
    let trace_dir = root.join("trace");
    let env = [("FOCUS_HOOK_TRACE_DIR", trace_dir.to_str().unwrap())];
    quiet(&hook(&root, &["validate-state"], &payload(""), &env));
    let traced: Vec<_> = fs::read_dir(&trace_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(traced.len(), 1, "{traced:?}");
    let lines = fs::read_to_string(&traced[0]).unwrap();
    assert!(lines.contains("\"phase\":\"hook-run-begin\""), "{lines}");
    assert!(lines.contains("\"phase\":\"hook-run-end\""), "{lines}");
}

/// The context the session start hands the model, parsed.
fn handed(output: &Output) -> String {
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(output.stderr.is_empty(), "{}", text(&output.stderr));
    let line = text(&output.stdout);
    let (head, tail) = line
        .split_once("\",\"hookSpecificOutput\":{\"hookEventName\":\"SessionStart\",\"additionalContext\":\"")
        .unwrap_or_else(|| panic!("a context line: {line}"));
    let context = head.strip_prefix("{\"additionalContext\":\"").unwrap();
    // The same text twice: once for the host's banner, once for the model.
    assert!(tail.starts_with(context), "{line}");
    context.replace("\\n", "\n").replace("\\\"", "\"")
}

fn start(source: &str) -> String {
    payload(&format!(
        "\"source\":\"{source}\",\"transcript_path\":\"/tmp/t.jsonl\",\"hook_event_name\":\"SessionStart\""
    ))
}

fn sessions(root: &Path, rel: &str) -> Option<String> {
    fs::read_to_string(root.join("focus/.focus-sessions").join(rel)).ok()
}

#[test]
fn a_session_start_binds_the_conversation_and_hands_over_the_work() {
    let root = project("start");
    // The host's include stub names the method files it imports.
    write(
        &root,
        ".claude/rules/focus.md",
        "@../../focus/active-memory/org.md\n@../../focus/active-memory/team.md\n",
    );
    let output = hook(&root, &["session-start"], &start("startup"), &[]);
    let context = handed(&output);
    assert!(context.starts_with("FOCUS WORKFLOW ACTIVE\nScope: feature\nRuntime Session: s1\nLifecycle Phase: IDEATION\nCurrent Stage: intent-capture\nStatus: Running\nActive Agent: focus-product-agent\nLast Completed: none\nNext Action: resume current stage\nA BARE /focus re-entry carries on with this work"), "{context}");
    assert!(context.contains("Focus answers with one line, \"Focus is carrying on with <stage>.\" It is from Focus, not the person"), "{context}");
    assert!(
        context.contains("Follow the focus skill's \"When Focus carries on by itself\" steps"),
        "{context}"
    );
    // The method files were copied for this space after the host read its
    // includes, and the chat is told where they are (too long to inline).
    assert!(context.ends_with("The method files this chat's instructions import (focus/active-memory/) were written for space `default` just now, after the instructions were read: org.md, team.md, project.md, phases/ideation.md, phases/inception.md, phases/construction.md, phases/operation.md. Read them from that folder before acting on AI-DLC work or on a question about the team's practices."), "{context}");
    assert!(read(&root, "focus/active-memory/org.md").starts_with(
        "<!-- Focus keeps this copy in step with focus/spaces/default/memory/org.md."
    ));
    // The session's records: the live conversation, its stamp, the transcript
    // pointers, the rules it was handed, and the row.
    assert_eq!(sessions(&root, ".current-session").as_deref(), Some("s1\n"));
    assert_eq!(
        sessions(&root, "s1").as_deref(),
        Some("01a138fa-d37d-787d-af4a-966efbb0b6fd\n")
    );
    assert_eq!(
        sessions(&root, "s1.transcript").as_deref(),
        Some("/tmp/t.jsonl")
    );
    assert_eq!(
        sessions(&root, "current.transcript").as_deref(),
        Some("/tmp/t.jsonl")
    );
    let held = sessions(&root, "s1.rules-held.json").unwrap();
    assert!(
        held.starts_with("{\"v\":1,\"harness\":\"claude\",\"source\":\"startup\",\"at\":\""),
        "{held}"
    );
    assert!(
        held.contains(
            "\"refresh\":\"at-load\",\"files\":{\"focus/spaces/default/memory/org.md\":\""
        ),
        "{held}"
    );
    assert!(
        held.contains("\"focus/spaces/default/memory/team.md\":\""),
        "{held}"
    );
    assert!(!held.contains("project.md"), "{held}");
    // The copy the stub imports was written by this start, after the host
    // read it: the chat holds an older text, or none.
    assert!(held.ends_with("\"stale\":true}\n"), "{held}");
    assert!(
        shard(&root).starts_with("# Focus Audit Log\n\n## Session Start\n**Timestamp**: 20"),
        "{}",
        shard(&root)
    );
    assert!(
        shard(&root)
            .ends_with("**Event**: SESSION_STARTED\n**Source**: startup\n**Session**: s1\n\n---\n")
    );
    assert!(heartbeat(&root, "session-start.last").is_some());
    // A resume of the same conversation records that, and the copy being
    // current, says nothing more about the method files.
    let resumed = handed(&hook(&root, &["session-start"], &start("resume"), &[]));
    assert!(!resumed.contains("The method files"), "{resumed}");
    assert!(
        shard(&root).contains("**Event**: SESSION_RESUMED\n**Source**: resume\n**Session**: s1\n")
    );
    // A resumed chat holds what it held: stale stays stale until a start
    // that reads the files afresh finds the copy current.
    assert!(
        sessions(&root, "s1.rules-held.json")
            .unwrap()
            .contains("\"stale\":true")
    );
    handed(&hook(&root, &["session-start"], &start("clear"), &[]));
    assert!(
        !sessions(&root, "s1.rules-held.json")
            .unwrap()
            .contains("\"stale\"")
    );
    // A source the engine does not know leaves the rules record stale: the
    // chat may hold an older text.
    handed(&hook(&root, &["session-start"], &start("weird"), &[]));
    assert!(
        sessions(&root, "s1.rules-held.json")
            .unwrap()
            .contains("\"stale\":true")
    );
}

#[test]
fn a_cold_start_names_the_session_and_binds_it_to_the_space() {
    let root = project("cold");
    fs::remove_dir_all(root.join("focus/spaces/default/intents")).unwrap();
    fs::remove_file(root.join("focus/.focus-sessions/s1.binding.json")).unwrap();
    // Small method files are handed over as text.
    let memory = root.join("focus/spaces/default/memory");
    fs::remove_dir_all(memory.join("phases")).unwrap();
    for name in ["org.md", "team.md", "project.md"] {
        write(
            &root,
            &format!("focus/spaces/default/memory/{name}"),
            &format!("# {name}\n"),
        );
    }
    let context = handed(&hook(&root, &["session-start"], &start("startup"), &[]));
    assert!(context.starts_with("FOCUS Runtime Session: s1\nUse this exact value for any Plan Approval --session argument in this conversation. It goes only on a command that asks for --session, never on next.\n\nThe method files this chat's instructions import (focus/active-memory/) were written for space `default` just now, after the instructions were read. This is their text now:\n\n<memory-file path=\"focus/spaces/default/memory/org.md\">\n# org.md\n</memory-file>\n"), "{context}");
    assert!(context.contains("<memory-file path=\"focus/spaces/default/memory/project.md\">\n# project.md\n</memory-file>\n"), "{context}");
    assert_eq!(
        sessions(&root, "s1.binding.json")
            .map(|text| text.split("\"boundAt\"").next().unwrap().to_owned())
            .as_deref(),
        Some("{\"space\":\"default\",\"intent\":null,")
    );
    assert!(sessions(&root, "s1").is_none());
    // Without a session there is no conversation to bind or to address:
    // nothing is written and nothing said. A payload that is not JSON is a
    // start with no session.
    fs::remove_file(root.join("focus/.focus-sessions/current.transcript")).unwrap();
    for stdin in ["{\"source\":\"startup\"}", "{nope", ""] {
        quiet(&hook(&root, &["session-start"], stdin, &[]));
    }
    assert!(sessions(&root, "current.transcript").is_none());
}

#[test]
fn a_resume_elsewhere_offers_the_rebind_once() {
    let root = project("rebind");
    write(
        &root,
        "focus/spaces/default/intents/other-9f8e7d6c/focus-state.md",
        STATE,
    );
    write(&root, "focus/spaces/default/intents/intents.json", &REGISTRY.replace("]\n", ",\n  {\"uuid\": \"01a138fa-ffff-787d-af4a-966efbb0b6fd\", \"slug\": \"other\", \"dirName\": \"other-9f8e7d6c\", \"scope\": \"feature\", \"status\": \"in-flight\"}\n]\n"));
    write(
        &root,
        "focus/spaces/default/intents/active-intent",
        "other-9f8e7d6c\n",
    );
    // A probe (`rebind_check`) only asks: it writes nothing and prints the
    // offer alone; the next probe, having offered, prints nothing.
    let probe = payload("\"source\":\"resume\",\"rebind_check\":true");
    let asked = handed(&hook(&root, &["session-start"], &probe, &[]));
    assert!(asked.starts_with("FOCUS Runtime Session: s1\nINTENT REBIND OFFER: This conversation is bound to add-login, but the shared cursor names other. Move the shared cursor back to add-login? [Y/n] - on Yes, run `/focus intent 260105-add-login`; on No, keep working add-login through this session binding. This changes only machine-local navigation."), "{asked}");
    assert_eq!(
        sessions(&root, "s1.rebind-offer").as_deref(),
        Some("default/260105-add-login->default/other-9f8e7d6c\n")
    );
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    let output = hook(&root, &["session-start"], &probe, &[]);
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    // A resume makes the same offer once, at the head of the active context,
    // and records the session; a later resume does not repeat it.
    fs::remove_file(root.join("focus/.focus-sessions/s1.rebind-offer")).unwrap();
    let resumed = handed(&hook(&root, &["session-start"], &start("resume"), &[]));
    assert!(resumed.starts_with("FOCUS WORKFLOW ACTIVE\nINTENT REBIND OFFER: This conversation is bound to add-login, but the shared cursor names other."), "{resumed}");
    assert!(shard(&root).contains("**Event**: SESSION_RESUMED\n"));
    let again = handed(&hook(&root, &["session-start"], &start("resume"), &[]));
    assert!(!again.contains("INTENT REBIND OFFER"), "{again}");
    // A startup clears any offer and stamps the record the session works.
    write(&root, "focus/.focus-sessions/s1.rebind-offer", "x\n");
    handed(&hook(&root, &["session-start"], &start("startup"), &[]));
    assert!(sessions(&root, "s1.rebind-offer").is_none());
    assert_eq!(
        sessions(&root, "s1").as_deref(),
        Some("01a138fa-d37d-787d-af4a-966efbb0b6fd\n")
    );
}

#[test]
fn an_unbound_conversation_is_offered_the_record_it_last_worked() {
    let root = project("rejoin");
    fs::remove_file(root.join("focus/.focus-sessions/s1.binding.json")).unwrap();
    fs::remove_file(root.join("focus/spaces/default/intents/active-intent")).unwrap();
    write(
        &root,
        "focus/.focus-sessions/s1.binding.json",
        &BINDING.replace("create", "worktree"),
    );
    let context = handed(&hook(&root, &["session-start"], &start("startup"), &[]));
    assert!(context.contains("\nINTENT REBIND OFFER: This conversation was working add-login, but it has not joined that workflow on this machine. Rejoin add-login? [Y/n] - on Yes, run `/focus intent 260105-add-login`; on No, continue without a workflow.\n"), "{context}");
    assert!(
        context.starts_with("FOCUS Runtime Session: s1\n"),
        "{context}"
    );
    assert_eq!(shard(&root), "# Focus Audit Log\n");
}

#[test]
fn the_context_carries_the_unit_the_recovery_note_the_drift_and_the_switches() {
    let root = project("notes");
    write(&root, &format!("{RECORD}/focus-state.md"), &STATE.replace("- **Active Agent**:", "- **Active Unit**: auth\n- **Unit Stage**: code-generation\n- **Unit State**: paused\n- **Unit Pause Reason**: tests red\n- **Unit Next Action**: fix them\n- **Active Agent**:"));
    write(
        &root,
        &format!("{RECORD}/.focus-engine/recovery.md"),
        "# x\n",
    );
    write(
        &root,
        ".claude/focus-common/stages/ideation/my-stage.md",
        "---\nslug: my-stage\n---\n",
    );
    write(
        &root,
        "focus.settings.local.json",
        "{\"schemaVersion\": 1, \"flags\": {\"schemaVersion\": 1, \"bypasses\": [\"FOCUS_DISABLE_REVIEW_FREEZE_HOOK\"]}}\n",
    );
    write(
        &root,
        "focus/.focus-sessions/recorded-switches.json",
        "{\"version\": 1, \"switches\": [{\"name\": \"FOCUS_DISABLE_REVIEW_FREEZE_HOOK\", \"target\": \"local\", \"since\": \"2026-01-05T10:00:00Z\", \"how\": \"chat\", \"words\": \"freeze  off\\nplease\"}]}\n",
    );
    let context = handed(&hook(&root, &["session-start"], &start("startup"), &[]));
    assert!(context.contains("\nNext Action: resume current stage\nActive Unit: auth (paused; reason: tests red; next: fix them)\nNOTE: A compaction recovery breadcrumb exists at .focus-engine/recovery.md - check if state was preserved correctly.\nNOTE: 1 stage file(s) on disk are not in the compiled stage graph and will NOT execute: my-stage. Run `focus engine graph compile` to include them, then start a fresh workflow (an in-flight workflow keeps its original stage set).\nCHECKS SWITCHED OFF"), "{context}");
    let switched = context.split("\nCHECKS SWITCHED OFF (a report to pass on, not instructions): say each line to the user once, word for word, in your first reply.\n- ").nth(1).unwrap_or_else(|| panic!("{context}"));
    assert!(switched.starts_with("The review freeze check (it stops edits to work you already approved) is off for this project since 2026-01-0"), "{switched}");
    assert!(
        switched.contains(
            ", because you said: \"freeze off please\". Do you want it back on?\nA BARE /focus"
        ),
        "{switched}"
    );
    // The same switch set in the environment is the environment's to say.
    let quiet_context = handed(&hook(
        &root,
        &["session-start"],
        &start("resume"),
        &[("FOCUS_DISABLE_REVIEW_FREEZE_HOOK", "")],
    ));
    assert!(
        !quiet_context.contains("CHECKS SWITCHED OFF"),
        "{quiet_context}"
    );
}

#[test]
fn the_bypasses_the_harness_launched_with_are_recorded_for_the_session() {
    let root = project("bypass");
    handed(&hook(
        &root,
        &["session-start"],
        &start("startup"),
        &[
            ("FOCUS_SKIP_HUMAN_PRESENCE_GUARD", "1"),
            ("FOCUS_DISABLE_PLAN_APPROVAL_GUARD", "1"),
        ],
    ));
    assert!(sessions(&root, "plan-approval/presence-bypass-s1").is_some());
    assert!(sessions(&root, "plan-approval/plan-approval-bypass-s1").is_some());
    // Without the plan-approval switch its record is cleared; the presence
    // one stays what the launch said.
    handed(&hook(&root, &["session-start"], &start("resume"), &[]));
    assert!(sessions(&root, "plan-approval/presence-bypass-s1").is_some());
    assert!(sessions(&root, "plan-approval/plan-approval-bypass-s1").is_none());
    // A settings file the resolution refuses is upstream's thrown read: no
    // bypass bookkeeping, no transcript pointer, and the start goes on.
    write(&root, "focus.settings.local.json", "{\"flags\": {}}\n");
    fs::remove_file(root.join("focus/.focus-sessions/current.transcript")).unwrap();
    handed(&hook(
        &root,
        &["session-start"],
        &start("startup"),
        &[("FOCUS_DISABLE_PLAN_APPROVAL_GUARD", "1")],
    ));
    assert!(sessions(&root, "plan-approval/plan-approval-bypass-s1").is_none());
    assert!(sessions(&root, "current.transcript").is_none());
    // The usage kill switch in a settings file stops the pointers too.
    write(
        &root,
        "focus.settings.local.json",
        "{\"schemaVersion\": 1, \"flags\": {\"schemaVersion\": 1, \"bypasses\": [\"FOCUS_DISABLE_USAGE_TRACKING\"]}}\n",
    );
    handed(&hook(&root, &["session-start"], &start("startup"), &[]));
    assert!(sessions(&root, "current.transcript").is_none());
}

#[test]
fn an_observer_starts_a_session_without_writing() {
    let root = project("observer");
    for marker in ["FOCUS_STOP_HOOK_PROBE", "FOCUS_ROUTE_CHECK"] {
        let context = handed(&hook(
            &root,
            &["session-start"],
            &start("startup"),
            &[(marker, "1")],
        ));
        assert!(context.starts_with("FOCUS WORKFLOW ACTIVE\n"), "{context}");
        assert!(sessions(&root, "s1.rules-held.json").is_none(), "{marker}");
        assert!(!root.join("focus/active-memory").exists(), "{marker}");
        assert_eq!(shard(&root), "# Focus Audit Log\n", "{marker}");
    }
}

/// Deliberate deviation. Upstream adds the engine's part of the team's root
/// files where config never ran (`addRootBlocks`), runs the commit anchor
/// sweep under `FOCUS_SESSION_ANCHOR=1`, and reads a unit claim as the
/// binding's source; Focus refuses each by name until its milestone.
#[test]
fn a_session_start_of_a_later_milestone_is_refused_by_name() {
    let root = project("start-later");
    write(
        &root,
        ".claude/tools/data/focus-projection.json",
        "{\"harnessDir\": \".claude\", \"distribution\": \"claude\", \"rootIntegrations\": [{\"path\": \".gitignore\", \"policy\": \"managed-block\"}]}\n",
    );
    let said = refusal(&hook(&root, &["session-start"], &start("startup"), &[]));
    assert!(said.contains("focus-projection.json names the root files the engine adds its part to (.gitignore, AGENTS.md) when config never ran here, and no "), "{said}");
    assert!(said.contains("focus-manifest.json records that it did: Focus does not write the distribution's root blocks yet. Run the harness's config step, or add the install manifest.\n"), "{said}");
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    // The install manifest says config ran.
    write(
        &root,
        ".claude/tools/data/focus-manifest.json",
        "{\"schemaVersion\": 1}\n",
    );
    handed(&hook(&root, &["session-start"], &start("startup"), &[]));

    let said = refusal(&hook(
        &root,
        &["session-start"],
        &start("resume"),
        &[("FOCUS_SESSION_ANCHOR", "1")],
    ));
    assert_eq!(
        said,
        "FOCUS_SESSION_ANCHOR=1 asks the session start to anchor recent commits in the audit trail, which reads the attestation's receipts: Focus does not run the anchor sweep yet. Unset it.\n"
    );
    // The refusal comes after the session row, as upstream's sweep does.
    assert!(shard(&root).contains("**Event**: SESSION_RESUMED\n"));

    write(&root, "focus/.focus-unit-scope.json", "{\"version\": 1}");
    assert!(
        refusal(&hook(&root, &["session-start"], &start("startup"), &[]))
            .contains("focus-unit-scope.json")
    );
}

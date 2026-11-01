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
        refusal(&hook(&root, &["session-start"], &payload(""), &[])),
        "focus engine hook session-start: binds the conversation to a record and offers a resume, and Focus does not run that hook yet.\n"
    );
    assert_eq!(
        refusal(&hook(&root, &["guard-tool-call"], &payload(""), &[])),
        "focus engine hook guard-tool-call: runs the PreToolUse guards in one process, and Focus does not run that hook yet.\n"
    );
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    assert!(heartbeat(&root, "session-start.last").is_none());
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
        "focus/.focus-sessions/s1.rules-held.json",
        "{\"v\":2}",
    );
    assert!(
        refusal(&hook(&root, &["validate-state"], &payload(""), &[]))
            .contains("s1.rules-held.json records the rules handed to this conversation")
    );
    fs::remove_file(root.join("focus/.focus-sessions/s1.rules-held.json")).unwrap();

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

/// Deliberate deviation. Upstream walks the process tree to find which
/// conversation is calling when the payload names no session, and resolves
/// a delegated worktree to the main checkout's intent; Focus refuses both by
/// name. A hook stands outside on any other failure of the resolution, as
/// upstream's does.
#[test]
fn a_resolution_not_ported_refuses_the_hook_by_name() {
    let root = project("resolution");
    write(&root, "focus/.focus-sessions/pids/1.json", "{}\n");
    assert!(
        refusal(&hook(&root, &["validate-state"], "{}", &[]))
            .contains("pids records which process belongs to which conversation")
    );
    assert!(
        refusal(&hook(
            &root,
            &["log-subagent"],
            "{\"agent_type\":\"x\"}",
            &[]
        ))
        .contains("pids records which process belongs to which conversation")
    );
    // The payload's own session settles it, once its binding is there: a
    // session the project has no binding for is a conversation upstream
    // would place by the process tree.
    quiet(&hook(&root, &["validate-state"], &payload(""), &[]));
    quiet(&hook(
        &root,
        &["log-subagent"],
        &payload("\"agent_type\":\"x\""),
        &[],
    ));
    quiet(&hook(
        &root,
        &["session-end"],
        &payload("\"reason\":\"bound\""),
        &[],
    ));
    assert!(shard(&root).contains("SUBAGENT_COMPLETED"));
    assert!(shard(&root).contains("**Reason**: bound\n"));
    assert!(
        refusal(&hook(
            &root,
            &["validate-state"],
            "{\"session_id\":\"s2\"}",
            &[]
        ))
        .contains("pids records which process belongs to which conversation")
    );
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

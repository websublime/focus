//! `focus engine state`, run as a process on an installed project.
//!
//! What the verbs answer is held to upstream's by the recorded differential,
//! which replays on Unix. These tests run everywhere: they cover the
//! command's surface, what a write leaves behind, and each way Focus
//! deliberately differs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RECORD: &str = "focus/spaces/default/intents/add-login-1a2b3c4d";
const STATE: &str = "# Focus State\n\n## Workflow\n- **Scope**: feature\n- **Status**: In Progress\n- **Completed**: 2\n- **Guard Policy**: off (from scope feature)\n\n## Stage Progress\n- [x] intent-capture \u{2014} EXECUTE\n- [x] requirements-analysis \u{2014} EXECUTE\n- [-] code-generation \u{2014} EXECUTE\n- [ ] build-and-test \u{2014} EXECUTE\n- [S] nfr-design \u{2014} SKIP: not needed\n";
const DIRECT: [(&str, &str); 1] = [("FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS", "1")];

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

/// An installed Claude Code project with one intent record, its state file,
/// this clone's identity and an audit shard.
fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("focus-state-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    copy_dir(&fixtures.join("installed/claude"), &root);
    write(&root, &format!("{RECORD}/focus-state.md"), STATE);
    write(
        &root,
        "focus/spaces/default/intents/active-intent",
        "add-login-1a2b3c4d\n",
    );
    write(&root, "focus/.focus-clone-id", "abc123\nlaptop\n");
    write(
        &root,
        &format!("{RECORD}/audit/laptop-abc123.md"),
        "# Focus Audit Log\n",
    );
    root
}

fn state(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    command
        .args(["engine", "state"])
        .args(args)
        .current_dir(root);
    for (name, _) in std::env::vars() {
        if name.starts_with("FOCUS_") || name == "CLAUDE_PROJECT_DIR" {
            command.env_remove(name);
        }
    }
    for (name, value) in env {
        command.env(name, value);
    }
    command.output().expect("focus runs")
}

fn ok(output: &Output) -> String {
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout)
}

fn refusal(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    let err = text(&output.stderr);
    let parsed: serde_json::Value =
        serde_json::from_str(err.trim_end()).unwrap_or_else(|_| panic!("{err}"));
    parsed["error"].as_str().unwrap().to_owned()
}

fn shard(root: &Path) -> String {
    fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap()
}

fn state_file(root: &Path) -> String {
    fs::read_to_string(root.join(RECORD).join("focus-state.md")).unwrap()
}

#[test]
fn fields_and_checkboxes_are_read_and_written() {
    let root = project("fields");
    assert_eq!(ok(&state(&root, &["get", "Scope"], &[])), "feature\n");
    assert_eq!(
        ok(&state(&root, &["get", "Guard", "Policy"], &[])),
        "off (from scope feature)\n"
    );
    assert_eq!(ok(&state(&root, &["count", "completed"], &[])), "2\n");
    assert_eq!(ok(&state(&root, &["count", "skipped"], &[])), "1\n");
    assert_eq!(
        refusal(&state(&root, &["get", "Nope"], &[])),
        "Field not found: Nope"
    );

    let written = ok(&state(
        &root,
        &[
            "set",
            "Scope=bugfix",
            "Completed=+1",
            "Nope=1",
            "Scope=$1$$",
        ],
        &DIRECT,
    ));
    assert_eq!(written, "{\"updated\":true,\"fields\":4}\n");
    let after = state_file(&root);
    // The last write of a field wins, with its dollar patterns expanded as
    // JavaScript expands them; a field that is not there is not added.
    assert!(after.contains("- **Scope**: - **Scope**:$\n"), "{after}");
    assert!(after.contains("- **Completed**: 3\n"), "{after}");
    assert!(!after.contains("Nope"));
    assert_eq!(
        after.len(),
        STATE.len() + "- **Scope**:$".len() - "feature".len()
    );

    // Counting from a field that is no number is NaN, as `parseInt` says,
    // and from an empty one is counting from zero.
    ok(&state(&root, &["set", "Status=-1"], &DIRECT));
    assert_eq!(ok(&state(&root, &["get", "Status"], &[])), "NaN\n");
    ok(&state(&root, &["set", "Status=", "Status=+1"], &DIRECT));
    assert_eq!(ok(&state(&root, &["get", "Status"], &[])), "1\n");
    ok(&state(
        &root,
        &[
            "set",
            "Completed=-1",
            "Completed=-1",
            "Completed=-1",
            "Completed=-1",
        ],
        &DIRECT,
    ));
    assert_eq!(ok(&state(&root, &["get", "Completed"], &[])), "0\n");
    let now = ok(&state(&root, &["set", "Status=NOW"], &DIRECT));
    assert!(now.starts_with("{\"updated\":true"));
    let stamped = ok(&state(&root, &["get", "Status"], &[]));
    assert!(stamped.len() == 21 && stamped.ends_with("Z\n"), "{stamped}");

    let boxes = ok(&state(
        &root,
        &[
            "checkbox",
            "build-and-test=completed",
            "nfr-design=pending",
            "nope=completed",
        ],
        &DIRECT,
    ));
    assert_eq!(
        boxes,
        "{\"updated\":true,\"checkboxes\":3,\"completed_count\":3}\n"
    );
    let after = state_file(&root);
    assert!(
        after.contains(
            "- [x] build-and-test \u{2014} EXECUTE\n- [ ] nfr-design \u{2014} SKIP: not needed\n"
        ),
        "{after}"
    );
    assert!(after.contains("- **Completed**: 3\n"), "{after}");
    assert_eq!(
        refusal(&state(&root, &["checkbox", "build-and-test=done"], &DIRECT)),
        "Invalid state: done. Valid: pending, in-progress, awaiting-approval, revising, completed, skipped"
    );
    assert_eq!(
        refusal(&state(&root, &["set", "Scope=x", "nopair"], &DIRECT)),
        "Invalid field=value pair: nopair"
    );
    assert_eq!(
        refusal(&state(
            &root,
            &["set", "Construction Checkpoints=enabled"],
            &DIRECT
        )),
        "Construction Checkpoints cannot be changed with `focus engine state set`. Use `focus engine state set-construction-checkpoints <enabled|disabled>`."
    );
    // A batch that fails is not applied at all.
    assert!(state_file(&root).contains("- **Scope**: - **Scope**:$\n"));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_refusal_is_recorded_in_the_ledger_with_the_project_redacted() {
    let root = project("errors");
    let here = root.to_str().unwrap().to_owned();
    assert_eq!(
        refusal(&state(&root, &["get", &format!("{here}/x")], &[])),
        format!("Field not found: {here}/x")
    );
    let ledger = shard(&root);
    assert!(
        ledger.contains("**Event**: ERROR_LOGGED\n**Tool**: focus-state\n**Command**: focus-state get <project-dir>/x\n**Error**: Field not found: <project-dir>/x\n"),
        "{ledger}"
    );
    // The refusal of a selection Focus cannot resolve is printed alone, as
    // upstream makes it before anything records.
    write(&root, "focus/.focus-sessions/pids/1.json", "{}\n");
    let unresolved = refusal(&state(&root, &["get", "Scope"], &[]));
    assert!(
        unresolved.contains("cannot yet tell which conversation is calling"),
        "{unresolved}"
    );
    assert_eq!(shard(&root), ledger);
    fs::remove_dir_all(root.join("focus/.focus-sessions")).unwrap();
    // The row is the record's: with no state file there is none to write
    // beside, and the refusal is printed alone.
    fs::remove_file(root.join(RECORD).join("focus-state.md")).unwrap();
    let missing = refusal(&state(&root, &["get", "Scope"], &[]));
    assert!(missing.starts_with("State file not found: "), "{missing}");
    assert_eq!(shard(&root), ledger);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_write_passes_the_state_transition_fence_or_is_refused() {
    let root = project("gate");
    // Policy off, from the scope: the fence is down and says nothing.
    let quiet = state(&root, &["set", "Scope=x"], &[]);
    assert!(quiet.status.success());
    assert!(quiet.stderr.is_empty(), "{}", text(&quiet.stderr));
    let ledger = shard(&root);
    assert!(
        ledger.contains("**Event**: GUARD_STOOD_ASIDE\n**Guard**: state-transition\n**Authority**: none\n**Grant**: none\n**Actor**: main\n**Tool**: focus-state\n**Details**: focus-state set\n"),
        "{ledger}"
    );
    // Relaxed with the fence switched off by the person: it speaks.
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &STATE.replace(
            "off (from scope feature)",
            "relaxed (set by you)\n- **Guards Off**: state-transition (set by you)",
        ),
    );
    let spoken = state(&root, &["checkbox", "build-and-test=completed"], &[]);
    assert!(spoken.status.success());
    assert_eq!(
        text(&spoken.stderr),
        "Continuing past the state-transition check because it is off for this piece of work (set by you). Recorded in the audit trail: focus-state checkbox\n"
    );
    // Strict: refused, with the switch named; reads need no gate.
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &STATE.replace("off (from scope feature)", "strict (set by you)"),
    );
    let held = refusal(&state(&root, &["set", "Scope=y"], &[]));
    assert!(
        held.starts_with(
            "Stage status cannot be changed with `focus engine state set` because that bypasses"
        ) && held.contains("run `focus engine config set guard.state-transition off` yourself"),
        "{held}"
    );
    assert!(state_file(&root).contains("- **Scope**: feature\n"));
    assert_eq!(ok(&state(&root, &["get", "Scope"], &[])), "feature\n");
    // The team's memory holds strict: the switch is not offered.
    write(
        &root,
        "focus/spaces/default/memory/team.md",
        "## Guard Policy\n\nMode: strict\n",
    );
    let pinned = refusal(&state(&root, &["set", "Scope=y"], &[]));
    assert!(
        pinned.contains("Your team set Guard Policy to strict in"),
        "{pinned}"
    );
    // A policy that cannot be read: the fence stays up, and the repair is named.
    write(
        &root,
        "focus/spaces/default/memory/team.md",
        "## Guard Policy\n\nMode: maybe\n",
    );
    let unread = refusal(&state(&root, &["set", "Scope=y"], &[]));
    assert!(
        unread.contains(
            "A Guard Policy line in this space's org.md, team.md or project.md cannot be read"
        ),
        "{unread}"
    );
    // The person's switch, and the orchestrator's ownership.
    assert!(state(&root, &["set", "Scope=y"], &DIRECT).status.success());
    assert!(
        !state(
            &root,
            &["set", "Scope=z"],
            &[("FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS", "0")]
        )
        .status
        .success()
    );
    assert!(
        !state(
            &root,
            &["set", "Scope=z"],
            &[("FOCUS_STATE_TRANSITION_OWNER", "orchestrate:1")]
        )
        .status
        .success()
    );
    #[cfg(unix)]
    {
        let owner = format!("orchestrate:{}", std::process::id());
        assert!(
            state(
                &root,
                &["set", "Scope=z"],
                &[("FOCUS_STATE_TRANSITION_OWNER", &owner)]
            )
            .status
            .success()
        );
        assert_eq!(ok(&state(&root, &["get", "Scope"], &[])), "z\n");
    }
    // An archived workflow refuses every write.
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &STATE.replace("In Progress", "Archived"),
    );
    assert_eq!(
        refusal(&state(&root, &["set", "Scope=y"], &DIRECT)),
        "Workflow is Archived, so `focus engine state set` is refused. Bring it back first with `/focus intent unarchive <name>`."
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_observer_writes_nothing() {
    let root = project("observer");
    let before = (state_file(&root), shard(&root));
    for marker in ["FOCUS_STOP_HOOK_PROBE", "FOCUS_ROUTE_CHECK"] {
        let env = [(marker, "1"), DIRECT[0]];
        let stopped = refusal(&state(&root, &["set", "Scope=x"], &env));
        assert!(
            stopped.contains("reached the durable write primitive \"writeStateFile\""),
            "{stopped}"
        );
        assert!(
            refusal(&state(
                &root,
                &["checkbox", "build-and-test=completed"],
                &env
            ))
            .contains("writeStateFile")
        );
        // The refusal's own row is not written either, nor the lock's file.
        assert_eq!((state_file(&root), shard(&root)), before);
        assert!(!root.join("focus/.focus-lock").exists());
        assert_eq!(ok(&state(&root, &["get", "Scope"], &env)), "feature\n");
        // At the gate, the stood-aside row is the first write it reaches,
        // and that one is best effort: the state's write is where it stops.
        let at_gate = refusal(&state(&root, &["set", "Scope=x"], &[(marker, "1")]));
        assert!(at_gate.contains("\"writeStateFile\""), "{at_gate}");
        assert_eq!((state_file(&root), shard(&root)), before);
    }
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Upstream's usage lists its 33 verbs and its
/// refusal of a verb it does not know too; Focus lists the four it has, and
/// refuses the others by name, before any fence is consulted (upstream
/// passes them through the state-transition fence first).
#[test]
fn the_state_command_names_the_verbs_it_has() {
    let root = project("usage");
    assert_eq!(
        refusal(&state(&root, &[], &[])),
        "Usage: focus engine state <subcommand>. Valid: get, set, checkbox, count. Run with --help for detail."
    );
    assert_eq!(
        refusal(&state(&root, &["nope"], &[])),
        "Unknown subcommand: nope. Run focus engine state --help for usage."
    );
    for (verb, does) in [
        ("advance", "moves the workflow to its next stage"),
        ("approve", "records a gate's approval"),
        ("park", "parks the workflow"),
        ("unit", "runs a unit of work's step"),
        ("practices-promote", "promotes a practice"),
    ] {
        assert_eq!(
            refusal(&state(&root, &[verb, "x"], &[])),
            format!("{verb} {does}, and Focus does not do that yet.")
        );
    }
    let ledger = shard(&root);
    assert!(!ledger.contains("GUARD_STOOD_ASIDE"), "{ledger}");
    assert_eq!(ledger.matches("ERROR_LOGGED").count(), 7);
    for help in ["--help", "-h"] {
        let help = state(&root, &[help], &[]);
        assert!(help.status.success());
        assert!(
            text(&help.stdout)
                .starts_with("Usage: focus engine state <verb> [--project-dir <dir>]\n")
        );
    }
    // The project may be named before the verb.
    let named = Command::new(env!("CARGO_BIN_EXE_focus"))
        .args([
            "engine",
            "state",
            "--project-dir",
            root.to_str().unwrap(),
            "get",
            "Scope",
        ])
        .current_dir(std::env::temp_dir())
        .output()
        .unwrap();
    assert_eq!(ok(&named), "feature\n");
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Upstream takes a trailing `--project-dir` with no
/// value as part of the field; Focus refuses the flag, and prints that
/// refusal alone, as it is made before the record is known.
#[test]
fn a_flag_without_its_value_is_refused() {
    let root = project("flag");
    let output = state(&root, &["get", "Scope", "--project-dir"], &[]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "Missing required flag: --project-dir <value>\n"
    );
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Upstream sends a metric for the ERROR_LOGGED row
/// to the configured endpoint; Focus has no metrics yet and writes no row,
/// rather than leave the endpoint without what it expects. The refusal is
/// printed either way.
#[test]
fn an_error_is_not_recorded_when_a_metric_would_be_due() {
    let root = project("metrics");
    let endpoint = [("FOCUS_METRICS_ENDPOINT", "http://127.0.0.1:9/x")];
    assert_eq!(
        refusal(&state(&root, &["get", "Nope"], &endpoint)),
        "Field not found: Nope"
    );
    assert_eq!(shard(&root), "# Focus Audit Log\n");
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. A record carrying an active-directive or turn
/// marker: upstream reads the authority behind the action off them, and
/// keeps the state-write record beside the first. The hooks and the
/// orchestrator that write them are M4 and M5, so Focus refuses to decide a
/// fence with the markers there, and refuses to write the state beside the
/// active directive.
#[test]
fn a_record_with_a_marker_is_refused() {
    let root = project("markers");
    let engine = format!("{RECORD}/.focus-engine");
    write(&root, &format!("{engine}/active-directive.json"), "{}\n");
    let at_gate = refusal(&state(&root, &["set", "Scope=x"], &[]));
    assert!(
        at_gate.starts_with("Stage status cannot be changed"),
        "{at_gate}"
    );
    let direct = refusal(&state(&root, &["set", "Scope=x"], &DIRECT));
    assert!(
        direct.contains("active-directive.json says the engine has a step in hand"),
        "{direct}"
    );
    assert!(state_file(&root).contains("- **Scope**: feature\n"));
    assert_eq!(ok(&state(&root, &["get", "Scope"], &[])), "feature\n");
    fs::remove_file(root.join(&engine).join("active-directive.json")).unwrap();
    write(&root, &format!("{engine}/human-turn"), "");
    write(&root, &format!("{engine}/engine-touch"), "");
    assert!(
        refusal(&state(&root, &["set", "Scope=x"], &[]))
            .starts_with("Stage status cannot be changed")
    );
    assert!(state(&root, &["set", "Scope=x"], &DIRECT).status.success());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_write_waits_for_the_workspace_lock() {
    let root = project("locked");
    let layout = focus_workspace::Layout::new(&root, focus_harness::Harness::Claude);
    let held = focus_workspace::WorkspaceLock::acquire(&layout, "another session").unwrap();
    let wait = [("FOCUS_LOCK_TIMEOUT_MS", "300"), DIRECT[0]];
    for args in [
        ["set", "Scope=x"].as_slice(),
        ["checkbox", "build-and-test=completed"].as_slice(),
    ] {
        let blocked = refusal(&state(&root, args, &wait));
        assert!(
            blocked.starts_with("Timed out after 300ms waiting for the workspace lock at "),
            "{blocked}"
        );
    }
    assert_eq!(state_file(&root), STATE);
    // Reads wait for nothing.
    assert_eq!(ok(&state(&root, &["get", "Scope"], &wait)), "feature\n");
    drop(held);
    assert!(state(&root, &["set", "Scope=x"], &wait).status.success());
    let _ = fs::remove_dir_all(root);
}

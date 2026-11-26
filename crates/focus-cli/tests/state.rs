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
    // The project is found from the working directory, which the process
    // reads resolved (`/private/var/…` for a macOS temp dir), so the path
    // the ledger redacts is the resolved one.
    let here = fs::canonicalize(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let here = here
        .strip_prefix(r"\\?\")
        .map_or(here.clone(), str::to_owned);
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
    write(&root, ".focus/worktree-meta.json", "{}\n");
    let unresolved = refusal(&state(&root, &["get", "Scope"], &[]));
    assert!(
        unresolved.contains("does not work inside delegated worktrees yet"),
        "{unresolved}"
    );
    assert_eq!(shard(&root), ledger);
    fs::remove_file(root.join(".focus/worktree-meta.json")).unwrap();
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
/// refusal of a verb it does not know too; Focus lists the twenty-seven it has,
/// and refuses the others by name, before any fence is consulted (upstream
/// passes them through the state-transition fence first).
#[test]
fn the_state_command_names_the_verbs_it_has() {
    let root = project("usage");
    assert_eq!(
        refusal(&state(&root, &[], &[])),
        "Usage: focus engine state <subcommand>. Valid: get, set, checkbox, count, advance, finalize, complete-workflow, gate-start, approve, reject, revise, skip, resume, park, unpark, acknowledge-compaction, practices-event, reuse-artifact, lookup, set-skeleton-stance, set-construction-iteration, set-construction-checkpoints, set-construction-execution, set-construction-verification-command, set-unit-gate-rhythm, sync-unit-scope-stage, unit. Run with --help for detail."
    );
    assert_eq!(
        refusal(&state(&root, &["nope"], &[])),
        "Unknown subcommand: nope. Run focus engine state --help for usage."
    );
    for (verb, does) in [
        ("set-unit-ownership", "sets the units' ownership"),
        ("refresh-unit-progress", "refreshes the units' progress"),
        ("fork", "forks the state for a bolt"),
        ("merge", "merges a bolt's state"),
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
    // The turn markers are read: the engine's touch after the person's
    // turn covers nothing, and the fence, down for this scope, stands
    // aside; a person's turn after the engine's touch is a grant.
    write(&root, &format!("{engine}/human-turn"), "");
    write(&root, &format!("{engine}/engine-touch"), "");
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(root.join(&engine).join("engine-touch"))
        .unwrap()
        .set_modified(later)
        .unwrap();
    ok(&state(&root, &["set", "Scope=x"], &[]));
    let rows = shard(&root);
    assert!(
        rows.contains("**Event**: GUARD_STOOD_ASIDE\n**Guard**: state-transition\n**Authority**: none\n**Grant**: none\n"),
        "{rows}"
    );
    fs::File::options()
        .write(true)
        .open(root.join(&engine).join("human-turn"))
        .unwrap()
        .set_modified(later + std::time::Duration::from_secs(5))
        .unwrap();
    ok(&state(&root, &["set", "Scope=y"], &[]));
    let rows = shard(&root);
    assert!(
        rows.contains("**Event**: GUARD_STOOD_ASIDE\n**Guard**: state-transition\n**Authority**: grant\n**Grant**: turn-marker\n"),
        "{rows}"
    );
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

/// The state of a record upstream created on the feature scope with the
/// summary-confirmation ceremony off and the review class `none`: what a
/// transition reads, and no more.
const CREATED: &str = "# Focus State Tracking\n\n## Project Information\n- **Project**: Add login to the app\n- **Project Type**: Greenfield\n- **Scope**: feature\n- **Review Override**: none\n- **Guard Policy**: off (from scope feature)\n- **Summary Confirmation**: off (set by a command)\n\n## Execution Plan Summary\n- **Total Stages**: 32\n- **Completed**: 3\n- **In Progress**: intent-capture\n\n## Phase Progress\n- **Initialization**: Verified\n- **Ideation**: Active\n- **Inception**: Pending\n- **Construction**: Pending\n- **Operation**: Pending\n\n## Stage Progress\n- [x] workspace-scaffold \u{2014} EXECUTE\n- [x] workspace-detection \u{2014} EXECUTE\n- [x] state-init \u{2014} EXECUTE\n- [-] intent-capture \u{2014} EXECUTE\n- [ ] market-research \u{2014} EXECUTE\n- [ ] feasibility \u{2014} EXECUTE\n- [ ] scope-definition \u{2014} EXECUTE\n- [ ] team-formation \u{2014} EXECUTE\n- [ ] rough-mockups \u{2014} EXECUTE\n- [ ] approval-handoff \u{2014} EXECUTE\n- [ ] reverse-engineering \u{2014} SKIP\n- [ ] practices-discovery \u{2014} EXECUTE\n- [ ] requirements-analysis \u{2014} EXECUTE\n- [ ] user-stories \u{2014} EXECUTE\n- [ ] refined-mockups \u{2014} EXECUTE\n- [ ] domain-design \u{2014} EXECUTE\n- [ ] units-generation \u{2014} EXECUTE\n- [ ] contract-design \u{2014} EXECUTE\n- [ ] delivery-planning \u{2014} EXECUTE\n- [ ] functional-design \u{2014} EXECUTE\n- [ ] nfr-requirements \u{2014} EXECUTE\n- [ ] nfr-design \u{2014} EXECUTE\n- [ ] infrastructure-design \u{2014} EXECUTE\n- [ ] code-generation \u{2014} EXECUTE\n- [ ] build-and-test \u{2014} EXECUTE\n- [ ] ci-pipeline \u{2014} EXECUTE\n- [ ] deployment-pipeline \u{2014} EXECUTE\n- [ ] environment-provisioning \u{2014} EXECUTE\n- [ ] deployment-execution \u{2014} EXECUTE\n- [ ] observability-setup \u{2014} EXECUTE\n- [ ] incident-response \u{2014} EXECUTE\n- [ ] performance-validation \u{2014} EXECUTE\n- [ ] feedback-optimization \u{2014} EXECUTE\n\n## Current Status\n- **Lifecycle Phase**: IDEATION\n- **Current Stage**: intent-capture\n- **Next Stage**: market-research\n- **Status**: Running\n- **Last Updated**: 2026-01-05T10:00:00Z\n\n## Session Resume Point\n- **Last Completed Stage**: state-init\n- **Next Action**: Execute intent-capture\n";
const CREATED_RECORD: &str = "focus/spaces/default/intents/260105-add-login";
const CREATED_SHARD: &str = "# Focus Audit Log\n\n## Workflow Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: WORKFLOW_STARTED\n**Scope**: feature\n\n---\n\n## Stage Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: intent-capture\n**Agent**: focus-product-agent\n\n---\n";

/// An installed project with a created record at its first stage.
fn created(name: &str, state: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("focus-state-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    copy_dir(&fixtures.join("installed/claude"), &root);
    write(&root, &format!("{CREATED_RECORD}/focus-state.md"), state);
    write(
        &root,
        &format!("{CREATED_RECORD}/audit/laptop-abc123.md"),
        CREATED_SHARD,
    );
    write(
        &root,
        "focus/spaces/default/intents/active-intent",
        "260105-add-login\n",
    );
    write(
        &root,
        "focus/spaces/default/intents/intents.json",
        "[\n  {\n    \"uuid\": \"01a138fa-d37d-787d-af4a-966efbb0b6fd\",\n    \"slug\": \"add-login\",\n    \"dirName\": \"260105-add-login\",\n    \"scope\": \"feature\",\n    \"status\": \"in-flight\"\n  }\n]\n",
    );
    write(&root, "focus/.focus-clone-id", "abc123\nlaptop\n");
    root
}

fn created_shard(root: &Path) -> String {
    fs::read_to_string(root.join(CREATED_RECORD).join("audit/laptop-abc123.md")).unwrap()
}

fn created_state(root: &Path) -> String {
    fs::read_to_string(root.join(CREATED_RECORD).join("focus-state.md")).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn a_transition_runs_whole_with_its_rows_first() {
    let root = created("advance", CREATED);
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\r\nline\r\n",
    );
    let answer: serde_json::Value =
        serde_json::from_str(&ok(&state(&root, &["advance", "intent-capture"], &DIRECT))).unwrap();
    assert_eq!(answer["completed"], "intent-capture");
    assert_eq!(answer["started"], "market-research");
    assert_eq!(answer["phase"], "IDEATION");
    assert_eq!(answer["completed_count"], 4);
    assert_eq!(answer["next_after"], "feasibility");
    assert_eq!(answer["already_completed"], false);
    assert_eq!(
        answer["memory_path"],
        "focus/spaces/default/intents/260105-add-login/ideation/market-research/memory.md"
    );
    let state_file = created_state(&root);
    assert!(
        state_file.contains("- [x] intent-capture \u{2014} EXECUTE\n- [-] market-research"),
        "{state_file}"
    );
    assert!(
        state_file.contains("- **Current Stage**: market-research\n- **Next Stage**: feasibility"),
        "{state_file}"
    );
    assert!(state_file.contains("- **Completed**: 4\n"), "{state_file}");
    assert!(
        state_file.contains("- **Next Action**: Execute Market Research\n"),
        "{state_file}"
    );
    let ledger = created_shard(&root);
    assert!(
        ledger.contains(
            "**Event**: STAGE_COMPLETED\n**Stage**: intent-capture\n**Validation Basis**: "
        ),
        "{ledger}"
    );
    assert!(
        ledger.contains("**Details**: Stage Intent Capture & Framing completed\n"),
        "{ledger}"
    );
    assert!(ledger.ends_with("**Event**: STAGE_STARTED\n**Stage**: market-research\n**Agent**: focus-product-agent\n\n---\n"), "{ledger}");
    // The receipt hashes the artifact's path and its text with CRLF read as LF.
    let basis: serde_json::Value = serde_json::from_str(
        ledger
            .split("**Validation Basis**: ")
            .nth(1)
            .unwrap()
            .split('\n')
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(basis["schema"], 3);
    assert_eq!(basis["projectType"], "greenfield");
    assert_eq!(basis["inputs"], serde_json::json!([]));
    let statement = basis["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["artifact"] == "intent-statement")
        .unwrap();
    let path = format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md");
    let content = format!(
        r#"[{{"path":"{path}","present":true,"sha256":"sha256:{}"}}]"#,
        sha256_hex(b"# Intent\nline\n")
    );
    assert_eq!(
        statement["contentHash"],
        format!("sha256:{}", sha256_hex(content.as_bytes()))
    );
    assert_eq!(statement["presentCount"], 1);
    // A second advance of the same stage is a replay, and writes nothing.
    let again: serde_json::Value =
        serde_json::from_str(&ok(&state(&root, &["advance", "intent-capture"], &DIRECT))).unwrap();
    assert_eq!(again["replay"], true);
    assert_eq!(created_shard(&root), ledger);
    let resumed: serde_json::Value =
        serde_json::from_str(&ok(&state(&root, &["resume"], &[]))).unwrap();
    assert_eq!(resumed["current_stage"], "market-research");
    assert_eq!(resumed["gate_state"], "in-progress");
    assert_eq!(resumed["compaction_pending"], false);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_stage_without_its_artifacts_is_refused_before_anything_changes() {
    let root = created("artifacts", CREATED);
    // The guard's sentence, then its recovery question on a line of its
    // own; and the refusal counted in the series of the stage.
    let refused = refusal(&state(&root, &["advance", "intent-capture"], &DIRECT));
    let (sentence, ask) = refused
        .split_once('\n')
        .expect("a question after the sentence");
    assert_eq!(
        sentence,
        "Cannot complete \"intent-capture\": none of its declared artifacts exist under the intent's record directory. The stage protocol requires Intent Capture & Framing to produce output before the gate. Produce the artifacts before completing. (declared: intent-statement, stakeholder-map, intent-capture-questions)"
    );
    let ask: serde_json::Value = serde_json::from_str(ask).unwrap();
    assert_eq!(ask["kind"], "ask");
    assert_eq!(ask["ask_type"], "guard-recovery");
    assert_eq!(ask["stage"], "intent-capture");
    assert_eq!(
        ask["reason_codes"],
        serde_json::json!(["REQUIRED_ARTIFACTS_MISSING"])
    );
    let again = refusal(&state(&root, &["advance", "intent-capture"], &DIRECT));
    assert!(
        again.contains(" still can't go ahead as things stand"),
        "{again}"
    );
    assert_eq!(created_state(&root), CREATED);
    let ledger = created_shard(&root);
    assert!(ledger.contains("**Event**: ERROR_LOGGED\n**Tool**: focus-state\n**Command**: focus-state advance intent-capture\n"), "{ledger}");
    assert!(!ledger.contains("STAGE_COMPLETED"), "{ledger}");
    // The guard can be turned off for a run.
    let off = [
        ("FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS", "1"),
        ("FOCUS_SKIP_ARTIFACT_GUARD", "1"),
    ];
    assert!(
        ok(&state(&root, &["finalize", "intent-capture"], &off)).contains("\"completed_count\":4")
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_skip_routes_and_a_completion_closes_the_registry_row() {
    let root = created("skip", CREATED);
    let skipped: serde_json::Value = serde_json::from_str(&ok(&state(
        &root,
        &["skip", "intent-capture", "--route", "--reason", "not this"],
        &DIRECT,
    )))
    .unwrap();
    assert_eq!(skipped["started"], "market-research");
    assert_eq!(skipped["recovered"], false);
    assert!(created_shard(&root).contains("**Event**: STAGE_SKIPPED\n**Stage**: intent-capture\n**Reason**: not this\n**Skip Kind**: conditional-runtime\n"));
    assert_eq!(
        refusal(&state(
            &root,
            &["skip", "intent-capture", "--route", "--reason", "again"],
            &DIRECT
        )),
        "Cannot route skipped stage \"intent-capture\": Current Stage is \"market-research\"."
    );
    assert_eq!(
        refusal(&state(
            &root,
            &["skip", "market-research", "--route"],
            &DIRECT
        )),
        "focus engine state skip --route requires a nonblank --reason <text>."
    );
    assert_eq!(
        refusal(&state(
            &root,
            &["skip", "market-research", "--reason"],
            &DIRECT
        )),
        "--reason expects a value, got end of arguments."
    );
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/market-research/market-trends.md"),
        "# Trends\n",
    );
    let done: serde_json::Value = serde_json::from_str(&ok(&state(
        &root,
        &["complete-workflow", "market-research", "--reason", "enough"],
        &DIRECT,
    )))
    .unwrap();
    assert_eq!(done["status"], "Completed");
    assert_eq!(done["reason"], "enough");
    let registry =
        fs::read_to_string(root.join("focus/spaces/default/intents/intents.json")).unwrap();
    assert!(registry.contains("\"status\": \"complete\""), "{registry}");
    let ledger = created_shard(&root);
    assert!(ledger.contains("**Event**: WORKFLOW_COMPLETED\n**Scope**: feature\n**Details**: Scope: feature, 4 stages completed\n**Reason**: enough\n"), "{ledger}");
    assert!(created_state(&root).contains("- **Status**: Completed\n"));
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. A Construction stage's checkpoints and units and the
/// usage ledger's rollup are each M4's or M5's: upstream weighs them, Focus
/// refuses by name. (The evidence a transition owes to
/// `aidlc-log` records is weighed, a summary confirmation, a pipeline's
/// handoffs and a reviewer's receipt: their refusals are a guard's.)
#[test]
fn what_focus_cannot_weigh_yet_is_refused_by_name() {
    let root = created("by-name", CREATED);
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n",
    );
    let state_path = root.join(CREATED_RECORD).join("focus-state.md");
    let with = |state: &str| fs::write(&state_path, state).unwrap();
    with(&CREATED.replace(
        "- **Summary Confirmation**: off (set by a command)",
        "- **Summary Confirmation**: on (from scope feature)",
    ));
    assert!(
        refusal(&state(&root, &["advance", "intent-capture"], &DIRECT)).starts_with(
            "Refusing to complete \"intent-capture\": its question flow has no intent-capture-questions.md file."
        )
    );
    with(&CREATED.replace("- **Review Override**: none\n", ""));
    assert!(
        refusal(&state(&root, &["advance", "intent-capture"], &DIRECT)).starts_with(
            "Cannot complete \"intent-capture\" because focus-product-lead-agent has not reviewed the current output."
        )
    );
    with(
        &CREATED
            .replace("- [-] intent-capture", "- [x] intent-capture")
            .replace("- [ ] functional-design", "- [-] functional-design")
            .replace(
                "- **Current Stage**: intent-capture",
                "- **Current Stage**: functional-design",
            ),
    );
    assert!(
        refusal(&state(&root, &["advance", "functional-design"], &DIRECT))
            .starts_with("\"functional-design\" is a Construction stage")
    );
    with(CREATED);
    write(
        &root,
        "focus/.focus-sessions/usage-ledger.json",
        "{\"workflows\":{\"w\":{\"byStage\":{\"intent-capture\":{}},\"totals\":{\"tokens\":{\"input\":1}}}}}\n",
    );
    assert!(refusal(&state(&root, &["advance", "intent-capture"], &DIRECT)).ends_with("Focus does not read the usage ledger yet. Set FOCUS_DISABLE_USAGE_TRACKING=1 to complete without the rollup."));
    fs::remove_file(root.join("focus/.focus-sessions/usage-ledger.json")).unwrap();
    assert_eq!(
        refusal(&state(
            &root,
            &[
                "skip",
                "intent-capture",
                "--route",
                "--unit",
                "api",
                "--reason",
                "x"
            ],
            &DIRECT
        )),
        "focus engine state skip takes --route or --unit, not both."
    );
    assert_eq!(created_state(&root), CREATED);
    let _ = fs::remove_dir_all(root);
}

/// The refusal's JSON line as a whole, for a refusal that carries guidance.
fn refusal_json(output: &Output) -> serde_json::Value {
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    serde_json::from_str(text(&output.stderr).trim_end()).unwrap()
}

const QUIET: [(&str, &str); 2] = [
    ("FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS", "1"),
    ("FOCUS_SKIP_HUMAN_PRESENCE_GUARD", "1"),
];

#[test]
fn a_gate_opens_is_rejected_revised_and_approved() {
    let root = created("gate-flow", CREATED);
    let sensors_off = CREATED.replace(
        "- **Summary Confirmation**: off (set by a command)\n",
        "- **Summary Confirmation**: off (set by a command)\n- **Sensors**: off (set by a command)\n- **Revision Count**: 0\n",
    );
    fs::write(
        root.join(CREATED_RECORD).join("focus-state.md"),
        &sensors_off,
    )
    .unwrap();
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n\n```focus-decisions\ndecisions:\n  - id: D1\n    decision: Which store?\n```\n",
    );
    // Usage, and the states each verb admits.
    for verb in ["gate-start", "approve", "reject", "revise"] {
        let usage = refusal(&state(&root, &[verb], &QUIET));
        assert!(
            usage.starts_with(&format!("Usage: focus engine state {verb} <slug>")),
            "{verb}: {usage}"
        );
    }
    assert_eq!(
        refusal(&state(&root, &["revise", "intent-capture"], &QUIET)),
        "Stage intent-capture is in state 'in-progress' but command requires one of: revising"
    );
    // Opened: the row carries what the gate asks with, and a second opening revalidates.
    let opened: serde_json::Value = serde_json::from_str(&ok(&state(
        &root,
        &["gate-start", "intent-capture"],
        &QUIET,
    )))
    .unwrap();
    assert_eq!(opened["new_state"], "awaiting-approval");
    let ledger = created_shard(&root);
    assert!(
        ledger.ends_with("**Event**: STAGE_AWAITING_APPROVAL\n**Stage**: intent-capture\n**Next Stage**: market-research\n**Brief Digest**: none\n**Sensor State**: claim-sources@focus/spaces/default/intents/260105-add-login/ideation/intent-capture/intent-statement.md=not-run; required-sections@focus/spaces/default/intents/260105-add-login/ideation/intent-capture/intent-statement.md=not-run; upstream-coverage@focus/spaces/default/intents/260105-add-login/ideation/intent-capture/intent-statement.md=not-run\n**Open Decisions**: 1\n**Decisions**: D1\n\n---\n"),
        "{ledger}"
    );
    assert!(created_state(&root).contains("- [?] intent-capture"));
    let again: serde_json::Value = serde_json::from_str(&ok(&state(
        &root,
        &["gate-start", "intent-capture"],
        &QUIET,
    )))
    .unwrap();
    assert_eq!(again["revalidated"], true);
    // Rejected with feedback: revising, and the count goes up.
    let rejected: serde_json::Value = serde_json::from_str(&ok(&state(
        &root,
        &["reject", "intent-capture", "--feedback", "needs more"],
        &QUIET,
    )))
    .unwrap();
    assert_eq!(rejected["revision_count"], 1);
    let ledger = created_shard(&root);
    assert!(
        ledger.contains(
            "**Event**: GATE_REJECTED\n**Stage**: intent-capture\n**Feedback**: needs more\n"
        ),
        "{ledger}"
    );
    assert!(ledger.ends_with("**Event**: STAGE_REVISING\n**Stage**: intent-capture\n**Revision count**: 1\n**Feedback**: needs more\n\n---\n"), "{ledger}");
    assert!(created_state(&root).contains("- [R] intent-capture"));
    assert_eq!(
        refusal(&state(
            &root,
            &[
                "reject",
                "intent-capture",
                "--feedback",
                "a",
                "--reason",
                "b"
            ],
            &QUIET
        )),
        "Stage intent-capture is in state 'revising' but command requires one of: awaiting-approval, in-progress"
    );
    // Revised: awaiting again; approved: completed, and the next stage started.
    ok(&state(&root, &["revise", "intent-capture"], &QUIET));
    assert!(created_state(&root).contains("- [?] intent-capture"));
    let approved: serde_json::Value = serde_json::from_str(&ok(&state(
        &root,
        &[
            "approve",
            "intent-capture",
            "--user-input",
            "Approve and continue",
        ],
        &QUIET,
    )))
    .unwrap();
    assert_eq!(approved["completed"], "intent-capture");
    assert_eq!(approved["started"], "market-research");
    let ledger = created_shard(&root);
    assert!(ledger.contains("**Event**: GATE_APPROVED\n**Stage**: intent-capture\n**User Input**: Approve and continue\n**Brief Digest**: none\n**Decisions Accepted Open**: D1\n"), "{ledger}");
    assert!(
        ledger.contains("**Details**: Stage Intent Capture & Framing approved by gate\n"),
        "{ledger}"
    );
    let after = created_state(&root);
    assert!(
        after.contains("- [x] intent-capture \u{2014} EXECUTE\n- [-] market-research"),
        "{after}"
    );
    assert!(after.contains("- **Completed**: 4\n"), "{after}");
    let _ = fs::remove_dir_all(root);
}

/// A decision that is not the person's is refused for the agent, with the
/// guidance upstream prints beside the sentence.
#[test]
fn a_gate_decision_that_is_not_the_persons_is_refused_for_the_agent() {
    let root = created("gate-agent", CREATED);
    let awaiting = CREATED
        .replace(
            "- **Summary Confirmation**: off (set by a command)\n",
            "- **Summary Confirmation**: off (set by a command)\n- **Sensors**: off (set by a command)\n- **Revision Count**: 0\n",
        )
        .replace("- [-] intent-capture", "- [?] intent-capture");
    fs::write(root.join(CREATED_RECORD).join("focus-state.md"), &awaiting).unwrap();
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n",
    );
    let shown = format!(
        "{CREATED_SHARD}\n## Stage Awaiting Approval\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: STAGE_AWAITING_APPROVAL\n**Stage**: intent-capture\n\n---\n"
    );
    let shard_path = root.join(CREATED_RECORD).join("audit/laptop-abc123.md");
    fs::write(&shard_path, &shown).unwrap();
    // The person has not replied since the gate was shown: refused with how to tell, and a
    // turn that was only a command is named.
    let silent = refusal_json(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "Approve"],
        &DIRECT,
    ));
    assert_eq!(silent["agent_guidance"], "question-open");
    let sentence = silent["error"].as_str().unwrap();
    assert!(
        sentence.starts_with(
            "Cannot approve \"intent-capture\" because no new human reply has been received"
        ),
        "{sentence}"
    );
    // No heartbeat at all: the hooks never ran here, and the harness's own step is given.
    assert!(sentence.contains("because Focus's hooks are not running here, so do not ask them to answer again; do this instead: Do not run Focus's hook scripts yourself, do not offer to switch any Focus check off"), "{sentence}");
    assert!(
        sentence.contains("Then run `focus engine orchestrate next` and act on what it returns"),
        "{sentence}"
    );
    // A heartbeat long ago: the reply may have been missed.
    let heartbeat = format!("{CREATED_RECORD}/.focus-engine/hooks-health/record-human-turn.last");
    write(&root, &heartbeat, "2026-01-05T10:00:00Z\n");
    let missed = refusal(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "Approve"],
        &DIRECT,
    ));
    assert!(missed.ends_with("Your answer didn't reach Focus. Please give it once more. If it happens again, type /focus --doctor.\" Never offer to turn a check off for them."), "{missed}");
    // A heartbeat just now: the person has not answered yet.
    write(
        &root,
        &heartbeat,
        &format!(
            "{}\n",
            focus_audit::time::iso_timestamp(std::time::SystemTime::now())
        ),
    );
    let waiting = refusal(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "Approve"],
        &DIRECT,
    ));
    assert!(
        waiting.ends_with(
            "The person has not answered yet: end your turn; their next reply answers it."
        ),
        "{waiting}"
    );
    fs::remove_file(root.join(&heartbeat)).unwrap();
    // Unattended: the switch is named instead.
    let unattended = refusal(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "Approve"],
        &[
            ("FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS", "1"),
            ("FOCUS_UNATTENDED", "1"),
        ],
    ));
    assert!(unattended.contains("FOCUS_UNATTENDED=1 is set, so automated prompt submissions cannot count as a human reply."), "{unattended}");
    let command_turn = format!(
        "{shown}\n## Human Turn\n**Timestamp**: 2026-01-05T10:02:00Z\n**Event**: HUMAN_TURN\n**Session**: s1\n**Reply**: command\n\n---\n"
    );
    fs::write(&shard_path, &command_turn).unwrap();
    let commanded = refusal(&state(
        &root,
        &["reject", "intent-capture", "--feedback", "more"],
        &DIRECT,
    ));
    assert!(
        commanded.contains(
            "The person's message since then was a command to Focus, not a reply to this question"
        ),
        "{commanded}"
    );
    // With a reply since the gate was shown, the input is read: none passed (the person decided
    // something), cancellation boilerplate, a choice the agent attributed to itself.
    let replied = format!(
        "{shown}\n## Human Turn\n**Timestamp**: 2026-01-05T10:02:00Z\n**Event**: HUMAN_TURN\n**Session**: s1\n\n---\n"
    );
    fs::write(&shard_path, &replied).unwrap();
    let none = refusal_json(&state(&root, &["approve", "intent-capture"], &DIRECT));
    assert_eq!(none["agent_guidance"], "person-decided");
    assert!(
        none["error"]
            .as_str()
            .unwrap()
            .starts_with("Cannot approve \"intent-capture\" because no choice was passed"),
        "{none}"
    );
    for (input, says) in [
        (
            "Cancelled",
            "because the reply \"Cancelled\" is cancellation boilerplate",
        ),
        (
            "Answer - conductor's default",
            "(conductor-default: \"Answer - conductor's default\")",
        ),
        (
            "The decision was made by the assistant",
            "(model-authored-decision: \"decision was made by the assistant\")",
        ),
        (
            "approved by the stop hook",
            "(implied-confirmation: \"approved by the stop hook\")",
        ),
    ] {
        let refused = refusal_json(&state(
            &root,
            &["approve", "intent-capture", "--user-input", input],
            &DIRECT,
        ));
        assert_eq!(
            refused["agent_guidance"], "question-open",
            "{input}: {refused}"
        );
        assert!(
            refused["error"].as_str().unwrap().contains(says),
            "{input}: {refused}"
        );
    }
    assert!(
        refusal(&state(
            &root,
            &[
                "reject",
                "intent-capture",
                "--feedback",
                "the assistant rejected this"
            ],
            &DIRECT
        ))
        .contains("(model-authored-decision: \"assistant rejected this\")")
    );
    assert!(created_state(&root).contains("- [?] intent-capture"));
    // The reply admits the decision.
    ok(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "Approve"],
        &DIRECT,
    ));
    assert!(created_state(&root).contains("- [x] intent-capture"));
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. The sensors a gate fires and the units' gates are
/// the sensor tool's and the units' lifecycle (M5): where upstream reads
/// them, Focus refuses by name.
#[test]
fn what_a_gate_cannot_weigh_yet_is_refused_by_name() {
    let root = created("gate-by-name", CREATED);
    let state_path = root.join(CREATED_RECORD).join("focus-state.md");
    let with = |state: &str| fs::write(&state_path, state).unwrap();
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n",
    );
    // Sensors on (the field absent reads as on from the scope).
    let sensors = refusal(&state(&root, &["gate-start", "intent-capture"], &QUIET));
    assert!(
        sensors.starts_with("Opening the gate of \"intent-capture\" fires its gate sensors ("),
        "{sensors}"
    );
    assert!(sensors.ends_with("Focus does not run sensors yet. Turn the ceremony off for this piece of work (`--sensors off` at creation, or FOCUS_DISABLE_SENSORS=1) to open the gate without them."), "{sensors}");
    let quiet_off: Vec<(&str, &str)> = QUIET
        .iter()
        .copied()
        .chain([("FOCUS_DISABLE_SENSORS", "1")])
        .collect();
    ok(&state(&root, &["gate-start", "intent-capture"], &quiet_off));
    // A team's unit gate.
    let team = created_state(&root)
        .replace("- **Revision Count**: 0\n", "")
        .replace(
            "- **Summary Confirmation**: off (set by a command)\n",
            "- **Summary Confirmation**: off (set by a command)\n- **Unit Ownership**: team\n",
        );
    with(&team);
    assert_eq!(
        refusal(&state(
            &root,
            &[
                "approve",
                "intent-capture",
                "--unit",
                "api",
                "--user-input",
                "Approve"
            ],
            &quiet_off
        )),
        "Stage \"intent-capture\" is not a per-unit Construction stage; team unit gates do not apply."
    );
    with(
        &team
            .replace("- [?] intent-capture", "- [x] intent-capture")
            .replace("- [ ] functional-design", "- [?] functional-design"),
    );
    let unit = refusal(&state(
        &root,
        &[
            "approve",
            "functional-design",
            "--unit",
            "api",
            "--user-input",
            "Approve",
        ],
        &quiet_off,
    ));
    assert!(unit.ends_with("Focus does not run units yet."), "{unit}");
    with(&team.replace("- **Unit Ownership**: team\n", ""));
    assert_eq!(
        refusal(&state(
            &root,
            &["gate-start", "intent-capture", "--unit", "api"],
            &quiet_off
        )),
        "--unit gate decisions require Unit Ownership: team."
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn parking_records_whose_stop_it_was_and_unparking_clears_it() {
    let root = created(
        "park",
        &CREATED.replace(
            "## Current Status\n",
            "## Runtime State\n- **Revision Count**: 0\n\n## Current Status\n",
        ),
    );
    let shard_path = root.join(CREATED_RECORD).join("audit/laptop-abc123.md");
    // Nobody spoke: parked by the engine, with no Parked By.
    let parked: serde_json::Value =
        serde_json::from_str(&ok(&state(&root, &["park"], &DIRECT))).unwrap();
    assert_eq!(parked["parked"], true);
    assert_eq!(parked["stage"], "intent-capture");
    let after = created_state(&root);
    assert!(
        after.contains("- **Parked At Stage**: intent-capture\n"),
        "{after}"
    );
    assert!(!after.contains("Parked By"), "{after}");
    assert!(
        created_shard(&root).contains("**Event**: WORKFLOW_PARKED\n**Stage**: intent-capture\n")
    );
    let unparked: serde_json::Value =
        serde_json::from_str(&ok(&state(&root, &["unpark"], &DIRECT))).unwrap();
    assert_eq!(unparked["was_parked"], true);
    assert!(!created_state(&root).contains("Parked"));
    assert!(created_shard(&root).contains("**Event**: WORKFLOW_UNPARKED\n"));
    let again: serde_json::Value =
        serde_json::from_str(&ok(&state(&root, &["unpark"], &DIRECT))).unwrap();
    assert_eq!(again["was_parked"], false);
    assert_eq!(created_shard(&root).matches("WORKFLOW_UNPARKED").count(), 1);
    // The person spoke, and their one approval since did not use the turn up: parked by them.
    fs::write(
        &shard_path,
        format!("{CREATED_SHARD}\n## Human Turn\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: HUMAN_TURN\n**Session**: s1\n\n---\n\n## Gate Approved\n**Timestamp**: 2026-01-05T10:02:00Z\n**Event**: GATE_APPROVED\n**Stage**: intent-capture\n**User Input**: Approve\n\n---\n"),
    )
    .unwrap();
    ok(&state(&root, &["park"], &DIRECT));
    assert!(created_state(&root).contains("- **Parked By**: person\n"));
    ok(&state(&root, &["unpark"], &DIRECT));
    // Unattended, the turn counts for nothing.
    assert!(
        ok(&state(
            &root,
            &["park"],
            &[
                ("FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS", "1"),
                ("FOCUS_UNATTENDED", "1")
            ]
        ))
        .contains("\"parked\":true")
    );
    assert!(!created_state(&root).contains("Parked By"));
    ok(&state(&root, &["unpark"], &DIRECT));
    // The autonomous grant: an unattended run never parks, and nobody's stop is refused.
    let state_path = root.join(CREATED_RECORD).join("focus-state.md");
    let autonomous = created_state(&root).replace(
        "## Runtime State\n",
        "## Runtime State\n- **Construction Autonomy Mode**: autonomous\n",
    );
    fs::write(&state_path, &autonomous).unwrap();
    assert!(
        refusal(&state(
            &root,
            &["park"],
            &[
                ("FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS", "1"),
                ("FOCUS_UNATTENDED", "1")
            ]
        ))
        .starts_with("Refusing to park: FOCUS_UNATTENDED=1 is set")
    );
    ok(&state(&root, &["park"], &DIRECT));
    fs::write(&shard_path, CREATED_SHARD).unwrap();
    ok(&state(&root, &["unpark"], &DIRECT));
    assert!(
        refusal(&state(&root, &["park"], &DIRECT))
            .starts_with("Refusing to park: Construction Autonomy Mode is autonomous and no reply")
    );
    // Deliberate deviation: a team's units are parked from the checkout's unit scope stamp (M4).
    fs::write(
        &state_path,
        CREATED.replace(
            "## Current Status\n",
            "## Runtime State\n- **Unit Ownership**: team\n\n## Current Status\n",
        ),
    )
    .unwrap();
    assert!(refusal(&state(&root, &["park"], &DIRECT)).ends_with("Focus does not run units yet."));
    assert!(
        refusal(&state(&root, &["unpark"], &DIRECT)).ends_with("Focus does not run units yet.")
    );
    let _ = fs::remove_dir_all(root);
}

fn jump(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    command
        .args(["engine", "jump"])
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

#[test]
fn a_jump_resolves_and_moves_the_work() {
    let root = created("jump", CREATED);
    assert_eq!(
        refusal(&jump(&root, &[], &[])),
        "Unknown subcommand: undefined. Valid: resolve, execute, reopen"
    );
    assert!(
        refusal(&jump(&root, &["resolve"], &[])).starts_with("Usage: resolve --stage <slug|#>")
    );
    let resolved: serde_json::Value =
        serde_json::from_str(&ok(&jump(&root, &["resolve", "--stage", "1.3"], &[]))).unwrap();
    assert_eq!(resolved["target_slug"], "feasibility");
    assert_eq!(resolved["direction"], "forward");
    assert_eq!(
        resolved["affected_stages"],
        serde_json::json!(["market-research"])
    );
    assert_eq!(resolved["valid"], true);
    assert_eq!(
        refusal(&jump(
            &root,
            &["resolve", "--stage", "reverse-engineering"],
            &[]
        )),
        "Stage \"reverse-engineering\" is skipped for scope \"feature\". Use `next --stage reverse-engineering`, which handles a stage the plan skips."
    );
    let phase: serde_json::Value =
        serde_json::from_str(&ok(&jump(&root, &["resolve", "--phase", "2"], &[]))).unwrap();
    assert_eq!(phase["target_slug"], "practices-discovery");
    // Forward: the stages passed over are skipped, the phase rows follow, the notice says the way back.
    let moved: serde_json::Value = serde_json::from_str(&ok(&jump(
        &root,
        &[
            "execute",
            "--target",
            "requirements-analysis",
            "--direction",
            "forward",
        ],
        &[],
    )))
    .unwrap();
    assert_eq!(
        moved["stages_skipped"],
        serde_json::json!([
            "market-research",
            "feasibility",
            "scope-definition",
            "team-formation",
            "rough-mockups",
            "approval-handoff",
            "practices-discovery",
            "intent-capture"
        ])
    );
    assert_eq!(
        moved["notice"],
        "Moved to Requirements Analysis; skipped Intent Capture & Framing, Market Research, Feasibility & Constraints, Scope Definition, Team Formation, Rough Mockups, Approval & Handoff and Practices Discovery. You can go back to Intent Capture & Framing any time."
    );
    let after = created_state(&root);
    assert!(
        after.contains("- [S] intent-capture \u{2014} EXECUTE\n- [S] market-research"),
        "{after}"
    );
    assert!(after.contains("- [-] requirements-analysis"), "{after}");
    assert!(
        after.contains("- **Ideation**: Verified\n- **Inception**: Active"),
        "{after}"
    );
    assert!(after.contains("- **Lifecycle Phase**: INCEPTION\n- **Current Stage**: requirements-analysis\n- **Next Stage**: user-stories"), "{after}");
    let ledger = created_shard(&root);
    assert!(ledger.contains("**Event**: STAGE_SKIPPED\n**Stage**: market-research\n**Reason**: Skipped by jump to requirements-analysis (forward)\n**Skip Kind**: jump\n"), "{ledger}");
    assert!(ledger.contains("**Event**: PHASE_COMPLETED\n**From phase**: ideation\n**To phase**: inception\n**Stages completed**: 3\n**Details**: Phase boundary crossed via forward jump\n"), "{ledger}");
    assert!(ledger.contains("**Event**: STAGE_JUMPED\n**Direction**: FORWARD\n**Source**: intent-capture\n**Target**: requirements-analysis\n**Scope**: feature\n**Details**: FORWARD jump from intent-capture to requirements-analysis (2.3). Scope: feature.\n"), "{ledger}");
    // Both rows of the jump carry the source baseline, and its listing is kept beside the record under its hash.
    let baseline = ledger
        .rsplit("**Source Baseline**: ")
        .next()
        .and_then(|rest| rest.strip_suffix("\n\n---\n"))
        .unwrap()
        .to_owned();
    assert!(ledger.ends_with(&format!("**Event**: STAGE_STARTED\n**Stage**: requirements-analysis\n**Agent**: focus-product-agent\n**Source Baseline**: {baseline}\n\n---\n")), "{ledger}");
    assert!(
        ledger.contains(&format!(
            "(2.3). Scope: feature.\n**Source Baseline**: {baseline}\n\n---\n"
        )),
        "{ledger}"
    );
    let hash = baseline.strip_prefix("sha256:").unwrap();
    assert_eq!(hash.len(), 64, "{baseline}");
    assert!(
        root.join(CREATED_RECORD)
            .join(".focus-engine/source-review/code-generation")
            .join(format!("baseline-{}.tsv", &hash[..12]))
            .is_file(),
        "{baseline}"
    );
    // Backward: the target and what follows are reset, and the artifacts and reviews behind are named.
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n\n## Review\n- R-01 New: x\n",
    );
    write(
        &root,
        &format!("{CREATED_RECORD}/inception/requirements-analysis/requirements.md"),
        "# R\n\n## Review\n- R-01 New: y\n",
    );
    let back: serde_json::Value = serde_json::from_str(&ok(&jump(
        &root,
        &[
            "execute",
            "--target",
            "intent-capture",
            "--direction",
            "backward",
        ],
        &[],
    )))
    .unwrap();
    // The skipped stages and the one in progress are reset; pending ones are left as they are.
    assert_eq!(back["stages_reset"].as_array().unwrap().len(), 9);
    assert_eq!(
        back["notice"],
        "Moved back to Intent Capture & Framing. You can return to Requirements Analysis any time."
    );
    let ledger = created_shard(&root);
    // The target's declared artifacts, present or not; the existing ones behind it; the reviews behind it (the target's own section is not behind).
    assert!(ledger.contains("**Changed Upstream Artifacts**: [\"focus/spaces/default/intents/260105-add-login/ideation/intent-capture/intent-capture-questions.md\",\"focus/spaces/default/intents/260105-add-login/ideation/intent-capture/intent-statement.md\",\"focus/spaces/default/intents/260105-add-login/ideation/intent-capture/stakeholder-map.md\"]\n**Invalidated Downstream Artifacts**: [\"focus/spaces/default/intents/260105-add-login/inception/requirements-analysis/requirements.md\"]\n**Invalidated Downstream Reviews**: [\"focus/spaces/default/intents/260105-add-login/inception/requirements-analysis/requirements.md#Review\"]\n"), "{ledger}");
    let after = created_state(&root);
    assert!(
        after.contains("- [-] intent-capture \u{2014} EXECUTE\n- [ ] market-research"),
        "{after}"
    );
    assert!(
        after.contains("- **Ideation**: Active\n- **Inception**: Pending"),
        "{after}"
    );
    // Deliberate deviations: the directive marker and a bound session are M4/M5.
    assert!(
        refusal(&jump(
            &root,
            &[
                "execute",
                "--target",
                "feasibility",
                "--direction",
                "backward"
            ],
            &[("FOCUS_SESSION_OVERRIDE", "s1")]
        ))
        .contains("pending person lines")
    );
    write(
        &root,
        &format!("{CREATED_RECORD}/.focus-engine/active-directive.json"),
        "{}\n",
    );
    assert!(
        refusal(&jump(
            &root,
            &[
                "execute",
                "--target",
                "feasibility",
                "--direction",
                "forward"
            ],
            &[]
        ))
        .ends_with("Focus does not read it yet.")
    );
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. A registry row whose `repos` is not a list of
/// names: upstream falls over on it with what its runtime says of the
/// value, and takes an empty name for the project itself. A jump walks the
/// repositories as source: it refuses the row, and moves nothing.
#[test]
fn a_jump_refuses_repositories_that_are_not_names() {
    for (name, repos) in [
        ("number", "[\"app\", 7]"),
        ("text", "\"app\""),
        ("empty", "[\"app\", \"\"]"),
    ] {
        let root = created(&format!("jump-repos-{name}"), CREATED);
        write(
            &root,
            "focus/spaces/default/intents/intents.json",
            &format!(
                "[\n  {{\n    \"uuid\": \"01a138fa-d37d-787d-af4a-966efbb0b6fd\",\n    \"slug\": \"add-login\",\n    \"dirName\": \"260105-add-login\",\n    \"scope\": \"feature\",\n    \"status\": \"in-flight\",\n    \"repos\": {repos}\n  }}\n]\n"
            ),
        );
        let said = refusal(&jump(
            &root,
            &[
                "execute",
                "--target",
                "feasibility",
                "--direction",
                "forward",
            ],
            &[],
        ));
        assert!(
            said.ends_with("intents.json lists the repositories of 260105-add-login as something that is not a list of names. Fix the row's `repos`: a list of the member repositories' directory names, or nothing for a project that is one repository."),
            "{repos}: {said}"
        );
        assert_eq!(created_state(&root), CREATED, "{repos}");
        let _ = fs::remove_dir_all(root);
    }
}

/// Deliberate deviation. A backward jump names the reviews it invalidates.
/// Where the list of findings of a stage behind the jump holds what Focus
/// refuses to read (a decision a gate's row holds, edited by hand, with a
/// list where a text belongs) upstream reads it as text and jumps; Focus
/// refuses the jump with what is wrong, and moves nothing.
#[test]
fn a_backward_jump_refuses_a_review_it_cannot_read() {
    use sha2::{Digest, Sha256};
    let mut state = CREATED.replace("- [-] intent-capture ", "- [x] intent-capture ");
    for slug in [
        "market-research",
        "feasibility",
        "scope-definition",
        "team-formation",
        "rough-mockups",
    ] {
        state = state.replace(&format!("- [ ] {slug} "), &format!("- [x] {slug} "));
    }
    let state = state
        .replace("- [ ] approval-handoff ", "- [-] approval-handoff ")
        .replace(
            "- **Current Stage**: intent-capture",
            "- **Current Stage**: approval-handoff",
        )
        .replace(
            "- **In Progress**: intent-capture",
            "- **In Progress**: approval-handoff",
        );
    let root = created("jump-review", &state);
    let wires = format!("{CREATED_RECORD}/ideation/rough-mockups/wireframes.md");
    write(&root, &wires, "# Wireframes\n");
    // A review of the stage behind the jump, with one finding.
    let reviewer = "focus-product-lead-agent";
    let fingerprint = format!("sha256:{}", "c".repeat(64));
    let request = format!("review:{}", "1".repeat(32));
    let record = format!(
        "{{\n  \"version\": 1,\n  \"stage\": \"rough-mockups\",\n  \"unit\": null,\n  \"workflow\": null,\n  \"attempt\": \"0000000000000000\",\n  \"iteration\": 1,\n  \"reviewer\": \"{reviewer}\",\n  \"verdict\": \"NOT-READY\",\n  \"request_id\": \"{request}\",\n  \"request_challenge\": null,\n  \"artifact_fingerprint\": \"{fingerprint}\",\n  \"source_fingerprint\": null,\n  \"unit_source_fingerprint\": null,\n  \"findings\": [\n    {{\n      \"id\": \"R-01\",\n      \"severity\": \"Major\",\n      \"location\": \"wireframes.md > Home\",\n      \"finding\": \"It names no user\",\n      \"required_action\": \"Name the user\",\n      \"status\": \"New\"\n    }}\n  ],\n  \"body\": \"It reads well.\\n\",\n  \"recorded_at\": \"2026-01-05T10:00:00Z\"\n}}\n"
    );
    let record_rel = ".focus-engine/reviews/rough-mockups/stage/0000000000000000/1.json";
    write(&root, &format!("{CREATED_RECORD}/{record_rel}"), &record);
    let digest: String = Sha256::digest(record.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let scope = format!("**Stage**: rough-mockups\n**Reviewer**: {reviewer}\n**Iteration**: 1\n");
    let reviewed = format!(
        "\n## Review Requested\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: REVIEW_REQUESTED\n{scope}**Artifact Fingerprint**: {fingerprint}\n**Request Id**: {request}\n\n---\n\n## Review Completed\n**Timestamp**: 2026-01-05T10:02:00Z\n**Event**: REVIEW_COMPLETED\n{scope}**Verdict**: NOT-READY\n**Request Fingerprint**: {fingerprint}\n**Artifact Fingerprint**: {fingerprint}\n**Request Id**: {request}\n**Review Record**: {record_rel}\n**Review Record Digest**: sha256:{digest}\n\n---\n"
    );
    let shard_path = root.join(CREATED_RECORD).join("audit/laptop-abc123.md");
    let back = [
        "execute",
        "--target",
        "team-formation",
        "--direction",
        "backward",
    ];
    // The decision as a gate writes it is read, and the jump would go
    // through; with a list where the finding's id belongs it is refused.
    let decided = |id: &str| {
        format!(
            "\n## Gate Approved\n**Timestamp**: 2026-01-05T10:09:00Z\n**Event**: GATE_APPROVED\n**Stage**: rough-mockups\n**Review Finding Dispositions**: {{\"version\":1,\"dispositions\":[{{\"artifact\":\"{wires}\",\"id\":{id},\"fingerprint\":\"sha256:{}\",\"status\":\"Accepted risk\"}}]}}\n\n---\n",
            "b".repeat(64)
        )
    };
    fs::write(
        &shard_path,
        format!("{CREATED_SHARD}{reviewed}{}", decided("[\"R-01\"]")),
    )
    .unwrap();
    let said = refusal(&jump(&root, &back, &[]));
    assert!(
        said.contains("A backward jump names the reviews it invalidates, and the review of \"rough-mockups\" cannot be read: the Review Finding Dispositions field holds a decision that is not written as text: it is not read."),
        "{said}"
    );
    assert_eq!(created_state(&root), state);
    // Written as text, the same decision is read, and the jump names the
    // review among what it invalidates.
    fs::write(
        &shard_path,
        format!("{CREATED_SHARD}{reviewed}{}", decided("\"R-01\"")),
    )
    .unwrap();
    let moved = jump(&root, &back, &[]);
    assert!(moved.status.success(), "{}", text(&moved.stderr));
    assert!(
        fs::read_to_string(&shard_path)
            .unwrap()
            .contains(&format!("{wires}#Review")),
        "{}",
        fs::read_to_string(&shard_path).unwrap()
    );
    let _ = fs::remove_dir_all(root);
}

/// `lookup` answers from the stage graph and the scopes, in one line, and
/// only `next-stage` reads the state.
///
/// Deliberate deviation: upstream looks a scope and a phase up in plain
/// objects, so the names every object has (`constructor`, `__proto__`,
/// `toString`) are found there, and each command then falls over or
/// answers from the state alone. Focus has no scope and no phase by those
/// names.
#[test]
fn lookup_answers_from_the_graph_and_the_scopes() {
    let root = created("lookup", CREATED);
    let before = created_shard(&root);
    let look = |args: &[&str]| state(&root, &[&["lookup"], args].concat(), &[]);
    assert_eq!(ok(&look(&["phase-of", "intent-capture"])), "ideation\n");
    assert_eq!(ok(&look(&["phase-of", "1.1"])), "ideation\n");
    assert_eq!(ok(&look(&["number-of", "intent-capture"])), "1.1\n");
    assert_eq!(
        ok(&look(&["agent-for", "intent-capture"])),
        "focus-product-agent\n"
    );
    assert_eq!(
        ok(&look(&["next-stage", "intent-capture", "feature"])),
        "market-research\n"
    );
    assert_eq!(
        ok(&look(&["next-stage", "intent-capture", "nope"])),
        "none\n"
    );
    assert_eq!(
        ok(&look(&["first-in-phase", "IDEATION", "feature"])),
        "intent-capture\n"
    );
    assert_eq!(ok(&look(&["first-in-phase", "nope", "feature"])), "none\n");
    assert_eq!(
        ok(&look(&["validate-stage", "nope"])),
        "{\"valid\":false,\"input\":\"nope\"}\n"
    );
    assert_eq!(
        ok(&look(&["validate-phase", "3"])),
        "{\"valid\":true,\"canonical\":\"construction\",\"number\":\"3\",\"display\":\"CONSTRUCTION\"}\n"
    );
    let stages: serde_json::Value =
        serde_json::from_str(&ok(&look(&["stages-in-scope", "bugfix"]))).unwrap();
    let stages = stages.as_array().unwrap();
    assert!(stages.len() > 20);
    assert!(stages.iter().any(|stage| stage["action"] == "EXECUTE"));
    assert!(stages.iter().any(|stage| stage["action"] == "SKIP"));
    assert_eq!(stages[0]["phase"], "initialization");
    // Nothing was written: a question leaves no row.
    assert_eq!(created_shard(&root), before);
    assert_eq!(refusal(&look(&["phase-of", "nope"])), "Unknown stage: nope");
    assert_eq!(
        refusal(&look(&["stages-in-scope", "nope"])),
        "Unknown scope: nope"
    );
    assert_eq!(
        refusal(&look(&["nope"])),
        "Unknown lookup subcommand: nope. Valid: phase-of, next-stage, agent-for, number-of, stages-in-scope, first-in-phase, validate-stage, validate-phase"
    );
    assert_eq!(
        refusal(&look(&[])),
        "Usage: focus engine state lookup <subcommand> [args...]"
    );
    // The deviation: the names of an object are names of nothing.
    for name in ["constructor", "__proto__"] {
        assert_eq!(
            ok(&look(&["validate-phase", name])),
            format!("{{\"valid\":false,\"input\":\"{name}\"}}\n")
        );
        assert_eq!(
            refusal(&look(&["stages-in-scope", name])),
            format!("Unknown scope: {name}")
        );
        assert_eq!(ok(&look(&["next-stage", "intent-capture", name])), "none\n");
    }
    assert_eq!(
        ok(&look(&["first-in-phase", "ideation", "toString"])),
        "none\n"
    );
    let _ = fs::remove_dir_all(root);
}

/// The two verbs that only leave a row. `acknowledge-compaction` records
/// the person's choice once, while a compaction is waiting; and
/// `practices-event` writes the fields in the order an object of
/// upstream's lists them (a key that is a number first).
#[test]
fn the_verbs_that_only_leave_a_row() {
    let root = created("rows", CREATED);
    let shard_path = root.join(CREATED_RECORD).join("audit/laptop-abc123.md");
    let waiting = "No pending compaction to acknowledge (latest SESSION_COMPACTED already followed by stage activity or recovery).";
    let ack = |choice: &[&str]| state(&root, &[&["acknowledge-compaction"], choice].concat(), &[]);
    assert_eq!(
        refusal(&ack(&[])),
        "Usage: focus engine state acknowledge-compaction --choice <continue|review|restart>"
    );
    assert_eq!(
        refusal(&ack(&["--choice", "nope"])),
        "Invalid --choice: nope. Valid: continue, review, restart"
    );
    assert_eq!(refusal(&ack(&["--choice", "review"])), waiting);
    fs::write(
        &shard_path,
        format!(
            "{CREATED_SHARD}\n## Session Compacted\n**Timestamp**: 2026-01-05T10:05:00Z\n**Event**: SESSION_COMPACTED\n**Stage**: intent-capture\n\n---\n"
        ),
    )
    .unwrap();
    assert_eq!(
        ok(&ack(&["--choice", "review"])),
        "{\"acknowledged\":true,\"choice\":\"review\",\"current_stage\":\"intent-capture\"}\n"
    );
    let after = created_shard(&root);
    assert!(
        after.contains(
            "**Event**: RECOVERY_COMPLETED\n**Choice**: review\n**Current Stage**: intent-capture\n"
        ),
        "{after}"
    );
    // Once: the recovery it recorded closes the window.
    assert_eq!(refusal(&ack(&["--choice", "review"])), waiting);

    let says = |args: &[&str]| state(&root, &[&["practices-event"], args].concat(), &[]);
    assert_eq!(
        refusal(&says(&[])),
        "Usage: focus engine state practices-event --type <discovered|override|empty> [--field \"Key: Value\"]..."
    );
    assert_eq!(
        refusal(&says(&["--type", "affirmed"])),
        "PRACTICES_AFFIRMED is reserved for practices-promote so the audit receipt cannot be minted without successful memory promotion."
    );
    assert_eq!(
        refusal(&says(&["--type", "nope"])),
        "Invalid --type: nope. Must be discovered, override, or empty."
    );
    assert_eq!(
        ok(&says(&[
            "--type",
            "override",
            "--field",
            "b: 1",
            "--field",
            "a: two",
            "--field",
            "no colon",
            "--field",
            "__proto__: gone",
            "--field",
            "b:  last  ",
        ])),
        "{\"emitted\":\"PRACTICES_OVERRIDE\",\"fields_count\":2}\n"
    );
    let after = created_shard(&root);
    assert!(
        after.contains("**Event**: PRACTICES_OVERRIDE\n**b**: last\n**a**: two\n"),
        "{after}"
    );
    // A field is named by a letter first; of two that are not, the one
    // refused is the first an object lists: the number, by its value.
    assert_eq!(
        refusal(&says(&[
            "--type", "override", "--field", "-1: x", "--field", "10: ten", "--field", "2: two",
        ])),
        "Invalid audit field key: \"2\". Field keys must match /^[A-Za-z][A-Za-z0-9 ._()/-]*$/ so they remain one Markdown label on one physical line."
    );
    for (kind, event) in [
        ("discovered", "PRACTICES_DISCOVERED"),
        ("empty", "PRACTICES_SECTION_EMPTY"),
    ] {
        assert_eq!(
            ok(&says(&["--type", kind])),
            format!("{{\"emitted\":\"{event}\",\"fields_count\":0}}\n")
        );
    }
    let _ = fs::remove_dir_all(root);
}

/// `reuse-artifact` records what becomes of artifacts a stage already has.
/// For a stage run by itself the row carries the run, and is taken only
/// while that run is open.
///
/// Deliberate deviation: keeping the CodeKB of a `reverse-engineering` run
/// by itself. After the repository and the store are checked, upstream
/// asks whether the store is current, from the scope its scan recorded;
/// Focus reads no scan yet and refuses that question by name.
#[test]
fn reuse_artifact_records_what_becomes_of_what_a_stage_has() {
    let root = created("reuse", CREATED);
    let reuse = |args: &[&str]| state(&root, &[&["reuse-artifact"], args].concat(), &[]);
    assert_eq!(
        refusal(&reuse(&[])),
        "Usage: focus engine state reuse-artifact <slug> --decision <keep|modify|redo> --artifacts <csv> [--repo <repo>] [--single]"
    );
    assert_eq!(
        refusal(&reuse(&["intent-capture", "--artifacts", "a.md"])),
        "Missing --decision <keep|modify|redo>"
    );
    assert_eq!(
        refusal(&reuse(&["intent-capture", "--decision", "keep"])),
        "Missing --artifacts <csv>"
    );
    assert_eq!(
        refusal(&reuse(&[
            "intent-capture",
            "--decision",
            "nope",
            "--artifacts",
            "a.md"
        ])),
        "Invalid decision: nope. Must be keep, modify, or redo."
    );
    assert_eq!(
        refusal(&reuse(&[
            "nope",
            "--decision",
            "keep",
            "--artifacts",
            "a.md"
        ])),
        "Unknown stage: nope"
    );
    assert_eq!(
        ok(&reuse(&[
            "intent-capture",
            "--decision",
            "modify",
            "--artifacts",
            "a.md, b.md",
            "--repo",
            "app",
        ])),
        "{\"slug\":\"intent-capture\",\"decision\":\"modify\",\"artifacts\":\"a.md, b.md\",\"repo\":\"app\",\"emitted\":\"ARTIFACT_REUSED\"}\n"
    );
    assert!(
        created_shard(&root).contains(
            "**Event**: ARTIFACT_REUSED\n**Stage**: intent-capture\n**Decision**: modify\n**Artifacts**: a.md, b.md\n**Repo**: app\n"
        ),
        "{}",
        created_shard(&root)
    );
    // With no repository registered, the project's own store goes by the
    // folder's name: that name as `--repo` is the root, and is not recorded.
    let folder = root.file_name().unwrap().to_str().unwrap().to_owned();
    assert_eq!(
        ok(&reuse(&[
            "reverse-engineering",
            "--decision",
            "keep",
            "--artifacts",
            "a.md",
            "--repo",
            &folder,
        ])),
        "{\"slug\":\"reverse-engineering\",\"decision\":\"keep\",\"artifacts\":\"a.md\",\"emitted\":\"ARTIFACT_REUSED\"}\n"
    );
    // A stage run by itself.
    let alone = [
        "market-research",
        "--decision",
        "keep",
        "--artifacts",
        "a.md",
        "--single",
    ];
    assert_eq!(
        refusal(&reuse(&alone)),
        "Cannot record isolated reuse for \"market-research\": run next --stage market-research --single first."
    );
    let shard_path = root.join(CREATED_RECORD).join("audit/laptop-abc123.md");
    let started = |stage: &str| {
        format!(
            "\n## Stage Started\n**Timestamp**: 2026-01-05T11:00:00Z\n**Event**: STAGE_STARTED\n**Stage**: {stage}\n**Workflow**: single-stage:{stage}\n\n---\n"
        )
    };
    let before = created_shard(&root);
    fs::write(
        &shard_path,
        format!("{before}{}", started("market-research")),
    )
    .unwrap();
    assert_eq!(
        ok(&reuse(&alone)),
        "{\"slug\":\"market-research\",\"decision\":\"keep\",\"artifacts\":\"a.md\",\"single\":true,\"emitted\":\"ARTIFACT_REUSED\"}\n"
    );
    assert!(
        created_shard(&root)
            .contains("**Artifacts**: a.md\n**Workflow**: single-stage:market-research\n")
    );
    // The CodeKB kept by a reverse-engineering run by itself: the store
    // must be the record's own, and then the deviation.
    fs::write(
        &shard_path,
        format!("{before}{}", started("reverse-engineering")),
    )
    .unwrap();
    let store = format!("focus/spaces/default/codekb/{folder}/");
    let kept = |artifacts: &str, more: &[&str]| {
        reuse(
            &[
                &[
                    "reverse-engineering",
                    "--decision",
                    "keep",
                    "--artifacts",
                    artifacts,
                    "--single",
                ],
                more,
            ]
            .concat(),
        )
    };
    assert_eq!(
        refusal(&kept(&store, &["--repo", "app"])),
        "Cannot record isolated reverse-engineering reuse: this intent has no registered repo identity; omit --repo."
    );
    assert_eq!(
        refusal(&kept("elsewhere/", &[])),
        format!(
            "Cannot record isolated reverse-engineering reuse: --artifacts must be the exact current store {store}."
        )
    );
    let rows = created_shard(&root).matches("ARTIFACT_REUSED").count();
    // (The store may be given with spaces around it, or with the other
    // slash: it is the same store.)
    for given in [format!("  {store} "), store.replace('/', "\\")] {
        assert!(
            refusal(&kept(&given, &[]))
                .starts_with("Keeping the CodeKB of a reverse-engineering run by itself asks"),
            "{given}"
        );
    }
    assert_eq!(
        refusal(&kept(&store, &[])),
        format!(
            "Keeping the CodeKB of a reverse-engineering run by itself asks whether the store \"{folder}\" is current, which is read off the scope its scan recorded: Focus does not read that yet (the scan arrives with a later slice)."
        )
    );
    assert_eq!(
        created_shard(&root).matches("ARTIFACT_REUSED").count(),
        rows
    );
    let _ = fs::remove_dir_all(root);
}

/// What a gate records of a review's findings: an approval accepts the risk
/// of each one still open, and a request for changes says of the one it
/// names that it is none.
///
/// Deliberate deviation: a decision a gate's row holds, edited by hand so a
/// list stands where the finding's id belongs. Upstream reads the list as
/// text and decides the gate; Focus refuses the approval, and the request
/// for changes that names a finding, and records neither.
/// The created record's state with the settings a creation writes.
fn running() -> String {
    let state = CREATED.replace(
        "## Phase Progress\n",
        "## Runtime State\n- **Revision Count**: 0\n- **Construction Checkpoints**: enabled\n- **Construction Iteration**: unit-major\n- **Construction Execution**: serial\n\n## Phase Progress\n",
    );
    assert_ne!(state, CREATED);
    state
}

/// How Construction runs is the person's to set. Before Construction a
/// setting is a preference anyone records; during it, a change is theirs:
/// asked for since the last decision, or chosen at a policy question.
#[test]
fn how_construction_runs_is_the_persons_to_set() {
    let root = created("policy", &running());
    let set = |args: &[&str], env: &[(&str, &str)]| state(&root, args, env);
    assert_eq!(
        ok(&set(&["set-skeleton-stance", "scope-dependent"], &[])),
        "{\"updated\":true,\"skeleton_stance\":\"scope-dependent\"}\n"
    );
    assert!(created_state(&root).contains("- **Skeleton Stance**: scope-dependent\n"));
    assert_eq!(
        refusal(&set(&["set-skeleton-stance", "maybe"], &[])),
        "Invalid skeleton stance \"maybe\". Valid: on, off, scope-dependent."
    );
    // Before Construction: recorded, with what it was.
    assert_eq!(
        ok(&set(&["set-construction-iteration", "stage-major"], &[])),
        "{\"updated\":true,\"construction_iteration\":\"stage-major\",\"notice\":\"Construction now goes stage by stage (it built one Unit at a time). You can switch back any time.\"}\n"
    );
    assert!(created_shard(&root).contains(
        "**Event**: CONSTRUCTION_POLICY_SET\n**Field**: Construction Iteration\n**Value**: stage-major\n**Previous Value**: unit-major\n**Construction Iteration**: stage-major\n**Construction Checkpoints**: enabled\n"
    ));
    assert_eq!(
        ok(&set(&["set-construction-iteration", "stage-major"], &[])),
        "{\"updated\":true,\"construction_iteration\":\"stage-major\",\"notice\":\"Construction already goes stage by stage.\"}\n"
    );
    assert_eq!(
        created_shard(&root)
            .matches("CONSTRUCTION_POLICY_SET")
            .count(),
        1
    );
    assert_eq!(
        ok(&set(&["set-construction-execution", "swarm"], &[])),
        "{\"updated\":true,\"field\":\"Construction Execution\",\"value\":\"swarm\",\"notice\":\"Construction now builds the Units in parallel (it built the Units one after another). You can switch back any time.\"}\n"
    );
    assert_eq!(
        refusal(&set(&["set-construction-iteration", "unit-major"], &[])),
        "Select Construction Execution: serial before switching to unit-major iteration."
    );
    assert_eq!(
        ok(&set(&["set-construction-checkpoints", "disabled"], &[])),
        "{\"updated\":true,\"field\":\"Construction Checkpoints\",\"value\":\"disabled\",\"notice\":\"Construction checkpoints (a stop after each Unit for you to check and approve it) are off for this work now (they were on). You can switch back any time.\"}\n"
    );
    assert_eq!(
        refusal(&set(&["set-construction-execution", "serial"], &[])),
        "Enable Construction Checkpoints before selecting Construction Execution."
    );
    let _ = fs::remove_dir_all(&root);

    // During Construction.
    let built = running().replace(
        "- **Lifecycle Phase**: IDEATION",
        "- **Lifecycle Phase**: CONSTRUCTION",
    );
    let root = created("policy-built", &built);
    let set = |args: &[&str], env: &[(&str, &str)]| state(&root, args, env);
    let recovery = "When the person asks for this Construction change, run focus engine state set-construction-checkpoints, set-construction-execution, or set-construction-iteration with the value they asked for, then say its notice line to them. Do not ask them to confirm it, and never change it on your own.";
    assert_eq!(
        refusal(&set(&["set-construction-checkpoints", "disabled"], &[])),
        format!(
            "No message from the person since the last decision asks to change Construction Checkpoints. Change it only when they ask for it, never to clear a refusal or on your own. {recovery}"
        )
    );
    assert!(created_state(&root).contains("- **Construction Checkpoints**: enabled\n"));
    // (What it already is needs nobody.)
    assert_eq!(
        ok(&set(&["set-construction-checkpoints", "enabled"], &[])),
        "{\"updated\":true,\"field\":\"Construction Checkpoints\",\"value\":\"enabled\",\"notice\":\"Construction checkpoints (a stop after each Unit for you to check and approve it) are already on for this work.\"}\n"
    );
    // The person asked: their words go on the row.
    let shard_rel = format!("{CREATED_RECORD}/audit/laptop-abc123.md");
    let asked = format!(
        "{CREATED_SHARD}\n## Human Turn\n**Timestamp**: 2026-01-05T10:09:00Z\n**Event**: HUMAN_TURN\n**Session**: s1\n\n---\n"
    );
    write(&root, &shard_rel, &asked);
    write(
        &root,
        &format!("{CREATED_RECORD}/.focus-engine/gate-words/s1.json"),
        &format!(
            "{{\"version\":1,\"session\":\"s1\",\"shard\":\"{shard_rel}\",\"dropped\":0,\"messages\":[{{\"offset\":{},\"text\":\"turn the checkpoints off\"}}]}}",
            CREATED_SHARD.len() + 20
        ),
    );
    assert_eq!(
        refusal(&set(
            &["set-construction-checkpoints", "disabled"],
            &[("FOCUS_UNATTENDED", "1")]
        )),
        format!(
            "An unattended run does not change Construction Checkpoints: it changes when the person asks for it. {recovery}"
        )
    );
    assert!(
        ok(&set(&["set-construction-checkpoints", "disabled"], &[]))
            .contains("are off for this work now (they were on)")
    );
    assert!(
        created_shard(&root).contains(
            "**Field**: Construction Checkpoints\n**Value**: disabled\n**Previous Value**: enabled\n**Construction Iteration**: unit-major\n**Construction Checkpoints**: disabled\n**Person Reply**: turn the checkpoints off\n"
        ),
        "{}",
        created_shard(&root)
    );
    let _ = fs::remove_dir_all(&root);

    // Chosen at a policy question: the receipt is enough, once, for its value.
    let root = created("policy-receipt", &built);
    let set = |args: &[&str], env: &[(&str, &str)]| state(&root, args, env);
    write(
        &root,
        &shard_rel,
        &format!(
            "{CREATED_SHARD}\n## Construction Policy Recorded\n**Timestamp**: 2026-01-05T10:08:00Z\n**Event**: CONSTRUCTION_POLICY_RECORDED\n**Checkpoint**: Construction Policy\n**Field**: Construction Iteration\n**Value**: stage-major\n**Session**: s1\n**User Input**: Approve\n\n---\n"
        ),
    );
    assert!(
        ok(&set(
            &["set-construction-iteration", "stage-major"],
            &[("FOCUS_UNATTENDED", "1")]
        ))
        .contains("Construction now goes stage by stage")
    );
    assert!(!created_shard(&root).contains("Person Reply"));
    assert!(
        refusal(&set(&["set-construction-iteration", "unit-major"], &[]))
            .starts_with("No message from the person since the last decision asks to change Construction Iteration.")
    );
    let _ = fs::remove_dir_all(&root);
}

/// Deliberate deviation. Upstream looks the value a setting had up in a
/// plain object, so a name every object has is found there with nothing to
/// say, and the notice reads `(undefined)`. Focus knows no such value.
#[test]
fn a_setting_that_was_an_objects_name_is_a_value_like_any_other() {
    let root = created(
        "policy-object",
        &running().replace(
            "- **Construction Iteration**: unit-major",
            "- **Construction Iteration**: constructor",
        ),
    );
    assert_eq!(
        ok(&state(
            &root,
            &["set-construction-iteration", "stage-major"],
            &[]
        )),
        "{\"updated\":true,\"construction_iteration\":\"stage-major\",\"notice\":\"Construction now goes stage by stage. You can switch back any time.\"}\n"
    );
    assert!(created_shard(&root).contains("**Previous Value**: constructor\n"));
    let _ = fs::remove_dir_all(&root);
}

/// The command that verifies a unit is written to the state only with the
/// receipt of the person's approval for that very command.
#[test]
fn the_verification_command_needs_the_persons_receipt() {
    let root = created("policy-command", &running());
    let set = |args: &[&str]| {
        state(
            &root,
            &[&["set-construction-verification-command"], args].concat(),
            &[],
        )
    };
    assert_eq!(
        refusal(&set(&[])),
        "Usage: focus engine state set-construction-verification-command --command-file <record-relative path> (or one positional command argument)."
    );
    assert_eq!(
        refusal(&set(&["a\u{200b}b"])),
        "Construction verification command must contain no display-spoofing characters. Put complex checks in a script and record its invocation."
    );
    assert_eq!(
        refusal(&set(&["a\tb"])),
        "Construction verification command must be nonblank, at most 1024 characters, and contain no control characters. Put multiline checks in a script and record its invocation."
    );
    let unapproved = refusal(&set(&["cargo test"]));
    assert!(
        unapproved.starts_with("No current VERIFICATION_COMMAND_RECORDED with matching Command SHA-256 and User Input: Approve authorizes this command. Write the proposed command to <record>/verification-command.txt with the harness file-write tool"),
        "{unapproved}"
    );
    assert!(
        unapproved.ends_with("Apply the receipt with focus engine state set-construction-verification-command --command-file verification-command.txt."),
        "{unapproved}"
    );
    assert!(!created_state(&root).contains("Construction Verification Command"));
    let command = "echo $HOME $1 $&";
    let digest = sha256_hex(command.as_bytes());
    let receipt = |digest: &str| {
        format!(
            "{CREATED_SHARD}\n## Verification Command Recorded\n**Timestamp**: 2026-01-05T10:08:00Z\n**Event**: VERIFICATION_COMMAND_RECORDED\n**Checkpoint**: Construction Verification Command\n**Command SHA-256**: {digest}\n**User Input**: Approve\n\n---\n"
        )
    };
    let shard_rel = format!("{CREATED_RECORD}/audit/laptop-abc123.md");
    write(&root, &shard_rel, &receipt(&sha256_hex(b"make check")));
    assert_eq!(
        ok(&set(&["make check"])),
        format!(
            "{{\"updated\":true,\"command_sha256\":\"{}\",\"command_label\":\"make check\"}}\n",
            sha256_hex(b"make check")
        )
    );
    // Another command, from a file of the record: it replaces the first,
    // dollar signs and all, and says so.
    write(&root, &shard_rel, &receipt(&digest));
    write(
        &root,
        &format!("{CREATED_RECORD}/checks/command.txt"),
        &format!("  {command}\n"),
    );
    assert_eq!(
        ok(&set(&["--command-file", "checks\\command.txt"])),
        format!(
            "{{\"updated\":true,\"command_sha256\":\"{digest}\",\"command_label\":\"{command}\",\"notice\":\"Using `{command}` from here on.\"}}\n"
        )
    );
    assert!(created_state(&root).contains(&format!(
        "- **Construction Verification Command**: {command}\n"
    )));
    // (Bytes that are no text are read as the character that stands for
    // them, and it is that reading the person approved.)
    fs::write(
        root.join(CREATED_RECORD).join("checks/command.txt"),
        b"cargo \xff test\n",
    )
    .unwrap();
    let read = "cargo \u{fffd} test";
    write(&root, &shard_rel, &receipt(&sha256_hex(read.as_bytes())));
    assert!(
        ok(&set(&["--command-file", "checks/command.txt"]))
            .contains(&format!("\"command_label\":\"{read}\"")),
    );
    for (file, says) in [
        ("../x.txt", "path component \"..\" escapes its anchor "),
        ("/x.txt", "path escapes its anchor lexically: /x.txt"),
        ("\\x.txt", "path escapes its anchor lexically: \\x.txt"),
        ("c:x.txt", "path escapes its anchor lexically: c:x.txt"),
    ] {
        let refused = refusal(&set(&["--command-file", file]));
        assert!(refused.starts_with(says), "{refused}");
    }
    let _ = fs::remove_dir_all(&root);
}

/// Deliberate deviation. `Unit Ownership: team` is deferred: upstream sets
/// the gate rhythm of a team's units and syncs a claimed unit's stage;
/// Focus refuses both by name. Without a team both refuse as upstream does.
#[test]
fn a_teams_units_are_refused_by_name() {
    let root = created("policy-solo", &running());
    assert_eq!(
        refusal(&state(&root, &["set-unit-gate-rhythm", "unit-end"], &[])),
        "Unit gate rhythm requires Unit Ownership: team. Run focus engine state set-unit-ownership team first."
    );
    assert_eq!(
        refusal(&state(&root, &["set-unit-gate-rhythm", "nope"], &[])),
        "Invalid unit gate rhythm \"nope\". Valid: per-stage, unit-end."
    );
    assert_eq!(
        refusal(&state(
            &root,
            &["sync-unit-scope-stage", "code-generation", "--unit", "api"],
            &DIRECT
        )),
        "Scoped Unit stage sync requires a live Unit claim."
    );
    assert_eq!(
        refusal(&state(
            &root,
            &["sync-unit-scope-stage", "code-generation"],
            &DIRECT
        )),
        "Usage: focus engine state sync-unit-scope-stage <stage> --unit <unit>"
    );
    let _ = fs::remove_dir_all(&root);
    let team = running().replace(
        "- **Revision Count**: 0\n",
        "- **Revision Count**: 0\n- **Unit Ownership**: team\n",
    );
    assert_ne!(team, running());
    let root = created("policy-team", &team);
    assert_eq!(
        refusal(&state(&root, &["set-unit-gate-rhythm", "unit-end"], &[])),
        "Unit Ownership is team, and the gate rhythm of a team's units is frozen by what its units have done since the workflow started or was jumped: Focus does not run a team's units."
    );
    assert_eq!(
        refusal(&state(
            &root,
            &["sync-unit-scope-stage", "code-generation", "--unit", "api"],
            &DIRECT
        )),
        "Unit Ownership is team, and a unit's stage is synced from the scope stamp of the checkout that claimed it, checked against the claim registry: Focus does not run a team's units."
    );
    assert_eq!(
        refusal(&state(
            &root,
            &["set-construction-iteration", "stage-major"],
            &[]
        )),
        "Construction Iteration cannot leave unit-major while Unit Ownership is team. Set unit ownership to solo first."
    );
    assert_eq!(created_state(&root), team);
    let _ = fs::remove_dir_all(&root);
}

/// The units of the created record, and a state in Construction that
/// plans them.
const UNITS: &str = "# Dependencies\n\n```yaml\nunits:\n  - name: api\n    depends_on: [auth]\n  - name: auth\n    kind: service\n    depends_on: []\n```\n";

fn with_units(name: &str, more_state: &str, rows: &str) -> PathBuf {
    let state = running()
        .replace(
            "- **Lifecycle Phase**: IDEATION",
            "- **Lifecycle Phase**: CONSTRUCTION",
        )
        .replace(
            "- **Revision Count**: 0\n",
            &format!("- **Revision Count**: 0\n{more_state}"),
        );
    let root = created(name, &state);
    write(
        &root,
        &format!("{CREATED_RECORD}/inception/units-generation/unit-of-work-dependency.md"),
        UNITS,
    );
    write(
        &root,
        &format!("{CREATED_RECORD}/audit/laptop-abc123.md"),
        &format!("{CREATED_SHARD}{rows}"),
    );
    root
}

fn unit_row(kind: &str, unit: &str, more: &str) -> String {
    format!(
        "\n## Unit\n**Timestamp**: 2026-01-05T10:09:00Z\n**Event**: UNIT_{kind}\n**Stage**: functional-design\n**Unit**: {unit}\n**Run floor**: WORKFLOW_STARTED:2026-01-05T10:00:00Z#1\n{more}\n---\n"
    )
}

const UNIT: [&str; 5] = ["unit", "complete", "--stage", "functional-design", "--unit"];

/// A completion is a receipt with evidence: of the unit that is open and
/// in progress, with every artifact its kind owes on disk. The row carries
/// the floor of the attempt, and the state's mirror of the open unit goes.
#[test]
fn a_units_completion_is_a_receipt_with_evidence() {
    let mirror = "- **Active Unit**: auth\n- **Unit Stage**: functional-design\n- **Unit State**: in-progress\n";
    let root = with_units("unit-complete", mirror, &unit_row("STARTED", "auth", ""));
    let complete = |unit: &str| state(&root, &[&UNIT[..], &[unit]].concat(), &[]);
    assert_eq!(
        refusal(&complete("auth")),
        "Refusing to complete unit \"auth\" for \"functional-design\": required artifacts are missing on disk (entities, rules, functional-spec, traceability). Write the unit's artifacts before completing it."
    );
    assert_eq!(
        refusal(&complete("api")),
        "Refusing to complete unit \"api\" for \"functional-design\": it is not the active unit (active: \"auth\", in-progress)."
    );
    assert_eq!(
        refusal(&complete("billing")),
        "Refusing unit complete for \"billing\": it is not in the authoritative unit DAG."
    );
    for file in [
        "entities.md",
        "rules.md",
        "functional-spec.md",
        "traceability.json",
    ] {
        write(
            &root,
            &format!("{CREATED_RECORD}/construction/auth/functional-design/{file}"),
            "x\n",
        );
    }
    let done = ok(&complete("auth"));
    assert!(
        done.starts_with(
            "{\"emitted\":\"UNIT_COMPLETED\",\"stage\":\"functional-design\",\"unit\":\"auth\",\"timestamp\":\""
        ),
        "{done}"
    );
    assert!(
        created_shard(&root).contains(
            "**Event**: UNIT_COMPLETED\n**Stage**: functional-design\n**Unit**: auth\n**Run floor**: WORKFLOW_STARTED:2026-01-05T10:00:00Z#1\n"
        ),
        "{}",
        created_shard(&root)
    );
    let after = created_state(&root);
    assert!(
        !after.contains("Active Unit")
            && !after.contains("Unit Stage")
            && !after.contains("Unit State")
    );
    assert_eq!(
        refusal(&complete("auth")),
        "Refusing to complete unit \"auth\" for \"functional-design\": it is already completed. Nothing more to do for it; run `focus engine orchestrate next` for the next step."
    );
    let _ = fs::remove_dir_all(&root);

    let root = with_units(
        "unit-paused",
        "",
        &format!(
            "{}{}",
            unit_row("STARTED", "auth", ""),
            unit_row("PAUSED", "auth", "**Reason**: lunch\n").replace("10:09:00", "10:10:00")
        ),
    );
    assert_eq!(
        refusal(&state(&root, &[&UNIT[..], &["auth"]].concat(), &[])),
        "Refusing to complete unit \"auth\" for \"functional-design\": it is paused (reason: lunch). Resume it first (focus engine state unit resume --stage functional-design --unit auth); a paused unit's work is not done."
    );
    let _ = fs::remove_dir_all(&root);
}

/// The created record moved to Functional Design, the first stage that runs
/// for each unit.
fn design_is_current(root: &Path) {
    let at_design = created_state(root)
        .replace("- [-] intent-capture ", "- [x] intent-capture ")
        .replace("- [ ] functional-design ", "- [-] functional-design ")
        .replace(
            "- **Current Stage**: intent-capture",
            "- **Current Stage**: functional-design",
        );
    assert!(
        at_design.contains("- [-] functional-design ")
            && at_design.contains("- **Current Stage**: functional-design")
    );
    write(
        root,
        &format!("{CREATED_RECORD}/focus-state.md"),
        &at_design,
    );
}

/// A stage's step that does not apply to one unit is skipped for that unit
/// alone, with the floor of the attempt on the row. When no unit owes the
/// stage any more the stage itself is skipped: ticked off when it is ahead,
/// routed when it is the current one.
#[test]
fn a_stage_is_skipped_for_one_unit_and_then_for_all() {
    let root = with_units("skip-unit", "", "");
    let skip = |stage: &str, unit: &str| {
        state(
            &root,
            &[
                "skip",
                stage,
                "--unit",
                unit,
                "--reason",
                "nothing to design",
            ],
            &DIRECT,
        )
    };
    // (The created record stands at Intent Capture, which runs once.)
    assert_eq!(
        refusal(&skip("functional-design", "auth")),
        "Cannot skip \"functional-design\" for unit \"auth\": a one-unit skip needs Construction Iteration: unit-major with solo units, and a per-unit Construction stage at or after Current Stage \"intent-capture\", or one approved before it."
    );
    design_is_current(&root);
    assert_eq!(
        refusal(&state(
            &root,
            &["skip", "functional-design", "--unit", "auth"],
            &DIRECT
        )),
        "focus engine state skip --unit requires a nonblank --reason <text>."
    );
    assert_eq!(
        refusal(&skip("functional-design", "billing")),
        "Cannot skip \"functional-design\" for unit \"billing\": it is not in the authoritative unit DAG."
    );
    // A stage ahead: one unit, then the other, and the stage is ticked off.
    assert_eq!(
        ok(&skip("nfr-design", "auth")),
        "{\"slug\":\"nfr-design\",\"unit\":\"auth\",\"new_state\":\"unit-skipped\",\"owed_by\":[\"api\"]}\n"
    );
    assert!(
        created_shard(&root).contains(
            "**Event**: UNIT_SKIPPED\n**Stage**: nfr-design\n**Unit**: auth\n**Reason**: nothing to design\n**Run floor**: WORKFLOW_STARTED:2026-01-05T10:00:00Z#1\n"
        ),
        "{}",
        created_shard(&root)
    );
    let ticked = ok(&skip("nfr-design", "api"));
    assert!(
        ticked.starts_with(
            "{\"slug\":\"nfr-design\",\"unit\":\"api\",\"new_state\":\"skipped\",\"timestamp\":\""
        ),
        "{ticked}"
    );
    assert!(created_state(&root).contains("- [S] nfr-design "));
    assert!(created_shard(&root).contains(
        "**Event**: STAGE_SKIPPED\n**Stage**: nfr-design\n**Reason**: No unit needs this stage; the last, \"api\", was skipped: nothing to design\n**Skip Kind**: conditional-runtime\n"
    ));
    // The current stage: the last unit's skip routes it.
    assert!(ok(&skip("functional-design", "api")).contains("\"owed_by\":[\"auth\"]"));
    let routed = ok(&skip("functional-design", "auth"));
    assert!(!routed.contains("unit-skipped"), "{routed}");
    let after = created_state(&root);
    assert!(
        after.contains("- [S] functional-design ")
            && !after.contains("- **Current Stage**: functional-design"),
        "{after}"
    );
    let _ = fs::remove_dir_all(&root);
}

/// Deliberate deviation. The skip of the last unit that owes the current
/// stage routes the stage, and a routed skip carries the usage rollup, which
/// Focus does not read yet: with usage on the ledger it is refused by name
/// before the unit's own row is written, where upstream writes the row and
/// routes with the rollup. A stage ahead is ticked off, not routed, and
/// carries none.
#[test]
fn the_last_units_skip_is_refused_ahead_of_its_row_while_usage_is_owed() {
    let root = with_units("skip-unit-usage", "", "");
    design_is_current(&root);
    let skip = |stage: &str, unit: &str, env: &[(&str, &str)]| {
        state(
            &root,
            &[
                "skip",
                stage,
                "--unit",
                unit,
                "--reason",
                "nothing to design",
            ],
            env,
        )
    };
    write(
        &root,
        "focus/.focus-sessions/usage-ledger.json",
        "{\"workflows\":{\"w\":{\"byStage\":{\"intent-capture\":{}},\"totals\":{\"tokens\":{\"input\":1}}}}}\n",
    );
    // (While another unit owes the stage nothing is routed.)
    assert!(ok(&skip("functional-design", "api", &DIRECT)).contains("\"owed_by\":[\"auth\"]"));
    // (A unit whose kind owes the stage nothing does not keep it owed.)
    let dependencies =
        format!("{CREATED_RECORD}/inception/units-generation/unit-of-work-dependency.md");
    write(
        &root,
        &dependencies,
        &UNITS.replace(
            "  - name: auth\n",
            "  - name: pkg\n    kind: packaging\n    depends_on: []\n  - name: auth\n",
        ),
    );
    let rows = created_shard(&root).matches("UNIT_SKIPPED").count();
    assert_eq!(rows, 1);
    assert!(
        refusal(&skip("functional-design", "auth", &DIRECT)).ends_with(
            "Focus does not read the usage ledger yet. Set FOCUS_DISABLE_USAGE_TRACKING=1 to complete without the rollup."
        )
    );
    assert_eq!(created_shard(&root).matches("UNIT_SKIPPED").count(), rows);
    write(&root, &dependencies, UNITS);
    assert!(ok(&skip("nfr-design", "api", &DIRECT)).contains("\"owed_by\":[\"auth\"]"));
    assert!(ok(&skip("nfr-design", "auth", &DIRECT)).contains("\"new_state\":\"skipped\""));
    let untracked = [DIRECT[0], ("FOCUS_DISABLE_USAGE_TRACKING", "1")];
    let routed = ok(&skip("functional-design", "auth", &untracked));
    assert!(!routed.contains("unit-skipped"), "{routed}");
    assert!(created_state(&root).contains("- [S] functional-design "));
    let _ = fs::remove_dir_all(&root);
}

/// A per-unit stage is started again for the named units alone: each gets
/// the row its own rejection would write, a redo also the answer to the
/// re-use question, and no other unit's work is touched. From a later stage
/// of the phase that runs once, the work moves back with no jump row.
#[test]
fn a_per_unit_stage_is_reopened_for_named_units() {
    let root = with_units(
        "reopen-units",
        "- **Active Unit**: auth\n- **Unit Stage**: functional-design\n- **Unit State**: in-progress\n",
        "",
    );
    design_is_current(&root);
    let reopen = |args: &[&str]| jump(&root, &[&["reopen"], args].concat(), &[]);
    let of = ["--target", "functional-design", "--units"];
    for (args, why) in [
        (
            &["--target", "functional-design"][..],
            "Usage: reopen --target <slug> [--stages <slug[,slug...]>] --units <unit[,unit...]> [--via redo|change] [--scope <scope>]",
        ),
        (
            &["--target", "intent-capture", "--units", "auth"][..],
            "Not a per-unit stage: intent-capture",
        ),
        (
            &[
                "--target",
                "functional-design",
                "--units",
                "auth",
                "--via",
                "later",
            ][..],
            "Unknown --via: later (only \"redo\" or \"change\")",
        ),
        (
            &[
                "--target",
                "functional-design",
                "--units",
                "auth",
                "--stages",
                "nfr-design",
            ][..],
            "--stages must start with the target \"functional-design\"",
        ),
        (
            &[
                "--target",
                "functional-design",
                "--units",
                "auth",
                "--stages",
                "functional-design,build-and-test",
            ][..],
            "Not a per-unit stage: build-and-test",
        ),
        (
            &["--target", "functional-design", "--units", "auth,a/b"][..],
            "Invalid Unit name: a/b",
        ),
    ] {
        assert_eq!(refusal(&reopen(args)), why);
    }
    assert!(!created_shard(&root).contains("GATE_REJECTED"));
    assert_eq!(
        ok(&reopen(
            &[
                &of[..],
                &[
                    "auth, api",
                    "--stages",
                    "functional-design, nfr-design",
                    "--via",
                    "redo"
                ]
            ]
            .concat()
        )),
        "{\"reopened\":\"functional-design\",\"stages\":[\"functional-design\",\"nfr-design\"],\"units\":[\"auth\",\"api\"],\"state_updated\":true,\"audit_appended\":true}\n"
    );
    let ledger = created_shard(&root);
    for unit in ["auth", "api"] {
        assert!(
            ledger.contains(&format!(
                "**Event**: GATE_REJECTED\n**Stage**: functional-design\n**Gate Stages**: functional-design, nfr-design\n**Gate Scope**: unit-end\n**Unit**: {unit}\n**Reopen**: redo\n**Feedback**: Redid Functional Design for unit {unit} at the person's request (redo on re-entry).\n"
            )),
            "{ledger}"
        );
        assert!(
            ledger.contains(&format!(
                "**Event**: ARTIFACT_REUSED\n**Stage**: functional-design\n**Decision**: redo\n**Artifacts**: construction/{unit}/functional-design/\n**Unit**: {unit}\n**Source**: Redo on re-entry\n"
            )),
            "{ledger}"
        );
    }
    let state = created_state(&root);
    assert!(
        !state.contains("Active Unit")
            && !state.contains("Unit Stage")
            && !state.contains("Unit State"),
        "{state}"
    );
    assert!(state.contains("- **Current Stage**: functional-design"));
    // A plain reopen says so, and a change with nothing said has its own words.
    ok(&reopen(&[&of[..], &["api"]].concat()));
    ok(&reopen(&[&of[..], &["api", "--via", "change"]].concat()));
    let ledger = created_shard(&root);
    assert!(ledger.contains("**Unit**: api\n**Reopen**: jump\n**Feedback**: Reopened Functional Design for unit api (/focus --stage functional-design).\n"), "{ledger}");
    assert!(ledger.contains("**Unit**: api\n**Reopen**: change\n**User Input**: Request Changes\n**Feedback**: Reopened Functional Design for unit api for a change.\n"), "{ledger}");
    // From Build and Test, which runs once: the work moves back to the target.
    let mut testing = created_state(&root)
        .replace(
            "- **Current Stage**: functional-design",
            "- **Current Stage**: build-and-test",
        )
        .replace("- [ ] build-and-test ", "- [-] build-and-test ");
    for slug in [
        "functional-design",
        "nfr-requirements",
        "nfr-design",
        "infrastructure-design",
        "code-generation",
    ] {
        testing = testing
            .replace(&format!("- [-] {slug} "), &format!("- [x] {slug} "))
            .replace(&format!("- [ ] {slug} "), &format!("- [x] {slug} "));
    }
    assert!(
        testing.contains("- [-] build-and-test ") && testing.contains("- [x] code-generation ")
    );
    write(&root, &format!("{CREATED_RECORD}/focus-state.md"), &testing);
    let jumps = created_shard(&root).matches("STAGE_JUMPED").count();
    let moved = ok(&reopen(&["--target", "nfr-design", "--units", "auth"]));
    assert!(
        moved.starts_with("{\"reopened\":\"nfr-design\",\"stages\":[\"nfr-design\"],\"units\":[\"auth\"],\"stages_reset\":[\"nfr-design\","),
        "{moved}"
    );
    let state = created_state(&root);
    assert!(
        state.contains("- **Current Stage**: nfr-design")
            && state.contains("- [-] nfr-design ")
            && state.contains("- [x] nfr-requirements ")
            && state.contains("- [ ] build-and-test "),
        "{state}"
    );
    assert_eq!(created_shard(&root).matches("STAGE_JUMPED").count(), jumps);
    let _ = fs::remove_dir_all(&root);
}

/// Named units move forward alone: each step named is skipped for each of
/// them by the state tool's own one-unit skip, and nothing else moves. What
/// that tool refuses is the jump's refusal, after a row of its own.
#[test]
fn named_units_move_forward_by_their_own_skips() {
    let root = with_units(
        "units-forward",
        "- **Active Unit**: auth\n- **Unit Stage**: functional-design\n- **Unit State**: in-progress\n",
        "",
    );
    design_is_current(&root);
    let onward = |units: &str, stages: &str, more: &[&str]| {
        jump(
            &root,
            &[
                &[
                    "execute",
                    "--target",
                    "code-generation",
                    "--units",
                    units,
                    "--stages",
                    stages,
                ][..],
                more,
            ]
            .concat(),
            &[],
        )
    };
    let forward = ["--direction", "forward"];
    assert_eq!(
        refusal(&onward(
            "auth",
            "functional-design",
            &["--direction", "backward"]
        )),
        "execute --units moves Units forward to a per-unit stage: --direction forward --units <unit[,unit...]> [--stages <slug[,slug...]>]"
    );
    assert_eq!(
        refusal(&onward("a/b", "functional-design", &forward)),
        "Invalid Unit name: a/b"
    );
    assert_eq!(
        refusal(&onward(
            "auth",
            "functional-design,code-generation",
            &forward
        )),
        "Not a per-unit stage before code-generation: code-generation"
    );
    assert!(!created_shard(&root).contains("UNIT_SKIPPED"));
    let moved = ok(&onward("auth", "functional-design, nfr-design", &forward));
    assert!(
        moved.starts_with("{\"direction\":\"forward\",\"target\":\"code-generation\",\"target_phase\":\"CONSTRUCTION\",\"units\":[\"auth\"],\"stages_skipped\":[\"functional-design\",\"nfr-design\"],\"stages_reset\":[],\"state_updated\":true,\"audit_appended\":true,\"completed_count\":4,\"workflow_stopped\":false,\"timestamp\":\""),
        "{moved}"
    );
    let ledger = created_shard(&root);
    for stage in ["functional-design", "nfr-design"] {
        assert!(
            ledger.contains(&format!(
                "**Event**: UNIT_SKIPPED\n**Stage**: {stage}\n**Unit**: auth\n**Reason**: Skipped by jump to code-generation (forward)\n"
            )),
            "{ledger}"
        );
    }
    assert!(
        !ledger.contains("STAGE_JUMPED") && !ledger.contains("STAGE_SKIPPED"),
        "{ledger}"
    );
    let state = created_state(&root);
    assert!(
        state.contains("- **Current Stage**: functional-design") && !state.contains("Active Unit"),
        "{state}"
    );
    // No step named: nothing is skipped, and nothing is written but the date.
    let nothing = ok(&jump(
        &root,
        &[
            "execute",
            "--target",
            "code-generation",
            "--direction",
            "forward",
            "--units",
            "api",
        ],
        &[],
    ));
    assert!(nothing.contains("\"stages_skipped\":[],\"stages_reset\":[],\"state_updated\":true,\"audit_appended\":false"), "{nothing}");
    // The state tool's refusal is the jump's, and each tool leaves its row.
    assert_eq!(
        refusal(&onward("api,billing", "nfr-requirements", &forward)),
        "Cannot skip \"nfr-requirements\" for unit \"billing\": it is not in the authoritative unit DAG."
    );
    let ledger = created_shard(&root);
    assert!(ledger.contains("**Stage**: nfr-requirements\n**Unit**: api\n**Reason**: Skipped by jump to code-generation (forward)\n"), "{ledger}");
    let errors: Vec<&str> = ledger.split("**Event**: ERROR_LOGGED\n").skip(1).collect();
    let last_two = &errors[errors.len() - 2..];
    assert!(
        last_two[0].starts_with("**Tool**: focus-state\n**Command**: focus-state skip nfr-requirements --unit billing --reason Skipped by jump to code-generation (forward) --project-dir "),
        "{}",
        last_two[0]
    );
    assert!(last_two[1].starts_with("**Tool**: focus-jump\n**Command**: focus-jump execute --target code-generation --units api,billing --stages nfr-requirements --direction forward\n"), "{}", last_two[1]);
    for row in last_two {
        assert!(row.contains("**Error**: Cannot skip \"nfr-requirements\" for unit \"billing\": it is not in the authoritative unit DAG.\n"), "{row}");
    }
    let _ = fs::remove_dir_all(&root);
}

/// Deliberate deviation. The one-unit skips of `execute --units` are the
/// state tool's, and meet what that tool refuses by name in Focus: the
/// usage rollup a routed skip carries, and a state write beside the
/// engine's active-directive marker. Upstream skips, routes and writes.
#[test]
fn a_units_jump_meets_what_the_state_tool_refuses_by_name() {
    let root = with_units("units-forward-by-name", "", "");
    design_is_current(&root);
    let onward = |units: &str| {
        jump(
            &root,
            &[
                "execute",
                "--target",
                "nfr-requirements",
                "--direction",
                "forward",
                "--units",
                units,
                "--stages",
                "functional-design",
            ],
            &[],
        )
    };
    write(
        &root,
        "focus/.focus-sessions/usage-ledger.json",
        "{\"workflows\":{\"w\":{\"byStage\":{\"intent-capture\":{}},\"totals\":{\"tokens\":{\"input\":1}}}}}\n",
    );
    assert!(
        refusal(&onward("api,auth")).ends_with(
            "Focus does not read the usage ledger yet. Set FOCUS_DISABLE_USAGE_TRACKING=1 to complete without the rollup."
        )
    );
    assert_eq!(
        created_shard(&root).matches("UNIT_SKIPPED").count(),
        1,
        "{}",
        created_shard(&root)
    );
    fs::remove_file(root.join("focus/.focus-sessions/usage-ledger.json")).unwrap();
    write(
        &root,
        &format!("{CREATED_RECORD}/.focus-engine/active-directive.json"),
        "{}\n",
    );
    assert!(
        refusal(&onward("auth")).ends_with(
            "which Focus does not keep yet. Remove the marker, or write the state with the engine's own tools."
        )
    );
    // (As for the state tool's own verbs, the refusal is at the state's
    // write: the unit's row is in the ledger by then.)
    assert_eq!(created_shard(&root).matches("UNIT_SKIPPED").count(), 2);
    assert!(created_state(&root).contains("- **Current Stage**: functional-design"));
    let _ = fs::remove_dir_all(&root);
}

/// Deliberate deviation. Before a start, a pause or a resumption upstream
/// asks the orchestration engine which unit it routes now, and a completion
/// in a wave settles an entry of the batch it routes. Focus has no
/// orchestrator yet: the four refuse by name, after every refusal that
/// comes before the question.
#[test]
fn a_units_step_that_asks_the_route_is_refused_by_name() {
    let root = with_units("unit-route", "", "");
    let unit = |args: &[&str]| state(&root, &[&["unit"], args].concat(), &[]);
    let of = ["--stage", "functional-design", "--unit", "auth"];
    for (action, more) in [
        ("start", &[][..]),
        ("pause", &["--reason", "lunch", "--next-action", "eat"][..]),
        ("resume", &[][..]),
    ] {
        assert_eq!(
            refusal(&unit(&[&[action], &of[..], more].concat())),
            format!(
                "unit {action} asks the orchestration engine which unit it routes now, so that a receipt cannot land in a wave or on a unit the engine did not route: Focus has no orchestrator yet."
            )
        );
    }
    assert_eq!(
        refusal(&unit(&[&["complete"], &of[..], &["--wave"]].concat())),
        "unit complete --wave settles an entry of the wave the orchestration engine routes: Focus has no orchestrator yet."
    );
    // What comes before the question is refused as upstream refuses it.
    assert_eq!(
        refusal(&unit(&[&["pause"], &of[..]].concat())),
        "unit pause requires --reason <text> (why the unit stopped)."
    );
    assert_eq!(
        refusal(&unit(&[
            "start",
            "--stage",
            "functional-design",
            "--unit",
            "billing"
        ])),
        "Refusing unit start for \"billing\": it is not in the authoritative unit DAG."
    );
    assert_eq!(
        refusal(&unit(&[
            "start",
            "--stage",
            "intent-capture",
            "--unit",
            "auth"
        ])),
        "Stage \"intent-capture\" is not per-unit (for_each: unit-of-work); unit receipts do not apply."
    );
    assert!(!created_shard(&root).contains("UNIT_"));
    let _ = fs::remove_dir_all(&root);
}

/// Deliberate deviation. Three questions about a unit that Focus does not
/// answer yet, each refused by name: a team's unit (claimed by a checkout),
/// a swarm (whether it owns the units of a subagent's stage), and a
/// completion the person picked on a recovery ask (read off the
/// active-directive marker).
#[test]
fn a_units_step_over_what_focus_does_not_run_is_refused_by_name() {
    let root = with_units("unit-team", "- **Unit Ownership**: team\n", "");
    assert_eq!(
        refusal(&state(&root, &[&UNIT[..], &["auth"]].concat(), &[])),
        "Unit Ownership is team, and a team's unit is worked in the checkout that claimed it, checked against the claim registry: Focus does not run a team's units."
    );
    let _ = fs::remove_dir_all(&root);

    let root = with_units(
        "unit-swarm",
        "- **Construction Autonomy Mode**: autonomous\n",
        "",
    );
    let stage_major = created_state(&root)
        .replace(
            "- **Construction Iteration**: unit-major",
            "- **Construction Iteration**: stage-major",
        )
        .replace(
            "- **Construction Checkpoints**: enabled",
            "- **Construction Checkpoints**: disabled",
        );
    write(
        &root,
        &format!("{CREATED_RECORD}/focus-state.md"),
        &stage_major,
    );
    let of = |stage: &'static str| ["unit", "complete", "--stage", stage, "--unit", "auth"];
    assert_eq!(
        refusal(&state(&root, &of("code-generation"), &[])),
        "Construction is built as a swarm here, and whether the swarm owns the units of \"code-generation\" (a subagent's stage) is the swarm referee's question: Focus runs no swarm yet."
    );
    // (A stage the conductor runs itself is not a swarm's.)
    assert!(
        refusal(&state(&root, &of("functional-design"), &[]))
            .starts_with("Refusing to complete unit \"auth\" for \"functional-design\": it is not the active unit")
    );
    let _ = fs::remove_dir_all(&root);

    let root = with_units("unit-picked", "", "");
    write(
        &root,
        &format!("{CREATED_RECORD}/.focus-engine/active-directive.json"),
        "{}\n",
    );
    assert_eq!(
        refusal(&state(&root, &[&UNIT[..], &["auth"]].concat(), &[])),
        "The engine has a step in hand for this record, and whether the person picked recording this completion on its recovery ask is read off that step's marker: Focus does not read it yet."
    );
    // (Whatever stands at the marker's name: the writer refuses it too.)
    let marker = root
        .join(CREATED_RECORD)
        .join(".focus-engine/active-directive.json");
    fs::remove_file(&marker).unwrap();
    fs::create_dir_all(&marker).unwrap();
    assert!(
        refusal(&state(&root, &[&UNIT[..], &["auth"]].concat(), &[]))
            .starts_with("The engine has a step in hand for this record,")
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn a_gate_records_what_the_person_decided_about_findings() {
    use sha2::{Digest, Sha256};
    let state_text = CREATED
        .replace("- [-] intent-capture ", "- [?] intent-capture ")
        .replace(
            "- **Review Override**: none",
            "- **Review Override**: none\n- **Sensors**: off (set by a command)",
        );
    let root = created("gate-findings", &state_text);
    let statement = format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md");
    write(&root, &statement, "# Intent\n");
    let reviewer = "focus-product-lead-agent";
    let fingerprint = format!("sha256:{}", "c".repeat(64));
    let request = format!("review:{}", "1".repeat(32));
    let finding = |id: &str| {
        format!(
            "    {{\n      \"id\": \"{id}\",\n      \"severity\": \"Major\",\n      \"location\": \"Intent\",\n      \"finding\": \"Finding {id}\",\n      \"required_action\": \"Fix {id}\",\n      \"status\": \"New\"\n    }}"
        )
    };
    let record = format!(
        "{{\n  \"version\": 1,\n  \"stage\": \"intent-capture\",\n  \"unit\": null,\n  \"workflow\": null,\n  \"attempt\": \"0000000000000000\",\n  \"iteration\": 1,\n  \"reviewer\": \"{reviewer}\",\n  \"verdict\": \"NOT-READY\",\n  \"request_id\": \"{request}\",\n  \"request_challenge\": null,\n  \"artifact_fingerprint\": \"{fingerprint}\",\n  \"source_fingerprint\": null,\n  \"unit_source_fingerprint\": null,\n  \"findings\": [\n{},\n{}\n  ],\n  \"body\": \"It reads well.\\n\",\n  \"recorded_at\": \"2026-01-05T10:00:00Z\"\n}}\n",
        finding("R-02"),
        finding("R-01")
    );
    let record_rel = ".focus-engine/reviews/intent-capture/stage/0000000000000000/1.json";
    write(&root, &format!("{CREATED_RECORD}/{record_rel}"), &record);
    let digest: String = Sha256::digest(record.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let scope = format!("**Stage**: intent-capture\n**Reviewer**: {reviewer}\n**Iteration**: 1\n");
    let reviewed = format!(
        "\n## Review Requested\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: REVIEW_REQUESTED\n{scope}**Artifact Fingerprint**: {fingerprint}\n**Request Id**: {request}\n\n---\n\n## Review Completed\n**Timestamp**: 2026-01-05T10:02:00Z\n**Event**: REVIEW_COMPLETED\n{scope}**Verdict**: NOT-READY\n**Request Fingerprint**: {fingerprint}\n**Artifact Fingerprint**: {fingerprint}\n**Request Id**: {request}\n**Review Record**: {record_rel}\n**Review Record Digest**: sha256:{digest}\n\n---\n"
    );
    let opened = "\n## Stage Awaiting Approval\n**Timestamp**: 2026-01-05T10:03:00Z\n**Event**: STAGE_AWAITING_APPROVAL\n**Stage**: intent-capture\n\n---\n";
    let shard_path = root.join(CREATED_RECORD).join("audit/laptop-abc123.md");
    let shard = format!("{CREATED_SHARD}{reviewed}{opened}");
    fs::write(&shard_path, &shard).unwrap();
    let approve = ["approve", "intent-capture", "--user-input", "Approve"];
    let no = |spec: &'static str| {
        [
            "reject",
            "intent-capture",
            "--feedback",
            "needs more",
            "--reject-finding",
            spec,
        ]
    };
    // The deviation first: nothing is decided and nothing recorded.
    let listed = format!(
        "\n## Gate Approved\n**Timestamp**: 2026-01-05T09:00:00Z\n**Event**: GATE_APPROVED\n**Stage**: intent-capture\n**Review Finding Dispositions**: {{\"version\":1,\"dispositions\":[{{\"artifact\":\"{statement}\",\"id\":[\"R-01\"],\"fingerprint\":\"sha256:{}\",\"status\":\"Accepted risk\"}}]}}\n\n---\n",
        "b".repeat(64)
    );
    fs::write(&shard_path, format!("{shard}{listed}")).unwrap();
    let not_text = "the Review Finding Dispositions field holds a decision that is not written as text: it is not read";
    assert_eq!(refusal(&state(&root, &approve, &QUIET)), not_text);
    assert_eq!(refusal(&state(&root, &no("R-01=x"), &QUIET)), not_text);
    assert_eq!(created_state(&root), state_text);
    assert!(!created_shard(&root).contains("## Gate Rejected"));
    assert_eq!(created_shard(&root).matches("## Gate Approved").count(), 1);
    fs::write(&shard_path, &shard).unwrap();
    // A finding that is not there is named back with the ones that are,
    // and one named twice is refused.
    assert_eq!(
        refusal(&state(&root, &no("R-09=x"), &QUIET)),
        format!(
            "Cannot reject R-09: it is not a current review finding for this gate. Current rejectable findings: {statement}#R-01, {statement}#R-02."
        )
    );
    assert_eq!(
        refusal(&state(
            &root,
            &[
                "reject",
                "intent-capture",
                "--feedback",
                "needs more",
                "--reject-finding",
                "R-01=x",
                "--reject-finding",
                "intent-statement#R-01=y"
            ],
            &QUIET
        )),
        format!(
            "Finding {statement}#R-01 cannot appear more than once across --reject-finding and --reopen-finding. Keep only the intended decision."
        )
    );
    // (A refusal leaves its own row, and no decision.)
    assert!(!created_shard(&root).contains("**Event**: GATE_"));
    assert_eq!(created_state(&root), state_text);
    // The approval accepts the risk of both, in the order of their ids,
    // each with the record of the review that raised it.
    ok(&state(&root, &approve, &QUIET));
    let after = created_shard(&root);
    let row = after
        .lines()
        .find(|line| line.starts_with("**Review Finding Dispositions**: "))
        .unwrap_or_else(|| panic!("{after}"));
    let decided: serde_json::Value =
        serde_json::from_str(&row["**Review Finding Dispositions**: ".len()..]).unwrap();
    assert_eq!(decided["version"], 1);
    let decisions = decided["dispositions"].as_array().unwrap();
    assert_eq!(decisions.len(), 2, "{row}");
    for (decision, id) in decisions.iter().zip(["R-01", "R-02"]) {
        assert_eq!(decision["artifact"], statement.as_str());
        assert_eq!(decision["id"], id);
        assert_eq!(decision["status"], "Accepted risk");
        assert_eq!(decision["decided_at_severity"], "Major");
        assert_eq!(decision["reviewed_record"]["path"], record_rel);
        assert_eq!(
            decision["reviewed_record"]["digest"],
            format!("sha256:{digest}").as_str()
        );
    }
    let _ = fs::remove_dir_all(root);
}

/// The words the human-turn hook kept for the chat, read at the gate: the
/// person's exact pick decides against a report of the other choice, their
/// words ride on the receipt, and their own feedback replaces the
/// conductor's.
#[test]
fn the_persons_own_words_at_the_gate_decide_and_are_recorded() {
    let root = created("gate-words", CREATED);
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n",
    );
    // The presence guard stays on: the pick is read only where a person
    // answers the gate, so the shard carries their turns after it opened.
    let quiet_off: Vec<(&str, &str)> = DIRECT
        .iter()
        .copied()
        .chain([
            ("FOCUS_DISABLE_SENSORS", "1"),
            ("FOCUS_SESSION_OVERRIDE", "s1"),
        ])
        .collect();
    ok(&state(&root, &["gate-start", "intent-capture"], &quiet_off));
    let turn = |root: &Path| {
        let dir = root.join(CREATED_RECORD).join("audit");
        let path = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "md"))
            .unwrap();
        let mut rows = fs::read_to_string(&path).unwrap();
        rows.push_str("\n## Human Turn\n**Timestamp**: 2099-01-01T00:00:00Z\n**Event**: HUMAN_TURN\n**Session**: s1\n**Origin**: person\n**Source**: typed\n\n---\n");
        fs::write(&path, rows).unwrap();
    };
    turn(&root);
    let shard_rel = {
        let dir = root.join(CREATED_RECORD).join("audit");
        let name = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .find(|name| name.ends_with(".md"))
            .unwrap();
        format!("{CREATED_RECORD}/audit/{name}")
    };
    let record = |messages: &[&str]| {
        let messages: Vec<String> = messages
            .iter()
            .enumerate()
            .map(|(n, text)| {
                format!(
                    "{{\"offset\":{},\"text\":{}}}",
                    100_000 + n,
                    serde_json::to_string(text).unwrap()
                )
            })
            .collect();
        write(
            &root,
            &format!("{CREATED_RECORD}/.focus-engine/gate-words/s1.json"),
            &format!(
                "{{\"version\":1,\"session\":\"s1\",\"shard\":{},\"dropped\":0,\"messages\":[{}]}}\n",
                serde_json::to_string(&shard_rel).unwrap(),
                messages.join(",")
            ),
        );
    };
    // Their latest message picked Request Changes: an approval is refused.
    record(&["looks fine", "2"]);
    let said = refusal(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "Approve"],
        &quiet_off,
    ));
    assert!(
        said.starts_with("The person picked Request Changes at the \"intent-capture\" gate. Report that, or ask them if you read their words differently."),
        "{said}"
    );
    // Their words are the feedback; the conductor's text, when it says
    // something else, is kept beside them.
    record(&["Request Changes", "the login flow must use oauth"]);
    let output = state(
        &root,
        &["reject", "intent-capture", "--feedback", "use oauth"],
        &quiet_off,
    );
    let out: serde_json::Value = serde_json::from_str(&ok(&output)).unwrap();
    assert_eq!(out["feedback"], "the login flow must use oauth");
    assert_eq!(out["feedback_source"], "person");
    let rows = created_shard(&root);
    assert!(
        rows.contains("**Event**: GATE_REJECTED\n**Stage**: intent-capture\n**Feedback**: the login flow must use oauth\n**Conductor Summary**: use oauth\n"),
        "{rows}"
    );
    assert!(rows.contains("**Event**: STAGE_REVISING\n**Stage**: intent-capture\n**Revision count**: 1\n**Feedback**: the login flow must use oauth\n"), "{rows}");
    // Shown again, their pick of Approve rides on the receipt as their words.
    ok(&state(&root, &["revise", "intent-capture"], &quiet_off));
    turn(&root);
    record(&["Approve"]);
    let said = refusal(&state(
        &root,
        &["reject", "intent-capture", "--feedback", "needs more"],
        &quiet_off,
    ));
    assert!(
        said.starts_with(
            "Refusing to reject \"intent-capture\": the person picked Approve at this gate."
        ),
        "{said}"
    );
    ok(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "yes"],
        &quiet_off,
    ));
    let rows = created_shard(&root);
    assert!(
        rows.contains("**Event**: GATE_APPROVED\n**Stage**: intent-capture\n**User Input**: Approve\n**Person Reply**: Approve\n"),
        "{rows}"
    );
    // A record of another chat, another shard, or one the hook could not
    // keep whole, reads as no words.
    let root = created("gate-words-none", CREATED);
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n",
    );
    ok(&state(&root, &["gate-start", "intent-capture"], &quiet_off));
    turn(&root);
    for text in [
        "{\"version\":1,\"session\":\"s2\",\"shard\":\"x\",\"dropped\":0,\"messages\":[{\"offset\":100000,\"text\":\"2\"}]}",
        "{\"version\":1,\"session\":\"s1\",\"shard\":\"other/audit/x.md\",\"dropped\":0,\"messages\":[{\"offset\":100000,\"text\":\"2\"}]}",
        "{nope",
    ] {
        write(
            &root,
            &format!("{CREATED_RECORD}/.focus-engine/gate-words/s1.json"),
            text,
        );
        let said = refusal(&state(&root, &["reject", "intent-capture"], &quiet_off));
        assert!(said.starts_with("Refusing to reject \"intent-capture\": Request Changes requires nonblank revision feedback"), "{text}: {said}");
    }
    // Accept as-is is their pick only once the revision cap offers it, and
    // then it names the approval whatever the conductor typed.
    let record = |root: &Path, messages: &[&str]| {
        let shard_rel = {
            let dir = root.join(CREATED_RECORD).join("audit");
            let name = fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .find(|name| name.ends_with(".md"))
                .unwrap();
            format!("{CREATED_RECORD}/audit/{name}")
        };
        let messages: Vec<String> = messages
            .iter()
            .enumerate()
            .map(|(n, text)| {
                format!(
                    "{{\"offset\":{},\"text\":{}}}",
                    100_000 + n,
                    serde_json::to_string(text).unwrap()
                )
            })
            .collect();
        write(
            root,
            &format!("{CREATED_RECORD}/.focus-engine/gate-words/s1.json"),
            &format!(
                "{{\"version\":1,\"session\":\"s1\",\"shard\":{},\"dropped\":0,\"messages\":[{}]}}\n",
                serde_json::to_string(&shard_rel).unwrap(),
                messages.join(",")
            ),
        );
    };
    record(&root, &["Accept as-is"]);
    ok(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "Approve"],
        &quiet_off,
    ));
    let rows = created_shard(&root);
    assert!(
        rows.contains("**Event**: GATE_APPROVED\n**Stage**: intent-capture\n**User Input**: Approve\n**Person Reply**: Accept as-is\n"),
        "{rows}"
    );
    let root = created(
        "gate-words-capped",
        &CREATED.replace(
            "## Current Status\n",
            "## Runtime State\n- **Revision Count**: 3\n\n## Current Status\n",
        ),
    );
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n",
    );
    ok(&state(&root, &["gate-start", "intent-capture"], &quiet_off));
    turn(&root);
    record(&root, &["3"]);
    let said = refusal(&state(
        &root,
        &["reject", "intent-capture", "--feedback", "needs more"],
        &quiet_off,
    ));
    assert!(
        said.starts_with(
            "Refusing to reject \"intent-capture\": the person picked Accept as-is at this gate."
        ),
        "{said}"
    );
    ok(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "yes"],
        &quiet_off,
    ));
    let rows = created_shard(&root);
    assert!(
        rows.contains("**Event**: GATE_APPROVED\n**Stage**: intent-capture\n**User Input**: Accept as-is\n**Person Reply**: 3\n"),
        "{rows}"
    );
}

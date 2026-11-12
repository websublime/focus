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
/// refusal of a verb it does not know too; Focus lists the thirteen it has,
/// and refuses the others by name, before any fence is consulted (upstream
/// passes them through the state-transition fence first).
#[test]
fn the_state_command_names_the_verbs_it_has() {
    let root = project("usage");
    assert_eq!(
        refusal(&state(&root, &[], &[])),
        "Usage: focus engine state <subcommand>. Valid: get, set, checkbox, count, advance, finalize, complete-workflow, gate-start, approve, reject, revise, skip, resume, park, unpark. Run with --help for detail."
    );
    assert_eq!(
        refusal(&state(&root, &["nope"], &[])),
        "Unknown subcommand: nope. Run focus engine state --help for usage."
    );
    for (verb, does) in [
        ("set-unit-ownership", "sets the units' ownership"),
        (
            "acknowledge-compaction",
            "acknowledges a context compaction",
        ),
        ("fork", "forks the state for a bolt"),
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
    assert_eq!(
        refusal(&state(&root, &["advance", "intent-capture"], &DIRECT)),
        "Cannot complete \"intent-capture\": none of its declared artifacts exist under the intent's record directory. The stage protocol requires Intent Capture & Framing to produce output before the gate. Produce the artifacts before completing. (declared: intent-statement, stakeholder-map, intent-capture-questions)"
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

/// Deliberate deviation. The evidence a transition owes to `aidlc-log`
/// records (a summary confirmation, a reviewer's receipt, a pipeline's
/// handoffs), a Construction stage's checkpoints and units, the usage
/// ledger's rollup and a unit's skip are each M4's or M5's: upstream weighs
/// them, Focus refuses by name.
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
        refusal(&state(&root, &["advance", "intent-capture"], &DIRECT))
            .starts_with("\"intent-capture\" owes a summary confirmation")
    );
    with(&CREATED.replace("- **Review Override**: none\n", ""));
    assert!(refusal(&state(&root, &["advance", "intent-capture"], &DIRECT)).starts_with("\"intent-capture\" owes a review receipt from focus-product-lead-agent (its review class resolves to advisory)"));
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
    assert!(
        refusal(&state(
            &root,
            &["skip", "intent-capture", "--unit", "api", "--reason", "x"],
            &DIRECT
        ))
        .starts_with("skip --unit skips one unit of work's step (\"api\")")
    );
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

/// Deliberate deviation. The sensors a gate fires, a review's findings, the
/// gate words and the units' gates are the sensor tool's, the review brief's,
/// the human-turn hook's and the units' lifecycle (M4, M5): where upstream
/// reads them, Focus refuses by name.
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
    // A `## Review` section in the review artifact.
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n\n## Review\n- R-01 New: the title\n",
    );
    let findings = refusal(&state(
        &root,
        &["approve", "intent-capture", "--user-input", "Approve"],
        &quiet_off,
    ));
    assert!(findings.ends_with("intent-statement.md carries a `## Review` section, whose findings the gate would record on its decision: Focus does not read review findings yet."), "{findings}");
    assert_eq!(
        refusal(&state(
            &root,
            &[
                "reject",
                "intent-capture",
                "--feedback",
                "x",
                "--reject-finding",
                "intent-statement#R-01=no"
            ],
            &quiet_off
        )),
        "--reject-finding and --reopen-finding record the person's decision on a review's findings, which Focus does not read yet."
    );
    write(
        &root,
        &format!("{CREATED_RECORD}/ideation/intent-capture/intent-statement.md"),
        "# Intent\n",
    );
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
    // Deliberate deviations: units, the directive marker, a bound session and reopen are M4/M5.
    assert!(
        refusal(&jump(
            &root,
            &[
                "execute",
                "--target",
                "feasibility",
                "--direction",
                "forward",
                "--units",
                "api"
            ],
            &[]
        ))
        .ends_with("Focus does not run units yet.")
    );
    assert!(
        refusal(&jump(
            &root,
            &["reopen", "--target", "functional-design", "--units", "api"],
            &[]
        ))
        .ends_with("Focus does not run units yet.")
    );
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

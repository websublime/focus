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
        "Usage: focus engine state <subcommand>. Valid: get, set, checkbox, count, advance, finalize, complete-workflow, skip, resume. Run with --help for detail."
    );
    assert_eq!(
        refusal(&state(&root, &["nope"], &[])),
        "Unknown subcommand: nope. Run focus engine state --help for usage."
    );
    for (verb, does) in [
        ("gate-start", "opens a stage's approval gate"),
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

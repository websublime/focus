//! `focus engine runtime`, run as a process on an installed project.
//!
//! What the command answers for a ledger is held to upstream's by the
//! recorded differential, which replays on Unix. These tests run everywhere:
//! they cover the command's own surface, the two things a compile writes,
//! and each way Focus deliberately differs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RECORD: &str = "focus/spaces/default/intents/add-login-1a2b3c4d";

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

fn row(event: &str, at: &str, fields: &[&str]) -> String {
    let mut block = format!("\n## Row\n**Timestamp**: {at}\n**Event**: {event}\n");
    for field in fields {
        block.push_str(field);
        block.push('\n');
    }
    block + "\n---\n"
}

/// An installed Claude Code project with one intent record, its state file
/// and this clone's identity.
fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("focus-runtime-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    copy_dir(&fixtures.join("installed/claude"), &root);
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        "# State\n\n- **Scope**: feature\n",
    );
    write(
        &root,
        "focus/spaces/default/intents/active-intent",
        "add-login-1a2b3c4d\n",
    );
    write(&root, "focus/.focus-clone-id", "abc123\nlaptop\n");
    root
}

/// A workflow: one stage approved with an empty diary, one with entries,
/// one still open.
fn with_workflow(root: &Path) {
    let ledger = "# Focus Audit Log\n".to_owned()
        + &row(
            "WORKFLOW_STARTED",
            "2026-01-05T10:00:00Z",
            &["**Scope**: bugfix"],
        )
        + &row(
            "STAGE_STARTED",
            "2026-01-05T10:01:00Z",
            &["**Stage**: intent-capture"],
        )
        + &row(
            "STAGE_COMPLETED",
            "2026-01-05T10:05:00Z",
            &["**Stage**: intent-capture"],
        )
        + &row(
            "STAGE_STARTED",
            "2026-01-05T10:06:00Z",
            &["**Stage**: requirements-analysis"],
        )
        + &row(
            "STAGE_COMPLETED",
            "2026-01-05T10:09:00Z",
            &["**Stage**: requirements-analysis"],
        )
        + &row(
            "STAGE_STARTED",
            "2026-01-05T10:10:00Z",
            &["**Stage**: code-generation"],
        );
    write(root, &format!("{RECORD}/audit/laptop-abc123.md"), &ledger);
    write(
        root,
        &format!("{RECORD}/ideation/intent-capture/memory.md"),
        "## Deviations\n- one\n\n## Tradeoffs\n- two\n",
    );
    write(
        root,
        &format!("{RECORD}/inception/requirements-analysis/memory.md"),
        "## Deviations\n",
    );
}

fn runtime(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    command
        .args(["engine", "runtime"])
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

fn refusal(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    assert!(output.stdout.is_empty(), "{}", text(&output.stdout));
    text(&output.stderr)
}

#[test]
fn a_workflow_is_compiled_read_and_summed_up() {
    let root = project("workflow");
    with_workflow(&root);
    let graph_path = root.join(RECORD).join("runtime-graph.json");
    let ledger_path = root.join(RECORD).join("audit/laptop-abc123.md");

    let compiled = runtime(&root, &["compile"], &[]);
    assert!(compiled.status.success(), "{}", text(&compiled.stderr));
    let answer: serde_json::Value = serde_json::from_slice(&compiled.stdout).unwrap();
    assert!(
        Path::new(answer["written"].as_str().unwrap())
            .ends_with("focus/spaces/default/intents/add-login-1a2b3c4d/runtime-graph.json")
    );
    let bytes = fs::read(&graph_path).unwrap();
    assert!(bytes.ends_with(b"}\n"));
    let graph: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(graph["workflow_id"], "2026-01-05T10:00:00Z");
    // The state file's scope, not the ledger's.
    assert_eq!(graph["scope"], "feature");
    let stages = graph["stages"].as_array().unwrap();
    assert_eq!(stages.len(), 3);
    assert_eq!(stages[0]["agent"], "focus-product-agent");
    assert_eq!(stages[0]["memory_entries"], 2);
    assert_eq!(stages[1]["memory_entries"], 0);
    assert_eq!(stages[2]["memory_entries"], serde_json::Value::Null);
    assert_eq!(
        stages[2]["memory_path"],
        format!("{RECORD}/construction/code-generation/memory.md")
    );

    // The approved stage with an empty diary is an event, written once:
    // compiling again writes the same graph and no second event.
    let events = |ledger: &str| ledger.matches("**Event**: MEMORY_EMPTY").count();
    let ledger = fs::read_to_string(&ledger_path).unwrap();
    assert_eq!(events(&ledger), 1);
    assert!(
        ledger.ends_with("**Event**: MEMORY_EMPTY\n**Stage**: requirements-analysis\n\n---\n"),
        "{ledger}"
    );
    let again = runtime(&root, &["compile"], &[]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert_eq!(fs::read(&graph_path).unwrap(), bytes);
    assert_eq!(fs::read_to_string(&ledger_path).unwrap(), ledger);

    let read = runtime(&root, &["read", "requirements-analysis"], &[]);
    assert!(read.status.success(), "{}", text(&read.stderr));
    let row: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(row, stages[1]);
    assert_eq!(
        refusal(&runtime(&root, &["read", "nope"], &[])),
        "focus runtime read: no row for slug \"nope\"\n"
    );
    assert_eq!(
        refusal(&runtime(&root, &["read"], &[])),
        "focus runtime read: stage slug required\n"
    );

    let summary = runtime(&root, &["summary", "--json"], &[]);
    assert!(summary.status.success(), "{}", text(&summary.stderr));
    let numbers: serde_json::Value = serde_json::from_slice(&summary.stdout).unwrap();
    assert_eq!(
        numbers["stages"],
        serde_json::json!({"total": 3, "approved": 2, "failed": 0, "pending": 1})
    );
    assert_eq!(numbers["duration_minutes"], 9);
    let page = text(&runtime(&root, &["summary"], &[]).stdout);
    assert!(
        page.starts_with("Session Summary\n===============\n"),
        "{page}"
    );
    assert!(
        page.contains("  construction   0/1 approved, 1 pending\n"),
        "{page}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_empty_diary_is_recorded_once_for_each_approval() {
    let ledger_path = format!("{RECORD}/audit/laptop-abc123.md");
    let events_after = |name: &str, already: &str| {
        let root = project(name);
        with_workflow(&root);
        let ledger = fs::read_to_string(root.join(&ledger_path)).unwrap() + already;
        write(&root, &ledger_path, &ledger);
        let compiled = runtime(&root, &["compile"], &[]);
        assert!(compiled.status.success(), "{}", text(&compiled.stderr));
        let written = fs::read_to_string(root.join(&ledger_path)).unwrap();
        let _ = fs::remove_dir_all(root);
        written.matches("**Event**: MEMORY_EMPTY").count()
            - already.matches("**Event**: MEMORY_EMPTY").count()
    };
    let recorded =
        |at: &str, stage: &str| row("MEMORY_EMPTY", at, &[&format!("**Stage**: {stage}")]);
    // The stage was approved at 10:09 with nothing in its diary.
    assert_eq!(events_after("empty-none", ""), 1);
    // Recorded at or after that approval: nothing more to write.
    assert_eq!(
        events_after(
            "empty-after",
            &recorded("2026-01-05T10:09:00Z", "requirements-analysis")
        ),
        0
    );
    // Recorded before it, for an earlier approval: this one is new.
    assert_eq!(
        events_after(
            "empty-before",
            &recorded("2026-01-05T10:08:59Z", "requirements-analysis")
        ),
        1
    );
    // Another stage's record says nothing about this one.
    assert_eq!(
        events_after(
            "empty-other",
            &recorded("2026-01-05T10:30:00Z", "intent-capture")
        ),
        1
    );
}

#[test]
fn before_there_is_anything_to_read_the_graph_says_only_the_scope() {
    // No state file: nothing is written, and that is no failure.
    let root = project("early");
    fs::remove_file(root.join(RECORD).join("focus-state.md")).unwrap();
    let skipped = runtime(&root, &["compile"], &[]);
    assert!(skipped.status.success());
    assert!(skipped.stdout.is_empty());
    assert_eq!(
        text(&skipped.stderr),
        "focus runtime: no focus-state.md, skipping (likely pre-init)\n"
    );
    assert!(!root.join(RECORD).join("runtime-graph.json").exists());
    assert_eq!(
        refusal(&runtime(&root, &["summary"], &[])),
        "focus runtime summary: no runtime-graph.json found \u{2014} run a workflow first\n"
    );

    // A state file and no ledger: a graph with no stages.
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        "- **Scope**: bugfix\n",
    );
    let compiled = runtime(&root, &["compile"], &[]);
    assert!(compiled.status.success(), "{}", text(&compiled.stderr));
    assert_eq!(
        fs::read_to_string(root.join(RECORD).join("runtime-graph.json")).unwrap(),
        "{\n  \"workflow_id\": \"\",\n  \"scope\": \"bugfix\",\n  \"started_at\": \"\",\n  \"stages\": []\n}\n"
    );
    // It needs no stage graph, and a workflow does.
    fs::remove_file(root.join(".claude/tools/data/stage-graph.json")).unwrap();
    assert!(runtime(&root, &["compile"], &[]).status.success());
    with_workflow(&root);
    let without = refusal(&runtime(&root, &["compile"], &[]));
    assert!(
        without.starts_with("focus runtime compile: Stage graph not readable at "),
        "{without}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_units_order_is_in_the_graph_when_the_block_can_be_read() {
    let root = project("units");
    with_workflow(&root);
    let units = format!("{RECORD}/inception/units-generation/unit-of-work-dependency.md");
    write(
        &root,
        &units,
        "```yaml\nunits:\n  - name: api\n    depends_on: [auth]\n  - name: auth\n    kind: service\n```\n",
    );
    assert!(runtime(&root, &["compile"], &[]).status.success());
    let graph: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(RECORD).join("runtime-graph.json")).unwrap())
            .unwrap();
    assert_eq!(
        graph["bolt_dag"],
        serde_json::json!({
            "units": [
                {"name": "api", "depends_on": ["auth"]},
                {"name": "auth", "depends_on": [], "kind": "service"},
            ],
            "batches": [["auth"], ["api"]],
        })
    );
    // A block that cannot be read gives no order, says why, and stops nothing.
    write(
        &root,
        &units,
        "```yaml\nunits:\n  - name: api\n    depends_on: [nobody]\n```\n",
    );
    let compiled = runtime(&root, &["compile"], &[]);
    assert!(compiled.status.success());
    assert_eq!(
        text(&compiled.stderr),
        "focus runtime: unit-of-work-dependency.md edge block malformed (unit \"api\" depends on unknown unit \"nobody\"); bolt_dag node omitted\n"
    );
    let graph: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(RECORD).join("runtime-graph.json")).unwrap())
            .unwrap();
    assert!(graph.get("bolt_dag").is_none());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn an_observer_compiles_nothing() {
    let root = project("observer");
    with_workflow(&root);
    let ledger_path = root.join(RECORD).join("audit/laptop-abc123.md");
    let ledger = fs::read(&ledger_path).unwrap();
    for marker in ["FOCUS_STOP_HOOK_PROBE", "FOCUS_ROUTE_CHECK"] {
        // The event for the empty diary comes first, and is where it stops.
        let stopped = refusal(&runtime(&root, &["compile"], &[(marker, "1")]));
        assert_eq!(
            stopped,
            format!(
                "focus runtime compile: An engine observer ({marker}) reached the durable write primitive \"appendAuditBlockAtPath\". Observers read the current directive and must change no project bytes. This is an engine defect, not a workflow problem.\n"
            )
        );
        assert_eq!(fs::read(&ledger_path).unwrap(), ledger);
        assert!(!root.join(RECORD).join("runtime-graph.json").exists());
        // Not even the lock an observer would have waited at.
        assert!(!root.join("focus/.focus-lock").exists());
        assert!(!root.join("focus/.focus-lock.owner").exists());
    }
    // With nothing to record, it stops at the graph.
    fs::remove_file(
        root.join(RECORD)
            .join("inception/requirements-analysis/memory.md"),
    )
    .unwrap();
    let stopped = refusal(&runtime(&root, &["compile"], &[("FOCUS_ROUTE_CHECK", "1")]));
    assert!(
        stopped.contains("the durable write primitive \"writeFileAtomic\""),
        "{stopped}"
    );
    assert!(!root.join(RECORD).join("runtime-graph.json").exists());
    assert!(!root.join("focus/.focus-lock").exists());
    // The verbs that write nothing answer an observer as they answer anyone.
    let read = runtime(&root, &["read", "x"], &[("FOCUS_STOP_HOOK_PROBE", "1")]);
    assert_eq!(
        refusal(&read),
        "focus runtime read: no row for slug \"x\"\n"
    );
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Upstream's usage lists `fragment-fork` and
/// `fragment-merge`, which copy the graph into a bolt's worktree and remove
/// it again. Focus has no bolts or worktrees yet: its usage lists the verbs
/// it has, and it refuses those two by name.
#[test]
fn the_runtime_command_names_the_verbs_it_has() {
    let root = project("usage");
    assert_eq!(
        refusal(&runtime(&root, &[], &[])),
        "Usage: focus engine runtime <subcommand>. Valid: compile, read, summary. Run with --help for detail.\n"
    );
    assert_eq!(
        refusal(&runtime(&root, &["nope"], &[])),
        "Unknown subcommand: nope. Run focus engine runtime --help for usage.\n"
    );
    for verb in ["fragment-fork", "fragment-merge"] {
        assert_eq!(
            refusal(&runtime(&root, &[verb, "--slug", "one"], &[])),
            format!(
                "{verb} copies the runtime graph into a bolt's worktree or removes it again, and Focus does not work with bolts and worktrees yet.\n"
            )
        );
    }
    for help in ["--help", "-h"] {
        let help = runtime(&root, &[help], &[]);
        assert!(help.status.success());
        assert!(
            text(&help.stdout)
                .starts_with("Usage: focus engine runtime <verb> [--project-dir <dir>]\n")
        );
    }
    // The project may be named before the verb.
    let elsewhere = std::env::temp_dir();
    let named = Command::new(env!("CARGO_BIN_EXE_focus"))
        .args([
            "engine",
            "runtime",
            "--project-dir",
            root.to_str().unwrap(),
            "compile",
        ])
        .current_dir(elsewhere)
        .output()
        .unwrap();
    assert!(named.status.success(), "{}", text(&named.stderr));
    assert!(root.join(RECORD).join("runtime-graph.json").exists());
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Upstream ignores an argument it does not know, a
/// flag given a second time and a flag that came without its value; Focus
/// refuses each, as every command of the engine does.
#[test]
fn the_runtime_command_refuses_what_it_does_not_know() {
    let root = project("stray");
    with_workflow(&root);
    assert_eq!(
        refusal(&runtime(&root, &["compile", "now"], &[])),
        "focus runtime compile: Unexpected argument: now\n"
    );
    assert_eq!(
        refusal(&runtime(&root, &["summary", "--yaml"], &[])),
        "focus runtime summary: Unexpected argument: --yaml\n"
    );
    assert_eq!(
        refusal(&runtime(&root, &["summary", "--json", "--json"], &[])),
        "focus runtime summary: Unexpected argument: --json\n"
    );
    let here = root.to_str().unwrap();
    assert_eq!(
        refusal(&runtime(
            &root,
            &["compile", "--project-dir", here, "--project-dir", here],
            &[]
        )),
        "focus runtime compile: Unexpected argument: --project-dir\n"
    );
    assert_eq!(
        refusal(&runtime(&root, &["compile", "--project-dir"], &[])),
        "Missing required flag: --project-dir <value>\n"
    );
    assert!(!root.join(RECORD).join("runtime-graph.json").exists());
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Upstream sends a metric for each event it appends
/// to the configured endpoint. Focus sends none yet, and refuses the append
/// instead of leaving the endpoint without what it expects: a compile that
/// has an empty diary to record stops, and writes neither event nor graph.
#[test]
fn a_compile_does_not_record_an_event_it_cannot_send_a_metric_for() {
    let root = project("metrics");
    with_workflow(&root);
    let ledger_path = root.join(RECORD).join("audit/laptop-abc123.md");
    let ledger = fs::read(&ledger_path).unwrap();
    let endpoint = [("FOCUS_METRICS_ENDPOINT", "http://127.0.0.1:9/x")];
    assert_eq!(
        refusal(&runtime(&root, &["compile"], &endpoint)),
        "focus runtime compile: FOCUS_METRICS_ENDPOINT is set, and Focus does not send metrics yet. Unset it to write to the audit ledger.\n"
    );
    assert_eq!(fs::read(&ledger_path).unwrap(), ledger);
    assert!(!root.join(RECORD).join("runtime-graph.json").exists());
    // With no event to record there is no metric to send, and it compiles.
    fs::remove_file(
        root.join(RECORD)
            .join("inception/requirements-analysis/memory.md"),
    )
    .unwrap();
    assert!(runtime(&root, &["compile"], &endpoint).status.success());
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Whether a sensor that never ended counts as
/// incomplete in a stage that is still open depends on how long ago it
/// fired, and upstream asks JavaScript's `Date.parse`, which reads a text
/// with no zone in the machine's own. Focus refuses the compile and names
/// the timestamp. A word where a timestamp goes is no date to either engine,
/// and is no obstacle.
#[test]
fn a_compile_stops_at_a_timestamp_it_would_have_to_guess() {
    let root = project("timestamps");
    with_workflow(&root);
    let ledger_path = format!("{RECORD}/audit/laptop-abc123.md");
    let ledger = fs::read_to_string(root.join(&ledger_path)).unwrap();
    let fired = |at: &str| {
        row(
            "SENSOR_FIRED",
            at,
            &[
                "**Stage slug**: code-generation",
                "**Sensor ID**: linter",
                "**Fire id**: aaaaaaaa",
            ],
        )
    };
    write(
        &root,
        &ledger_path,
        &(ledger.clone() + &fired("2026-01-05T10:11:00")),
    );
    let stopped = refusal(&runtime(&root, &["compile"], &[]));
    assert!(
        stopped.starts_with("focus runtime compile: The sensor linter fired in code-generation, which is still open,")
            && stopped.contains("the timestamp 2026-01-05T10:11:00 is not an instant Focus reads"),
        "{stopped}"
    );
    assert!(!root.join(RECORD).join("runtime-graph.json").exists());

    write(&root, &ledger_path, &(ledger + &fired("yesterday")));
    assert!(runtime(&root, &["compile"], &[]).status.success());
    let read = runtime(&root, &["read", "code-generation"], &[]);
    let row: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(row["sensor_firings"], serde_json::json!([]));
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. On a `runtime-graph.json` that is not shaped as
/// the engine writes it, upstream fails with what its runtime says of the
/// value (`undefined is not an object (evaluating 'graph.stages')`). Focus
/// says the file is not a runtime graph and how to write it again.
#[test]
fn a_file_that_is_not_a_runtime_graph_is_said_to_be_one() {
    let root = project("not-a-graph");
    let graph = format!("{RECORD}/runtime-graph.json");
    let wrong = |what: &str| {
        format!(
            "runtime-graph.json is not a runtime graph as the engine writes it: {what}. Run `focus engine runtime compile` to write it again.\n"
        )
    };
    write(&root, &graph, "[]\n");
    assert_eq!(
        refusal(&runtime(&root, &["read", "x"], &[])),
        format!("focus runtime read: {}", wrong("it has no list `stages`"))
    );
    assert_eq!(
        refusal(&runtime(&root, &["summary"], &[])),
        format!("focus runtime summary: {}", wrong("it is not an object"))
    );
    write(&root, &graph, "{\"stages\": [null]}\n");
    assert_eq!(
        refusal(&runtime(&root, &["read", "x"], &[])),
        format!(
            "focus runtime read: {}",
            wrong("an entry of `stages` is null")
        )
    );
    // Not JSON at all: both engines say so, each in its parser's words.
    write(&root, &graph, "{ nope");
    assert!(refusal(&runtime(&root, &["read", "x"], &[])).starts_with("focus runtime read: "));
    // A row is printed as it is, whatever else it holds.
    write(
        &root,
        &graph,
        "{\"stages\": [7, {\"stage_slug\": \"x\", \"2\": 1, \"1\": [1.0, -0.0]}]}\n",
    );
    let read = runtime(&root, &["read", "x"], &[]);
    assert_eq!(
        text(&read.stdout),
        "{\n  \"1\": [\n    1,\n    0\n  ],\n  \"2\": 1,\n  \"stage_slug\": \"x\"\n}\n"
    );
    let _ = fs::remove_dir_all(root);
}

fn edit_stage_graph(root: &Path, change: impl FnOnce(&mut Vec<serde_json::Value>)) {
    let path = root.join(".claude/tools/data/stage-graph.json");
    let mut stages: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    change(&mut stages);
    fs::write(&path, serde_json::to_vec_pretty(&stages).unwrap()).unwrap();
}

fn stage<'a>(stages: &'a mut [serde_json::Value], slug: &str) -> &'a mut serde_json::Value {
    stages
        .iter_mut()
        .find(|stage| stage["slug"] == slug)
        .expect("the stage graph has the stage")
}

/// Deliberate deviation. On a `stage-graph.json` that is not shaped as
/// `focus engine graph compile` writes it, upstream fails with what its
/// runtime says of the value (`The "paths[1]" property must be of type
/// string, got undefined`, `null is not an object (evaluating 's.enabled')`)
/// or, in a summary, counts a stage whose phase is a number under that
/// number's name. Focus says the stage graph is not one the compile wrote.
#[test]
fn a_stage_graph_the_compile_did_not_write_is_refused() {
    let again = "The stage graph is written by `focus engine graph compile`: compile it again.\n";
    let root = project("stage-graph");
    with_workflow(&root);
    let graph_path = root.join(RECORD).join("runtime-graph.json");
    assert!(runtime(&root, &["compile"], &[]).status.success());
    let compiled = fs::read(&graph_path).unwrap();
    let original = fs::read(root.join(".claude/tools/data/stage-graph.json")).unwrap();
    let restore = || {
        fs::write(root.join(".claude/tools/data/stage-graph.json"), &original).unwrap();
    };

    // A stage the workflow ran, with no phase: where its diary is cannot be
    // told. The graph compiled before stays as it was.
    edit_stage_graph(&root, |stages| {
        stage(stages, "requirements-analysis")
            .as_object_mut()
            .unwrap()
            .remove("phase");
    });
    assert_eq!(
        refusal(&runtime(&root, &["compile"], &[])),
        format!(
            "focus runtime compile: stage-graph.json gives the stage requirements-analysis no phase that is text, so where its diary is cannot be told. {again}"
        )
    );
    assert_eq!(fs::read(&graph_path).unwrap(), compiled);
    // A summary needs no diary: to it, as to upstream, the phase is unknown.
    let summary = runtime(&root, &["summary", "--json"], &[]);
    assert!(summary.status.success(), "{}", text(&summary.stderr));
    let numbers: serde_json::Value = serde_json::from_slice(&summary.stdout).unwrap();
    assert_eq!(numbers["by_phase"]["unknown"]["total"], 1);

    // So it is when the phase is written as `null`.
    restore();
    edit_stage_graph(&root, |stages| {
        stage(stages, "requirements-analysis")["phase"] = serde_json::Value::Null;
    });
    let summary = runtime(&root, &["summary", "--json"], &[]);
    assert!(summary.status.success(), "{}", text(&summary.stderr));
    let numbers: serde_json::Value = serde_json::from_slice(&summary.stdout).unwrap();
    assert_eq!(numbers["by_phase"]["unknown"]["total"], 1);

    // A phase that is not text.
    restore();
    edit_stage_graph(&root, |stages| {
        stage(stages, "intent-capture")["phase"] = serde_json::json!(7);
    });
    assert_eq!(
        refusal(&runtime(&root, &["summary", "--json"], &[])),
        format!(
            "focus runtime summary: stage-graph.json gives the stage intent-capture a phase that is not text. {again}"
        )
    );
    assert!(refusal(&runtime(&root, &["compile"], &[])).contains("no phase that is text"));

    // A row that is no stage at all, and a file that is no list of them.
    restore();
    edit_stage_graph(&root, |stages| stages.insert(3, serde_json::Value::Null));
    for verb in ["compile", "summary"] {
        let refused = refusal(&runtime(&root, &[verb], &[]));
        assert!(
            refused.starts_with(&format!("focus runtime {verb}: Stage graph at "))
                && refused.ends_with("has a row that is null, not a stage\n"),
            "{refused}"
        );
    }
    fs::write(
        root.join(".claude/tools/data/stage-graph.json"),
        "{\"stages\": []}\n",
    )
    .unwrap();
    let refused = refusal(&runtime(&root, &["compile"], &[]));
    assert!(refused.ends_with("is not a list of stages\n"), "{refused}");
    assert_eq!(fs::read(&graph_path).unwrap(), compiled);

    // What the compile could have written is read as upstream reads it: a
    // slug the graph has twice is the later row.
    restore();
    edit_stage_graph(&root, |stages| {
        let mut later = stage(stages, "intent-capture").clone();
        later["phase"] = serde_json::json!("operation");
        later["lead_agent"] = serde_json::json!("the-later-row");
        stages.push(later);
    });
    assert!(runtime(&root, &["compile"], &[]).status.success());
    let read = runtime(&root, &["read", "intent-capture"], &[]);
    let row: serde_json::Value = serde_json::from_slice(&read.stdout).unwrap();
    assert_eq!(row["agent"], "the-later-row");
    assert_eq!(
        row["memory_path"],
        format!("{RECORD}/operation/intent-capture/memory.md")
    );
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Upstream keeps the phases of a summary as the keys
/// of a plain object, so a stage whose phase is named after something every
/// object inherits (`constructor`) is counted on that inherited value and
/// its phase is not listed. Focus lists the phase like any other.
#[test]
fn a_phase_named_after_an_object_member_is_listed() {
    let root = project("constructor");
    with_workflow(&root);
    assert!(runtime(&root, &["compile"], &[]).status.success());
    edit_stage_graph(&root, |stages| {
        stage(stages, "intent-capture")["phase"] = serde_json::json!("constructor");
    });
    let summary = runtime(&root, &["summary", "--json"], &[]);
    assert!(summary.status.success(), "{}", text(&summary.stderr));
    let numbers: serde_json::Value = serde_json::from_slice(&summary.stdout).unwrap();
    assert_eq!(
        numbers["by_phase"]["constructor"],
        serde_json::json!({"total": 1, "approved": 1, "failed": 0, "pending": 0})
    );
    let page = text(&runtime(&root, &["summary"], &[]).stdout);
    assert!(page.contains("  constructor"), "{page}");
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. Upstream finds the calling conversation by walking
/// the process tree, resolves a delegated worktree to the main checkout's
/// intent, and lets a unit claim name the audit shard. Focus has ported none
/// of the three, and a compile refuses while the file that asks for one is
/// there: it reads no ledger it might have chosen wrongly and writes no
/// graph.
#[test]
fn a_compile_does_not_guess_whose_record_it_is_compiling() {
    for (name, marker, says) in [
        (
            "sessions",
            "focus/.focus-sessions/pids/1.json",
            "Focus cannot yet tell which conversation is calling",
        ),
        (
            "worktree",
            ".focus/worktree-meta.json",
            "Focus does not work inside delegated worktrees yet",
        ),
        (
            "claim",
            "focus/.focus-unit-scope.json",
            "Focus does not work with unit claims yet",
        ),
    ] {
        let root = project(&format!("unported-{name}"));
        with_workflow(&root);
        write(&root, marker, "{}\n");
        let ledger_path = root.join(RECORD).join("audit/laptop-abc123.md");
        let ledger = fs::read(&ledger_path).unwrap();
        let refused = refusal(&runtime(&root, &["compile"], &[]));
        assert!(
            refused.starts_with("focus runtime compile: ") && refused.contains(says),
            "{refused}"
        );
        assert_eq!(fs::read(&ledger_path).unwrap(), ledger);
        assert!(!root.join(RECORD).join("runtime-graph.json").exists());
        let _ = fs::remove_dir_all(root);
    }
}

/// Deliberate deviation. With the workspace directory a link, upstream
/// leaves out the shards behind it, so it sees no workflow, and writes an
/// empty graph through the link. Focus takes no lock and writes nothing
/// through a link, and says which directory is one.
#[cfg(unix)]
#[test]
fn a_compile_writes_nothing_through_a_workspace_that_is_a_link() {
    let root = project("workspace-link");
    with_workflow(&root);
    fs::rename(root.join("focus"), root.join("moved")).unwrap();
    std::os::unix::fs::symlink(root.join("moved"), root.join("focus")).unwrap();
    assert_eq!(
        refusal(&runtime(&root, &["compile"], &[])),
        "focus runtime compile: focus is a link, so Focus changed nothing there. Replace the link with a real folder or file, then run this again.\n"
    );
    let record = root.join("moved/spaces/default/intents/add-login-1a2b3c4d");
    assert!(!record.join("runtime-graph.json").exists());
    assert!(!root.join("moved/.focus-lock").exists());
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. With a backslash in the project's path, upstream's
/// shard reader takes it for a separator, finds no shard and compiles a
/// graph with no stages. To Focus, where a backslash is no separator, it is
/// a character of the name, and the ledger is read.
#[cfg(unix)]
#[test]
fn a_backslash_in_the_projects_path_is_a_character_of_it() {
    let outer = project("backslash");
    let root = outer.join("back\\slash");
    copy_dir(&outer.join(".claude"), &root.join(".claude"));
    fs::rename(outer.join("focus"), root.join("focus")).unwrap();
    with_workflow(&root);
    let compiled = runtime(&outer, &["compile", "--project-dir", "back\\slash"], &[]);
    assert!(compiled.status.success(), "{}", text(&compiled.stderr));
    let graph: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(RECORD).join("runtime-graph.json")).unwrap())
            .unwrap();
    assert_eq!(graph["stages"].as_array().unwrap().len(), 3);
    let _ = fs::remove_dir_all(outer);
}

#[test]
fn a_compile_writes_under_the_workspace_lock() {
    let root = project("locked");
    let graph_path = root.join(RECORD).join("runtime-graph.json");
    let ledger_path = root.join(RECORD).join("audit/laptop-abc123.md");
    let layout = focus_workspace::Layout::new(&root, focus_harness::Harness::Claude);
    let wait = [("FOCUS_LOCK_TIMEOUT_MS", "300")];
    let timed_out = |output: &Output| {
        let message = refusal(output);
        assert!(
            message.starts_with(
                "focus runtime compile: Timed out after 300ms waiting for the workspace lock at "
            ),
            "{message}"
        );
    };

    // Before there is a ledger: the graph that says only the scope is written
    // under the lock too.
    let held = focus_workspace::WorkspaceLock::acquire(&layout, "another session").unwrap();
    timed_out(&runtime(&root, &["compile"], &wait));
    assert!(!graph_path.exists());

    // With a workflow: neither the event nor the graph.
    with_workflow(&root);
    let ledger = fs::read_to_string(&ledger_path).unwrap();
    timed_out(&runtime(&root, &["compile"], &wait));
    assert!(!graph_path.exists());
    assert_eq!(fs::read_to_string(&ledger_path).unwrap(), ledger);

    // The ledger is read again once the lock is held. While a compile waits,
    // whoever holds the lock records the empty diary; the compile, let in,
    // finds it recorded and writes no second event.
    let mut waiting = Command::new(env!("CARGO_BIN_EXE_focus"));
    waiting
        .args(["engine", "runtime", "compile"])
        .current_dir(&root)
        .env("FOCUS_LOCK_TIMEOUT_MS", "30000")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (name, _) in std::env::vars() {
        if name != "FOCUS_LOCK_TIMEOUT_MS"
            && (name.starts_with("FOCUS_") || name == "CLAUDE_PROJECT_DIR")
        {
            waiting.env_remove(name);
        }
    }
    let waiting = waiting.spawn().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let recorded = row(
        "MEMORY_EMPTY",
        "2026-01-05T10:09:00Z",
        &["**Stage**: requirements-analysis"],
    );
    fs::write(&ledger_path, ledger + &recorded).unwrap();
    drop(held);
    let done = waiting.wait_with_output().unwrap();
    assert!(done.status.success(), "{}", text(&done.stderr));
    assert!(graph_path.exists());
    assert_eq!(
        fs::read_to_string(&ledger_path)
            .unwrap()
            .matches("**Event**: MEMORY_EMPTY")
            .count(),
        1
    );
    let _ = fs::remove_dir_all(root);
}

/// The project's path is settled by reading it, before anything is read
/// through it: `link/focus/..` is `link`, wherever the link leads. Asking the
/// disk instead finds the same project under another name, the ledger's
/// shards under that name are not the project's, and a graph with no stages
/// would be written over the right one.
#[cfg(unix)]
#[test]
fn a_project_named_through_a_link_and_dots_is_the_same_project() {
    let root = project("link-dots");
    with_workflow(&root);
    std::os::unix::fs::symlink(&root, root.join("lnk")).unwrap();
    for named in ["lnk/focus/..", "lnk/./focus/../", "focus/.."] {
        let _ = fs::remove_file(root.join(RECORD).join("runtime-graph.json"));
        let compiled = runtime(&root, &["compile", "--project-dir", named], &[]);
        assert!(compiled.status.success(), "{}", text(&compiled.stderr));
        let graph: serde_json::Value = serde_json::from_slice(
            &fs::read(root.join(RECORD).join("runtime-graph.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(graph["stages"].as_array().unwrap().len(), 3, "{named}");
    }
    let _ = fs::remove_dir_all(root);
}

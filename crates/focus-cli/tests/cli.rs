//! The `focus` binary, run as a process against the vendored fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn focus(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_focus"))
        .args(args)
        .output()
        .expect("focus runs")
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("focus-cli-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A Claude Code project as upstream's installer leaves it.
fn installed(name: &str) -> PathBuf {
    let root = scratch(name);
    copy_dir(&fixtures().join("installed/claude"), &root);
    root
}

fn focus_in(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    command.args(args).current_dir(dir);
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

#[test]
fn compile_writes_upstreams_bytes_and_check_agrees() {
    let core = fixtures().join("core");
    let out = scratch("compile");
    for harness in ["claude", "cursor", "copilot"] {
        let dir = out.join(harness);
        let run = focus(&[
            "engine",
            "graph",
            "compile",
            "--harness",
            harness,
            "--core",
            core.to_str().unwrap(),
            "--out",
            dir.to_str().unwrap(),
        ]);
        assert!(run.status.success(), "{}", text(&run.stderr));
        for file in ["stage-graph.json", "scope-grid.json"] {
            let golden = fixtures().join("golden").join(harness).join(file);
            assert_eq!(
                fs::read(dir.join(file)).unwrap(),
                fs::read(golden).unwrap(),
                "{harness} {file}"
            );
        }
        let golden_dir = fixtures().join("golden").join(harness);
        let check = focus(&[
            "engine",
            "graph",
            "compile",
            "--harness",
            harness,
            "--core",
            core.to_str().unwrap(),
            "--check",
            golden_dir.to_str().unwrap(),
        ]);
        assert!(check.status.success(), "{}", text(&check.stderr));
    }
    let _ = fs::remove_dir_all(out);
}

#[test]
fn check_reports_drift_with_the_command_to_run() {
    let core = fixtures().join("core");
    // Claude's compiled graph names `.claude/sensors/...`; Cursor's must not match it.
    let claude = fixtures().join("golden/claude");
    let run = focus(&[
        "engine",
        "graph",
        "compile",
        "--harness",
        "cursor",
        "--core",
        core.to_str().unwrap(),
        "--check",
        claude.to_str().unwrap(),
    ]);
    assert_eq!(run.status.code(), Some(1));
    assert_eq!(
        text(&run.stderr),
        "stage-graph.json is out of date. Run `focus engine graph compile` to regenerate.\n"
    );
}

fn make_stale(root: &Path) {
    let stage = root.join(".claude/focus-common/stages/ideation/intent-capture.md");
    let edited =
        fs::read_to_string(&stage)
            .unwrap()
            .replacen("condition: ", "condition: changed ", 1);
    fs::write(&stage, edited).unwrap();
}

#[test]
fn compile_rewrites_an_installed_tree_in_place() {
    let root = installed("in-place");
    let graph = root.join(".claude/tools/data/stage-graph.json");
    let before = fs::read(&graph).unwrap();
    let project = [
        "--project-dir",
        root.to_str().unwrap(),
        "--harness",
        "claude",
    ];
    let run = |verb: &[&str]| focus(&[&["engine", "graph"], verb, &project].concat());

    assert!(run(&["compile", "--check"]).status.success());
    make_stale(&root);
    let stale = run(&["compile", "--check"]);
    assert_eq!(stale.status.code(), Some(1));
    assert_eq!(fs::read(&graph).unwrap(), before, "--check writes nothing");

    let compiled = run(&["compile"]);
    assert!(compiled.status.success(), "{}", text(&compiled.stderr));
    assert!(compiled.stdout.is_empty() && compiled.stderr.is_empty());
    assert_ne!(fs::read(&graph).unwrap(), before);
    assert!(run(&["compile", "--check"]).status.success());
    // No temporary file and no owner note are left behind.
    let leftovers: Vec<String> = fs::read_dir(root.join(".claude/tools/data"))
        .unwrap()
        .chain(fs::read_dir(root.join("focus")).unwrap())
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".tmp") || name.ends_with(".owner"))
        .collect();
    assert_eq!(leftovers, Vec::<String>::new());
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_compile_waits_for_the_workspace_lock_and_says_who_holds_it() {
    let root = installed("locked");
    make_stale(&root);
    let graph = root.join(".claude/tools/data/stage-graph.json");
    let before = fs::read(&graph).unwrap();
    let args = [
        "engine",
        "graph",
        "compile",
        "--project-dir",
        root.to_str().unwrap(),
        "--harness",
        "claude",
    ];

    let layout = focus_workspace::Layout::new(&root, focus_harness::Harness::Claude);
    let held = focus_workspace::WorkspaceLock::acquire(&layout, "another session").unwrap();
    let blocked = focus_in(&root, &args, &[("FOCUS_LOCK_TIMEOUT_MS", "300")]);
    assert_eq!(blocked.status.code(), Some(1));
    let message = text(&blocked.stderr);
    assert!(
        message.starts_with(
            "focus graph compile: Timed out after 300ms waiting for the workspace lock at "
        ),
        "{message}"
    );
    assert!(
        message.contains(&format!(
            "held by pid {} (another session) for ",
            std::process::id()
        )),
        "{message}"
    );
    assert_eq!(
        fs::read(&graph).unwrap(),
        before,
        "nothing is written without the lock"
    );

    drop(held);
    let free = focus_in(&root, &args, &[("FOCUS_LOCK_TIMEOUT_MS", "300")]);
    assert!(free.status.success(), "{}", text(&free.stderr));
    assert_ne!(fs::read(&graph).unwrap(), before);
    let _ = fs::remove_dir_all(root);
}

#[test]
fn concurrent_compiles_leave_one_consistent_result() {
    let root = installed("concurrent");
    make_stale(&root);
    let args = [
        "engine",
        "graph",
        "compile",
        "--project-dir",
        root.to_str().unwrap(),
        "--harness",
        "claude",
    ];
    let runs: Vec<_> = (0..6)
        .map(|_| {
            Command::new(env!("CARGO_BIN_EXE_focus"))
                .args(args)
                .spawn()
                .unwrap()
        })
        .collect();
    for mut run in runs {
        assert!(run.wait().unwrap().success());
    }
    let check = focus(&[
        "engine",
        "graph",
        "compile",
        "--check",
        "--project-dir",
        root.to_str().unwrap(),
        "--harness",
        "claude",
    ]);
    assert!(check.status.success(), "{}", text(&check.stderr));
    let _ = fs::remove_dir_all(root);
}

/// The form that compiles outside a project never writes into an installed
/// tree: that happens only under the workspace lock.
#[test]
fn the_outside_form_does_not_write_into_an_installed_tree() {
    let root = installed("outside");
    make_stale(&root);
    let harness = root.join(".claude");
    let data = harness.join("tools/data");
    let graph = data.join("stage-graph.json");
    let installed_bytes = fs::read(&graph).unwrap();
    let base = [
        "engine",
        "graph",
        "compile",
        "--harness",
        "claude",
        "--installed",
        harness.to_str().unwrap(),
    ];

    let implicit = focus(&base);
    let explicit = focus(&[&base[..], &["--out", data.to_str().unwrap()]].concat());
    for run in [&implicit, &explicit] {
        assert_eq!(run.status.code(), Some(1));
        assert!(
            text(&run.stderr).contains("this form does not write into an installed tree"),
            "{}",
            text(&run.stderr)
        );
    }
    assert_eq!(fs::read(&graph).unwrap(), installed_bytes);

    // Nor into any other installed tree, whatever it reads from.
    let core = fixtures().join("core");
    let from_core = focus(&[
        "engine",
        "graph",
        "compile",
        "--harness",
        "claude",
        "--core",
        core.to_str().unwrap(),
        "--out",
        data.to_str().unwrap(),
    ]);
    assert_eq!(from_core.status.code(), Some(1));
    assert!(
        text(&from_core.stderr).contains("this form does not write into an installed tree"),
        "{}",
        text(&from_core.stderr)
    );
    assert_eq!(fs::read(&graph).unwrap(), installed_bytes);

    // Writing elsewhere is fine, and --check sees the stale file.
    let elsewhere = root.join("out");
    let run = focus(&[&base[..], &["--out", elsewhere.to_str().unwrap()]].concat());
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert_ne!(
        fs::read(elsewhere.join("stage-graph.json")).unwrap(),
        installed_bytes
    );
    let check = focus(&[&base[..], &["--check"]].concat());
    assert_eq!(check.status.code(), Some(1));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_project_and_harness_are_found_from_where_the_command_runs() {
    let root = installed("discovery");
    let nested = root.join("src/deep/inside");
    fs::create_dir_all(&nested).unwrap();

    // No flag and no variable: the nearest directory with a harness is the project.
    let run = focus_in(&nested, &["engine", "graph", "scope", "bugfix"], &[]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    assert_eq!(text(&run.stdout).lines().next(), Some("workspace-scaffold"));

    // A host-provided project directory wins over the walk, and FOCUS_PROJECT_DIR over that.
    let elsewhere = scratch("discovery-elsewhere");
    let host = focus_in(
        &nested,
        &["engine", "graph", "topo"],
        &[("CLAUDE_PROJECT_DIR", elsewhere.to_str().unwrap())],
    );
    assert_eq!(host.status.code(), Some(1));
    assert!(
        text(&host.stderr).starts_with("focus graph topo: No harness is installed in "),
        "{}",
        text(&host.stderr)
    );
    let named = focus_in(
        &nested,
        &["engine", "graph", "topo"],
        &[
            ("CLAUDE_PROJECT_DIR", elsewhere.to_str().unwrap()),
            ("FOCUS_PROJECT_DIR", root.to_str().unwrap()),
        ],
    );
    assert!(named.status.success(), "{}", text(&named.stderr));

    // A second harness makes the choice ambiguous until it is named.
    fs::create_dir_all(root.join(".cursor/tools/data")).unwrap();
    fs::write(
        root.join(".cursor/tools/data/harness.json"),
        "{\"name\":\"cursor\"}",
    )
    .unwrap();
    let ambiguous = focus_in(&nested, &["engine", "graph", "topo"], &[]);
    assert_eq!(ambiguous.status.code(), Some(1));
    assert!(
        text(&ambiguous.stderr).ends_with(
            "has several harnesses installed (claude, cursor). Pass --harness <name>.\n"
        ),
        "{}",
        text(&ambiguous.stderr)
    );
    for choice in [
        focus_in(
            &nested,
            &["engine", "graph", "topo", "--harness", "claude"],
            &[],
        ),
        focus_in(
            &nested,
            &["engine", "graph", "topo"],
            &[("FOCUS_HARNESS", "claude")],
        ),
    ] {
        assert!(choice.status.success(), "{}", text(&choice.stderr));
    }
    let _ = fs::remove_dir_all(root);
    let _ = fs::remove_dir_all(elsewhere);
}

#[test]
fn resolve_is_gated_and_prints_a_plan() {
    let root = installed("resolve");
    let args = ["engine", "graph", "resolve", "bugfix", "--stdout"];
    let gated = focus_in(&root, &args, &[]);
    assert_eq!(gated.status.code(), Some(1));
    assert_eq!(
        text(&gated.stderr),
        "focus graph resolve is gated behind FOCUS_GRAPH_RESOLVE=1 (rollout flag).\n"
    );
    let open = focus_in(&root, &args, &[("FOCUS_GRAPH_RESOLVE", "1")]);
    assert!(open.status.success(), "{}", text(&open.stderr));
    let plan: serde_json::Value = serde_json::from_slice(&open.stdout).unwrap();
    assert_eq!(plan.as_array().unwrap().len(), 33);
    assert_eq!(
        plan[0],
        serde_json::json!({"slug": "workspace-scaffold", "phase": "initialization", "action": "EXECUTE"})
    );

    // Without --stdout the plan goes into the active intent record, which
    // nothing in Focus can locate yet; a named file works.
    let no_target = focus_in(
        &root,
        &["engine", "graph", "resolve", "bugfix"],
        &[("FOCUS_GRAPH_RESOLVE", "1")],
    );
    assert_eq!(no_target.status.code(), Some(1));
    assert!(text(&no_target.stderr).contains("Pass --stdout, or set FOCUS_PLAN_PATH to a file."));
    let target = root.join("plan.json");
    let written = focus_in(
        &root,
        &["engine", "graph", "resolve", "bugfix"],
        &[
            ("FOCUS_GRAPH_RESOLVE", "1"),
            ("FOCUS_PLAN_PATH", target.to_str().unwrap()),
        ],
    );
    assert!(written.status.success(), "{}", text(&written.stderr));
    assert_eq!(fs::read(&target).unwrap(), open.stdout);
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. With `stage-graph.json` missing, upstream's
/// `compile --check` surfaces the runtime's file-not-found error. Focus says
/// what a caller needs to know: the file is out of date, and how to rebuild it.
#[test]
fn check_on_a_tree_without_a_compiled_graph_says_it_is_out_of_date() {
    let root = installed("check-missing");
    fs::remove_file(root.join(".claude/tools/data/stage-graph.json")).unwrap();
    let run = focus_in(&root, &["engine", "graph", "compile", "--check"], &[]);
    assert_eq!(run.status.code(), Some(1));
    assert_eq!(
        text(&run.stderr),
        "stage-graph.json is out of date. Run `focus engine graph compile` to regenerate.\n"
    );
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. When a scope file does not load, upstream's
/// `validate-grid --matched` swallows the error, takes the scope's Guard
/// Policy default to be "strict", and prints a verdict about Guard Policy.
/// Focus reports the scope file, which is the actual problem.
#[test]
fn a_broken_scope_file_is_reported_not_turned_into_a_verdict() {
    let root = installed("broken-scope");
    let scope = root.join(".claude/scopes/focus-bugfix.md");
    let edited = fs::read_to_string(&scope)
        .unwrap()
        .replace("skeleton: off", "skeleton: maybe");
    fs::write(&scope, edited).unwrap();
    let grid: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".claude/tools/data/scope-grid.json")).unwrap())
            .unwrap();
    fs::write(
        root.join("proposal.json"),
        serde_json::json!({"stages": grid["bugfix"]["stages"], "guardPolicy": "off"}).to_string(),
    )
    .unwrap();
    let run = focus_in(
        &root,
        &[
            "engine",
            "graph",
            "validate-grid",
            "--proposal",
            "proposal.json",
            "--matched",
            "bugfix",
        ],
        &[],
    );
    assert_eq!(run.status.code(), Some(1));
    assert!(run.stdout.is_empty());
    assert!(
        text(&run.stderr).ends_with(
            "focus-bugfix.md has invalid skeleton value \"maybe\". Expected \"on\" or \"off\".\n"
        ),
        "{}",
        text(&run.stderr)
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn queries_read_a_compiled_graph() {
    let graph = fixtures().join("golden/claude/stage-graph.json");
    let graph = graph.to_str().unwrap();
    let topo = focus(&["engine", "graph", "topo", "--graph", graph]);
    let order = text(&topo.stdout);
    assert_eq!(order.lines().count(), 33);
    assert_eq!(order.lines().next(), Some("workspace-scaffold"));
    assert_eq!(order.lines().last(), Some("feedback-optimization"));

    let producers = focus(&[
        "engine",
        "graph",
        "producers",
        "requirements",
        "--graph",
        graph,
    ]);
    assert_eq!(text(&producers.stdout), "requirements-analysis\n");
    let nobody = focus(&[
        "engine",
        "graph",
        "producers",
        "no-such-artifact",
        "--graph",
        graph,
    ]);
    assert!(nobody.status.success() && nobody.stdout.is_empty());
}

#[test]
fn mistakes_are_named_not_ignored() {
    let cases: [(&[&str], &str); 4] = [
        (
            &[
                "engine",
                "graph",
                "compile",
                "--harness",
                "kiro",
                "--core",
                "x",
                "--out",
                "y",
            ],
            "focus graph compile: Unknown harness: kiro. Valid: claude, cursor, copilot\n",
        ),
        (
            &[
                "engine",
                "graph",
                "compile",
                "--harness",
                "claude",
                "--core",
                "x",
                "--installed",
                "y",
                "--out",
                "z",
            ],
            "focus graph compile: Pass exactly one of --core <dir> or --installed <dir>\n",
        ),
        (
            &["engine", "graph", "topo", "--graph", "x", "--verbose"],
            "focus graph topo: Unexpected argument: --verbose\n",
        ),
        (
            &["engine", "graph", "frobnicate"],
            "Unknown subcommand: frobnicate. Valid: ars, artifacts, compile, consumers, cycles, export, producers, resolve, scope, topo, validate-grid, validate-scope\n",
        ),
    ];
    for (args, expected) in cases {
        let run = focus(args);
        assert_eq!(run.status.code(), Some(1), "{args:?}");
        assert_eq!(text(&run.stderr), expected, "{args:?}");
    }
}

#[cfg(unix)]
#[test]
fn an_argument_that_is_not_unicode_is_refused_as_a_mistake() {
    use std::os::unix::ffi::OsStrExt;

    let run = Command::new(env!("CARGO_BIN_EXE_focus"))
        .args(["engine", "graph", "topo", "--graph"])
        .arg(std::ffi::OsStr::from_bytes(b"/tmp/\xff/graph.json"))
        .output()
        .expect("focus runs");
    assert_eq!(run.status.code(), Some(1));
    assert!(
        text(&run.stderr).starts_with("Argument 5 is not valid Unicode: "),
        "{}",
        text(&run.stderr)
    );
}

#[test]
fn every_kind_of_oracle_case_is_answered_by_some_crate() {
    let cases = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle/cases.ndjson");
    let run = focus(&["dev", "oracle", cases.to_str().unwrap()]);
    assert!(run.status.success(), "{}", text(&run.stderr));
    let answers = text(&run.stdout);
    assert_eq!(
        answers.lines().count(),
        fs::read_to_string(&cases).unwrap().lines().count()
    );
    let orphan = answers
        .lines()
        .find(|line| line.contains("\"out\":{\"unsupported\":"));
    assert_eq!(orphan, None);
}

#[test]
fn compiling_where_nothing_is_installed_creates_nothing() {
    let empty = scratch("not-a-project");
    let run = focus_in(
        &empty,
        &["engine", "graph", "compile", "--harness", "claude"],
        &[],
    );
    assert_eq!(run.status.code(), Some(1));
    assert!(
        text(&run.stderr)
            .starts_with("focus graph compile: compile requires an installed project harness at "),
        "{}",
        text(&run.stderr)
    );
    assert_eq!(fs::read_dir(&empty).unwrap().count(), 0);
    let _ = fs::remove_dir_all(empty);
}

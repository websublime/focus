//! `focus engine utility`, run as a process on an installed project.
//!
//! What `detect` answers is held to upstream's by the recorded differential,
//! which replays on Unix. These tests run everywhere: they cover the
//! command's surface, what a refusal leaves behind, and each way Focus
//! deliberately differs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RECORD: &str = "focus/spaces/default/intents/add-login-1a2b3c4d";
const STATE: &str =
    "# Focus State\n\n## Workflow\n- **Scope**: feature\n- **Status**: In Progress\n";

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
    let root = std::env::temp_dir().join(format!("focus-utility-{}-{name}", std::process::id()));
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

fn utility(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    command
        .args(["engine", "utility"])
        .args(args)
        .current_dir(root);
    for (name, _) in std::env::vars() {
        if name.starts_with("FOCUS_") || name == "CLAUDE_PROJECT_DIR" {
            command.env_remove(name);
        }
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

#[test]
fn detect_scans_the_project_and_names_where_the_scopes_live() {
    let root = project("detect");
    let empty = ok(&utility(&root, &["detect"]));
    // The project is found from the working directory, which the process
    // reads as the system gives it: resolved on macOS (`/private/var/…` for
    // a temp dir), in its short form on a Windows runner. Either spelling
    // of the root is read as `<root>`.
    let real = fs::canonicalize(&root).unwrap();
    let real = real
        .to_str()
        .unwrap()
        .strip_prefix(r"\\?\")
        .map_or_else(|| real.clone(), PathBuf::from);
    let rooted = |text: &str| {
        text.replace(&real.display().to_string(), "<root>")
            .replace(&root.display().to_string(), "<root>")
    };
    let scopes = Path::new("<root>")
        .join(".claude")
        .join("scopes")
        .display()
        .to_string();
    let grid = Path::new("<root>")
        .join(".claude")
        .join("tools")
        .join("data")
        .join("scope-grid.json")
        .display()
        .to_string();
    assert_eq!(
        rooted(&empty),
        format!(
            "Project type: Greenfield\nLanguages: Unknown\nFrameworks: Unknown\nBuild system: Unknown\nScopes dir: {scopes}\nScope grid: {grid}\nProposal file: focus/spaces/default/intents/.focus-engine/composer-proposal.json\nValid scopes: bugfix, classic, enterprise, express, feature, infra, mvp, poc, refactor, security-patch, workshop\n"
        )
    );
    write(&root, "src/a.ts", "");
    write(&root, "src/b.ts", "");
    write(&root, "c.js", "");
    write(&root, "package.json", r#"{"dependencies":{"react":"18"}}"#);
    write(&root, "vite.config.ts", "");
    write(&root, "pnpm-lock.yaml", "");
    write(
        &root,
        ".gitmodules",
        "[submodule \"one\"]\npath = libs/one\nurl = u\n[submodule \"two\"]\npath = two\n",
    );
    write(&root, "libs/one/.git", "gitdir: x\n");
    let scanned = ok(&utility(&root, &["detect"]));
    assert!(
        scanned.starts_with("Project type: Brownfield\nLanguages: TypeScript, JavaScript\nFrameworks: Vite, React\nBuild system: pnpm (package.json)\nSubmodules: 2 declared, 1 uninitialized\nScopes dir: "),
        "{scanned}"
    );
    let json = ok(&utility(&root, &["detect", "--json"]));
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["projectType"], "Brownfield");
    assert_eq!(parsed["languages"], "TypeScript, JavaScript");
    assert_eq!(parsed["buildSystem"], "pnpm (package.json)");
    assert_eq!(
        parsed["submodules"],
        serde_json::json!([
            {"name": "one", "path": "libs/one", "url": "u", "initialized": true},
            {"name": "two", "path": "two", "url": "", "initialized": false},
        ])
    );
    // A path in JSON carries its separators escaped, so it is read first.
    assert_eq!(rooted(parsed["scopesDir"].as_str().unwrap()), scopes);
    assert_eq!(rooted(parsed["scopeGridPath"].as_str().unwrap()), grid);
    assert_eq!(
        parsed["proposalPath"],
        "focus/spaces/default/intents/.focus-engine/composer-proposal.json"
    );
    assert_eq!(parsed["scopes"][0], "bugfix");
    assert!(parsed.get("nestedRoot").is_none());
    // The keys in upstream's order.
    assert!(json.starts_with("{\"projectType\":\"Brownfield\",\"languages\":"));
    // A nested project, once the root holds no code of its own.
    for name in [
        "src",
        "c.js",
        "package.json",
        "vite.config.ts",
        "pnpm-lock.yaml",
        ".gitmodules",
        "libs",
    ] {
        let path = root.join(name);
        if path.is_dir() {
            fs::remove_dir_all(path).unwrap();
        } else {
            fs::remove_file(path).unwrap();
        }
    }
    write(&root, "services/api/go.mod", "");
    write(&root, "services/api/main.go", "");
    let nested = ok(&utility(&root, &["detect", "--json"]));
    let parsed: serde_json::Value = serde_json::from_str(&nested).unwrap();
    assert_eq!(parsed["nestedRoot"], "services/api");
    assert_eq!(parsed["languages"], "Go");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn now_prints_the_engines_time() {
    let root = project("now");
    let stamp = ok(&utility(&root, &["now"]));
    assert!(stamp.len() == 21 && stamp.ends_with("Z\n"), "{stamp}");
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_refusal_is_recorded_beside_the_record_named() {
    let root = project("errors");
    let unknown = refusal(&utility(&root, &["nope", "--intent", "add-login-1a2b3c4d"]));
    assert!(
        unknown.starts_with("Unknown command \"nope\"."),
        "{unknown}"
    );
    let ledger = shard(&root);
    assert!(
        ledger.contains("**Event**: ERROR_LOGGED\n**Tool**: focus-utility\n**Command**: focus-utility nope --intent add-login-1a2b3c4d\n**Error**: Unknown command \"nope\"."),
        "{ledger}"
    );
    // A record that is not there is no place to write, and the refusal is
    // printed alone.
    refusal(&utility(
        &root,
        &["nope", "--intent", "fix-logout-9f8e7d6c"],
    ));
    assert_eq!(shard(&root), ledger);
    // A `--space` or `--intent` with no value, or a blank one, names nothing:
    // the row goes beside the active record.
    refusal(&utility(&root, &["nope", "--space"]));
    refusal(&utility(&root, &["nope", "--intent", " "]));
    let rows = shard(&root);
    assert_eq!(rows.matches("**Event**: ERROR_LOGGED").count(), 3, "{rows}");
    assert!(
        rows.contains("**Command**: focus-utility nope --space\n"),
        "{rows}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn the_utility_names_the_verbs_it_has() {
    let root = project("verbs");
    let help = ok(&utility(&root, &["--help"]));
    assert!(
        help.starts_with("Usage: focus engine utility <verb>"),
        "{help}"
    );
    // Deliberate deviation: upstream refuses a missing verb as "undefined"
    // and an unknown one with its 34 verbs listed; Focus lists its own.
    assert_eq!(
        refusal(&utility(&root, &[])),
        "Usage: focus engine utility <verb>. Valid: detect, intent-create, now. Run with --help for detail."
    );
    assert_eq!(
        refusal(&utility(&root, &["nope"])),
        "Unknown command \"nope\". Run `focus engine utility --help` for what this tool can do.\n\nAvailable commands: detect, intent-create, now\nCommon options: [--project-dir <path>] [--json]"
    );
    // Deliberate deviation: a verb upstream has and Focus does not yet is
    // refused by name.
    assert_eq!(
        refusal(&utility(&root, &["status"])),
        "status prints the workflow's status, and Focus does not do that yet."
    );
    assert_eq!(
        refusal(&utility(&root, &["init"])),
        "init is the retired name of intent-create, and Focus does not do that yet."
    );
    // Deliberate deviation: upstream scans whatever directory is named and
    // reports the scopes of its own install; Focus scans the project it is
    // installed in.
    let elsewhere = root.join("elsewhere");
    write(&elsewhere, "x.py", "");
    let refused = refusal(&utility(
        &root,
        &["detect", "--project-dir", elsewhere.to_str().unwrap()],
    ));
    assert!(
        refused.starts_with("Claude Code is not installed in ")
            && refused
                .ends_with(": `focus engine utility detect` scans the project it is installed in."),
        "{refused}"
    );
    let _ = fs::remove_dir_all(root);
}

fn state(root: &Path, args: &[&str]) -> Output {
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
    command.output().expect("focus runs")
}

#[test]
fn intent_create_mints_a_record_and_selects_it() {
    let root = project("create");
    fs::remove_dir_all(root.join(RECORD)).unwrap();
    fs::remove_file(root.join("focus/spaces/default/intents/active-intent")).unwrap();
    let summary = ok(&utility(
        &root,
        &[
            "intent-create",
            "--scope",
            "feature",
            "--arguments",
            "Add login to the app",
            "--label",
            "add login",
        ],
    ));
    let intents = root.join("focus/spaces/default/intents");
    let dir_name = fs::read_dir(&intents)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .find(|name| name.ends_with("-add-login"))
        .expect("a record directory named after the day and the label");
    assert_eq!(dir_name.len(), "261014-add-login".len());
    assert_eq!(
        summary,
        format!(
            "Intent created: {dir_name} (space: default)\nState initialized: feature scope, 32 stages, Standard depth\nProject type: Greenfield\nLanguages: Unknown\nFrameworks: Unknown\nBuild System: Unknown\nFirst post-init stage: intent-capture (IDEATION)\n"
        )
    );
    let record = intents.join(&dir_name);
    let state_text = fs::read_to_string(record.join("focus-state.md")).unwrap();
    assert!(state_text.starts_with("# Focus State Tracking\n\n## Project Information\n- **Project**: Add login to the app\n- **Project Description Source**: project-description.json\n- **Project Type**: Greenfield\n- **Project Type Source**: workspace scan\n- **Scope**: feature\n- **Start Date**: "), "{state_text}");
    assert!(
        state_text.contains(
            "- **Guard Policy**: off (from scope feature)\n- **Sensors**: on (from scope feature)\n"
        ),
        "{state_text}"
    );
    assert!(state_text.contains("### IDEATION PHASE\n- [-] intent-capture \u{2014} EXECUTE\n- [ ] market-research \u{2014} EXECUTE\n"), "{state_text}");
    assert!(
        state_text
            .contains("- **Stages to Skip**: 2.1 (reverse-engineering \u{2014} greenfield)\n"),
        "{state_text}"
    );
    assert_eq!(
        fs::read_to_string(record.join("project-description.json")).unwrap(),
        "\"Add login to the app\"\n"
    );
    assert_eq!(
        fs::read_to_string(intents.join("active-intent")).unwrap(),
        format!("{dir_name}\n")
    );
    let registry = fs::read_to_string(intents.join("intents.json")).unwrap();
    assert!(registry.contains(&format!("\"dirName\": \"{dir_name}\",\n    \"scope\": \"feature\",\n    \"status\": \"in-flight\"")), "{registry}");
    assert!(record.join(".focus-engine/engine-touch").is_file());
    assert!(record.join(".focus-engine/creation-receipt").is_file());
    for phase in [
        "initialization",
        "ideation",
        "inception",
        "construction",
        "operation",
        "verification",
    ] {
        assert!(record.join(phase).is_dir(), "{phase}");
    }
    assert!(root.join("focus/active-memory/org.md").is_file());
    let ledger = fs::read_to_string(record.join("audit/laptop-abc123.md")).unwrap();
    assert!(
        ledger.starts_with("# Focus Audit Log\n\n## Workflow Start\n"),
        "{ledger}"
    );
    assert!(ledger.contains("**Event**: WORKFLOW_STARTED\n**Scope**: feature\n**Request**: /focus Add login to the app\n"), "{ledger}");
    assert_eq!(ledger.matches("**Event**: ").count(), 15, "{ledger}");
    assert!(ledger.ends_with("**Event**: STAGE_STARTED\n**Stage**: intent-capture\n**Agent**: focus-product-agent\n\n---\n"), "{ledger}");
    // The record is the active one: the state command reads it.
    assert_eq!(ok(&state(&root, &["get", "Scope"])), "feature\n");
    assert_eq!(ok(&state(&root, &["count", "completed"])), "3\n");
    // The same label on the same day gets a counter.
    let again = ok(&utility(
        &root,
        &["intent-create", "--scope", "bugfix", "--label", "add login"],
    ));
    assert!(
        again.starts_with(&format!(
            "Intent created: {dir_name}-2 (space: default)\nState initialized: bugfix scope, "
        )),
        "{again}"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn intent_create_refuses_before_writing() {
    let root = project("refused");
    let intents = root.join("focus/spaces/default/intents");
    let before: Vec<String> = fs::read_dir(&intents)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    let help = ok(&utility(&root, &["intent-create", "--help"]));
    assert!(
        help.starts_with("Usage: focus engine utility intent-create --scope <scope>"),
        "{help}"
    );
    assert!(
        refusal(&utility(&root, &["intent-create"]))
            .starts_with("intent-create refused: no --scope, --arguments, or --label given.")
    );
    assert!(
        refusal(&utility(
            &root,
            &["intent-create", "--scope", "feature", "stray"]
        ))
        .starts_with("intent-create does not accept positional arguments, but received \"stray\".")
    );
    assert_eq!(
        refusal(&utility(
            &root,
            &[
                "intent-create",
                "--scope",
                "feature",
                "--label",
                " ",
                "--depth"
            ]
        )),
        "intent-create refused: --label, --depth require a nonblank value."
    );
    assert!(
        refusal(&utility(&root, &["intent-create", "--scope", "nope"]))
            .starts_with("Unknown scope: \"nope\". Valid scopes: bugfix, classic, ")
    );
    assert_eq!(
        refusal(&utility(
            &root,
            &[
                "intent-create",
                "--scope",
                "feature",
                "--skip",
                "nope",
                "--add",
                "workspace-scaffold"
            ]
        )),
        "intent-create refused: --skip names \"nope\", which is not a stage. --add names \"workspace-scaffold\", an initialization stage; those always run."
    );
    assert_eq!(
        refusal(&utility(
            &root,
            &["intent-create", "--scope", "feature", "--label", "help"]
        )),
        "\"help\" is a reserved name and cannot be an intent label. Pick a label that describes the work."
    );
    let lowered = refusal(&utility(
        &root,
        &[
            "intent-create",
            "--scope",
            "enterprise",
            "--label",
            "x",
            "--guard-policy",
            "off",
        ],
    ));
    assert!(lowered.starts_with("Creating this intent with Guard Policy off would lower fences, which is the person's call."), "{lowered}");
    // Deliberate deviation: an engine question's answer is the question
    // store's (M5); upstream looks the question up, Focus refuses the flag.
    assert!(
        refusal(&utility(&root, &["intent-create", "--request", "q-1"]))
            .starts_with("--request q-1 names an engine question's answer")
    );
    let after: Vec<String> = fs::read_dir(&intents)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(before, after);
    // Every refusal above left its row beside the active record.
    assert_eq!(shard(&root).matches("**Event**: ERROR_LOGGED").count(), 8);
    let _ = fs::remove_dir_all(root);
}

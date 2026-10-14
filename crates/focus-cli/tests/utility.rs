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
    let json = rooted(&ok(&utility(&root, &["detect", "--json"])));
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
    assert_eq!(parsed["scopesDir"], scopes);
    assert_eq!(parsed["scopeGridPath"], grid);
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
        "Usage: focus engine utility <verb>. Valid: detect, now. Run with --help for detail."
    );
    assert_eq!(
        refusal(&utility(&root, &["nope"])),
        "Unknown command \"nope\". Run `focus engine utility --help` for what this tool can do.\n\nAvailable commands: detect, now\nCommon options: [--project-dir <path>] [--json]"
    );
    // Deliberate deviation: a verb upstream has and Focus does not yet is
    // refused by name.
    assert_eq!(
        refusal(&utility(&root, &["status"])),
        "status prints the workflow's status, and Focus does not do that yet."
    );
    assert_eq!(
        refusal(&utility(&root, &["intent-create", "--scope", "feature"])),
        "intent-create creates an intent record, and Focus does not do that yet."
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

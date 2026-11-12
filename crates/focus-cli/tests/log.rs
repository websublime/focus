//! `focus engine log`: what the differential cannot say for Focus, because
//! upstream does something else there on purpose. Everything the two agree
//! on is the differential's (`log/*`) and the recording's.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const RECORD: &str = "focus/spaces/default/intents/add-login-1a2b3c4d";
const STATE: &str = "# Focus State\n\n## Workflow\n- **Scope**: feature\n- **Status**: In Progress\n- **Summary Confirmation**: on (from scope feature)\n\n## Stage Progress\n- [-] intent-capture \u{2014} EXECUTE\n";
const SHARD: &str = "# Focus Audit Log\n";

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

/// An installed project with one piece of work, whose stages owe a summary
/// confirmation.
fn project(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("focus-log-{}-{name}", std::process::id()));
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
    write(&root, &format!("{RECORD}/audit/laptop-abc123.md"), SHARD);
    root
}

fn log(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    for (name, _) in std::env::vars() {
        if name.starts_with("FOCUS_") || name.starts_with("CLAUDE") {
            command.env_remove(name);
        }
    }
    command
        .args(["engine", "log"])
        .args(args)
        .arg("--project-dir")
        .arg(root)
        .output()
        .unwrap()
}

/// The sentence a refusal prints, and the row it leaves.
fn refusal(root: &Path, args: &[&str]) -> String {
    let before = fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap();
    let output = log(root, args);
    assert_eq!(output.status.code(), Some(1), "{args:?}");
    assert!(output.stdout.is_empty(), "{args:?}");
    let err = String::from_utf8_lossy(&output.stderr).into_owned();
    let parsed: serde_json::Value =
        serde_json::from_str(err.trim_end()).unwrap_or_else(|_| panic!("{err}"));
    let said = parsed["error"].as_str().unwrap().to_owned();
    let after = fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap();
    let row = after.strip_prefix(&before).unwrap();
    assert!(row.contains("**Event**: ERROR_LOGGED\n"), "{row}");
    assert!(row.contains("**Tool**: focus-log\n"), "{row}");
    assert!(!row.contains("DECISION_RECORDED"), "{row}");
    said
}

const ASK: [&str; 5] = [
    "decision",
    "--stage",
    "intent-capture",
    "--decision",
    "Which database?",
];

fn asking(more: &[&'static str]) -> Vec<&'static str> {
    ASK.iter().chain(more).copied().collect()
}

/// Deliberate deviation. `answer`, `answers`, `link` and `review` are the
/// slices after this one (2b to 2e): upstream runs them, Focus names each
/// as a verb it does not have yet.
#[test]
fn the_verbs_that_are_not_ported_are_refused_by_name() {
    let root = project("verbs");
    for verb in ["answer", "answers", "link", "review"] {
        assert_eq!(
            refusal(&root, &[verb, "--stage", "intent-capture"]),
            format!(
                "log {verb} is not ported yet: Focus records the questions of a stage (log decision), and not yet their answers, a pipeline's handoffs or the reviews."
            )
        );
    }
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. A question the engine pairs with a record of its
/// own (a summary confirmation, Construction's checkpoints, Plan Approval),
/// a question about one unit of work, a batch of Plan Approval questions,
/// and the message that already answered a question: upstream records each
/// with what it needs; Focus refuses each by name until the slice that
/// brings it, and records nothing.
#[test]
fn the_questions_that_are_not_ported_are_refused_by_name() {
    let root = project("questions");
    for (more, said) in [
        (
            vec!["--checkpoint", "summary-confirmation"],
            "--checkpoint summary-confirmation records the question of a stage's summary confirmation, and Focus does not record summary confirmations yet.",
        ),
        (
            vec!["--checkpoint", "verification-command"],
            "--checkpoint verification-command records a question of Construction's checkpoints, and Focus does not run Construction yet.",
        ),
        (
            vec!["--checkpoint", "construction-policy"],
            "--checkpoint construction-policy records a question of Construction's checkpoints, and Focus does not run Construction yet.",
        ),
        (
            vec!["--checkpoint", "plan-approval"],
            "--checkpoint plan-approval records the Plan Approval question, and Focus does not run Plan Approval yet.",
        ),
        (
            vec!["--unit", "api"],
            "--unit records a question about one unit of work, and Focus does not run units yet.",
        ),
        (
            vec!["--batch-file", "b.json"],
            "--batch-file records the Plan Approval questions of several units of work at once, and Focus does not run Plan Approval yet.",
        ),
        (
            vec!["--message", "m-1"],
            "--message names the message of the person that answered a question logged after it, and Focus does not read the message store here yet.",
        ),
        (
            vec!["--options", "Looks correct,Request changes"],
            "\"intent-capture\" owes a consolidated summary confirmation, and this question offers its two choices: it is recorded with `--checkpoint summary-confirmation --questions-file <path>`, and Focus does not record summary confirmations yet.",
        ),
    ] {
        assert_eq!(refusal(&root, &asking(&more)), said, "{more:?}");
    }
    // The summary's two choices are known however they are capitalised,
    // decorated or ended.
    let owed = "\"intent-capture\" owes a consolidated summary confirmation, and this question offers its two choices: it is recorded with `--checkpoint summary-confirmation --questions-file <path>`, and Focus does not record summary confirmations yet.";
    for options in [
        "LOOKS CORRECT!, request changes (Recommended), ",
        "Looks correct.,Request changes!!",
        " ,Request changes,,Looks correct",
    ] {
        assert_eq!(
            refusal(&root, &asking(&["--options", options])),
            owed,
            "{options}"
        );
    }
    // Any other pair, one choice alone and three of them are ordinary questions.
    for options in [
        "Looks correct,Other",
        "Looks correct",
        "Looks correct?,Request changes",
        "Looks correct,Request changes,Looks correct",
    ] {
        let output = log(&root, &asking(&["--options", options]));
        assert_eq!(output.status.code(), Some(0), "{options}");
    }
    // A question with other options is an ordinary one, and is recorded.
    let output = log(&root, &asking(&["--options", "Postgres,SQLite"]));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"emitted\":\"DECISION_RECORDED\",\"stage\":\"intent-capture\"}\n"
    );
    let ledger = fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap();
    assert!(
        ledger.ends_with("**Event**: DECISION_RECORDED\n**Stage**: intent-capture\n**Decision**: Which database?\n**Options**: Postgres,SQLite\n\n---\n"),
        "{ledger}"
    );
    let _ = fs::remove_dir_all(root);
}

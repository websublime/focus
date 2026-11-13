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

/// Deliberate deviation. `link` and `review` are slices to come (2d and
/// 2e): upstream runs them, Focus names each as a verb it does not have
/// yet.
#[test]
fn the_verbs_that_are_not_ported_are_refused_by_name() {
    let root = project("verbs");
    for verb in ["link", "review"] {
        assert_eq!(
            refusal(&root, &[verb, "--stage", "intent-capture"]),
            format!(
                "log {verb} is not ported yet: Focus records the questions of a stage and their answers (log decision, log answer, log answers), and not yet a pipeline's handoffs or the reviews."
            )
        );
    }
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. What an answer may carry that Focus has not
/// ported: its text in a file, a choice the person left to the agent, the
/// engine's own questions, a unit of work. Upstream reads what each needs;
/// Focus refuses each by name and records nothing.
#[test]
fn the_answers_that_are_not_ported_are_refused_by_name() {
    let root = project("answers");
    let reply = [
        "answer",
        "--stage",
        "intent-capture",
        "--details",
        "Postgres",
    ];
    let with = |more: &[&'static str]| -> Vec<&'static str> {
        reply.iter().chain(more).copied().collect()
    };
    let plan = "and Focus does not run Plan Approval yet.";
    for (args, said) in [
        (
            vec!["answer", "--stage", "intent-capture", "--details-file", "a.txt"],
            "--details-file passes text in a file the agent wrote, and Focus does not read an answer's text from a file yet.".to_owned(),
        ),
        (
            with(&["--on-instruction-file", "i.txt"]),
            "--on-instruction-file passes text in a file the agent wrote, and Focus does not read an answer's text from a file yet.".to_owned(),
        ),
        (
            with(&["--on-instruction", "you decide"]),
            "--on-instruction records a choice the person left to the agent, and Focus does not record that yet.".to_owned(),
        ),
        (
            with(&["--checkpoint", "guard-recovery"]),
            "--checkpoint guard-recovery records the remedy the person picked after a refusal, and Focus does not ask the recovery question yet.".to_owned(),
        ),
        (
            with(&["--checkpoint", "summary-confirmation"]),
            "--checkpoint summary-confirmation records the person's choice at a stage's summary confirmation, and Focus does not record summary confirmations yet.".to_owned(),
        ),
        (
            with(&["--checkpoint", "verification-command"]),
            "--checkpoint verification-command records an answer at one of Construction's checkpoints, and Focus does not run Construction yet.".to_owned(),
        ),
        (
            with(&["--checkpoint", "construction-policy"]),
            "--checkpoint construction-policy records an answer at one of Construction's checkpoints, and Focus does not run Construction yet.".to_owned(),
        ),
        (
            with(&["--checkpoint", "plan-approval"]),
            format!("--checkpoint plan-approval records the Plan Approval choice, {plan}"),
        ),
        (
            with(&["--override", "because"]),
            format!("--override records a break-glass reason for Plan Approval, {plan}"),
        ),
        (
            with(&["--override-file", "o.txt"]),
            format!("--override-file records a break-glass reason for Plan Approval, {plan}"),
        ),
        (
            with(&["--batch-file", "b.json"]),
            format!("--batch-file records the Plan Approval choices of several units of work at once, {plan}"),
        ),
        (
            with(&["--unit", "api"]),
            "--unit records an answer about one unit of work, and Focus does not run units yet.".to_owned(),
        ),
    ] {
        assert_eq!(refusal(&root, &args), said, "{args:?}");
    }
    // The summary of a stage that owes one, as the question open for it.
    write(
        &root,
        &format!("{RECORD}/audit/laptop-abc123.md"),
        &format!(
            "{}\n## Decision Recorded\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: DECISION_RECORDED\n**Stage**: intent-capture\n**Decision**: Does this look right?\n**Options**: Looks correct,Request changes\n\n---\n\n## Human Turn\n**Timestamp**: 2026-01-05T10:02:00Z\n**Event**: HUMAN_TURN\n**Session**: s1\n\n---\n",
            fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap()
        ),
    );
    let owed = "\"intent-capture\" owes a consolidated summary confirmation, and the question open for it is that summary: its answer is recorded with `--checkpoint summary-confirmation --questions-file <path>`, and Focus does not record summary confirmations yet.";
    let looks = [
        "answer",
        "--stage",
        "intent-capture",
        "--details",
        "Looks correct",
    ];
    assert_eq!(refusal(&root, &looks), owed);
    // The same when the summary was recorded with its checkpoint.
    let row = |event: &str, minute: u32, fields: &str| {
        format!(
            "\n## Row\n**Timestamp**: 2026-01-05T10:{minute:02}:00Z\n**Event**: {event}\n**Stage**: intent-capture\n{fields}\n---\n"
        )
    };
    let shard = format!("{RECORD}/audit/laptop-abc123.md");
    write(
        &root,
        &shard,
        &format!(
            "{SHARD}{}",
            row(
                "DECISION_RECORDED",
                1,
                "**Decision**: Right?\n**Checkpoint**: Consolidated Summary Confirmation\n"
            )
        ),
    );
    assert_eq!(refusal(&root, &looks), owed);
    // Three questions, each with another after it: position orders the two
    // of one shard, the instant orders the shards. The questions of the
    // latest instant stand for all, and that one is the summary.
    let summary = "**Decision**: Right?\n**Options**: Looks correct,Request changes\n";
    write(
        &root,
        &shard,
        &format!(
            "{SHARD}{}{}",
            row("DECISION_RECORDED", 7, summary),
            row("DECISION_RECORDED", 5, "**Decision**: Which database?\n")
        ),
    );
    write(
        &root,
        &format!("{RECORD}/audit/desktop-def456.md"),
        &format!(
            "{SHARD}{}",
            row("DECISION_RECORDED", 6, "**Decision**: Which cache?\n")
        ),
    );
    assert_eq!(refusal(&root, &looks), owed);
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. A question the engine pairs with a record of its
/// own (a summary confirmation, Construction's checkpoints, Plan Approval),
/// a question about one unit of work and a batch of Plan Approval
/// questions: upstream records each with what it needs; Focus refuses each
/// by name until the slice that brings it, and records nothing.
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

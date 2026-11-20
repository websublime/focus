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

/// Deliberate deviation. What a review may carry that Focus has not
/// ported: one unit's review (slice 5). Upstream reads what it needs; Focus
/// refuses it by name and records nothing.
#[test]
fn a_units_review_is_refused_by_name() {
    let root = project("reviews");
    assert_eq!(
        refusal(
            &root,
            &[
                "review",
                "--stage",
                "intent-capture",
                "--reviewer",
                "focus-product-lead-agent",
                "--iteration",
                "1",
                "--unit",
                "api",
            ]
        ),
        "log review --unit is not ported yet: Focus records the review of a stage, and not yet the review of one unit (its claim, the plan of units, the unit's source and its manifest)."
    );
    let _ = fs::remove_dir_all(root);
}

/// What a recording cannot carry of `log review`, compared live by the
/// differential (`log/review/request/source_bound`): the request of a stage
/// that writes the workspace is bound to the project's source, which is the
/// whole tree, and a recording of this tool keeps the workspace only. The
/// row carries the source's fingerprint, and the listing behind it is kept
/// under the fingerprint's own start.
#[test]
fn a_request_of_a_stage_that_writes_the_workspace_is_bound_to_the_source() {
    let root = project("source");
    let graph_path = root.join(".claude/tools/data/stage-graph.json");
    let mut graph: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&graph_path).unwrap()).unwrap();
    for stage in graph.as_array_mut().unwrap() {
        if stage["slug"] == "intent-capture" {
            stage["workspace_requires"] = serde_json::json!(true);
        }
    }
    fs::write(&graph_path, serde_json::to_string_pretty(&graph).unwrap()).unwrap();
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &STATE.replace(
            "- **Summary Confirmation**: on (from scope feature)",
            "- **Summary Confirmation**: off (set by a command)\n- **Review Override**: advisory",
        ),
    );
    for name in [
        "intent-statement",
        "stakeholder-map",
        "intent-capture-questions",
    ] {
        write(
            &root,
            &format!("{RECORD}/ideation/intent-capture/{name}.md"),
            "# Written\n",
        );
    }
    write(&root, "src/app.py", "print('hello')\n");
    let output = log(
        &root,
        &[
            "review",
            "--stage",
            "intent-capture",
            "--reviewer",
            "focus-product-lead-agent",
            "--iteration",
            "1",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ledger = fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap();
    let fingerprint = ledger
        .lines()
        .find_map(|line| line.strip_prefix("**Source Fingerprint**: "))
        .unwrap_or_else(|| panic!("{ledger}"));
    assert_eq!(fingerprint.len(), 64, "{fingerprint}");
    let kept = fs::read_to_string(root.join(RECORD).join(format!(
        ".focus-engine/source-review/intent-capture/workspace-{}.tsv",
        &fingerprint[..12]
    )))
    .unwrap();
    assert!(
        kept.starts_with(&format!("workspace\t{fingerprint}\t")),
        "{kept}"
    );
    assert!(kept.contains("\tsrc/app.py\t100644\t"), "{kept}");
    let _ = fs::remove_dir_all(root);
}

const REVIEW_ASK: [&str; 7] = [
    "review",
    "--stage",
    "intent-capture",
    "--reviewer",
    "focus-product-lead-agent",
    "--iteration",
    "1",
];

const REVIEW: &str =
    "**Verdict:** READY\n**Reviewer:** focus-product-lead-agent\n**Iteration:** 1\n";

/// A project whose stage has its outputs and a review asked for: where the
/// project is, by its real path as a person reads it (a temporary directory
/// is behind a link on macOS and may be a short name on Windows), and the
/// file the request said the reviewer writes.
fn asked_for_a_review(name: &str) -> (PathBuf, PathBuf) {
    let root = fs::canonicalize(project(name)).unwrap();
    let root = match root.to_str().unwrap().strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => root,
    };
    write(
        &root,
        &format!("{RECORD}/focus-state.md"),
        &STATE.replace(
            "- **Summary Confirmation**: on (from scope feature)",
            "- **Summary Confirmation**: off (set by a command)\n- **Review Override**: advisory",
        ),
    );
    for name in [
        "intent-statement",
        "stakeholder-map",
        "intent-capture-questions",
    ] {
        write(
            &root,
            &format!("{RECORD}/ideation/intent-capture/{name}.md"),
            "# Written\n",
        );
    }
    let output = log(&root, &REVIEW_ASK);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let said: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let draft = root.join(said["reviewFile"].as_str().unwrap());
    (root, draft)
}

/// A review file named on the command line is the request's own file or it
/// is refused: a file elsewhere in the project, and a copy beside the
/// review. Named, the file stays where it is after the verdict. (On every
/// host: the recording that holds this is replayed on Unix only.)
#[test]
fn a_named_review_file_is_the_requests_own() {
    let (root, draft) = asked_for_a_review("named-review");
    fs::write(&draft, REVIEW).unwrap();
    let named = |file: &str| -> Vec<String> {
        REVIEW_ASK
            .iter()
            .copied()
            .chain(["--verdict", "READY", "--review-file", file])
            .map(str::to_owned)
            .collect()
    };
    let refused = |file: &str| {
        let args = named(file);
        refusal(&root, &args.iter().map(String::as_str).collect::<Vec<_>>())
    };

    write(&root, "x.md", REVIEW);
    assert!(
        refused("x.md").ends_with(
            "the review file x.md is not a plain readable file (the path is outside the active intent record)."
        ),
        "{}",
        refused("x.md")
    );
    let copy = draft.with_file_name("copy.review.md");
    fs::write(&copy, REVIEW).unwrap();
    let copy = copy.to_str().unwrap().to_owned();
    assert!(
        refused(&copy).contains(" is not the review file for iteration 1. "),
        "{}",
        refused(&copy)
    );

    let args = named(draft.to_str().unwrap());
    let output = log(&root, &args.iter().map(String::as_str).collect::<Vec<_>>());
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let ledger = fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap();
    assert!(ledger.contains("**Event**: REVIEW_COMPLETED\n"), "{ledger}");
    assert!(draft.exists(), "the named file stays");
    let _ = fs::remove_dir_all(root);
}

/// What a recording cannot carry of a verdict, compared live by the
/// differential (`log/review/verdict/review_file_a_link`,
/// `log/review/verdict/review_folder_a_link`): a link is no review file,
/// where the file goes or on the way to it, and no recording holds a link.
#[cfg(unix)]
#[test]
fn a_review_file_that_is_a_link_or_behind_one_is_refused() {
    let (root, draft) = asked_for_a_review("linked-review");
    let review = REVIEW;
    let verdict: Vec<&str> = REVIEW_ASK
        .iter()
        .copied()
        .chain(["--verdict", "READY"])
        .collect();

    // The file is a link to a review written elsewhere.
    write(&root, "elsewhere/review.md", review);
    std::os::unix::fs::symlink(root.join("elsewhere/review.md"), &draft).unwrap();
    let refused = refusal(&root, &verdict);
    assert!(
        refused.contains("is not a plain readable file (") && refused.contains(" is a symlink, "),
        "{refused}"
    );

    // The file is a plain one, in a folder that is a link.
    fs::remove_file(&draft).unwrap();
    let folder = draft.parent().unwrap();
    fs::rename(folder, root.join("moved")).unwrap();
    std::os::unix::fs::symlink(root.join("moved"), folder).unwrap();
    fs::write(root.join("moved").join(draft.file_name().unwrap()), review).unwrap();
    let refused = refusal(&root, &verdict);
    assert!(
        refused.contains("is not a plain readable file (") && refused.contains(" is a symlink, "),
        "{refused}"
    );
    assert!(
        !fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md"))
            .unwrap()
            .contains("REVIEW_COMPLETED"),
    );
    let _ = fs::remove_dir_all(root);
}

/// What a recording cannot carry of `log link`, compared live by the
/// differential (`log/link/root_handoff_by_its_name`,
/// `log/link/handoff_a_second_name`): the developer's handoff may carry the
/// name of the project's own folder, which is the replay's own; and a
/// handoff with a second name is refused, a link no recording holds.
#[cfg(unix)]
#[test]
fn a_handoff_by_the_projects_name_and_one_with_a_second_name() {
    const STARTED: &str = "\n## Workflow Started\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: WORKFLOW_STARTED\n**Scope**: feature\n\n---\n";
    let scans = format!("{RECORD}/inception/reverse-engineering");
    let returned = |root: &Path, file: &str| {
        log(
            root,
            &[
                "link",
                "--stage",
                "reverse-engineering",
                "--link",
                "focus-developer-agent",
                "--artifact",
                file,
            ],
        )
    };
    let shard =
        |root: &Path| fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap();

    let root = project("link-name");
    write(
        &root,
        &format!("{RECORD}/audit/laptop-abc123.md"),
        &format!("{SHARD}{STARTED}"),
    );
    let name = root.file_name().unwrap().to_str().unwrap().to_owned();
    let named = format!("{scans}/developer-scan-{name}.md");
    write(&root, &named, "# Developer scan\n");
    // Another folder's name is no name of this one's.
    let other = format!("{scans}/developer-scan-elsewhere.md");
    write(&root, &other, "# Developer scan\n");
    assert_eq!(
        refusal(
            &root,
            &[
                "link",
                "--stage",
                "reverse-engineering",
                "--link",
                "focus-developer-agent",
                "--artifact",
                &other
            ]
        ),
        format!(
            "Cannot record reverse-engineering developer link: --artifact must resolve to {scans}/developer-scan.md."
        )
    );
    let output = returned(&root, &named);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "{\"emitted\":\"PIPELINE_LINK_COMPLETED\",\"stage\":\"reverse-engineering\",\"link\":\"focus-developer-agent\"}\n"
    );
    let row = shard(&root);
    assert!(
        row.contains(&format!("**Position**: 1/1\n**Artifact Path**: {named}\n")),
        "{row}"
    );
    assert!(!row.contains("**Repo**"), "{row}");
    let _ = fs::remove_dir_all(root);

    let root = project("link-second-name");
    write(
        &root,
        &format!("{RECORD}/audit/laptop-abc123.md"),
        &format!("{SHARD}{STARTED}"),
    );
    let scan = format!("{scans}/developer-scan.md");
    write(&root, &scan, "# Developer scan\n");
    fs::hard_link(root.join(&scan), root.join("other-name.md")).unwrap();
    let output = returned(&root, &scan);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        said.starts_with("{\"error\":\"Cannot record reverse-engineering developer link: handoff file must be a regular file with no symlink path components ("),
        "{said}"
    );
    assert!(!shard(&root).contains("PIPELINE_LINK_COMPLETED"));
    // With the one name it is the handoff.
    fs::remove_file(root.join("other-name.md")).unwrap();
    assert_eq!(returned(&root, &scan).status.code(), Some(0));
    assert!(shard(&root).contains("PIPELINE_LINK_COMPLETED"));
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. What an answer may carry that Focus has not
/// ported: the engine's own questions, a unit of work, the summary of a
/// stage that runs per unit. Upstream reads what each needs; Focus refuses
/// each by name and records nothing.
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
            with(&["--checkpoint", "guard-recovery"]),
            "--checkpoint guard-recovery records the remedy the person picked after a refusal, and Focus does not ask the recovery question yet.".to_owned(),
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
        (
            vec![
                "answer",
                "--stage",
                "functional-design",
                "--details",
                "Looks correct",
                "--checkpoint",
                "summary-confirmation",
                "--questions-file",
                "x-questions.md",
            ],
            "\"functional-design\" runs per unit of work, and a unit's summary confirmation binds to the unit's own attempt: Focus does not run units yet.".to_owned(),
        ),
    ] {
        assert_eq!(refusal(&root, &args), said, "{args:?}");
    }
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation. A question the engine pairs with a record of its
/// own (Construction's checkpoints, Plan Approval), a question about one
/// unit of work and a batch of Plan Approval questions: upstream records
/// each with what it needs; Focus refuses each by name until the slice that
/// brings it, and records nothing.
#[test]
fn the_questions_that_are_not_ported_are_refused_by_name() {
    let root = project("questions");
    for (more, said) in [
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
    ] {
        assert_eq!(refusal(&root, &asking(&more)), said, "{more:?}");
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

/// Deliberate deviation. The questions file of a summary is hashed into the
/// receipt a gate weighs. Upstream opens it by name: it reads through a
/// link, a linked folder or a second hard link, says of a link to nothing
/// that the file does not exist, and falls over on a folder. Focus reads it
/// as every file of the record that decides something, a regular file with
/// one name reached through no link, and records nothing otherwise.
#[cfg(unix)]
#[test]
fn the_questions_file_of_a_summary_is_not_opened_by_name() {
    use std::os::unix::fs::symlink;
    const SUMMARY: &str =
        "## Q1\n\n[Answer]: A\n\n## Consolidated Summary Confirmation\n\nA tool.\n\n[Answer]:\n";
    let file = format!("{RECORD}/ideation/intent-capture/intent-capture-questions.md");
    let asks = |file: &str| -> Vec<String> {
        ASK.iter()
            .copied()
            .chain([
                "--options",
                "Looks correct,Request changes",
                "--checkpoint",
                "summary-confirmation",
                "--questions-file",
                file,
            ])
            .map(str::to_owned)
            .collect()
    };
    let said = |root: &Path| {
        let args = asks(&file);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        refusal(root, &args)
    };
    let is_link = |said: &str, what: &str| {
        assert!(
            said.ends_with(&format!(
                "{what} is a symlink, and no path component may be a symlink here. A redirected container directory is refused exactly like a symlink found INSIDE an already-trusted one."
            )),
            "{said}"
        );
    };

    // A link to a questions file outside the record, and one to nothing.
    for target in ["elsewhere-questions.md", "nowhere-questions.md"] {
        let root = project(&format!("summary-link-{}", &target[..2]));
        write(&root, "elsewhere-questions.md", SUMMARY);
        fs::create_dir_all(root.join(&file).parent().unwrap()).unwrap();
        symlink(root.join(target), root.join(&file)).unwrap();
        is_link(&said(&root), "intent-capture-questions.md");
        let _ = fs::remove_dir_all(root);
    }
    // The stage's folder is a link to one outside the record.
    let root = project("summary-behind");
    write(&root, "elsewhere/intent-capture-questions.md", SUMMARY);
    fs::create_dir_all(root.join(RECORD).join("ideation")).unwrap();
    symlink(
        root.join("elsewhere"),
        root.join(RECORD).join("ideation/intent-capture"),
    )
    .unwrap();
    is_link(&said(&root), "ideation/intent-capture");
    let _ = fs::remove_dir_all(root);
    // A second name for the file, anywhere on the disk.
    let root = project("summary-second-name");
    write(&root, &file, SUMMARY);
    fs::hard_link(root.join(&file), root.join("other-name.md")).unwrap();
    let refused = said(&root);
    assert!(
        refused.starts_with(
            "summary questions file is multiply linked (a hardlink) and is not trusted: "
        ),
        "{refused}"
    );
    let _ = fs::remove_dir_all(root);
    // A folder with the file's name.
    let root = project("summary-folder");
    write(&root, &format!("{file}/x"), "x");
    let refused = said(&root);
    assert!(
        refused.starts_with("summary questions file is not a regular file"),
        "{refused}"
    );
    let _ = fs::remove_dir_all(root);
    // The file as it should be is recorded, bound to its name.
    let root = project("summary-plain");
    write(&root, &file, SUMMARY);
    let args = asks(&file);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = log(&root, &args);
    assert_eq!(output.status.code(), Some(0));
    let ledger = fs::read_to_string(root.join(RECORD).join("audit/laptop-abc123.md")).unwrap();
    assert!(
        ledger.ends_with(&format!(
            "**Checkpoint**: Consolidated Summary Confirmation\n**Questions File**: {file}\n\n---\n"
        )),
        "{ledger}"
    );
    let _ = fs::remove_dir_all(root);
}

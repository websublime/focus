//! `focus engine review-brief`, run as a process on an installed project.
//!
//! What `context` prints is held to upstream's by the recorded
//! differential, which replays on Unix. These tests run everywhere: they
//! cover the command's surface and each way Focus deliberately differs.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const RECORD: &str = "focus/spaces/default/intents/260105-add-login";
const STATE: &str = "# Focus State Tracking\n\n## Project Information\n- **Project**: Add login to the app\n- **Scope**: feature\n- **Review Override**: advisory\n\n## Stage Progress\n- [-] intent-capture \u{2014} EXECUTE\n- [ ] functional-design \u{2014} EXECUTE\n\n## Current Status\n- **Lifecycle Phase**: IDEATION\n- **Current Stage**: intent-capture\n- **Status**: Running\n";
const SHARD: &str = "# Focus Audit Log\n\n## Workflow Start\n**Timestamp**: 2026-01-05T10:00:00Z\n**Event**: WORKFLOW_STARTED\n**Scope**: feature\n\n---\n";
const NOTHING: &str = "_No review findings were recorded._\n";

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

/// An installed project with one piece of work and its ledger.
fn project(name: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("focus-review-brief-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
    copy_dir(&fixtures.join("installed/claude"), &root);
    write(&root, &format!("{RECORD}/focus-state.md"), STATE);
    write(&root, &format!("{RECORD}/audit/laptop-abc123.md"), SHARD);
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

fn brief(root: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    command
        .args(["engine", "review-brief"])
        .args(args)
        .current_dir(root);
    for (name, _) in std::env::vars() {
        if name.starts_with("FOCUS_") || name == "CLAUDE_PROJECT_DIR" || name == "HARNESS" {
            command.env_remove(name);
        }
    }
    command.output().expect("focus runs")
}

/// What a refused command says: one JSON line on stderr, and nothing else.
fn refusal(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stdout));
    assert_eq!(text(&output.stdout), "");
    let said: serde_json::Value = serde_json::from_str(text(&output.stderr).trim_end()).unwrap();
    said["error"].as_str().unwrap().to_owned()
}

fn shown(output: &Output) -> String {
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stderr), "");
    text(&output.stdout)
}

/// Every file under the project, with what it holds.
fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(dir: &Path, found: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, found);
            } else {
                found.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    let mut found = Vec::new();
    walk(root, &mut found);
    found.sort();
    found
}

/// The rows and the record of one finished review of `stage` (of one of its
/// units, when one is named), with one finding.
fn review(root: &Path, stage: &str, unit: Option<&str>, reviewer: &str, finding: &str) -> String {
    let fingerprint = format!("sha256:{}", "c".repeat(64));
    let request = format!("review:{}", "1".repeat(32));
    let unit_json = unit.map_or("null".to_owned(), |unit| format!("\"{unit}\""));
    let record = format!(
        "{{\n  \"version\": 1,\n  \"stage\": \"{stage}\",\n  \"unit\": {unit_json},\n  \"workflow\": null,\n  \"attempt\": \"0000000000000000\",\n  \"iteration\": 1,\n  \"reviewer\": \"{reviewer}\",\n  \"verdict\": \"NOT-READY\",\n  \"request_id\": \"{request}\",\n  \"request_challenge\": null,\n  \"artifact_fingerprint\": \"{fingerprint}\",\n  \"source_fingerprint\": null,\n  \"unit_source_fingerprint\": null,\n  \"findings\": [\n    {{\n      \"id\": \"R-01\",\n      \"severity\": \"Major\",\n      \"location\": \"Intent\",\n      \"finding\": \"{finding}\",\n      \"required_action\": \"Name the user\",\n      \"status\": \"New\"\n    }}\n  ],\n  \"body\": \"It reads well.\\n\",\n  \"recorded_at\": \"2026-01-05T10:00:00Z\"\n}}\n"
    );
    let scope_dir = unit.map_or("stage".to_owned(), |unit| format!("units/{unit}"));
    let record_rel = format!(".focus-engine/reviews/{stage}/{scope_dir}/0000000000000000/1.json");
    write(root, &format!("{RECORD}/{record_rel}"), &record);
    let digest: String = Sha256::digest(record.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let unit_line = unit.map_or(String::new(), |unit| format!("**Unit**: {unit}\n"));
    let scope =
        format!("**Stage**: {stage}\n{unit_line}**Reviewer**: {reviewer}\n**Iteration**: 1\n");
    format!(
        "\n## Review Requested\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: REVIEW_REQUESTED\n{scope}**Artifact Fingerprint**: {fingerprint}\n**Request Id**: {request}\n\n---\n\n## Review Completed\n**Timestamp**: 2026-01-05T10:02:00Z\n**Event**: REVIEW_COMPLETED\n{scope}**Verdict**: NOT-READY\n**Request Fingerprint**: {fingerprint}\n**Artifact Fingerprint**: {fingerprint}\n**Request Id**: {request}\n**Review Record**: {record_rel}\n**Review Record Digest**: sha256:{digest}\n\n---\n"
    )
}

#[test]
fn the_command_reads_its_arguments_as_upstream_does() {
    let root = project("surface");
    let before = snapshot(&root);
    // The command is the first argument, and every one after it a flag and
    // its value: where the project is, is one of them.
    assert_eq!(refusal(&brief(&root, &[])), "Missing --stage <slug>.");
    assert_eq!(
        refusal(&brief(&root, &["context", "stage"])),
        "Expected --flag value, got \"stage\"."
    );
    assert_eq!(
        refusal(&brief(&root, &["context", "--stage"])),
        "Expected --flag value, got \"--stage\"."
    );
    assert_eq!(
        refusal(&brief(&root, &["--stage", "intent-capture", "context"])),
        "Expected --flag value, got \"intent-capture\"."
    );
    assert_eq!(
        refusal(&brief(&root, &["context", "--stage", "nope"])),
        "Unknown stage: nope"
    );
    assert_eq!(
        refusal(&brief(&root, &["nope", "--stage", "intent-capture"])),
        "Unknown subcommand: nope. Valid: review, context, summary."
    );
    // A flag given twice is read at its last value; one nobody reads is
    // passed over, as upstream passes over it.
    assert_eq!(
        shown(&brief(
            &root,
            &[
                "context",
                "--stage",
                "nope",
                "--stage",
                "intent-capture",
                "--why",
                "first"
            ]
        )),
        NOTHING
    );
    // The project named by the flag is the one read.
    let elsewhere = std::env::temp_dir();
    let mut away = Command::new(env!("CARGO_BIN_EXE_focus"));
    away.args([
        "engine",
        "review-brief",
        "context",
        "--stage",
        "intent-capture",
    ])
    .args(["--project-dir", root.to_str().unwrap()])
    .current_dir(&elsewhere);
    for (name, _) in std::env::vars() {
        if name.starts_with("FOCUS_") || name == "CLAUDE_PROJECT_DIR" || name == "HARNESS" {
            away.env_remove(name);
        }
    }
    assert_eq!(shown(&away.output().unwrap()), NOTHING);
    // Deliberate deviation: upstream reads `--help` as a command like any
    // other and asks for the stage; Focus prints what the tool does, as
    // every tool of its does.
    for help in ["--help", "-h"] {
        let help = shown(&brief(&root, &[help]));
        assert!(
            help.starts_with("Usage: focus engine review-brief <command> --stage <slug>"),
            "{help}"
        );
    }
    assert_eq!(snapshot(&root), before);
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation: the brief shown at a gate and the brief of a
/// summary confirmation are not ported yet. Upstream prints them (and keeps
/// the gate's beside the review's record); Focus refuses each by name, after
/// what upstream itself refuses first, and writes nothing.
#[test]
fn the_briefs_focus_does_not_write_yet_are_refused_by_name() {
    let root = project("not-ported");
    let before = snapshot(&root);
    let stage = ["--stage", "intent-capture"];
    for why in [&[][..], &["--why", "later"], &["--why", ""]] {
        assert_eq!(
            refusal(&brief(&root, &[&["review"], &stage[..], why].concat())),
            "Review brief requires --why <first|revision|stale>."
        );
    }
    for why in ["first", "revision", "stale"] {
        assert_eq!(
            refusal(&brief(
                &root,
                &[&["review"], &stage[..], &["--why", why]].concat()
            )),
            "The brief shown at a gate (`review-brief review`) reads what changed since the review and what the person decided, and keeps what it printed beside the review's record: Focus does not write it yet."
        );
    }
    for questions in [&[][..], &["--questions-file", ""]] {
        assert_eq!(
            refusal(&brief(
                &root,
                &[&["summary"], &stage[..], questions].concat()
            )),
            "Summary brief requires --questions-file <path>."
        );
    }
    assert_eq!(
        refusal(&brief(
            &root,
            &[&["summary"], &stage[..], &["--questions-file", "q.md"]].concat()
        )),
        "The brief of a summary confirmation (`review-brief summary`) reads the stage's questions and what was confirmed before: Focus does not write it yet."
    );
    assert_eq!(snapshot(&root), before);
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation: a decision a gate's row holds, edited by hand so a
/// list stands where the finding's id belongs. Upstream reads the list as
/// text and shows the finding as decided; Focus refuses to show a list it
/// could not read whole.
#[test]
fn a_decision_that_is_not_text_is_refused_not_shown() {
    let root = project("decision");
    let statement = format!("{RECORD}/ideation/intent-capture/intent-statement.md");
    write(&root, &statement, "# Intent\n");
    let rows = review(
        &root,
        "intent-capture",
        None,
        "focus-product-lead-agent",
        "It names no user",
    );
    let decided = |id: &str| {
        format!(
            "\n## Gate Approved\n**Timestamp**: 2026-01-05T10:09:00Z\n**Event**: GATE_APPROVED\n**Stage**: intent-capture\n**Review Finding Dispositions**: {{\"version\":1,\"dispositions\":[{{\"artifact\":\"{statement}\",\"id\":{id},\"fingerprint\":\"sha256:{}\",\"status\":\"Accepted risk\"}}]}}\n\n---\n",
            "b".repeat(64)
        )
    };
    let shard = format!("{RECORD}/audit/laptop-abc123.md");
    let context = ["context", "--stage", "intent-capture"];
    // Nothing decided: the finding is open.
    write(&root, &shard, &format!("{SHARD}{rows}"));
    let open = shown(&brief(&root, &context));
    assert!(
        open.contains(&format!("**Review artifact:** `{statement}`")),
        "{open}"
    );
    assert!(
        open.contains("| R-01 | Major | Intent | It names no user | Name the user |  |"),
        "{open}"
    );
    assert!(
        open.contains("_No settled decisions are carried into this review._"),
        "{open}"
    );
    // Decided as a gate writes it: the finding is shown settled.
    write(
        &root,
        &shard,
        &format!("{SHARD}{rows}{}", decided("\"R-01\"")),
    );
    let settled = shown(&brief(&root, &context));
    assert!(!settled.contains("_No settled decisions"), "{settled}");
    assert!(settled.contains("Accepted risk"), "{settled}");
    // With a list for the id, it is refused.
    write(
        &root,
        &shard,
        &format!("{SHARD}{rows}{}", decided("[\"R-01\"]")),
    );
    assert_eq!(
        refusal(&brief(&root, &context)),
        "the Review Finding Dispositions field holds a decision that is not written as text: it is not read"
    );
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation: the units of a stage that runs for each unit, when
/// no plan names them, are the folders found under `construction/`.
/// Upstream shows them in the order the host lists the folder in, which no
/// two file systems agree on; Focus shows them by name.
#[test]
fn units_found_by_listing_are_shown_by_name() {
    let root = project("units");
    let reviewer = "focus-architecture-reviewer-agent";
    let spec =
        |unit: &str| format!("{RECORD}/construction/{unit}/functional-design/functional-spec.md");
    // Written last to first, so no listing by age gives the order by name.
    let mut rows = String::new();
    for unit in ["web", "auth", "api"] {
        write(&root, &spec(unit), "# Spec\n");
        rows.push_str(&review(
            &root,
            "functional-design",
            Some(unit),
            reviewer,
            &format!("Finding of {unit}"),
        ));
    }
    write(
        &root,
        &format!("{RECORD}/audit/laptop-abc123.md"),
        &format!("{SHARD}{rows}"),
    );
    let all = shown(&brief(&root, &["context", "--stage", "functional-design"]));
    let at = |unit: &str| {
        all.find(&format!("**Review artifact:** `{}`", spec(unit)))
            .unwrap_or_else(|| panic!("{unit}: {all}"))
    };
    assert!(at("api") < at("auth") && at("auth") < at("web"), "{all}");
    for unit in ["api", "auth", "web"] {
        assert!(all.contains(&format!("Finding of {unit}")), "{all}");
    }
    // One unit asked for: its list alone.
    let one = shown(&brief(
        &root,
        &["context", "--stage", "functional-design", "--unit", "auth"],
    ));
    assert!(one.contains("Finding of auth"), "{one}");
    assert!(
        !one.contains("Finding of api") && !one.contains("Finding of web"),
        "{one}"
    );
    assert_eq!(
        shown(&brief(
            &root,
            &["context", "--stage", "functional-design", "--unit", "nope"],
        )),
        NOTHING
    );
    let _ = fs::remove_dir_all(root);
}

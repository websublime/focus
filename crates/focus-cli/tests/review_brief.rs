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

/// The brief shown at a gate is printed and kept as printed, under the
/// digest of its text; the brief of a summary confirmation names the
/// questions file and what will be generated.
#[test]
fn the_two_briefs_are_printed_and_the_gate_s_is_kept() {
    let root = project("briefs");
    let stage = ["--stage", "intent-capture"];
    for why in [&[][..], &["--why", "later"], &["--why", ""]] {
        assert_eq!(
            refusal(&brief(&root, &[&["review"], &stage[..], why].concat())),
            "Review brief requires --why <first|revision|stale>."
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
    let statement = format!("{RECORD}/ideation/intent-capture/intent-statement.md");
    write(&root, &statement, "# Intent\n");
    let rows = review(
        &root,
        "intent-capture",
        None,
        "focus-product-lead-agent",
        "It names no user",
    );
    write(
        &root,
        &format!("{RECORD}/audit/laptop-abc123.md"),
        &format!("{SHARD}{rows}"),
    );
    let before = snapshot(&root);
    let said = shown(&brief(
        &root,
        &[&["review"], &stage[..], &["--why", "first"]].concat(),
    ));
    assert!(
        said.starts_with("**Stage:** Intent Capture & Framing\n**Review outcome:** Concerns remain for your decision.\n**Why now:** First review completed.\n\n"),
        "{said}"
    );
    assert!(
        said.contains(&format!("**Review artifact:** `{statement}`")),
        "{said}"
    );
    assert!(
        said.ends_with("**Decision options:**\n- **Approve** - continue with the open findings accepted.\n- **Request Changes** - return to the listed artifacts so the required actions can be addressed.\n"),
        "{said}"
    );
    // Kept: the text under its own digest, and the record of the latest.
    let digest: String = Sha256::digest(said.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let briefs = format!("{RECORD}/.focus-engine/reviews/intent-capture/stage/briefs");
    assert_eq!(
        fs::read_to_string(root.join(&briefs).join(format!("{digest}.md"))).unwrap(),
        said
    );
    let latest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(root.join(&briefs).join("latest.json")).unwrap())
            .unwrap();
    assert_eq!(latest["digest"], digest.as_str());
    assert_eq!(latest["why"], "first");
    assert_eq!(snapshot(&root).len(), before.len() + 2);
    // A unit named as nothing, or by what is no unit's name, keeps nothing.
    let stale = shown(&brief(
        &root,
        &[
            &["review"],
            &stage[..],
            &["--why", "stale", "--unit", "a/b"],
        ]
        .concat(),
    ));
    assert!(
        stale.contains(
            "**Why now:** Re-check required: the previous review receipt is no longer valid."
        ),
        "{stale}"
    );
    assert_eq!(snapshot(&root).len(), before.len() + 2);
    // The summary's brief: the questions file must be there, in the record.
    let questions = format!("{RECORD}/ideation/intent-capture/intent-capture-questions.md");
    let asks = |file: &str| {
        brief(
            &root,
            &[
                "summary",
                "--stage",
                "intent-capture",
                "--questions-file",
                file,
            ],
        )
    };
    assert_eq!(
        refusal(&asks(&questions)),
        format!(
            "Summary confirmation questions file must exist inside the active intent record: {questions}"
        )
    );
    write(&root, &questions, "# Questions\n");
    write(&root, "notes.md", "# Notes\n");
    assert_eq!(
        refusal(&asks("notes.md")),
        "Summary confirmation questions file must exist inside the active intent record: notes.md"
    );
    let confirm = shown(&asks(&questions));
    assert!(
        confirm.starts_with(&format!(
            "**Stage:** Intent Capture & Framing\n**Confirming:** Consolidated answers in `{questions}` before generating `{statement}`, "
        )),
        "{confirm}"
    );
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation: the brief of one unit of a stage that runs for
/// each unit depends on whether the approval ahead is the unit's own, and a
/// unit's recovery review names the source it was bound to. Upstream reads
/// both off the units' records; Focus refuses each by name.
#[test]
fn what_a_brief_would_read_of_units_is_refused_by_name() {
    let root = project("brief-units");
    assert_eq!(
        refusal(&brief(
            &root,
            &[
                "review",
                "--stage",
                "functional-design",
                "--why",
                "first",
                "--unit",
                "api"
            ],
        )),
        "The brief of one unit of \"functional-design\" depends on whether the approval ahead is the unit's own (its team's gate, or its Construction checkpoint), which is read off the units' gates: Focus does not read those yet (units arrive with a later slice)."
    );
    // (With no unit named, the stage's final gate shows every unit's.)
    assert!(
        shown(&brief(
            &root,
            &["review", "--stage", "functional-design", "--why", "first"],
        ))
        .starts_with("**Stage:** Functional Design\n")
    );
    let of_unit = |event: &str, title: &str, at: &str, more: &str| {
        format!(
            "\n## {title}\n**Timestamp**: 2026-01-05T10:0{at}:00Z\n**Event**: {event}\n**Stage**: intent-capture\n**Unit**: api\n**Reviewer**: focus-product-lead-agent\n**Iteration**: 1\n{more}**Unit Source Fingerprint**: {}\n\n---\n",
            at.repeat(64)
        )
    };
    write(
        &root,
        &format!("{RECORD}/audit/laptop-abc123.md"),
        &format!(
            "{SHARD}{}{}",
            of_unit(
                "REVIEW_COMPLETED",
                "Review Completed",
                "5",
                "**Verdict**: READY\n"
            ),
            of_unit(
                "REVIEW_REQUESTED",
                "Review Requested",
                "9",
                "**Recovery**: stale-receipt\n**Recovery Cause**: artifact\n"
            )
        ),
    );
    let ask = |why: &'static str| {
        brief(
            &root,
            &["review", "--stage", "intent-capture", "--why", why],
        )
    };
    assert_eq!(
        refusal(&ask("stale")),
        "The recovery review of unit \"api\" of \"intent-capture\" names the source it was bound to, and what changed in it is read off the unit's source snapshots: Focus does not read those yet (units arrive with a later slice)."
    );
    // (Only the brief of a stale receipt reads what changed.)
    shown(&ask("revision"));
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

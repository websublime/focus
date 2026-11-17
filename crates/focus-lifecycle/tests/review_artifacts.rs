//! What a review is of, in projects laid out here: the answers a reader can
//! work out by hand, and the one place Focus says something upstream does
//! not. The oracle corpus (`review_snapshot`) holds the rest to upstream.

use std::fs;
use std::path::{Path, PathBuf};

use focus_evidence::review::{LegacyAppendix, RequestBinding};
use focus_harness::Harness;
use focus_lifecycle::artifacts::review_artifact_entries;
use focus_lifecycle::review_artifacts::{
    Options, Snapshot, appended_after_request, fingerprint, request_artifacts_current, snapshot,
};
use focus_lifecycle::review_pending::{Asked, Currency, pending_request_currency};
use focus_lifecycle::workflow::Workflow;
use focus_workspace::Layout;
use focus_workspace::records::{Invoker, Selection, Selector, resolve_selection};
use sha2::{Digest, Sha256};

const STATE: &str = "# State\n\n## Workflow\n- **Scope**: feature\n\n## Stage Progress\n- [x] units-generation \u{2014} EXECUTE\n";

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The install the fixtures keep, to read a scratch project with.
fn installed() -> Workflow {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    Workflow::open(
        Layout::new(fixtures.join("upstream/installed/claude"), Harness::Claude),
        Selection {
            space: "default".to_owned(),
            intent: None,
            session_id: None,
            binding: None,
        },
        Invoker::default(),
    )
    .unwrap()
}

/// A project with one record, holding `files` (paths within the record).
fn project(name: &str, files: &[(&str, &[u8])]) -> (PathBuf, Workflow) {
    let root = std::env::temp_dir().join(format!(
        "focus-review-artifacts-{}-{name}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    let layout = Layout::new(root.clone(), Harness::Claude);
    let intents = layout.intents_dir("default");
    let record = intents.join("rec-1a2b3c4d");
    fs::create_dir_all(&record).unwrap();
    fs::write(intents.join("active-intent"), "rec-1a2b3c4d\n").unwrap();
    fs::write(record.join(focus_workspace::names::STATE_FILE), STATE).unwrap();
    for (rel, bytes) in files {
        let path = record.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let selection = resolve_selection(&layout, &Selector::default(), &Invoker::default()).unwrap();
    let wf = Workflow {
        layout,
        selection,
        invoker: Invoker::default(),
        install: installed().install,
    };
    (root, wf)
}

fn manifest(entries: &[(&str, String)]) -> String {
    let rows: Vec<String> = entries
        .iter()
        .map(|(path, value)| format!("[\"{path}\",\"{value}\"]"))
        .collect();
    format!(
        "sha256:{}",
        sha256(format!("[{}]", rows.join(",")).as_bytes())
    )
}

fn request(artifact_fingerprint: &str, prior_appendix: Option<bool>) -> RequestBinding {
    RequestBinding {
        artifact_fingerprint: artifact_fingerprint.to_owned(),
        request_id: None,
        legacy_appendix: prior_appendix.map(|prior_appendix| LegacyAppendix {
            artifact: "inception/user-stories/stories.md".to_owned(),
            offset: 0,
            prior_digest: None,
            prior_length: None,
            challenge: None,
            prior_appendix,
        }),
        source_fingerprint: None,
        unit_source_fingerprint: None,
        recovery_cause: None,
    }
}

#[test]
fn the_fingerprint_is_of_every_declared_artifact_by_its_logical_name() {
    let (root, wf) = project(
        "manifest",
        &[
            ("inception/user-stories/stories.md", b"# Stories\n"),
            // A checkout on Windows: the text is the same text.
            ("inception/user-stories/personas.md", b"# Personas\r\n"),
        ],
    );
    let stage = wf.stage("user-stories").unwrap().clone();
    let expected = manifest(&[
        (
            "inception/user-stories/personas.md",
            format!("sha256:{}", sha256(b"# Personas\n")),
        ),
        (
            "inception/user-stories/stories.md",
            format!("sha256:{}", sha256(b"# Stories\n")),
        ),
        (
            "inception/user-stories/traceability.json",
            "missing".to_owned(),
        ),
        (
            "inception/user-stories/user-stories-assessment.md",
            "missing".to_owned(),
        ),
    ]);
    assert_eq!(
        fingerprint(&wf, &stage, None, None, Options::default()),
        Some(expected.clone())
    );
    let taken = snapshot(&wf, &stage, None, Options::default()).unwrap();
    assert_eq!(
        taken,
        Snapshot {
            fingerprint: expected.clone(),
            review_artifact: "inception/user-stories/stories.md".to_owned(),
            body_fingerprints: vec![expected.clone()],
            appendix: Vec::new(),
        }
    );
    // What the stage owes is not all there.
    let all = Options {
        require_required_artifacts: true,
        ..Options::default()
    };
    assert_eq!(fingerprint(&wf, &stage, None, None, all), None);
    assert_eq!(snapshot(&wf, &stage, None, all), None);
    // A request for these bytes is current, and nothing was appended.
    assert!(request_artifacts_current(&request(&expected, None), &taken));
    assert!(!appended_after_request(&request(&expected, None), &taken));
    let _ = fs::remove_dir_all(root);
}

#[test]
fn a_review_appended_after_the_request_is_told_from_one_that_was_there() {
    let body = b"# Stories\n\nBody.\n";
    let (root, wf) = project(
        "appended",
        &[(
            "inception/user-stories/stories.md",
            b"# Stories\n\nBody.\n\n## Review\n\nVerdict: READY\n",
        )],
    );
    let stage = wf.stage("user-stories").unwrap().clone();
    let taken = snapshot(&wf, &stage, None, Options::default()).unwrap();
    let rest = |stories: &[u8]| {
        manifest(&[
            ("inception/user-stories/personas.md", "missing".to_owned()),
            (
                "inception/user-stories/stories.md",
                format!("sha256:{}", sha256(stories)),
            ),
            (
                "inception/user-stories/traceability.json",
                "missing".to_owned(),
            ),
            (
                "inception/user-stories/user-stories-assessment.md",
                "missing".to_owned(),
            ),
        ])
    };
    // The section starts at its heading; the body ends after one line end,
    // or where the heading starts.
    assert_eq!(taken.appendix, b"## Review\n\nVerdict: READY\n");
    assert_eq!(
        taken.body_fingerprints,
        vec![rest(body), rest(b"# Stories\n\nBody.\n\n")]
    );
    let before = rest(body);
    // Requested of the bytes before the section: the section came after.
    assert!(appended_after_request(&request(&before, None), &taken));
    assert!(!request_artifacts_current(&request(&before, None), &taken));
    // Under the appendix protocol the same request is still current, and
    // says itself whether a section was already there.
    assert!(request_artifacts_current(
        &request(&before, Some(false)),
        &taken
    ));
    assert!(appended_after_request(
        &request(&before, Some(false)),
        &taken
    ));
    assert!(!appended_after_request(
        &request(&before, Some(true)),
        &taken
    ));
    // Requested of the file as it is now: the section was there already.
    assert!(request_artifacts_current(
        &request(&taken.fingerprint, None),
        &taken
    ));
    assert!(!appended_after_request(
        &request(&taken.fingerprint, None),
        &taken
    ));
    let _ = fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn a_file_that_is_a_link_or_has_a_second_name_is_no_snapshot() {
    let (root, wf) = project(
        "untrusted",
        &[("inception/user-stories/stories.md", b"# Stories\n")],
    );
    let stage = wf.stage("user-stories").unwrap().clone();
    let record = wf.record_dir().unwrap();
    let dir = record.join("inception/user-stories");
    assert!(fingerprint(&wf, &stage, None, None, Options::default()).is_some());
    // A second name for an artifact.
    fs::hard_link(dir.join("stories.md"), root.join("second.md")).unwrap();
    assert_eq!(
        fingerprint(&wf, &stage, None, None, Options::default()),
        None
    );
    fs::remove_file(root.join("second.md")).unwrap();
    assert!(fingerprint(&wf, &stage, None, None, Options::default()).is_some());
    // An artifact that is a link.
    fs::write(root.join("kept.md"), "# Personas\n").unwrap();
    std::os::unix::fs::symlink(root.join("kept.md"), dir.join("personas.md")).unwrap();
    assert_eq!(
        fingerprint(&wf, &stage, None, None, Options::default()),
        None
    );
    assert_eq!(snapshot(&wf, &stage, None, Options::default()), None);
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation: a per-unit stage's units are found by listing the
/// record's construction folder, and upstream falls over on a file by that
/// name with what its runtime says of the listing (`ENOTDIR: not a
/// directory, scandir '…'`). Focus says what is wrong with the folder.
/// Neither has an answer to what a review of the stage is of.
#[test]
fn deliberate_deviation_a_construction_folder_that_cannot_be_listed() {
    let (root, wf) = project("unlistable", &[("construction", b"Not a folder.\n")]);
    let stage = wf.stage("functional-design").unwrap().clone();
    let refusal = review_artifact_entries(&wf, &stage, None)
        .unwrap_err()
        .to_string();
    assert!(
        refusal.ends_with(
            "construction is there and is not a folder that can be listed, and the artifacts of \"functional-design\" are found by listing it. Restore the folder, or move what is there away."
        ),
        "{refusal}"
    );
    assert_eq!(
        fingerprint(&wf, &stage, None, None, Options::default()),
        None
    );
    assert_eq!(snapshot(&wf, &stage, None, Options::default()), None);
    let _ = fs::remove_dir_all(root);
}

/// A request of a unit, in a per-unit stage that writes the workspace,
/// binds the unit's own source, which is not read yet: it is refused by
/// name, not answered without it. A stage run by itself binds none.
#[test]
fn a_units_request_that_binds_the_units_source_is_refused_until_units_arrive() {
    let (root, wf) = project(
        "unit-source",
        &[(
            "construction/api/code-generation/code-generation-plan.md",
            b"# Plan\n",
        )],
    );
    let stage = wf.stage("code-generation").unwrap().clone();
    let taken = snapshot(&wf, &stage, Some("api"), Options::default()).unwrap();
    let asked = request(&taken.fingerprint, None);
    let refusal = pending_request_currency(&wf, &stage, Some("api"), &asked, Asked::default())
        .unwrap_err()
        .to_string();
    assert_eq!(
        refusal,
        "A review request of unit \"api\" in \"code-generation\" binds the unit's own source, and Focus does not read that yet (units arrive with a later slice)."
    );
    let alone = Asked {
        single: true,
        ..Asked::default()
    };
    assert_eq!(
        pending_request_currency(&wf, &stage, Some("api"), &asked, alone).unwrap(),
        Currency {
            request_current: true,
            readable: true,
            // The request bound no source, and the stage writes it.
            modern_verdict_binding: false,
        }
    );
    // The stage as a whole binds no unit's source.
    assert!(pending_request_currency(&wf, &stage, None, &asked, Asked::default()).is_ok());
    let _ = fs::remove_dir_all(root);
}

/// A stage that runs for each unit and owes a summary confirmation is
/// refused by name: which units owe one is read off their rows, which
/// arrive with units. One that owes none is answered.
#[test]
fn the_summary_evidence_of_a_per_unit_stage_is_refused_until_units_arrive() {
    use focus_lifecycle::summary_evidence::{Asked, Evidence, check};
    let (root, wf) = project("per-unit-summary", &[]);
    let stage = wf.stage("functional-design").unwrap().clone();
    let asked = Asked {
        state: Some(STATE),
        ..Asked::default()
    };
    let refusal = check(&wf, &stage, asked).unwrap_err().to_string();
    assert_eq!(
        refusal,
        "\"functional-design\" runs for each unit and owes a summary confirmation: which units owe one is read off their rows, and Focus does not read those yet (units arrive with a later slice)."
    );
    let off = format!("{STATE}- **Summary Confirmation**: off\n");
    let asked = Asked {
        state: Some(&off),
        ..Asked::default()
    };
    assert_eq!(
        check(&wf, &stage, asked).unwrap(),
        Evidence::Ok {
            required: false,
            accepted_changes: Vec::new(),
            change_control_read: false,
        }
    );
    let _ = fs::remove_dir_all(root);
}

/// Deliberate deviation, as where the receipt is recorded (`focus-log`):
/// a questions file is read as every file of the record that decides
/// something. Upstream opens it by name, so a link there brings in a file
/// from outside the record; here it reads as a file with no answer.
#[cfg(unix)]
#[test]
fn deliberate_deviation_a_questions_file_that_is_a_link_has_no_answer() {
    use focus_lifecycle::summary_evidence::{Asked, Evidence, check};
    let (root, wf) = project("linked-questions", &[]);
    let stage = wf.stage("requirements-analysis").unwrap().clone();
    let dir = wf
        .record_dir()
        .unwrap()
        .join("inception/requirements-analysis");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        root.join("elsewhere.md"),
        "## Consolidated Summary Confirmation\n\n[Answer]: Looks correct\n",
    )
    .unwrap();
    std::os::unix::fs::symlink(
        root.join("elsewhere.md"),
        dir.join("requirements-analysis-questions.md"),
    )
    .unwrap();
    // A row on record, so the file is what is asked next.
    let audit = wf.record_dir().unwrap().join("audit");
    fs::create_dir_all(&audit).unwrap();
    fs::write(
        audit.join("laptop-abc123.md"),
        "# Focus Audit Log\n\n## Row\n**Timestamp**: 2026-01-05T10:00:01Z\n**Event**: WORKFLOW_STARTED\n**Scope**: feature\n---\n",
    )
    .unwrap();
    let asked = Asked {
        state: Some(STATE),
        ..Asked::default()
    };
    let Evidence::Refused { code, .. } = check(&wf, &stage, asked).unwrap() else {
        panic!("a linked questions file was read");
    };
    assert_eq!(code, "SUMMARY_ANSWER_INVALID");
    let _ = fs::remove_dir_all(root);
}

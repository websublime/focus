//! Replays the recorded differential.
//!
//! `tools/oracle/differential.py --record` runs upstream's engine and focus on
//! the same edited project trees and, when they agree, stores what upstream
//! did in each check: the tree it started from, the command, and the outcome
//! (exit code, both output streams, and for a compile the files it left).
//! This test rebuilds each tree, runs the same command through the `focus`
//! binary, and requires the same outcome, so the comparison keeps running
//! where there is no upstream checkout and no Bun.
//!
//! Everything recorded is stored under Focus's names: the recorder maps
//! upstream's vocabulary (`aidlc`) to Focus's before writing, trees, commands
//! and outcomes alike (`tools/oracle/names.py`).
//!
//! Before comparing, focus's output gets the same normalisation the recorder
//! applied to upstream's (see `normalise`); nothing else is forgiven. Checks
//! recorded as deliberate deviations are skipped here and pinned by their own
//! tests.
//!
//! Unix only. The recording was made on Linux, and on Windows the paths focus
//! prints inside its messages use the other separator; telling those apart
//! from backslashes that belong to JSON output needs the recorded text, which
//! long outputs do not keep (they are stored as digests).
#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use sha2::{Digest, Sha256};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
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

/// The vendored install with one recorded tree's differences applied.
fn materialise(overlay: &Value, root: &Path) {
    let _ = fs::remove_dir_all(root);
    copy_dir(&fixtures().join("upstream/installed/claude"), root);
    for (rel, content) in overlay.as_object().unwrap() {
        let path = root.join(rel);
        match content.as_str() {
            Some(text) => {
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, text).unwrap();
            }
            None => fs::remove_file(path).unwrap(),
        }
    }
}

fn replace_all(
    text: &str,
    pattern: &str,
    replacement: impl Fn(&regress::Match, &str) -> String,
) -> String {
    let regex = regress::Regex::with_flags(pattern, "m").unwrap();
    let mut out = String::new();
    let mut last = 0;
    for found in regex.find_iter(text) {
        out.push_str(&text[last..found.start()]);
        out.push_str(&replacement(&found, text));
        last = found.end();
    }
    out.push_str(&text[last..]);
    out
}

fn group<'a>(found: &regress::Match, text: &'a str, index: usize) -> &'a str {
    found.group(index).map_or("", |range| &text[range])
}

/// The recorder's `normalise`, for focus's side: the engine's name for
/// itself, the tree's location, and text the runtime wrote.
fn normalise(text: &str, root: &Path) -> String {
    let root = root.display().to_string();
    let mut text = text
        .replace(&format!("{root}{}", std::path::MAIN_SEPARATOR), "")
        .replace(&root, "<tree>");
    text = replace_all(&text, r"^focus graph ", |_, _| "graph ".to_owned());
    text = text
        .replace("focus engine graph", "<cli> graph")
        .replace("'focus config'", "'<cli> config'");
    text = replace_all(&text, r"\bFOCUS_([A-Z_]+)\b", |found, text| {
        format!("<ENV>_{}", group(found, text, 1))
    });
    text = text
        .replace("so Focus changed nothing", "so <engine> changed nothing")
        .replace("treated as a Focus persona", "treated as <engine> persona");
    for (pattern, keeps_tail) in [
        (
            r"(Stage graph not readable at [^:]+: ).*?(\. Reinstall)",
            true,
        ),
        (r"(is not valid JSON: ).*()", false),
        (r"(has an unparseable grid fence: ).*()", false),
        (r"(cannot read ars priors at [^:]+: ).*()", false),
        (
            r"(validate-grid: cannot read [^:]+: ).*?(\. Write the grid|$)",
            true,
        ),
    ] {
        text = replace_all(&text, pattern, |found, text| {
            let tail = if keeps_tail {
                group(found, text, 2)
            } else {
                ""
            };
            format!("{}<runtime>{tail}", group(found, text, 1))
        });
    }
    text
}

/// What a check may write, as the recorder snapshots it.
fn written_files(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    for rel in [
        ".claude/tools/data/stage-graph.json",
        ".claude/tools/data/scope-grid.json",
    ] {
        let text = fs::read_to_string(root.join(rel)).unwrap_or_else(|_| "<absent>".to_owned());
        files.insert(rel.to_owned(), text);
    }
    for dir in [".claude/scopes", "focus/scopes"] {
        let Ok(entries) = fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            let rel = format!("{dir}/{name}");
            files.insert(rel.clone(), fs::read_to_string(root.join(&rel)).unwrap());
        }
    }
    files
}

fn matches(expected: &Value, actual: &str) -> bool {
    match expected {
        Value::String(text) => text == actual,
        // Long output is recorded as its digest.
        digest => {
            let hash = Sha256::digest(actual.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            digest["sha256"] == hash.as_str() && digest["bytes"] == actual.len()
        }
    }
}

fn shown(expected: &Value) -> String {
    match expected {
        Value::String(text) => text.chars().take(400).collect(),
        digest => format!("<{} bytes, sha256 {}>", digest["bytes"], digest["sha256"]),
    }
}

/// Run one check in `root`; the problems found, empty when it matches.
fn replay(check: &Value, root: &Path) -> Vec<String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
    command.args(["engine", "graph"]);
    command.args(
        check["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap()),
    );
    command.args([
        "--project-dir",
        root.to_str().unwrap(),
        "--harness",
        "claude",
    ]);
    for (name, _) in std::env::vars() {
        if name.starts_with("FOCUS_") || name == "CLAUDE_PROJECT_DIR" {
            command.env_remove(name);
        }
    }
    for (name, value) in check["env"].as_object().unwrap() {
        command.env(format!("FOCUS_{name}"), value.as_str().unwrap());
    }
    command
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().unwrap();
    if let Some(input) = check["stdin"].as_str() {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();

    let mut actual: BTreeMap<String, String> = BTreeMap::new();
    actual.insert(
        "exit".to_owned(),
        output.status.code().unwrap_or(-1).to_string(),
    );
    actual.insert(
        "stdout".to_owned(),
        normalise(&String::from_utf8_lossy(&output.stdout), root),
    );
    actual.insert(
        "stderr".to_owned(),
        normalise(&String::from_utf8_lossy(&output.stderr), root),
    );
    let expected = check["expect"].as_object().unwrap();
    if expected.len() > 3 {
        // Files are compared as written.
        for (rel, text) in written_files(root) {
            actual.insert(rel, text);
        }
    }

    let mut problems = Vec::new();
    for (key, want) in expected {
        let got = actual.remove(key).unwrap_or_else(|| "<absent>".to_owned());
        if !matches(want, &got) {
            problems.push(format!(
                "{key}\n      upstream: {}\n      focus:    {}",
                shown(want),
                got.chars().take(400).collect::<String>()
            ));
        }
    }
    for (key, _) in actual {
        problems.push(format!("{key}: focus wrote a file upstream did not"));
    }
    problems
}

#[test]
fn focus_does_what_upstream_did_in_every_recorded_check() {
    let dir = fixtures().join("recorded");
    let trees: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("trees.json")).unwrap()).unwrap();
    let checks: Vec<Value> = fs::read_to_string(dir.join("checks.ndjson"))
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        checks.len() > 1000,
        "the recording is unexpectedly small: {} checks",
        checks.len()
    );

    let scratch = std::env::temp_dir().join(format!("focus-recorded-{}", std::process::id()));
    let _ = fs::remove_dir_all(&scratch);
    // Checks that only read share one copy of their tree; a check that writes
    // gets its own.
    let mut by_tree: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for check in &checks {
        by_tree
            .entry(check["tree"].as_str().unwrap())
            .or_default()
            .push(check);
    }
    let groups: Vec<(&str, Vec<&Value>)> = by_tree.into_iter().collect();
    let workers = std::thread::available_parallelism().map_or(2, usize::from);
    let failures: Vec<String> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|worker| {
                let (groups, trees, scratch) = (&groups, &trees, &scratch);
                scope.spawn(move || {
                    let mut failures = Vec::new();
                    for (index, (tree, checks)) in groups.iter().enumerate() {
                        if index % workers != worker {
                            continue;
                        }
                        let shared = scratch.join(format!("{tree}-shared"));
                        let mut shared_ready = false;
                        for (step, check) in checks.iter().enumerate() {
                            if !check["deviation"].is_null() {
                                continue;
                            }
                            let root = if check["writes"] == true {
                                let own = scratch.join(format!("{tree}-{step}"));
                                materialise(&trees[*tree], &own);
                                own
                            } else {
                                if !shared_ready {
                                    materialise(&trees[*tree], &shared);
                                    shared_ready = true;
                                }
                                shared.clone()
                            };
                            let problems = replay(check, &root);
                            if !problems.is_empty() {
                                failures.push(format!(
                                    "{}\n    {}",
                                    check["name"].as_str().unwrap(),
                                    problems.join("\n    ")
                                ));
                            }
                            if check["writes"] == true {
                                let _ = fs::remove_dir_all(&root);
                            }
                        }
                    }
                    failures
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().unwrap())
            .collect()
    });
    let _ = fs::remove_dir_all(&scratch);

    assert!(
        failures.is_empty(),
        "{} of {} recorded checks differ from upstream; first:\n{}",
        failures.len(),
        checks.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_recording_covers_every_verb_and_both_outcomes() {
    let text = fs::read_to_string(fixtures().join("recorded/checks.ndjson")).unwrap();
    let checks: Vec<Value> = text
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    for verb in [
        "compile",
        "topo",
        "cycles",
        "scope",
        "artifacts",
        "producers",
        "consumers",
        "validate-scope",
        "validate-grid",
        "ars",
        "resolve",
        "export",
    ] {
        let of_verb: Vec<&Value> = checks.iter().filter(|c| c["args"][0] == verb).collect();
        assert!(
            of_verb.iter().any(|c| c["expect"]["exit"] == "0"),
            "no recorded success for {verb}"
        );
        assert!(
            of_verb.iter().any(|c| c["expect"]["exit"] == "1")
                || verb == "artifacts"
                || verb == "topo",
            "no recorded failure for {verb}"
        );
    }
    let deviations = checks.iter().filter(|c| !c["deviation"].is_null()).count();
    assert!(
        deviations * 20 < checks.len(),
        "{deviations} of {} recorded checks are deviations; the replay proves little",
        checks.len()
    );
}

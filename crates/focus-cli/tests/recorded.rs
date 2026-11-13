//! Replays the recorded differential.
//!
//! `tools/oracle/differential.py --record` runs upstream's engine and focus on
//! the same edited project trees and, when they agree, stores what upstream
//! did in each check: the tree it started from, the command, and the outcome
//! (exit code, both output streams, and the files it left: for a compile the
//! compiled ones, for the audit command whatever changed in the workspace).
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

/// The vendored install with one recorded tree's differences applied; or,
/// for a check recorded on a project built from nothing (`empty`), only the
/// files the tree names.
fn materialise(overlay: &Value, root: &Path, empty: bool) {
    let _ = fs::remove_dir_all(root);
    if empty {
        fs::create_dir_all(root).unwrap();
    } else {
        copy_dir(&fixtures().join("upstream/installed/claude"), root);
    }
    for (rel, content) in overlay.as_object().unwrap() {
        // A record the recording minted is stored by its day placeholder.
        let rel = rel.replace("/intents/<day>-", &format!("/intents/{}-", today()));
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

/// `text` with the tree's location taken out: a path inside it reads
/// relative to it, and the tree itself reads `<tree>`. The location is
/// recognised both as given and with links resolved, because a path the
/// engine resolved (the install root) is printed resolved, and a temporary
/// directory is behind a link on macOS.
fn without_tree(text: &str, root: &Path) -> String {
    let mut text = text.to_owned();
    let mut spellings = vec![root.display().to_string()];
    if let Ok(real) = fs::canonicalize(root) {
        spellings.insert(0, real.display().to_string());
    }
    for root in spellings {
        text = text
            .replace(&format!("{root}{}", std::path::MAIN_SEPARATOR), "")
            .replace(&root, "<tree>");
    }
    text
}

/// Text a runtime wrote, after the engine's own words: kept up to where the
/// engine stops talking.
fn runtime_text(mut text: String) -> String {
    for (pattern, keeps_tail) in [
        (
            r"(Stage graph not readable at [^:]+: ).*?(\. Reinstall)",
            true,
        ),
        (r"(is not valid JSON: ).*()", false),
        (r"(: invalid JSON: ).*()", false),
        (r"(has an unparseable grid fence: ).*()", false),
        (r"(cannot read ars priors at [^:]+: ).*()", false),
        (
            r"(validate-grid: cannot read [^:]+: ).*?(\. Write the grid|$)",
            true,
        ),
        (
            r"(graph resolve: ).*: No such file or directory \(os error 2\)()",
            false,
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

/// The recorder's `normalise`, for focus's side: the engine's name for
/// itself, the tree's location, and text the runtime wrote.
fn normalise(text: &str, root: &Path) -> String {
    let mut text = without_tree(text, root);
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
    runtime_text(text)
}

/// A timestamp at or after this instant was written during the replay.
fn started() -> &'static str {
    static STARTED: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    STARTED.get_or_init(|| {
        focus_audit::time::iso_timestamp(
            std::time::SystemTime::now() - std::time::Duration::from_secs(2),
        )
    })
}

fn started_ms() -> u128 {
    static STARTED_MS: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    *STARTED_MS.get_or_init(|| {
        (std::time::SystemTime::now() - std::time::Duration::from_secs(2))
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis())
    })
}

/// The recorder's `normalise_audit`, for focus's side.
fn normalise_audit(text: &str, root: &Path) -> String {
    let mut text = without_tree(text, root);
    // The runtime command names itself at the start of what it says.
    text = replace_all(&text, r"^focus runtime(?=[ :])", |_, _| {
        "runtime".to_owned()
    });
    text = text.replace(
        "Run focus engine runtime --help",
        "Run <cli> runtime --help",
    );
    text = replace_all(
        &text,
        r"^(runtime (?:read|summary): )[^\n]* at line \d+ column \d+$",
        |found, text| format!("{}<runtime>", group(found, text, 1)),
    );
    text = replace_all(
        &text,
        r"^(runtime \w+: )[^\n]*: Is a directory \(os error 21\)$",
        |found, text| format!("{}<runtime>", group(found, text, 1)),
    );
    text = runtime_text(text);
    let mut text = text
        .replace("Usage: focus engine audit ", "Usage: <cli> audit ")
        .replace(
            "`focus engine orchestrate report`",
            "<cli> orchestrate report",
        )
        .replace("`focus engine log`", "<cli> log");
    // The state tool names itself, its verbs and the orchestrator's. A
    // memory file copied whole may name upstream's script too.
    text = replace_all(
        &text,
        r"focus-(state|orchestrate)\.ts(?: ([a-z-]+))?",
        |found, text| {
            let tool = group(found, text, 1);
            match found.group(2) {
                Some(range) => format!("<cli> {tool} {}", &text[range]),
                None => format!("<cli> {tool}"),
            }
        },
    );
    text = replace_all(
        &text,
        r"`focus engine (state|orchestrate)(?: ([^`]*))?`",
        |found, text| {
            let tool = group(found, text, 1);
            match found.group(2) {
                Some(range) => format!("<cli> {tool} {}", &text[range]),
                None => format!("<cli> {tool}"),
            }
        },
    );
    text = text
        .replace("Usage: focus engine state ", "Usage: <cli> state ")
        .replace("Run focus engine state --help", "Run <cli> state --help");
    // The state tool naming one of its own verbs in a sentence.
    text = replace_all(
        &text,
        r"(?<![`\w])focus engine state (?=[a-z-]+ )",
        |_, _| "<cli> state ".to_owned(),
    );
    text = text
        .replace("Usage: focus engine utility ", "Usage: <cli> utility ")
        .replace("how closely Focus checks", "how closely <engine> checks");
    // A validity receipt's hashes are taken over paths that carry the
    // workspace directory's name, so they differ from upstream's by
    // construction; the recorder verified each engine's against its own
    // tree and recorded them as <verified> (differential.py, SHAPES).
    text = replace_all(
        &text,
        r#""(structureHash|contentHash)":"sha256:[0-9a-f]{64}""#,
        |found, text| format!("\"{}\":\"<verified>\"", group(found, text, 1)),
    );
    // A summary's authorization id is taken over the questions file's
    // name, which carries the workspace directory's; recorded the same way.
    // (What is hashed is pinned in `focus-evidence::authorization`.)
    text = replace_all(
        &text,
        r#"(\*\*Summary Authorization Id\*\*: |"summary_authorization_id":"|^  "id": ")[0-9a-f]{64}"#,
        |found, text| format!("{}<verified>", group(found, text, 1)),
    );
    // A source baseline is the hash of a listing of hashes of the tree's
    // own files, recorded the same way (see `own_baseline`).
    text = replace_all(
        &text,
        r"(\*\*Source Baseline\*\*: sha256:)[0-9a-f]{64}",
        |found, text| format!("{}<verified>", group(found, text, 1)),
    );
    text = replace_all(
        &text,
        r"`focus engine config set (guard\.[a-z-]+) off`",
        |found, text| format!("<cli-config {} off>", group(found, text, 1)),
    );
    text = replace_all(
        &text,
        r"`focus engine config set guard-policy (strict|relaxed|off)`",
        |found, text| format!("<cli-config guard-policy {}>", group(found, text, 1)),
    );
    text = text.replace(" --harness claude\n", "\n");
    text = replace_all(
        &text,
        r#"(?:<project-dir>/)?[^\s"]*focus-state\.md: Is a directory \(os error 21\)"#,
        |_, _| "<runtime>".to_owned(),
    );
    text = replace_all(
        &text,
        r"(?<![\w-])focus-state(?![\w.-])(?: ([a-z-]+))?(?![\w-])",
        |found, text| match found.group(1) {
            Some(range) => format!("<cli> state {}", &text[range]),
            None => "<cli> state".to_owned(),
        },
    );
    // The utility and jump tools' own names in a row's Tool or Command.
    text = replace_all(&text, r"(?<![\w-])focus-utility(?![\w.-])", |_, _| {
        "<cli> utility".to_owned()
    });
    text = replace_all(&text, r"(?<![\w-])focus-jump(?![\w.-])", |_, _| {
        "<cli> jump".to_owned()
    });
    // The runtime tool naming itself inside a hook's drop line.
    text = replace_all(
        &text,
        r"(?<![\w-])focus runtime (compile|read|summary):",
        |found, text| format!("<cli> runtime {}:", group(found, text, 1)),
    );
    // A handoff receipt's instant in epoch milliseconds, written during the replay.
    text = replace_all(&text, r#""issuedAtMs":(\d{13})"#, |found, text| {
        let millis: u128 = group(found, text, 1).parse().unwrap_or(0);
        if millis >= started_ms() {
            "\"issuedAtMs\":<now-ms>".to_owned()
        } else {
            text[found.range()].to_owned()
        }
    });
    // A tool named in a sentence by its command.
    text = replace_all(
        &text,
        r"`focus engine ([a-z-]+)(?: ([^`]*))?`",
        |found, text| {
            let tool = group(found, text, 1);
            match found.group(2) {
                Some(range) => format!("<cli> {tool} {}", &text[range]),
                None => format!("<cli> {tool}"),
            }
        },
    );
    let day = today();
    text = replace_all(&text, &format!(r"(?<![0-9a-f]){day}-(?=[a-z])"), |_, _| {
        "<day>-".to_owned()
    });
    text = replace_all(&text, r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", |found, text| {
        let stamp = &text[found.range()];
        if stamp >= started() {
            "<now>".to_owned()
        } else {
            stamp.to_owned()
        }
    });
    replace_all(
        &text,
        r#"(could not be opened: [^\n(]*\()[^\n]*?\)(?="\}|\n|$)"#,
        |found, text| format!("{}<runtime>)", group(found, text, 1)),
    )
}

/// Every file of the workspace, as the recorder lists them: Focus's own lock
/// left out.
fn workspace_files(root: &Path) -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            // The process map (`.focus-sessions/pids/<pid>`) names this
            // host's own processes, the engine's ancestors at recording time:
            // the differential compares it live and records none of it.
            if path.is_dir() {
                if name != "pids" || !rel.contains(".focus-sessions") {
                    walk(root, &path, files);
                }
            } else if !name.starts_with(".focus-lock") {
                let mut text = String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned();
                if name.ends_with(".rules-held.json") {
                    text = own_hashes(root, &text);
                }
                if rel.contains("source-review")
                    && name.starts_with("baseline-")
                    && name.ends_with(".tsv")
                {
                    let (rel, text) = own_baseline(&rel, &name, &text);
                    files.insert(rel, text);
                    continue;
                }
                files.insert(rel, text);
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, &root.join("focus"), &mut files);
    files
}

/// A source baseline's snapshot is kept under the hash of its bytes and
/// lists the tree's source files with the hash of each: hashes that differ
/// between the engines by construction, since a tree's files carry the
/// engine's name. The recorder verified each engine's against its own tree
/// and recorded the name as `baseline-<verified>.tsv` and a file's or a
/// link's hash as `<own>`, the rows in an order that does not depend on the
/// engine's name; the replay writes Focus's the same way.
fn own_baseline(rel: &str, name: &str, text: &str) -> (String, String) {
    let mut rows: Vec<String> = text
        .split('\n')
        .filter(|row| !row.is_empty())
        .map(|row| {
            replace_all(
                row,
                r"^([^\t\\]*\t[^\t\\]+\t(?:100644|100755|120000)\t)[0-9a-f]{64}$",
                |found, row| format!("{}<own>", group(found, row, 1)),
            )
        })
        .collect();
    rows.sort_by_key(|row| replace_all(row, "[Ff][Oo][Cc][Uu][Ss]", |_, _| "\u{1}".to_owned()));
    let rel = format!(
        "{}baseline-<verified>.tsv",
        rel.strip_suffix(name).unwrap_or_default()
    );
    (rel, rows.iter().map(|row| format!("{row}\n")).collect())
}

/// A rules record hashes the memory files as each engine ships them, which
/// differ by the engine's name in their text: the recorder shows a hash
/// that is the tree's own file's as `<sha256>`, and the replay does the same.
fn own_hashes(root: &Path, text: &str) -> String {
    replace_all(text, r#""([^"]+\.md)":"([0-9a-f]{64})""#, |found, text| {
        let rel = group(found, text, 1);
        let digest = fs::read(root.join(rel))
            .map(|bytes| format!("{:x}", Sha256::digest(&bytes)))
            .unwrap_or_default();
        if digest == group(found, text, 2) {
            format!("\"{rel}\":\"<sha256>\"")
        } else {
            text[found.range()].to_owned()
        }
    })
}

/// The environment a check runs in: nothing of Focus's own, and no mark of
/// the host this replay happens to run under (a refusal names the host whose
/// terminal the command came from).
fn clean_env(command: &mut Command) {
    for (name, _) in std::env::vars() {
        let upper = name.to_uppercase();
        if name.starts_with("FOCUS_")
            || name == "CLAUDE_PROJECT_DIR"
            || upper == "TERM_PROGRAM"
            || [
                "CLAUDECODE",
                "CLAUDE_CODE_",
                "CODEX_",
                "CURSOR_",
                "KIRO_",
                "OPENCODE",
                "COPILOT_",
                "VSCODE_",
            ]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
        {
            command.env_remove(name);
        }
    }
}

/// The recorder's `minted`: a clone identity minted during the run, written
/// as placeholders in a shard's name and in the clone file.
fn minted(rel: &str, text: Option<String>) -> (String, Option<String>) {
    let shown = replace_all(
        rel,
        r"/audit/[a-z0-9][a-z0-9-]*-[0-9a-f]{12}\.md$",
        |_, _| "/audit/<host>-<token>.md".to_owned(),
    );
    let shown = shown.replace(&format!("/intents/{}-", today()), "/intents/<day>-");
    let text = match text {
        Some(text) if shown.ends_with("/.focus-clone-id") => {
            let regex = regress::Regex::new(r"^[0-9a-f]{12}\n[a-z0-9][a-z0-9-]*\n$").unwrap();
            Some(if regex.find(&text).is_some() {
                "<token>\n<host>\n".to_owned()
            } else {
                text
            })
        }
        Some(text) if shown.ends_with("/intents.json") || shown.ends_with("/creation-receipt") => {
            Some(replace_all(
                &text,
                r"[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}",
                |_, _| "<uuid>".to_owned(),
            ))
        }
        other => other,
    };
    (shown, text)
}

/// The day a record minted during the replay is named after, `YYMMDD`.
fn today() -> String {
    let stamp = focus_audit::time::iso_timestamp(std::time::SystemTime::now());
    format!("{}{}{}", &stamp[2..4], &stamp[5..7], &stamp[8..10])
}

/// Run an audit check's commands in order in `root`; the outcome in the
/// recorder's shape.
fn replay_audit(check: &Value, root: &Path) -> BTreeMap<String, String> {
    let before = workspace_files(root);
    let steps = check["steps"].as_array().unwrap();
    let (mut exits, mut outs, mut errs) = (Vec::new(), Vec::new(), Vec::new());
    for step in steps {
        let mut command = Command::new(env!("CARGO_BIN_EXE_focus"));
        let mut args: Vec<String> = step
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap().replace("{P}", root.to_str().unwrap()))
            .collect();
        // A step that names its own tool (`@state advance …`) runs that one.
        let tool = match args.first().and_then(|first| first.strip_prefix('@')) {
            Some(named) => {
                let named = named.to_owned();
                args.remove(0);
                named
            }
            None => check["tool"].as_str().unwrap().to_owned(),
        };
        command.args(["engine", &tool]);
        // A hook step is the hook's name and the host's payload, which it
        // reads on stdin.
        let stdin = if tool == "hook" {
            let payload = args.get(1).cloned().unwrap_or_default();
            args.truncate(1);
            Some(payload)
        } else {
            None
        };
        // A step that says where the project is says it itself, as in the
        // recorder.
        let placed = args.iter().any(|arg| arg == "--project-dir");
        command.args(args);
        if !placed {
            command.args(["--project-dir", root.to_str().unwrap()]);
        }
        command.args(["--harness", "claude"]);
        clean_env(&mut command);
        for (name, value) in check["env"].as_object().unwrap() {
            // A name given as `=NAME` is set as it is: the recorder's way of
            // naming a terminal's host.
            match name.strip_prefix('=') {
                Some(raw) => command.env(raw, value.as_str().unwrap()),
                None => command.env(format!("FOCUS_{name}"), value.as_str().unwrap()),
            };
        }
        let output = match stdin {
            None => command.current_dir(root).output().unwrap(),
            Some(payload) => {
                let mut child = command
                    .current_dir(root)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                use std::io::Write;
                // A hook that answers before reading its payload leaves the
                // pipe closed; what it printed is still the answer.
                let _ = child.stdin.take().unwrap().write_all(payload.as_bytes());
                child.wait_with_output().unwrap()
            }
        };
        exits.push(output.status.code().unwrap_or(-1).to_string());
        outs.push(normalise_audit(
            &String::from_utf8_lossy(&output.stdout),
            root,
        ));
        errs.push(normalise_audit(
            &String::from_utf8_lossy(&output.stderr),
            root,
        ));
    }
    let mark = |parts: Vec<String>| -> String {
        if parts.len() == 1 {
            return parts.into_iter().next().unwrap();
        }
        parts
            .iter()
            .enumerate()
            .map(|(index, part)| {
                if part.ends_with('\n') || part.is_empty() {
                    format!("[{index}] {part}")
                } else {
                    format!("[{index}] {part}\n")
                }
            })
            .collect()
    };
    let mut actual = BTreeMap::new();
    actual.insert("exit".to_owned(), exits.join(","));
    actual.insert("stdout".to_owned(), mark(outs));
    actual.insert("stderr".to_owned(), mark(errs));
    let after = workspace_files(root);
    let paths: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    for rel in paths {
        if before.get(rel) == after.get(rel) {
            continue;
        }
        let text = after.get(rel).map(|text| normalise_audit(text, root));
        let (shown, text) = minted(rel, text);
        actual.insert(shown, text.unwrap_or_else(|| "<removed>".to_owned()));
    }
    placeholder_messages(actual)
}

/// The recorder's `placeholder_messages`: the message records minted during
/// a run, numbered in the order the ledger names them (the `Message Id` of
/// each HUMAN_TURN row, in order; a record no row names after those, by its
/// `at` and id), and their ids written as placeholders wherever they appear.
fn placeholder_messages(files: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let file_re = regress::Regex::new(r"/\.focus-sessions/messages/([0-9a-f]{8})\.json$").unwrap();
    let mut minted_ids: Vec<(String, String)> = Vec::new();
    for (key, text) in &files {
        if let Some(found) = file_re.find(key)
            && text != "<removed>"
        {
            let id = key[found.group(1).unwrap()].to_owned();
            let at = focus_js::json::parse(text)
                .ok()
                .and_then(|value| value.get("at").and_then(Value::as_str).map(str::to_owned))
                .unwrap_or_default();
            minted_ids.push((at, id));
        }
    }
    if minted_ids.is_empty() {
        return files;
    }
    minted_ids.sort();
    let named: Vec<String> = minted_ids.into_iter().map(|(_, id)| id).collect();
    let row_re = regress::Regex::new(r"\*\*Message Id\*\*: ([0-9a-f]{8})\n").unwrap();
    let mut ordered: Vec<String> = Vec::new();
    for (key, text) in &files {
        if !key.contains("/audit/") {
            continue;
        }
        for found in row_re.find_iter(text) {
            let id = text[found.group(1).unwrap()].to_owned();
            if named.contains(&id) && !ordered.contains(&id) {
                ordered.push(id);
            }
        }
    }
    for id in &named {
        if !ordered.contains(id) {
            ordered.push(id.clone());
        }
    }
    let names: Vec<(String, String)> = ordered
        .into_iter()
        .enumerate()
        .map(|(n, id)| (id, format!("<message-{}>", n + 1)))
        .collect();
    files
        .into_iter()
        .map(|(mut key, mut text)| {
            for (id, name) in &names {
                key = key.replace(
                    &format!("/messages/{id}.json"),
                    &format!("/messages/{name}.json"),
                );
                text = text
                    .replace(&format!("\"id\":\"{id}\""), &format!("\"id\":\"{name}\""))
                    .replace(
                        &format!("**Message Id**: {id}\n"),
                        &format!("**Message Id**: {name}\n"),
                    );
            }
            (key, text)
        })
        .collect()
}

fn problems_in(
    expected: &serde_json::Map<String, Value>,
    mut actual: BTreeMap<String, String>,
) -> Vec<String> {
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
    // The plan `resolve` writes into the record it finds.
    fn plans(root: &Path, dir: &Path, files: &mut BTreeMap<String, String>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                plans(root, &path, files);
            } else if path.ends_with(".focus-engine/plan.json") {
                let rel = path.strip_prefix(root).unwrap().to_string_lossy();
                files.insert(rel.into_owned(), fs::read_to_string(&path).unwrap());
            }
        }
    }
    plans(root, &root.join("focus/spaces"), &mut files);
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
    if [
        "audit", "runtime", "state", "utility", "jump", "log", "hook",
    ]
    .contains(&check["tool"].as_str().unwrap_or_default())
    {
        return problems_in(
            check["expect"].as_object().unwrap(),
            replay_audit(check, root),
        );
    }
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
    clean_env(&mut command);
    for (name, value) in check["env"].as_object().unwrap() {
        match name.strip_prefix('=') {
            Some(raw) => command.env(raw, value.as_str().unwrap()),
            None => command.env(format!("FOCUS_{name}"), value.as_str().unwrap()),
        };
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

    problems_in(expected, actual)
}

#[test]
fn focus_does_what_upstream_did_in_every_recorded_check() {
    let dir = fixtures().join("recorded");
    let trees: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("trees.json")).unwrap()).unwrap();
    // `FOCUS_RECORDED_ONLY=<text>[,<text>]` replays the checks whose name
    // holds one of the texts, for working on a group; the whole recording
    // is the test.
    let only: Option<Vec<String>> = std::env::var("FOCUS_RECORDED_ONLY")
        .ok()
        .map(|list| list.split(',').map(str::to_owned).collect());
    let checks: Vec<Value> = fs::read_to_string(dir.join("checks.ndjson"))
        .unwrap()
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|check: &Value| {
            only.as_deref().is_none_or(|only| {
                only.iter()
                    .any(|text| check["name"].as_str().unwrap().contains(text.as_str()))
            })
        })
        .collect();
    assert!(
        only.is_some() || checks.len() > 1000,
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
                                materialise(&trees[*tree], &own, check["base"] == "empty");
                                own
                            } else {
                                if !shared_ready {
                                    materialise(&trees[*tree], &shared, check["base"] == "empty");
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
        "{} of {} recorded checks differ from upstream; first: {}",
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
    for verb in ["append", "append-batch", "append-raw", "history"] {
        let of_verb: Vec<&Value> = checks
            .iter()
            .filter(|c| {
                c["tool"] == "audit" && c["steps"][0][0] == verb && c["deviation"].is_null()
            })
            .collect();
        let exits = |code: &str| {
            of_verb
                .iter()
                .any(|c| c["expect"]["exit"].as_str().unwrap().split(',').next() == Some(code))
        };
        assert!(exits("0"), "no recorded success for audit {verb}");
        assert!(exits("1"), "no recorded failure for audit {verb}");
    }
    // A runtime check runs several verbs in a row; each must succeed in some
    // check and fail in some check, wherever in the row it stands.
    for verb in ["compile", "read", "summary"] {
        let exits = |code: &str| {
            checks
                .iter()
                .filter(|c| c["tool"] == "runtime" && c["deviation"].is_null())
                .any(|c| {
                    let codes = c["expect"]["exit"].as_str().unwrap().split(',');
                    c["steps"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .zip(codes)
                        .any(|(step, exit)| step[0] == verb && exit == code)
                })
        };
        assert!(exits("0"), "no recorded success for runtime {verb}");
        assert!(exits("1"), "no recorded failure for runtime {verb}");
    }
    for (tool, verb) in [
        ("state", "get"),
        ("state", "set"),
        ("state", "checkbox"),
        ("state", "count"),
        ("log", "decision"),
        ("log", "answers"),
        ("log", "answer"),
    ] {
        let exits = |code: &str| {
            checks
                .iter()
                .filter(|c| c["tool"] == tool && c["deviation"].is_null())
                .any(|c| {
                    let codes = c["expect"]["exit"].as_str().unwrap().split(',');
                    c["steps"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .zip(codes)
                        .any(|(step, exit)| step[0] == verb && exit == code)
                })
        };
        assert!(exits("0"), "no recorded success for {tool} {verb}");
        assert!(exits("1"), "no recorded failure for {tool} {verb}");
    }
    for verb in ["detect", "now"] {
        let ran = checks
            .iter()
            .filter(|c| c["tool"] == "utility" && c["deviation"].is_null())
            .filter(|c| {
                c["expect"]["exit"]
                    .as_str()
                    .unwrap()
                    .split(',')
                    .all(|exit| exit == "0")
            })
            .any(|c| {
                c["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|step| step[0] == verb)
            });
        assert!(ran, "no recorded success for utility {verb}");
    }
    // Each hook has a recorded run that wrote into the record, and one that
    // stood aside and wrote nothing but its heartbeat.
    for hook in [
        "validate-state",
        "session-end",
        "log-subagent",
        "sync-workflow-state",
        "write-audit-log",
    ] {
        let runs: Vec<&Value> = checks
            .iter()
            .filter(|c| c["tool"] == "hook" && c["deviation"].is_null())
            .filter(|c| {
                c["steps"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|step| step[0] == hook)
            })
            .filter(|c| {
                c["expect"]["exit"]
                    .as_str()
                    .unwrap()
                    .split(',')
                    .all(|exit| exit == "0")
            })
            .collect();
        let wrote = |c: &&Value| {
            c["expect"].as_object().unwrap().keys().any(|key| {
                key.ends_with("/audit/laptop-abc123.md") || key.ends_with("/focus-state.md")
            })
        };
        assert!(
            runs.iter().any(wrote),
            "no recorded run of {hook} wrote into the record"
        );
        assert!(
            runs.iter().any(|c| !wrote(c)),
            "no recorded run of {hook} stood aside"
        );
    }
    // A deviation is recorded and not replayed, so a recording made mostly
    // of them would prove little: they stay under a tenth of it, and for
    // each command what is replayed is at least twice what is not.
    let deviations = checks.iter().filter(|c| !c["deviation"].is_null()).count();
    assert!(
        deviations * 10 < checks.len(),
        "{deviations} of {} recorded checks are deviations; the replay proves little",
        checks.len()
    );
    for tool in [
        "graph", "audit", "runtime", "state", "utility", "jump", "log", "hook",
    ] {
        let of_tool: Vec<&Value> = checks
            .iter()
            .filter(|c| c["tool"].as_str().unwrap_or("graph") == tool)
            .collect();
        let apart = of_tool.iter().filter(|c| !c["deviation"].is_null()).count();
        assert!(
            !of_tool.is_empty() && apart * 3 <= of_tool.len(),
            "{apart} of the {} recorded checks of {tool} are deviations",
            of_tool.len()
        );
    }
}

//! The oracle protocol for the evidence (see `focus-graph`'s `oracle`
//! module for the protocol).
//!
//! `source_listing_parse` cases read a snapshot's text; `source_listing`
//! cases write a listing given as pairs, and say what changed between it
//! and a second one; `source_changed` cases take a recorded listing under
//! the boundary of a current one and tell the paths that changed;
//! `source_walk` cases walk a project laid out under a
//! scratch root; `summary` cases read a questions file's summary section:
//! the choice on it, and the hash of what was confirmed or why there is
//! none.

use std::path::Path;

use serde_json::{Value, json};

use crate::review;
use crate::review_record;
use crate::source::listing::{self, Listing};
use crate::source::raw;
use crate::source::state;
use crate::source::walk::{self, Budgets, Names, Options, Stop};
use crate::summary;

/// The kinds of case this crate answers.
pub const KINDS: [&str; 10] = [
    "source_listing_parse",
    "source_listing",
    "source_changed",
    "source_walk",
    "summary",
    "summary_input",
    "review_binding",
    "review_completion",
    "review_record",
    "review_text",
];

/// Lay the case's project out and walk it.
/// What the walk that took the .NET output directories gave for the last
/// `source_walk` case that was asked.
static EARLIER_OF_THE_CASE_BEFORE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn walk_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    // Under `/tmp` as the host really has it: where `/tmp` is itself a
    // link, a path inside the project would otherwise leave it and come
    // back, which on upstream's host it does not.
    let tmp = std::fs::canonicalize("/tmp").map_or_else(
        |_| "/tmp".to_owned(),
        |real| real.to_string_lossy().into_owned(),
    );
    let root = format!("{tmp}/oracle-{}", text("id"));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::create_dir_all(&root);
    // The names that carry the engine's are part of what is hashed, and
    // upstream's carry its own: the case gives them in pieces, so that this
    // answer is taken under the same ones and the two fingerprints can be
    // compared. A path of the case names the registry file as `{R}`, and
    // so do the answers.
    let piece = |at: usize| -> String {
        case["names"][at]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect()
    };
    let names = Names {
        filesystem: piece(0),
        workspace: piece(1),
        registry: piece(2),
    };
    let at = |rel: &str| format!("{root}/{}", rel.replace("{R}", &names.registry));
    let shown = |text: &str| text.replace(&names.registry, "{R}");
    let place = |rel: &str| {
        let path = at(rel);
        if let Some(parent) = Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        path
    };
    let mode = |path: &str, mode: u32| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
        }
        #[cfg(not(unix))]
        let _ = (path, mode);
    };
    for (rel, content) in case["files"].as_object().into_iter().flatten() {
        match content.as_str() {
            None => {
                let _ = std::fs::create_dir_all(at(rel));
            }
            Some(text) => {
                let path = place(rel);
                let _ = std::fs::write(&path, text);
                mode(&path, 0o644);
            }
        }
    }
    for (rel, hex) in case["hex"].as_object().into_iter().flatten() {
        let hex = hex.as_str().unwrap_or_default();
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .filter_map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).ok())
            .collect();
        let path = place(rel);
        let _ = std::fs::write(&path, bytes);
        mode(&path, 0o644);
    }
    for rel in case["exec"].as_array().into_iter().flatten() {
        mode(&at(rel.as_str().unwrap_or_default()), 0o755);
    }
    for link in case["links"].as_array().into_iter().flatten() {
        if let (Some(rel), Some(target)) = (link[0].as_str(), link[1].as_str()) {
            let path = place(rel);
            #[cfg(unix)]
            let _ = std::os::unix::fs::symlink(target, &path);
            #[cfg(not(unix))]
            let _ = (target, path);
        }
    }
    for rel in case["fifos"].as_array().into_iter().flatten() {
        let path = place(rel.as_str().unwrap_or_default());
        let _ = std::process::Command::new("mkfifo")
            .args(["-m", "644", &path])
            .status();
    }
    let env = |name: &str| {
        case["env"]
            .get(focus_workspace::env::full_name(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let options = Options {
        budgets: Budgets::from(env),
        raw_line_endings: env("TEST_RAW_LINE_ENDINGS").as_deref() == Some("1"),
        names: names.clone(),
    };
    let repos: Vec<String> = case["repos"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();
    let project = at(text("project"));
    let project = Path::new(&project);
    let walked = walk::workspace_source(
        project,
        &repos,
        case["dotnet"].as_bool().unwrap_or(true),
        &options,
    );
    // What a recorded value is told of this source, as a decision reads it
    // now: the value itself, what each earlier walk gives for the same
    // files, another value, and none.
    let same = match (
        &walked,
        state::workspace_source_state(project, &repos, &options),
    ) {
        (Ok(own), Ok(current)) => {
            let earlier = walk::workspace_source(project, &repos, false, &options).ok();
            let earlier_fingerprint = earlier.as_ref().map(|walked| walked.fingerprint.clone());
            let now = Some(current.fingerprint.as_str());
            // (The first is this case's own walk, which is the current
            // source's only when it took the same boundary.)
            let recorded = [
                Some(own.fingerprint.clone()),
                Some(current.fingerprint.clone()),
                state::legacy_alias(&current.fingerprint),
                raw::raw_form(&current.fingerprint),
                earlier.as_ref().map(|walked| walked.fingerprint.clone()),
                earlier.and_then(|walked| walked.legacy),
                Some("0".repeat(64)),
                None,
            ];
            let mut told: Vec<bool> = recorded
                .iter()
                .map(|value| state::same_workspace_source(value.as_deref(), now))
                .collect();
            told.push(state::same_workspace_source(None, None));
            told.push(state::same_workspace_source(now, None));
            // And what the walk that took the .NET output directories gave
            // for the case before this one: what a walk leaves for a
            // comparison is kept by the fingerprint, and is this source's
            // or nobody's.
            let mut before = EARLIER_OF_THE_CASE_BEFORE
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            told.push(state::same_workspace_source(before.as_deref(), now));
            *before = earlier_fingerprint;
            json!(told)
        }
        _ => Value::Null,
    };
    let _ = std::fs::remove_dir_all(&root);
    let source = match walked {
        Err(Stop::NotPorted(what)) => return json!({ "unsupported": what }),
        Err(Stop::Failed(failure)) => {
            return json!({
                "failure": {
                    "code": failure.code,
                    // What the system said is the host's, not the walk's.
                    "detail": failure.detail.split(": ").next().map(shown),
                    "path": failure.path.as_deref().map(shown),
                    "repo": failure.repo,
                }
            });
        }
        Ok(source) => source,
    };
    // A fingerprint is taken over the paths: one that holds the engine's
    // name differs between the engines by construction. The earlier walk's
    // also takes in the files left out by name since, and the engine's
    // settings files are among them.
    let holds_name = |text: &str| focus_js::re::test("focus", "i", text);
    let named = source
        .listing
        .iter()
        .any(|(key, _)| holds_name(&shown(key)));
    let legacy_named = named
        || ["files", "hex"].iter().any(|kind| {
            case[kind]
                .as_object()
                .into_iter()
                .flatten()
                .any(|(rel, _)| holds_name(rel.rsplit('/').next().unwrap_or(rel)))
        });
    let said = |named: bool, value: String| {
        if named { "<named>".to_owned() } else { value }
    };
    // The rows in an order that does not depend on the engine's name (the
    // order a snapshot is written in is the listing cases').
    let neutral = |row: &str| focus_js::re::replace("focus", "gi", row, "\u{1}");
    let text = shown(&listing::serialize(&source.listing).unwrap_or_else(|error| error));
    let mut rows: Vec<&str> = text.split('\n').filter(|row| !row.is_empty()).collect();
    rows.sort_by(|a, b| focus_js::order::cmp_utf16(&neutral(a), &neutral(b)));
    // The fingerprint as it was before line endings were read as LF, when
    // this walk read a file whose raw bytes differ from its text.
    let before = raw::raw_form(&source.fingerprint);
    json!({
        "fingerprint": said(named, source.fingerprint),
        "legacy": source.legacy.map(|legacy| said(legacy_named, legacy)),
        "raw": before.map(|raw| said(named, raw)),
        "dotnet": source.dotnet_output_seen,
        "rows": rows,
        "same": same,
    })
}

/// A listing given as pairs of a key and its entry.
#[must_use]
pub fn pairs(value: &Value) -> Listing {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pair| Some((pair.get(0)?.as_str()?, pair.get(1)?.as_str()?)))
        .collect()
}

fn written(listing: &Listing) -> Value {
    match listing::serialize(listing) {
        Ok(text) => json!({ "text": text }),
        Err(error) => json!({ "error": error }),
    }
}

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    match text("kind") {
        "source_listing_parse" => {
            let sha = listing::sha256(text("text"));
            match listing::parse(text("text")) {
                Err(error) => json!({ "error": error, "sha": sha }),
                Ok(None) => json!({ "listing": null, "sha": sha }),
                Ok(Some(parsed)) => json!({
                    "listing": written(&parsed),
                    "size": parsed.len(),
                    "sha": sha,
                }),
            }
        }
        "source_listing" => {
            let before = pairs(&case["before"]);
            let after = pairs(&case["after"]);
            json!({
                "before": written(&before),
                "after": written(&after),
                "changed": listing::changed_paths(&before, &after),
                "keys": listing::render_path_keys(before.iter().chain(after.iter()).map(|(key, _)| key)),
            })
        }
        "source_changed" => {
            // A recorded listing as today's walk would take it, what
            // changed between it and the current one, and paths as a
            // person is told them.
            let current = pairs(&case["current"]);
            let kept =
                listing::recorded_under_current_boundary(&pairs(&case["recorded"]), &current);
            let changed = listing::changed_paths(&kept, &current);
            let paths: Vec<String> = case["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect();
            let told = listing::render_changed_paths(&changed);
            json!({
                "kept": kept.iter().map(|(key, entry)| json!([key, entry])).collect::<Vec<_>>(),
                "changed": changed,
                "told": told,
                "paths": listing::render_changed_paths(&paths),
                "inert": paths.iter().map(|path| listing::inert_path(path)).collect::<Vec<_>>(),
            })
        }
        "source_walk" => walk_case(case),
        "summary" => {
            let content = text("content");
            let answer = match summary::answer(content) {
                Ok(answer) => answer,
                Err(refusal) => return json!({ "unreadable": refusal.to_string() }),
            };
            match summary::content_hash(content) {
                Ok(hash) => json!({ "answer": answer, "hash": hash }),
                Err(error) => json!({ "answer": answer, "error": error }),
            }
        }
        "summary_input" => {
            // Text, or bytes that are none (`hex`).
            let bytes = match case.get("hex").and_then(Value::as_str) {
                Some(hex) => unhex(hex),
                None => text("text").as_bytes().to_vec(),
            };
            match summary::review_input_fingerprint(&bytes) {
                Ok(fingerprint) => json!(fingerprint),
                Err(refusal) => json!({ "unreadable": refusal.to_string() }),
            }
        }
        "review_binding" => review_binding_case(case),
        "review_text" => {
            // What a reviewer wrote, checked and, for a file of its own,
            // with its headings made level 3. The bytes are the case's.
            use crate::review_text::{
                Expected, Validity, demote_review_headings, validate_review_appendix,
            };
            let bytes = match case.get("hex").and_then(Value::as_str) {
                Some(hex) => unhex(hex),
                None => text("text").as_bytes().to_vec(),
            };
            if text("op") == "demote" {
                return match demote_review_headings(&bytes) {
                    None => Value::Null,
                    // (The bytes are text: they were decoded to be changed.)
                    Some(demoted) => json!({
                        "text": String::from_utf8_lossy(&demoted.bytes),
                        "changed": demoted.changed,
                    }),
                };
            }
            let expected = &case["expected"];
            match validate_review_appendix(
                &bytes,
                &Expected {
                    verdict: expected["verdict"].as_str().unwrap_or_default(),
                    reviewer: expected["reviewer"].as_str().unwrap_or_default(),
                    iteration: expected["iteration"].as_f64().unwrap_or_default(),
                    review_challenge: expected["reviewChallenge"].as_str(),
                    standalone: expected["standalone"] == true,
                },
            ) {
                Validity::Valid => json!({ "valid": true }),
                Validity::Invalid { reason, heading } => {
                    if heading {
                        json!({ "valid": false, "heading": true, "reason": reason })
                    } else {
                        json!({ "valid": false, "reason": reason })
                    }
                }
            }
        }
        "review_record" => {
            let bytes = text("text").as_bytes();
            let completion = text("completion");
            let record = review_record::parse(bytes);
            let digest = review_record::digest(bytes);
            // The record is handed over as the reader of the record's folder
            // would: only under the digest of its own bytes.
            let read =
                |named: &review::RecordRef| record.clone().filter(|_| named.digest == digest);
            json!({
                "valid": record.is_some(),
                "digest": digest,
                "matches": record
                    .as_ref()
                    .map(|record| review_record::matches_completion(record, completion)),
                "carries": review::request_binding(text("request")).map(|request| {
                    review_record::completion_carries_verified_review(&request, completion, &read)
                }),
            })
        }
        "review_completion" => {
            let completion = text("completion");
            json!({
                "ref": review::record_ref(completion)
                    .map(|record| json!({ "path": record.path, "digest": record.digest })),
                "matches": review::request_binding(text("request"))
                    .map(|request| review::completion_matches_request(&request, completion)),
            })
        }
        other => json!({ "unsupported": other }),
    }
}

/// What a review request's row binds, whether a completion can be held to
/// it, and whether the row replaces the request pending at its pass.
fn review_binding_case(case: &Value) -> Value {
    let block = case["block"].as_str().unwrap_or_default();
    let binding = review::request_binding(block);
    let pending = match &case["pending"] {
        Value::Null => None,
        pending => Some(pending["block"].as_str().and_then(review::request_binding)),
    };
    let written = |binding: &review::RequestBinding| {
        json!({
            "artifactFingerprint": binding.artifact_fingerprint,
            "requestId": binding.request_id,
            "legacyAppendix": binding.legacy_appendix.as_ref().map(|legacy| json!({
                "artifact": legacy.artifact,
                "offset": legacy.offset,
                "priorDigest": legacy.prior_digest,
                "priorLength": legacy.prior_length,
                "challenge": legacy.challenge,
                "priorAppendix": legacy.prior_appendix,
            })),
            "sourceFingerprint": binding.source_fingerprint,
            "unitSourceFingerprint": binding.unit_source_fingerprint,
            "recoveryCause": binding.recovery_cause,
        })
    };
    json!({
        "binding": binding.as_ref().map(written),
        "modern": binding.as_ref().map(|binding| {
            review::binding_is_modern(binding, case["workspace_requires"] == true)
        }),
        "replaces": review::request_replaces(
            block,
            pending.as_ref().map(Option::as_ref),
        ),
    })
}

/// The bytes a case writes as pairs of hex digits.
#[must_use]
pub fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .filter_map(|at| u8::from_str_radix(hex.get(at * 2..at * 2 + 2)?, 16).ok())
        .collect()
}

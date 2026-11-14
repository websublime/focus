//! The oracle protocol for the evidence (see `focus-graph`'s `oracle`
//! module for the protocol).
//!
//! `source_listing_parse` cases read a snapshot's text; `source_listing`
//! cases write a listing given as pairs, and say what changed between it
//! and a second one; `source_walk` cases walk a project laid out under a
//! scratch root; `summary` cases read a questions file's summary section:
//! the choice on it, and the hash of what was confirmed or why there is
//! none.

use std::path::Path;

use serde_json::{Value, json};

use crate::review;
use crate::source::listing::{self, Listing};
use crate::source::raw;
use crate::source::walk::{self, Budgets, Names, Options, Stop};
use crate::summary;

/// The kinds of case this crate answers.
pub const KINDS: [&str; 5] = [
    "source_listing_parse",
    "source_listing",
    "source_walk",
    "summary",
    "review_binding",
];

/// Lay the case's project out and walk it.
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
    let walked = walk::workspace_source(
        Path::new(&at(text("project"))),
        &repos,
        case["dotnet"].as_bool().unwrap_or(true),
        &options,
    );
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
    })
}

fn pairs(value: &Value) -> Listing {
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
        "review_binding" => review_binding_case(case),
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

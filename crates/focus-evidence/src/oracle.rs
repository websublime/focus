//! The oracle protocol for the evidence (see `focus-graph`'s `oracle`
//! module for the protocol).
//!
//! `source_listing_parse` cases read a snapshot's text; `source_listing`
//! cases write a listing given as pairs, and say what changed between it
//! and a second one.

use serde_json::{Value, json};

use crate::source::listing::{self, Listing};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 2] = ["source_listing_parse", "source_listing"];

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
        other => json!({ "unsupported": other }),
    }
}

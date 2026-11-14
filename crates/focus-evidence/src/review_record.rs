//! The review a completion names.
//!
//! A verdict leaves the review as a JSON document under the record's engine
//! folder, and the `REVIEW_COMPLETED` row names it and pins its bytes: a
//! document edited afterwards stops being the review. This module reads the
//! document (`parseReviewRecordBytes`, `isReviewRecord`), says whether it is
//! the review its row describes (`reviewRecordMatchesCompletion`) and whether
//! a completion carries the verified review its request needs
//! (`completionCarriesVerifiedReview`, `aidlc-lib.ts` L17 324 to L17 427,
//! L17 663 and L17 509).
//!
//! # A value as text
//!
//! Three fields are tested as text whatever they are (`String(value)`): a
//! finding's id, the id of a related finding and an earlier decision. So a
//! list of one id reads as that id, as it does upstream.
//!
//! # Tests that change no answer
//!
//! - What that text is for a `null`, a flag or a number, alone or as an
//!   entry of a list: none of them reads as an id or as a decision whichever
//!   way it is written.
//! - A completion of a request with no id carries its review only when it
//!   names no record at all. One that names a record badly has already
//!   failed to answer the request.
//! - Reading a record starts by asking whether the path is a record's. A
//!   reference is only made from a path that is, so the answer is known.

use std::path::Path;

use focus_audit::read::block_field;
use focus_js::{json, num, re};
use focus_workspace::{files, links};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::review::{self, RequestBinding};

/// `ReviewRecord`, with the fields a completion is held to. The document is
/// kept whole for the readers of its findings.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub stage: String,
    pub unit: Option<String>,
    pub workflow: Option<String>,
    pub iteration: f64,
    pub reviewer: String,
    pub verdict: String,
    pub request_id: Option<String>,
    pub request_challenge: Option<String>,
    pub artifact_fingerprint: String,
    pub source_fingerprint: Option<String>,
    pub unit_source_fingerprint: Option<String>,
    pub document: Value,
}

/// `reviewRecordDigest`.
#[must_use]
pub fn digest(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

/// `String(value)` of a value a JSON document holds.
fn as_text(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => num::to_string(number.as_f64().unwrap_or(f64::NAN)),
        Value::String(text) => text.clone(),
        // A list is its entries joined by commas, an absent one as nothing.
        Value::Array(entries) => entries
            .iter()
            .map(|entry| match entry {
                Value::Null => String::new(),
                entry => as_text(entry),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// `validReviewFindingStatus`: one of the four fixed statuses, or a
/// rejection with its reason.
fn finding_status(status: &str) -> bool {
    matches!(status, "New" | "Unresolved" | "Resolved" | "Accepted risk")
        || status
            .strip_prefix("Rejected: ")
            .is_some_and(|reason| re::test(r"^\S[\s\S]*$", "", reason))
}

fn stored_finding(value: &Value) -> Option<&Map<String, Value>> {
    let finding = value.as_object()?;
    let text = |key: &str| finding.get(key).and_then(Value::as_str);
    let id = finding
        .get("id")
        .map_or_else(|| "undefined".to_owned(), as_text);
    (re::test("^R-[0-9]+$", "", &id)
        && ["severity", "location", "finding", "required_action"]
            .iter()
            .all(|key| text(key).is_some())
        && text("status").is_some_and(finding_status))
    .then_some(finding)
}

fn derived_finding(value: &Value) -> bool {
    let Some(finding) = stored_finding(value) else {
        return false;
    };
    let absent_or = |key: &str, test: &dyn Fn(&Value) -> bool| finding.get(key).is_none_or(test);
    ["decided_at_severity", "reviewer_note", "reopened_reason"]
        .iter()
        .all(|key| absent_or(key, &Value::is_string))
        && absent_or("earlier_decision", &|decision| {
            decision == "Accepted risk"
                || re::test(r"^Rejected: \S[\s\S]*$", "", &as_text(decision))
        })
        && absent_or("related_finding_id", &|id| {
            re::test("^R-[0-9]+$", "", &as_text(id))
        })
        && [
            "not_rechecked",
            "resolved_by_reviewer",
            "resolved_in_review",
            "introduced_in_review",
        ]
        .iter()
        .all(|key| absent_or(key, &Value::is_boolean))
}

/// `parseReviewRecordBytes`: the document `bytes` hold, when it is a review
/// record: every field there and of its kind.
#[must_use]
pub fn parse(bytes: &[u8]) -> Option<Record> {
    let document = json::parse(&String::from_utf8_lossy(bytes)).ok()?;
    let fields = document.as_object()?;
    let text = |key: &str| fields.get(key).and_then(Value::as_str).map(str::to_owned);
    // There, and text or null.
    let nullable = |key: &str| match fields.get(key)? {
        Value::Null => Some(None),
        Value::String(text) => Some(Some(text.clone())),
        _ => None,
    };
    let number = |key: &str| fields.get(key).and_then(Value::as_f64);
    if number("version") != Some(1.0) {
        return None;
    }
    let iteration = number("iteration").filter(|pass| num::is_integer(*pass) && *pass > 0.0)?;
    let verdict =
        text("verdict").filter(|verdict| matches!(verdict.as_str(), "READY" | "NOT-READY"))?;
    if !fields
        .get("findings")?
        .as_array()?
        .iter()
        .all(|finding| stored_finding(finding).is_some())
    {
        return None;
    }
    if let Some(derived) = fields.get("derived_findings")
        && !derived.as_array()?.iter().all(derived_finding)
    {
        return None;
    }
    text("attempt")?;
    text("body")?;
    text("recorded_at")?;
    Some(Record {
        stage: text("stage")?,
        unit: nullable("unit")?,
        workflow: nullable("workflow")?,
        iteration,
        reviewer: text("reviewer")?,
        verdict,
        request_id: nullable("request_id")?,
        request_challenge: nullable("request_challenge")?,
        artifact_fingerprint: text("artifact_fingerprint")?,
        source_fingerprint: nullable("source_fingerprint")?,
        unit_source_fingerprint: nullable("unit_source_fingerprint")?,
        document,
    })
}

/// `reviewRecordMatchesCompletion`: whether `record` is the review the
/// completion row `block` says it is. The digest binds the bytes to the row;
/// this binds what they say to the row's scope, so a true record of another
/// review cannot be named.
#[must_use]
pub fn matches_completion(record: &Record, block: &str) -> bool {
    let field = |name: &str| block_field(block, name);
    Some(record.stage.as_str()) == field("Stage")
        && record.unit.as_deref().unwrap_or_default() == field("Unit").unwrap_or_default()
        && record.workflow.as_deref().unwrap_or_default() == field("Workflow").unwrap_or_default()
        && field("Iteration").is_some_and(|pass| record.iteration == num::from_str(pass))
        && Some(record.reviewer.as_str()) == field("Reviewer")
        && Some(record.verdict.as_str()) == field("Verdict")
        && record.request_id.as_deref() == field("Request Id")
        && Some(record.artifact_fingerprint.as_str()) == field("Artifact Fingerprint")
        && record.source_fingerprint.as_deref() == field("Source Fingerprint")
        && record.unit_source_fingerprint.as_deref() == field("Unit Source Fingerprint")
        && record.request_challenge.as_deref() == field("Review Challenge")
}

/// `completionCarriesVerifiedReview`: whether the completion row `block`
/// answers `request` and carries the review in the form the request's era
/// needs. A request with an id needs a record that verifies; one from before
/// ids may be answered with no record, and any record it does name has to
/// verify too. `read` hands over the record a reference names, or nothing
/// when it is not there or is not the bytes the reference pins.
#[must_use]
pub fn completion_carries_verified_review(
    request: &RequestBinding,
    block: &str,
    read: &dyn Fn(&review::RecordRef) -> Option<Record>,
) -> bool {
    if !review::completion_matches_request(request, block) {
        return false;
    }
    match review::record_ref(block) {
        None => {
            let names_record = block_field(block, "Review Record").is_some()
                || block_field(block, "Review Record Digest").is_some();
            request.request_id.is_none() && !names_record
        }
        Some(named) => read(&named).is_some_and(|record| matches_completion(&record, block)),
    }
}

/// `REVIEW_RECORD_MAX_BYTES`: a review record is a small document; anything
/// larger is not one.
pub const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// `readReviewRecord`: the record the reference `named` names under the
/// record's folder, or nothing when it is not there, is not read as a plain
/// file of the record (a link on the way, a second name, too large), or is
/// not the bytes the reference pins: a record edited after it was recorded
/// is not the review.
#[must_use]
pub fn read(record: Option<&Path>, named: &review::RecordRef) -> Option<Record> {
    if !review::is_record_relative_path(&named.path) {
        return None;
    }
    let anchor = std::fs::canonicalize(record?).ok()?;
    let relative = Path::new(&named.path);
    links::refuse_link_in_chain(&anchor, relative).ok()?;
    let bytes =
        files::read_regular_within(&anchor.join(relative), "review record", Some(MAX_BYTES))
            .ok()?;
    (digest(&bytes) == named.digest)
        .then(|| parse(&bytes))
        .flatten()
}

//! The review a completion names.
//!
//! A verdict leaves the review as a JSON document under the record's engine
//! folder, and the `REVIEW_COMPLETED` row names it and pins its bytes: a
//! document edited afterwards stops being the review. This module reads the
//! document (`parseReviewRecordBytes`, `isReviewRecord`), says whether it is
//! the review its row describes (`reviewRecordMatchesCompletion`) and whether
//! a completion carries the verified review its request needs
//! (`completionCarriesVerifiedReview`, `aidlc-lib.ts` L17 324 to L17 427,
//! L17 663 and L17 509). And it writes the document a verdict leaves
//! (`serializeReviewRecord`, L17 256).
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
use crate::review_findings::{Finding, valid_status};

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
pub(crate) fn as_text(value: &Value) -> String {
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
        && text("status").is_some_and(valid_status))
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

/// Why a record's findings are not read: a value is a list where a text
/// belongs. The record's reader admits it, as upstream's patterns read a
/// list as its entries joined by commas; upstream then carries the list,
/// which equals no id and no decision. Focus does not (a deliberate
/// deviation).
pub const NOT_A_TEXT: &str = "the review record holds a finding with a value that is not written as text: its findings are not read";

/// The findings of a record's `key` list, each with what the list keeps
/// about it when `marks` asks for that.
fn listed(
    record: &Record,
    artifact: &str,
    key: &str,
    marks: bool,
) -> Result<Option<Vec<Finding>>, String> {
    let Some(findings) = record.document.get(key).and_then(Value::as_array) else {
        return Ok(None);
    };
    let mut shaped = Vec::new();
    for finding in findings {
        // There or not, and a text when there.
        let text = |key: &str| match finding.get(key) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(NOT_A_TEXT.to_owned()),
        };
        let needed = |key: &str| text(key).map(Option::unwrap_or_default);
        let mark = |key: &str| if marks { text(key) } else { Ok(None) };
        let flag = |key: &str| finding.get(key).filter(|_| marks).and_then(Value::as_bool);
        shaped.push(
            Finding {
                artifact: artifact.to_owned(),
                unit: record.unit.clone().filter(|unit| !unit.is_empty()),
                id: needed("id")?,
                severity: needed("severity")?,
                location: needed("location")?,
                finding: needed("finding")?,
                required_action: needed("required_action")?,
                status: needed("status")?,
                decided_at_severity: mark("decided_at_severity")?,
                reviewer_note: mark("reviewer_note")?,
                not_rechecked: flag("not_rechecked"),
                resolved_by_reviewer: flag("resolved_by_reviewer"),
                resolved_in_review: flag("resolved_in_review"),
                earlier_decision: mark("earlier_decision")?,
                reopened_reason: mark("reopened_reason")?,
                related_finding_id: mark("related_finding_id")?,
                introduced_in_review: flag("introduced_in_review"),
                ..Finding::default()
            }
            .with_fingerprint(),
        );
    }
    Ok(Some(shaped))
}

/// `reviewRecordFindings`: the findings of a record, as the gate and the
/// decisions go by them.
///
/// # Errors
///
/// A finding holds a list where a text belongs ([`NOT_A_TEXT`]).
pub fn findings(record: &Record, artifact: &str) -> Result<Vec<Finding>, String> {
    Ok(listed(record, artifact, "findings", false)?.unwrap_or_default())
}

/// `reviewRecordDerivedFindings`: the list the engine kept when the record
/// was written, or nothing when the record is from before it kept one.
///
/// # Errors
///
/// As [`findings`].
pub fn derived_findings(record: &Record, artifact: &str) -> Result<Option<Vec<Finding>>, String> {
    listed(record, artifact, "derived_findings", true)
}

/// `ReviewRecordFinding`: a finding as a record keeps it. The artifact and
/// the unit are the record's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFinding {
    pub id: String,
    pub severity: String,
    pub location: String,
    pub finding: String,
    pub required_action: String,
    pub status: String,
}

/// `ReviewRecordDerivedFinding`: a finding of the list derived from this
/// review and the ones before it, with what was decided about it since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedFinding {
    pub stored: StoredFinding,
    pub decided_at_severity: Option<String>,
    pub reviewer_note: Option<String>,
    pub not_rechecked: Option<bool>,
    pub resolved_by_reviewer: Option<bool>,
    pub resolved_in_review: Option<bool>,
    pub earlier_decision: Option<String>,
    pub reopened_reason: Option<String>,
    pub related_finding_id: Option<String>,
    pub introduced_in_review: Option<bool>,
}

/// `derivedRecordFinding` (`aidlc-log.ts`): one finding of the engine's
/// list as a review record stores it. A mark is written where the list
/// holds one, whatever it says: a `false` is not an absence.
#[must_use]
pub fn derived_record_finding(finding: &Finding) -> DerivedFinding {
    DerivedFinding {
        stored: StoredFinding {
            id: finding.id.clone(),
            severity: finding.severity.clone(),
            location: finding.location.clone(),
            finding: finding.finding.clone(),
            required_action: finding.required_action.clone(),
            status: finding.status.clone(),
        },
        decided_at_severity: finding.decided_at_severity.clone(),
        reviewer_note: finding.reviewer_note.clone(),
        not_rechecked: finding.not_rechecked,
        resolved_by_reviewer: finding.resolved_by_reviewer,
        resolved_in_review: finding.resolved_in_review,
        earlier_decision: finding.earlier_decision.clone(),
        reopened_reason: finding.reopened_reason.clone(),
        related_finding_id: finding.related_finding_id.clone(),
        introduced_in_review: finding.introduced_in_review,
    }
}

/// `ReviewRecord`, as a verdict writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub stage: String,
    pub unit: Option<String>,
    pub workflow: Option<String>,
    pub attempt: String,
    pub iteration: f64,
    pub reviewer: String,
    pub verdict: String,
    pub request_id: Option<String>,
    pub request_challenge: Option<String>,
    pub artifact_fingerprint: String,
    pub source_fingerprint: Option<String>,
    pub unit_source_fingerprint: Option<String>,
    pub findings: Vec<StoredFinding>,
    pub derived_findings: Option<Vec<DerivedFinding>>,
    pub body: String,
    pub recorded_at: String,
}

fn stored(finding: &StoredFinding) -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in [
        ("id", &finding.id),
        ("severity", &finding.severity),
        ("location", &finding.location),
        ("finding", &finding.finding),
        ("required_action", &finding.required_action),
        ("status", &finding.status),
    ] {
        out.insert(key.to_owned(), Value::String(value.clone()));
    }
    out
}

/// `serializeReviewRecord`: the record's canonical bytes: its keys in one
/// order, two spaces of indentation, one line end after it. A row names the
/// record by the hash of these bytes.
#[must_use]
pub fn serialize(record: &Written) -> String {
    let text = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
    let mut out = Map::new();
    let mut put = |key: &str, value: Value| {
        out.insert(key.to_owned(), value);
    };
    put("version", Value::from(1));
    put("stage", Value::String(record.stage.clone()));
    put("unit", text(&record.unit));
    put("workflow", text(&record.workflow));
    put("attempt", Value::String(record.attempt.clone()));
    put("iteration", Value::from(record.iteration));
    put("reviewer", Value::String(record.reviewer.clone()));
    put("verdict", Value::String(record.verdict.clone()));
    put("request_id", text(&record.request_id));
    put("request_challenge", text(&record.request_challenge));
    put(
        "artifact_fingerprint",
        Value::String(record.artifact_fingerprint.clone()),
    );
    put("source_fingerprint", text(&record.source_fingerprint));
    put(
        "unit_source_fingerprint",
        text(&record.unit_source_fingerprint),
    );
    put(
        "findings",
        Value::Array(
            record
                .findings
                .iter()
                .map(|finding| Value::Object(stored(finding)))
                .collect(),
        ),
    );
    if let Some(derived) = &record.derived_findings {
        let derived = derived.iter().map(|finding| {
            let mut out = stored(&finding.stored);
            let mut put = |key: &str, value: Option<Value>| {
                if let Some(value) = value {
                    out.insert(key.to_owned(), value);
                }
            };
            let said = |value: &Option<String>| value.clone().map(Value::String);
            let flag = |value: Option<bool>| value.map(Value::Bool);
            put("decided_at_severity", said(&finding.decided_at_severity));
            put("reviewer_note", said(&finding.reviewer_note));
            put("not_rechecked", flag(finding.not_rechecked));
            put("resolved_by_reviewer", flag(finding.resolved_by_reviewer));
            put("resolved_in_review", flag(finding.resolved_in_review));
            put("earlier_decision", said(&finding.earlier_decision));
            put("reopened_reason", said(&finding.reopened_reason));
            put("related_finding_id", said(&finding.related_finding_id));
            put("introduced_in_review", flag(finding.introduced_in_review));
            Value::Object(out)
        });
        put("derived_findings", Value::Array(derived.collect()));
    }
    put("body", Value::String(record.body.clone()));
    put("recorded_at", Value::String(record.recorded_at.clone()));
    format!("{}\n", json::stringify_pretty(&Value::Object(out)))
}

#[cfg(test)]
mod tests {
    use super::{NOT_A_TEXT, derived_findings, findings, parse};

    fn record(findings: &str, derived: &str) -> Vec<u8> {
        format!(
            r#"{{"version":1,"stage":"s","unit":null,"workflow":null,"attempt":"a","iteration":1,"reviewer":"r","verdict":"READY","request_id":null,"request_challenge":null,"artifact_fingerprint":"f","source_fingerprint":null,"unit_source_fingerprint":null,"findings":[{findings}],"derived_findings":[{derived}],"body":"","recorded_at":"t"}}"#
        )
        .into_bytes()
    }

    fn finding(id: &str, more: &str) -> String {
        format!(
            r#"{{"id":{id},"severity":"Major","location":"l","finding":"f","required_action":"a","status":"New"{more}}}"#
        )
    }

    /// Deliberate deviation. The record's reader admits a finding whose id,
    /// earlier decision or related id is a list that reads as the text, as
    /// upstream's patterns do; upstream then carries the list, which equals
    /// no id and no decision. Focus does not read such a record's findings.
    #[test]
    fn a_records_finding_written_with_a_list_for_a_text_is_not_read() {
        let refused = Err(NOT_A_TEXT.to_owned());
        let listed = parse(&record(&finding(r#"["R-01"]"#, ""), "")).expect("a record");
        assert_eq!(findings(&listed, "a.md"), refused);
        for more in [
            r#","earlier_decision":["Rejected: x"]"#,
            r#","related_finding_id":["R-02"]"#,
        ] {
            let derived = parse(&record(
                &finding("\"R-01\"", ""),
                &finding("\"R-01\"", more),
            ))
            .expect("a record");
            assert_eq!(
                derived_findings(&derived, "a.md").map(|_| ()),
                Err(NOT_A_TEXT.to_owned())
            );
            // What the record lists is read: the list the engine kept is
            // another reading.
            assert!(findings(&derived, "a.md").is_ok());
        }
    }
}

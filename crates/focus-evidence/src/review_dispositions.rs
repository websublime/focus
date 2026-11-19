//! What a person decided about a review's findings at a gate, as a ledger
//! row keeps it, and which rows speak for a review's scope.
//!
//! A gate row carries the decisions in one field, a small JSON document
//! ([`parse_field`], `parseDispositionField`, `aidlc-review-brief.ts`
//! L1 138). A decision that is not of the form is left out; the ones that
//! are come back as written, so two rows can be told to agree or not.
//! [`gate_applies_to_scope`] and [`redo_applies_to_scope`] say whether a
//! row is one the list of a stage and a Unit goes by (L585, L610).
//!
//! # Deliberate deviation
//!
//! Upstream tests a decision's id, fingerprint, status and digest with
//! patterns, and a pattern reads a list as its entries joined by commas: a
//! decision whose id is the list `["R-01"]` passes, and goes on as a list
//! that equals no finding's id. Focus does not carry a list where a text
//! belongs: the field is refused, with the sentence
//! [`NOT_A_TEXT`]. The same holds for a field that is the JSON `null`,
//! which upstream fails on with an error of its runtime.

use focus_audit::read::block_field;
use focus_js::{json, re, text as js_text};
use serde_json::Value;

use crate::review_list::Disposition;
use crate::review_record::as_text;

/// `REVIEW_FINDING_DISPOSITIONS_FIELD`.
pub const FIELD: &str = "Review Finding Dispositions";

/// Why a field of decisions is refused: one of its values is a list (or
/// the field is `null`) where upstream would read it as a text or fail.
pub const NOT_A_TEXT: &str = "the Review Finding Dispositions field holds a decision that is not written as text: it is not read";

/// A decision as read, and as written.
#[derive(Debug, Clone, PartialEq)]
pub struct Read {
    pub disposition: Disposition,
    /// The entry of the document, as it stands there.
    pub entry: Value,
}

impl Read {
    /// `JSON.stringify` of the entry: what two rows are compared by.
    #[must_use]
    pub fn written(&self) -> String {
        json::stringify(&self.entry)
    }
}

/// `dispositionKey`: the finding a decision is about.
#[must_use]
pub fn key(artifact: &str, id: &str) -> String {
    format!("{artifact}\0{id}")
}

/// How a value stands against a pattern upstream tests it with.
enum Tested<'v> {
    /// A text that matches.
    Text(&'v str),
    /// Not of the form: the decision is left out.
    No,
    /// Not a text, and a match all the same as upstream reads it.
    Coerced,
}

/// `pattern.test(value ?? "")`, for the patterns here: none of them
/// matches a value that is not there.
fn tested<'v>(value: Option<&'v Value>, matches: &dyn Fn(&str) -> bool) -> Tested<'v> {
    match value {
        None | Some(Value::Null) => Tested::No,
        Some(Value::String(text)) if matches(text) => Tested::Text(text),
        Some(Value::String(_)) => Tested::No,
        Some(other) if matches(&as_text(other)) => Tested::Coerced,
        Some(_) => Tested::No,
    }
}

fn is_digest(text: &str) -> bool {
    re::test("^sha256:[0-9a-f]{64}$", "", text)
}

fn is_status(text: &str) -> bool {
    text == "Accepted risk"
        || re::test(r"^Rejected: \S[\s\S]*$", "", text)
        || re::test(r"^Reopened: \S[\s\S]*$", "", text)
}

/// One entry of the document: a decision, nothing when it is not of the
/// form, or the refusal.
fn entry(value: &Value) -> Result<Option<Disposition>, String> {
    let Some(row) = value.as_object() else {
        return Ok(None);
    };
    let Some(Value::String(artifact)) = row.get("artifact") else {
        return Ok(None);
    };
    // Each test in upstream's order: the first that fails leaves the
    // decision out, whatever stands after it.
    let mut coerced = false;
    let mut text = |tested: Tested<'_>| match tested {
        Tested::Text(text) => Some(text.to_owned()),
        Tested::No => None,
        Tested::Coerced => {
            coerced = true;
            Some(String::new())
        }
    };
    let Some(id) = text(tested(row.get("id"), &|id| re::test("^R-[0-9]+$", "", id))) else {
        return Ok(None);
    };
    let Some(fingerprint) = text(tested(row.get("fingerprint"), &is_digest)) else {
        return Ok(None);
    };
    // (`status === "Accepted risk"` is no pattern: a list is never that.)
    let status = match row.get("status") {
        Some(Value::String(status)) => {
            text(tested(row.get("status"), &is_status)).map(|_| status.clone())
        }
        other => text(tested(other, &|status| {
            re::test(r"^Rejected: \S[\s\S]*$", "", status)
                || re::test(r"^Reopened: \S[\s\S]*$", "", status)
        })),
    };
    let Some(status) = status else {
        return Ok(None);
    };
    let decided_at_severity = match row.get("decided_at_severity") {
        None => None,
        Some(Value::String(severity)) => Some(severity.clone()),
        Some(_) => return Ok(None),
    };
    let reviewed_record = match row.get("reviewed_record") {
        None => None,
        Some(Value::Object(record)) => {
            let Some(Value::String(path)) = record.get("path") else {
                return Ok(None);
            };
            let Some(digest) = text(tested(record.get("digest"), &is_digest)) else {
                return Ok(None);
            };
            Some((path.clone(), digest))
        }
        Some(_) => return Ok(None),
    };
    if coerced {
        return Err(NOT_A_TEXT.to_owned());
    }
    Ok(Some(Disposition {
        artifact: artifact.clone(),
        id,
        fingerprint,
        status,
        decided_at_severity,
        reviewed_record,
    }))
}

/// `parseDispositionField`: the decisions a gate row's field holds. A field
/// that is not there, is not JSON or is not the document holds none.
///
/// # Errors
///
/// A decision is of the form only as upstream reads a list for a text, or
/// the field is `null` ([`NOT_A_TEXT`]).
pub fn parse_field(value: Option<&str>) -> Result<Vec<Read>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    // (An empty field is no JSON either.)
    let Ok(parsed) = json::parse(value) else {
        return Ok(Vec::new());
    };
    if parsed.is_null() {
        return Err(NOT_A_TEXT.to_owned());
    }
    let (Some(version), Some(Value::Array(dispositions))) =
        (parsed.get("version"), parsed.get("dispositions"))
    else {
        return Ok(Vec::new());
    };
    if version.as_f64() != Some(1.0) {
        return Ok(Vec::new());
    }
    let mut read = Vec::new();
    for value in dispositions {
        if let Some(disposition) = entry(value)? {
            read.push(Read {
                disposition,
                entry: value.clone(),
            });
        }
    }
    Ok(read)
}

/// `parsedDispositions`: the decisions of a ledger row.
///
/// # Errors
///
/// As [`parse_field`].
pub fn of_block(block: &str) -> Result<Vec<Read>, String> {
    parse_field(block_field(block, FIELD))
}

/// `gateAppliesToScope`: whether the row is a gate's answer for the stage
/// (its own, or one of the stages it names) and, when a Unit is asked for,
/// for that Unit or for none.
#[must_use]
pub fn gate_applies_to_scope(event: &str, block: &str, stage: &str, unit: Option<&str>) -> bool {
    if event != "GATE_APPROVED" && event != "GATE_REJECTED" {
        return false;
    }
    let named = block_field(block, "Gate Stages")
        .unwrap_or("")
        .split(',')
        .any(|entry| js_text::trim(entry) == stage && !stage.is_empty());
    if block_field(block, "Stage") != Some(stage) && !named {
        return false;
    }
    match (unit, block_field(block, "Unit")) {
        (Some(unit), Some(of_row)) => of_row == unit,
        _ => true,
    }
}

/// `pathUnit`: the Unit a path under `construction/<unit>/<stage>/` is of.
#[must_use]
pub fn path_unit(path: &str, stage: &str) -> Option<String> {
    let found = re::find("(?:^|/)construction/([^/]+)/([^/]+)/", "", path)?;
    (re::group(path, &found, 2) == Some(stage))
        .then(|| re::group(path, &found, 1).map(str::to_owned))
        .flatten()
}

/// `redoAppliesToScope`: whether the row is a Redo from scratch that
/// begins a new list for the scope. The row carries no Unit, so on a
/// per-Unit stage the Units are read from its artifact paths; a row that
/// names no Unit's path covers every Unit.
#[must_use]
pub fn redo_applies_to_scope(event: &str, block: &str, stage: &str, unit: Option<&str>) -> bool {
    if event != "ARTIFACT_REUSED"
        || block_field(block, "Stage") != Some(stage)
        || block_field(block, "Workflow").is_some()
        || block_field(block, "Decision").map(js_text::to_lower_case) != Some("redo".to_owned())
    {
        return false;
    }
    let Some(unit) = unit else {
        return true;
    };
    let units: Vec<String> = block_field(block, "Artifacts")
        .unwrap_or("")
        .split(',')
        // (Upstream turns the separators of the system it runs on into
        // `/` first. A row's paths are written with `/`, and Focus reads
        // them the same on every system.)
        .filter_map(|path| path_unit(js_text::trim(path), stage))
        .collect();
    units.is_empty() || units.iter().any(|of_path| of_path == unit)
}

#[cfg(test)]
mod tests {
    use super::{NOT_A_TEXT, parse_field};

    const DIGEST: &str = "sha256:abababababababababababababababababababababababababababababababab";

    fn field(decision: &str) -> String {
        format!(r#"{{"version":1,"dispositions":[{decision}]}}"#)
    }

    /// Deliberate deviation. Upstream tests a decision's id, fingerprint,
    /// status and digest with patterns, which read a list as its entries
    /// joined by commas: the decision passes and goes on holding a list.
    /// Focus refuses the field. And a field that is `null`, which upstream
    /// fails on with an error of its runtime, is refused the same way.
    #[test]
    fn a_decision_written_with_a_list_for_a_text_is_refused() {
        for decision in [
            format!(
                r#"{{"artifact":"a.md","id":["R-01"],"fingerprint":"{DIGEST}","status":"Accepted risk"}}"#
            ),
            format!(
                r#"{{"artifact":"a.md","id":"R-01","fingerprint":[["{DIGEST}"]],"status":"Accepted risk"}}"#
            ),
            format!(
                r#"{{"artifact":"a.md","id":"R-01","fingerprint":"{DIGEST}","status":["Rejected: a","b"]}}"#
            ),
            format!(
                r#"{{"artifact":"a.md","id":"R-01","fingerprint":"{DIGEST}","status":"Accepted risk","reviewed_record":{{"path":"p","digest":["{DIGEST}"]}}}}"#
            ),
        ] {
            assert_eq!(
                parse_field(Some(&field(&decision))),
                Err(NOT_A_TEXT.to_owned()),
                "{decision}"
            );
        }
        assert_eq!(parse_field(Some("null")), Err(NOT_A_TEXT.to_owned()));
        // A list that is not of the form either way leaves the decision
        // out, as upstream leaves it, and so does one after a value that
        // already fails.
        for decision in [
            format!(
                r#"{{"artifact":"a.md","id":["R-01","R-02"],"fingerprint":"{DIGEST}","status":"Accepted risk"}}"#
            ),
            format!(
                r#"{{"artifact":"a.md","id":"R-01","fingerprint":"{DIGEST}","status":["Accepted risk"]}}"#
            ),
            r#"{"artifact":"a.md","id":["R-01"],"fingerprint":"short","status":"Accepted risk"}"#
                .to_owned(),
        ] {
            assert_eq!(
                parse_field(Some(&field(&decision))),
                Ok(Vec::new()),
                "{decision}"
            );
        }
    }
}

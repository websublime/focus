//! The validity receipt a completion leaves: what the stage read and wrote,
//! fingerprinted, so a later edit to either can be told from the record.
//!
//! The receipt rides on the `STAGE_COMPLETED` row as its `Validation Basis`
//! field: the stage's graph contract, the project type, and one entry per
//! input and output artifact with a hash of the set of files it resolved to
//! and a hash of their contents. Line endings are no change: text is read
//! with CRLF as LF. A receipt that cannot be captured is a `Validation
//! Warning` field instead, and never stops the completion.
//!
//! Mirrors `stageValidationAuditFields`, `captureStageValidationBasis`,
//! `graphContractFingerprint`, `fingerprintInstance` and
//! `aggregateArtifactBasis` in `aidlc-validity.ts`, and `committedTextBytes`
//! in `aidlc-lib.ts`.

use focus_graph::runtime::Stage;
use focus_js::json;
use focus_js::order::{cmp_utf16, locale_compare};
use focus_js::text::trim;
use focus_js::{EngineError, Result};
use focus_state::fields::get_field;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::artifacts::{Instance, resolve_artifact_instances};
use crate::workflow::Workflow;

/// The `STAGE_COMPLETED` field carrying the receipt.
pub const VALIDATION_BASIS_FIELD: &str = "Validation Basis";
/// The field carrying why no receipt could be captured.
pub const VALIDATION_WARNING_FIELD: &str = "Validation Warning";
const VALIDATION_BASIS_SCHEMA: u64 = 3;

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `canonicalValue`: keys sorted at every level, `undefined` left out (a
/// serde value has none, so only the sorting remains).
fn canonical(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|a, b| cmp_utf16(a, b));
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), canonical(&map[key])))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// `canonicalJson`.
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    json::stringify(&canonical(value))
}

/// `committedTextBytes`: the bytes with each CRLF read as LF, when the
/// bytes are text with CRLFs; the bytes as they are otherwise (a lone CR,
/// a NUL, or too many control characters say they are not text).
#[must_use]
pub fn committed_text_bytes(bytes: &[u8]) -> Vec<u8> {
    if !bytes.contains(&13)
        || focus_workspace::env::var("TEST_RAW_LINE_ENDINGS").as_deref() == Some("1")
    {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut printable: u64 = 0;
    let mut nonprintable: i64 = 0;
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == 13 {
            if at + 1 == bytes.len() || bytes[at + 1] != 10 {
                return bytes.to_vec();
            }
            at += 1;
            continue;
        }
        if byte == 0 {
            return bytes.to_vec();
        }
        if byte == 127 || (byte < 32 && !matches!(byte, 10 | 8 | 9 | 27 | 12)) {
            nonprintable += 1;
        } else if byte != 10 {
            printable += 1;
        }
        out.push(byte);
        at += 1;
    }
    // A trailing DOS end-of-file mark is not counted against the text.
    if bytes.last() == Some(&26) {
        nonprintable -= 1;
    }
    if i64::try_from(printable >> 7).unwrap_or(i64::MAX) >= nonprintable {
        out
    } else {
        bytes.to_vec()
    }
}

/// `graphContractFingerprint`: the hash of what the graph says of the
/// stage's artifacts.
#[must_use]
pub fn graph_contract_fingerprint(stage: &Stage) -> String {
    let field = |key: &str| stage.raw.get(key).cloned();
    let mut contract = Map::new();
    contract.insert("slug".to_owned(), Value::String(stage.slug.clone()));
    contract.insert("phase".to_owned(), Value::String(stage.phase.clone()));
    for key in ["execution", "condition", "for_each", "workspace_requires"] {
        if let Some(value) = field(key) {
            contract.insert(key.to_owned(), value);
        }
    }
    contract.insert(
        "consumes".to_owned(),
        field("consumes").unwrap_or_else(|| json!([])),
    );
    contract.insert(
        "produces".to_owned(),
        field("produces").unwrap_or_else(|| json!([])),
    );
    contract.insert(
        "optional_produces".to_owned(),
        field("optional_produces").unwrap_or_else(|| json!([])),
    );
    contract.insert(
        "produces_kinds".to_owned(),
        field("produces_kinds").unwrap_or_else(|| json!({})),
    );
    format!(
        "sha256:{}",
        sha256_hex(canonical_json(&Value::Object(contract)).as_bytes())
    )
}

struct Fingerprint {
    instance: Instance,
    sha256: String,
    present: bool,
}

/// `fingerprintInstance`.
fn fingerprint(instance: Instance) -> Fingerprint {
    let path = &instance.absolute_path;
    if !path.exists() {
        return Fingerprint {
            instance,
            sha256: "missing".to_owned(),
            present: false,
        };
    }
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_file() => {
            return Fingerprint {
                instance,
                sha256: "not-a-file".to_owned(),
                present: false,
            };
        }
        Ok(_) => {}
        Err(err) => {
            return Fingerprint {
                sha256: format!("unreadable:{}", sha256_hex(err.to_string().as_bytes())),
                instance,
                present: false,
            };
        }
    }
    match std::fs::read(path) {
        Ok(bytes) => Fingerprint {
            instance,
            sha256: format!("sha256:{}", sha256_hex(&committed_text_bytes(&bytes))),
            present: true,
        },
        Err(err) => Fingerprint {
            sha256: format!("unreadable:{}", sha256_hex(err.to_string().as_bytes())),
            instance,
            present: false,
        },
    }
}

fn instance_key(print: &Fingerprint) -> String {
    format!(
        "{}\0{}\0{}",
        print.instance.unit.as_deref().unwrap_or(""),
        print.instance.unit_kind.as_deref().unwrap_or(""),
        print.instance.relative_path
    )
}

fn basis_key(basis: &Value) -> String {
    format!(
        "{}\0{}",
        basis["artifact"].as_str().unwrap_or(""),
        basis["producer"].as_str().unwrap_or("")
    )
}

fn sorted_bases(mut bases: Vec<Value>) -> Vec<Value> {
    bases.sort_by(|a, b| locale_compare(&basis_key(a), &basis_key(b)));
    bases
}

/// `aggregateArtifactBasis`: one receipt entry; `None` for an optional
/// artifact that is absent.
fn aggregate(
    artifact: &str,
    producer: &str,
    required: bool,
    instances: Vec<Instance>,
) -> Option<Value> {
    let mut prints: Vec<Fingerprint> = instances
        .into_iter()
        .map(fingerprint)
        .filter(|print| required || print.present)
        .collect();
    prints.sort_by(|a, b| locale_compare(&instance_key(a), &instance_key(b)));
    if !required && prints.is_empty() {
        return None;
    }
    let structure: Vec<Value> = prints
        .iter()
        .map(|print| {
            json!({
                "path": print.instance.relative_path,
                "unit": print.instance.unit,
                "unitKind": print.instance.unit_kind,
            })
        })
        .collect();
    let content: Vec<Value> = prints
        .iter()
        .map(|print| {
            json!({
                "path": print.instance.relative_path,
                "sha256": print.sha256,
                "present": print.present,
            })
        })
        .collect();
    Some(json!({
        "artifact": artifact,
        "producer": producer,
        "required": required,
        "instanceCount": prints.len(),
        "presentCount": prints.iter().filter(|print| print.present).count(),
        "structureHash": format!("sha256:{}", sha256_hex(canonical_json(&Value::Array(structure)).as_bytes())),
        "contentHash": format!("sha256:{}", sha256_hex(canonical_json(&Value::Array(content)).as_bytes())),
    }))
}

/// `projectTypeFrom`: `brownfield` or `greenfield`, read case-blind.
fn project_type(state: &str) -> Option<&'static str> {
    match get_field(state, "Project Type")?.to_lowercase().as_str() {
        "brownfield" => Some("brownfield"),
        "greenfield" => Some("greenfield"),
        _ => None,
    }
}

/// `uniqueProducer`: the one stage producing `artifact`.
fn unique_producer<'a>(artifact: &str, graph: &'a [Stage]) -> Result<&'a Stage> {
    let owners: Vec<&Stage> = graph
        .iter()
        .filter(|stage| stage.produces_artifact(artifact))
        .collect();
    if owners.len() != 1 {
        return Err(EngineError::new(format!(
            "Cannot capture validity for artifact \"{artifact}\": expected exactly one producer, found {}.",
            owners.len()
        )));
    }
    Ok(owners[0])
}

fn capture_inputs(
    wf: &Workflow,
    stage: &Stage,
    state: &str,
    kind: Option<&str>,
) -> Result<Vec<Value>> {
    let graph = &wf.install.graph;
    let mut inputs = Vec::new();
    // The node's own entries: `required` is owed unless it says `false`.
    let consumes = stage
        .raw
        .get("consumes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for consume in &consumes {
        let artifact = consume
            .get("artifact")
            .and_then(Value::as_str)
            .unwrap_or("");
        let conditional_on = consume
            .get("conditional_on")
            .and_then(Value::as_str)
            .filter(|on| !on.is_empty());
        let applies = conditional_on.is_none_or(|on| kind.is_none_or(|kind| on == kind));
        if !applies {
            continue;
        }
        let required = consume.get("required") != Some(&Value::Bool(false));
        let owners = graph
            .iter()
            .filter(|owner| owner.produces_artifact(artifact))
            .count();
        if !required && owners == 0 {
            continue;
        }
        let owner = unique_producer(artifact, graph)?;
        let instances = resolve_artifact_instances(wf, artifact, owner, state)?;
        if let Some(basis) = aggregate(artifact, &owner.slug, required, instances) {
            inputs.push(basis);
        }
    }
    Ok(sorted_bases(inputs))
}

fn capture_outputs(wf: &Workflow, stage: &Stage, state: &str) -> Result<Vec<Value>> {
    let mut required: Vec<&String> = Vec::new();
    for name in &stage.produces {
        if !required.contains(&name) {
            required.push(name);
        }
    }
    let mut optional: Vec<&String> = Vec::new();
    for name in &stage.optional_produces {
        if !optional.contains(&name) {
            optional.push(name);
        }
    }
    let mut outputs = Vec::new();
    for artifact in required.iter().chain(&optional) {
        let is_required = required.contains(artifact);
        let instances = resolve_artifact_instances(wf, artifact, stage, state)?;
        if let Some(basis) = aggregate(artifact, &stage.slug, is_required, instances) {
            outputs.push(basis);
        }
    }
    Ok(sorted_bases(outputs))
}

/// `captureStageValidationBasis`, as canonical JSON.
///
/// # Errors
///
/// An artifact has no single producer, or cannot be resolved.
pub fn capture(wf: &Workflow, stage: &Stage, state: &str) -> Result<String> {
    let kind = project_type(state);
    let inputs = capture_inputs(wf, stage, state, kind)?;
    let outputs = capture_outputs(wf, stage, state)?;
    Ok(canonical_json(&json!({
        "schema": VALIDATION_BASIS_SCHEMA,
        "graphContract": graph_contract_fingerprint(stage),
        "projectType": kind,
        "inputs": inputs,
        "outputs": outputs,
    })))
}

/// `stageValidationAuditFields`: the receipt field, or the warning.
#[must_use]
pub fn audit_fields(wf: &Workflow, stage: &Stage, state: &str) -> Vec<(&'static str, String)> {
    match capture(wf, stage, state) {
        Ok(basis) => vec![(VALIDATION_BASIS_FIELD, basis)],
        Err(err) => {
            let message = trim(&focus_js::re::replace(
                r"[\r\n  ]+",
                "g",
                err.message(),
                " ",
            ))
            .to_owned();
            vec![(
                VALIDATION_WARNING_FIELD,
                format!(
                    "Validity receipt omitted for stage \"{}\": {message}",
                    stage.slug
                ),
            )]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_sha(path: &std::path::Path) -> String {
        format!(
            "sha256:{}",
            sha256_hex(&committed_text_bytes(&std::fs::read(path).unwrap()))
        )
    }

    #[test]
    fn text_with_crlf_is_read_with_lf() {
        assert_eq!(committed_text_bytes(b"a\r\nb\r\n"), b"a\nb\n");
        assert_eq!(committed_text_bytes(b"a\nb\n"), b"a\nb\n");
        // A lone CR, a NUL, or no CRLF at all: the bytes as they are.
        assert_eq!(committed_text_bytes(b"a\rb"), b"a\rb");
        assert_eq!(committed_text_bytes(b"a\r\nb\0"), b"a\r\nb\0");
        assert_eq!(committed_text_bytes(b"a\r\n"), b"a\n");
        assert_eq!(committed_text_bytes(b"\r"), b"\r");
        // Mostly control characters is not text.
        assert_eq!(
            committed_text_bytes(b"\x01\x02\x03\r\n"),
            b"\x01\x02\x03\r\n"
        );
        // A trailing end-of-file mark is forgiven.
        assert_eq!(committed_text_bytes(b"a\r\n\x1a"), b"a\n\x1a");
    }

    #[test]
    fn canonical_json_sorts_keys_at_every_level() {
        let value = json!({"b": [{"z": 1, "a": null}], "a": {"y": true, "x": "s"}});
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"x":"s","y":true},"b":[{"a":null,"z":1}]}"#
        );
    }

    #[test]
    fn the_graph_contract_is_hashed_over_the_artifact_fields() {
        let node = json!({"slug": "s", "phase": "ideation", "name": "S", "produces": ["a"], "consumes": [{"artifact": "b", "required": true}]});
        let stage = &focus_graph::runtime::load_from_value(&json!([node]))[0];
        let expected = sha256_hex(r#"{"consumes":[{"artifact":"b","required":true}],"optional_produces":[],"phase":"ideation","produces":["a"],"produces_kinds":{},"slug":"s"}"#.as_bytes());
        assert_eq!(
            graph_contract_fingerprint(stage),
            format!("sha256:{expected}")
        );
    }

    #[test]
    fn an_entry_hashes_the_files_it_resolved_to() {
        let root = std::env::temp_dir().join(format!("focus-validity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("one.md"), "one\r\n").unwrap();
        let instance = |name: &str| Instance {
            artifact: "x".to_owned(),
            producer: "s".to_owned(),
            absolute_path: root.join(name),
            relative_path: format!("r/{name}"),
            unit: None,
            unit_kind: None,
        };
        let basis =
            aggregate("x", "s", true, vec![instance("two.md"), instance("one.md")]).unwrap();
        assert_eq!(basis["instanceCount"], 2);
        assert_eq!(basis["presentCount"], 1);
        let structure = r#"[{"path":"r/one.md","unit":null,"unitKind":null},{"path":"r/two.md","unit":null,"unitKind":null}]"#;
        assert_eq!(
            basis["structureHash"],
            format!("sha256:{}", sha256_hex(structure.as_bytes()))
        );
        let content = format!(
            r#"[{{"path":"r/one.md","present":true,"sha256":"{}"}},{{"path":"r/two.md","present":false,"sha256":"missing"}}]"#,
            file_sha(&root.join("one.md"))
        );
        assert_eq!(
            basis["contentHash"],
            format!("sha256:{}", sha256_hex(content.as_bytes()))
        );
        assert_eq!(
            file_sha(&root.join("one.md")),
            format!("sha256:{}", sha256_hex(b"one\n"))
        );
        // An optional artifact that is absent is no entry; a present one is.
        assert!(aggregate("x", "s", false, vec![instance("two.md")]).is_none());
        assert_eq!(
            aggregate("x", "s", false, vec![instance("one.md")]).unwrap()["instanceCount"],
            1
        );
        assert_eq!(
            aggregate("x", "s", true, vec![instance("dir")]).unwrap()["presentCount"],
            0
        );
        std::fs::create_dir_all(root.join("dir")).unwrap();
        let dir = aggregate("x", "s", true, vec![instance("dir")]).unwrap();
        let content = r#"[{"path":"r/dir","present":false,"sha256":"not-a-file"}]"#;
        assert_eq!(
            dir["contentHash"],
            format!("sha256:{}", sha256_hex(content.as_bytes()))
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

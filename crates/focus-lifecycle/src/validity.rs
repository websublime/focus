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
use focus_graph::vocabulary::artifact_filename;
use focus_js::json;
use focus_js::order::{cmp_utf16, locale_compare};
use focus_js::text::trim;
use focus_js::{EngineError, Result};
use focus_state::fields::get_field;
use focus_state::progress::{CheckboxState, parse_checkboxes};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::artifacts::{Instance, resolve_artifact_instances};
use crate::events::ordered_main_workflow_audit;
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
    /// The hash over the raw bytes, when line endings made it differ.
    raw_sha256: Option<String>,
    present: bool,
}

/// `RAW_FINGERPRINTS`: a content hash taken over raw line endings of a file
/// read in this process, mapped to that content's current hash; what a
/// recorded receipt from before line endings were read as LF reads as now.
static RAW_FINGERPRINTS: std::sync::Mutex<Vec<(String, String)>> =
    std::sync::Mutex::new(Vec::new());

/// `rememberRawFingerprint`.
fn remember_raw_fingerprint(raw: String, current: String) {
    if raw == current {
        return;
    }
    let mut known = RAW_FINGERPRINTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    known.retain(|(known_raw, _)| *known_raw != raw);
    known.push((raw, current));
}

/// `currentFingerprintForm`.
fn current_fingerprint_form(value: &str) -> String {
    RAW_FINGERPRINTS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .find(|(raw, _)| raw == value)
        .map_or_else(|| value.to_owned(), |(_, current)| current.clone())
}

/// `fingerprintInstance`.
fn fingerprint(instance: Instance) -> Fingerprint {
    let path = &instance.absolute_path;
    if !path.exists() {
        return Fingerprint {
            instance,
            sha256: "missing".to_owned(),
            raw_sha256: None,
            present: false,
        };
    }
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_file() => {
            return Fingerprint {
                instance,
                sha256: "not-a-file".to_owned(),
                raw_sha256: None,
                present: false,
            };
        }
        Ok(_) => {}
        Err(err) => {
            return Fingerprint {
                sha256: format!("unreadable:{}", sha256_hex(err.to_string().as_bytes())),
                instance,
                raw_sha256: None,
                present: false,
            };
        }
    }
    match std::fs::read(path) {
        Ok(bytes) => {
            let text = committed_text_bytes(&bytes);
            let raw_sha256 = (text != bytes).then(|| format!("sha256:{}", sha256_hex(&bytes)));
            Fingerprint {
                instance,
                sha256: format!("sha256:{}", sha256_hex(&text)),
                raw_sha256,
                present: true,
            }
        }
        Err(err) => Fingerprint {
            sha256: format!("unreadable:{}", sha256_hex(err.to_string().as_bytes())),
            instance,
            raw_sha256: None,
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
    let content_hash = format!(
        "sha256:{}",
        sha256_hex(canonical_json(&Value::Array(content)).as_bytes())
    );
    // As it was recorded before line endings were read as LF.
    if prints.iter().any(|print| print.raw_sha256.is_some()) {
        let raw: Vec<Value> = prints
            .iter()
            .map(|print| {
                json!({
                    "path": print.instance.relative_path,
                    "sha256": print.raw_sha256.clone().unwrap_or_else(|| print.sha256.clone()),
                    "present": print.present,
                })
            })
            .collect();
        remember_raw_fingerprint(
            format!(
                "sha256:{}",
                sha256_hex(canonical_json(&Value::Array(raw)).as_bytes())
            ),
            content_hash.clone(),
        );
    }
    Some(json!({
        "artifact": artifact,
        "producer": producer,
        "required": required,
        "instanceCount": prints.len(),
        "presentCount": prints.iter().filter(|print| print.present).count(),
        "structureHash": format!("sha256:{}", sha256_hex(canonical_json(&Value::Array(structure)).as_bytes())),
        "contentHash": content_hash,
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

// --- Reading receipts back ----------------------------------------------------

/// `ArtifactBasis`: one entry of a receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactBasis {
    pub artifact: String,
    pub producer: String,
    pub required: bool,
    pub instance_count: u64,
    pub present_count: u64,
    pub structure_hash: String,
    pub content_hash: String,
}

impl ArtifactBasis {
    fn key(&self) -> String {
        format!("{}\0{}", self.artifact, self.producer)
    }

    fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_object()?;
        // `Number.isInteger(n) && n >= 0`: a whole non-negative number.
        let count = |name: &str| -> Option<u64> {
            let number = map.get(name)?.as_f64()?;
            (number >= 0.0 && number.fract() == 0.0 && number <= 9_007_199_254_740_991.0)
                .then_some(number as u64)
        };
        let instance_count = count("instanceCount")?;
        let present_count = count("presentCount")?;
        if present_count > instance_count {
            return None;
        }
        Some(Self {
            artifact: map.get("artifact")?.as_str()?.to_owned(),
            producer: map.get("producer")?.as_str()?.to_owned(),
            required: map.get("required")?.as_bool()?,
            instance_count,
            present_count,
            structure_hash: map.get("structureHash")?.as_str()?.to_owned(),
            content_hash: map.get("contentHash")?.as_str()?.to_owned(),
        })
    }
}

/// `StageValidationBasis`: a receipt, read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Basis {
    pub graph_contract: String,
    pub project_type: Option<String>,
    pub inputs: Vec<ArtifactBasis>,
    pub outputs: Vec<ArtifactBasis>,
}

fn sorted_typed(mut bases: Vec<ArtifactBasis>) -> Vec<ArtifactBasis> {
    bases.sort_by(|a, b| locale_compare(&a.key(), &b.key()));
    bases
}

/// `parseStageValidationBasis`: a schema-3 receipt, or nothing for any
/// other text (an older schema, a number that is no count, a type that is
/// neither).
#[must_use]
pub fn parse_basis(raw: Option<&str>) -> Option<Basis> {
    let raw = raw.filter(|raw| !raw.is_empty())?;
    let parsed = json::parse(raw).ok()?;
    let map = parsed.as_object()?;
    // `candidate.schema !== 3`: the number three, however it was written.
    if map.get("schema")?.as_f64()? != 3.0 {
        return None;
    }
    let graph_contract = map.get("graphContract")?.as_str()?.to_owned();
    let project_type = match map.get("projectType")? {
        Value::Null => None,
        Value::String(kind) if kind == "brownfield" || kind == "greenfield" => Some(kind.clone()),
        _ => return None,
    };
    let bases = |name: &str| -> Option<Vec<ArtifactBasis>> {
        map.get(name)?
            .as_array()?
            .iter()
            .map(ArtifactBasis::from_value)
            .collect()
    };
    Some(Basis {
        graph_contract,
        project_type,
        inputs: sorted_typed(bases("inputs")?),
        outputs: sorted_typed(bases("outputs")?),
    })
}

/// `CompletionReceipts`: per stage, the receipt of its current attempt
/// (`current`) and the latest one written for it (`latest_known`).
#[derive(Debug, Default)]
pub struct Receipts {
    pub current: Vec<(String, Basis)>,
    pub latest_known: Vec<(String, Basis)>,
}

fn remove_slug(list: &mut Vec<(String, Basis)>, slug: &str) {
    list.retain(|(known, _)| known != slug);
}

fn set_slug(list: &mut Vec<(String, Basis)>, slug: &str, basis: Basis) {
    match list.iter_mut().find(|(known, _)| known == slug) {
        Some(slot) => slot.1 = basis,
        None => list.push((slug.to_owned(), basis)),
    }
}

/// `completionReceiptsFromAudit`.
#[must_use]
pub fn completion_receipts(audit: &str) -> Receipts {
    let mut receipts = Receipts::default();
    for event in ordered_main_workflow_audit(audit) {
        let Some(stage) = audit_field(&event.block, "Stage") else {
            continue;
        };
        if event.event == "STAGE_STARTED" {
            remove_slug(&mut receipts.current, stage);
            continue;
        }
        if event.event != "STAGE_COMPLETED" {
            continue;
        }
        match parse_basis(audit_field(&event.block, VALIDATION_BASIS_FIELD)) {
            Some(basis) => {
                set_slug(&mut receipts.current, stage, basis.clone());
                set_slug(&mut receipts.latest_known, stage, basis);
            }
            None => {
                remove_slug(&mut receipts.current, stage);
                remove_slug(&mut receipts.latest_known, stage);
            }
        }
    }
    receipts
}

fn audit_field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let label = format!("**{name}**:");
    block
        .split('\n')
        .find_map(|line| line.strip_prefix(label.as_str()))
        .map(trim)
        .filter(|value| !value.is_empty())
}

/// `artifactBasisChanges`: `<label>:<artifact>` for every entry that is not
/// the same on both sides, in key order.
fn basis_changes(label: &str, before: &[ArtifactBasis], after: &[ArtifactBasis]) -> Vec<String> {
    let mut keys: Vec<String> = before.iter().chain(after).map(ArtifactBasis::key).collect();
    keys.sort_by(|a, b| cmp_utf16(a, b));
    keys.dedup();
    keys.into_iter()
        .filter_map(|key| {
            let left = before.iter().find(|item| item.key() == key);
            let right = after.iter().find(|item| item.key() == key);
            if left == right {
                return None;
            }
            right
                .or(left)
                .map(|sample| format!("{label}:{}", sample.artifact))
        })
        .collect()
}

/// `contentOnlyChanges`: the changes that are only an edit to a document
/// that is still there (same producer, instances and presence; other
/// content), unless another kind of change names the same artifact.
fn content_only_changes(before: &Basis, after: &Basis) -> Vec<String> {
    let mut only: Vec<String> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for (label, left, right) in [
        ("input", &before.inputs, &after.inputs),
        ("output", &before.outputs, &after.outputs),
    ] {
        let mut keys: Vec<String> = left.iter().chain(right).map(ArtifactBasis::key).collect();
        keys.dedup();
        let mut seen: Vec<String> = Vec::new();
        for key in keys {
            if seen.contains(&key) {
                continue;
            }
            seen.push(key.clone());
            let was = left.iter().find(|item| item.key() == key);
            let now = right.iter().find(|item| item.key() == key);
            if was == now {
                continue;
            }
            let change = format!(
                "{label}:{}",
                now.or(was).map_or("", |item| item.artifact.as_str())
            );
            let edit_only = match (was, now) {
                (Some(was), Some(now)) => {
                    let mut was_blank = was.clone();
                    was_blank.content_hash.clear();
                    let mut now_blank = now.clone();
                    now_blank.content_hash.clear();
                    was_blank == now_blank
                }
                _ => false,
            };
            if edit_only {
                only.push(change);
            } else {
                other.push(change);
            }
        }
    }
    only.retain(|change| !other.contains(change));
    only
}

/// `basisInCurrentForm`.
fn basis_in_current_form(basis: &Basis) -> Basis {
    let current = |items: &[ArtifactBasis]| -> Vec<ArtifactBasis> {
        items
            .iter()
            .map(|item| ArtifactBasis {
                content_hash: current_fingerprint_form(&item.content_hash),
                ..item.clone()
            })
            .collect()
    };
    Basis {
        graph_contract: basis.graph_contract.clone(),
        project_type: basis.project_type.clone(),
        inputs: current(&basis.inputs),
        outputs: current(&basis.outputs),
    }
}

/// `diffStageValidationBasis`.
#[must_use]
pub fn diff_basis(before: &Basis, after: &Basis) -> Vec<String> {
    let mut changes = Vec::new();
    if before.graph_contract != after.graph_contract {
        changes.push("graph-contract".to_owned());
    }
    if before.project_type != after.project_type {
        changes.push("project-type".to_owned());
    }
    changes.extend(basis_changes("input", &before.inputs, &after.inputs));
    changes.extend(basis_changes("output", &before.outputs, &after.outputs));
    changes
}

/// `observedDependencyEdges`: producer → (consumer, artifact), from the
/// inputs the receipts recorded.
fn observed_edges(bases: &[(String, Basis)]) -> Vec<(String, Vec<(String, String)>)> {
    let mut edges: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (consumer, basis) in bases {
        for input in &basis.inputs {
            let key = format!("{}\0{consumer}\0{}", input.producer, input.artifact);
            if seen.contains(&key) {
                continue;
            }
            seen.push(key);
            let edge = (consumer.clone(), input.artifact.clone());
            match edges
                .iter_mut()
                .find(|(producer, _)| *producer == input.producer)
            {
                Some(slot) => slot.1.push(edge),
                None => edges.push((input.producer.clone(), vec![edge])),
            }
        }
    }
    for (_, outgoing) in &mut edges {
        outgoing.sort_by(|a, b| {
            locale_compare(&format!("{}\0{}", a.0, a.1), &format!("{}\0{}", b.0, b.1))
        });
    }
    edges
}

/// `StageValidityIssue`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub stage: String,
    pub direct: bool,
    pub reasons: Vec<String>,
    pub roots: Vec<String>,
    /// Only the stage's own documents were edited after it finished.
    pub edited: bool,
}

/// `propagateStageInvalidation`: the stale roots and the completed stages
/// that depend on them through the dependencies the receipts observed.
#[must_use]
pub fn propagate(
    stages: &[Stage],
    completed: &[String],
    direct_reasons: &[(String, Vec<String>)],
    bases: &[(String, Basis)],
) -> Vec<Issue> {
    struct Found {
        direct: bool,
        reasons: Vec<String>,
        roots: Vec<String>,
    }
    let edges = observed_edges(bases);
    let mut issues: Vec<(String, Found)> = Vec::new();
    let mut queue: Vec<(String, String)> = Vec::new();
    let mut visited: Vec<String> = Vec::new();
    let push_unique = |list: &mut Vec<String>, item: String| {
        if !list.contains(&item) {
            list.push(item);
        }
    };
    for stage in stages {
        let Some((_, reasons)) = direct_reasons.iter().find(|(slug, _)| *slug == stage.slug) else {
            continue;
        };
        if completed.contains(&stage.slug) {
            let mut unique = Vec::new();
            for reason in reasons {
                push_unique(&mut unique, reason.clone());
            }
            issues.push((
                stage.slug.clone(),
                Found {
                    direct: true,
                    reasons: unique,
                    roots: vec![stage.slug.clone()],
                },
            ));
        }
        queue.push((stage.slug.clone(), stage.slug.clone()));
    }
    let mut index = 0;
    while index < queue.len() {
        let (current, root) = queue[index].clone();
        index += 1;
        let visit_key = format!("{current}\0{root}");
        if visited.contains(&visit_key) {
            continue;
        }
        visited.push(visit_key);
        let outgoing = edges
            .iter()
            .find(|(producer, _)| *producer == current)
            .map(|(_, outgoing)| outgoing.clone())
            .unwrap_or_default();
        for (to, artifact) in outgoing {
            if !stages.iter().any(|stage| stage.slug == to) {
                continue;
            }
            let reason = format!("depends on stale stage \"{current}\" via artifact:{artifact}");
            if completed.contains(&to) {
                match issues.iter_mut().find(|(slug, _)| *slug == to) {
                    Some((_, existing)) => {
                        push_unique(&mut existing.roots, root.clone());
                        if !existing.direct {
                            push_unique(&mut existing.reasons, reason);
                        }
                    }
                    None => issues.push((
                        to.clone(),
                        Found {
                            direct: false,
                            reasons: vec![reason],
                            roots: vec![root.clone()],
                        },
                    )),
                }
            }
            queue.push((to, root.clone()));
        }
    }
    stages
        .iter()
        .filter_map(|stage| {
            let (_, found) = issues.iter().find(|(slug, _)| *slug == stage.slug)?;
            let mut reasons = found.reasons.clone();
            reasons.sort_by(|a, b| cmp_utf16(a, b));
            let mut roots = found.roots.clone();
            roots.sort_by(|a, b| cmp_utf16(a, b));
            Some(Issue {
                stage: stage.slug.clone(),
                direct: found.direct,
                reasons,
                roots,
                edited: false,
            })
        })
        .collect()
}

/// `StageValidityInspection`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inspection {
    pub issues: Vec<Issue>,
    pub untracked: Vec<String>,
    pub warnings: Vec<String>,
}

/// `workDependsOnProjectType`: a stage that builds, or whose inputs follow
/// the project type (a conditional consume).
fn work_depends_on_project_type(stage: &Stage) -> bool {
    stage.phase == "construction"
        || stage
            .raw
            .get("consumes")
            .and_then(Value::as_array)
            .is_some_and(|consumes| {
                consumes
                    .iter()
                    .any(|consume| consume.get("conditional_on").is_some())
            })
}

/// `inspectStageValidity`: the completed stages whose recorded receipt no
/// longer matches the tree, and the completed stages that depend on them.
/// `accept_content_changes` leaves an edit to a document's content alone.
#[must_use]
pub fn inspect(wf: &Workflow, state: &str, accept_content_changes: bool) -> Inspection {
    let stages = &wf.install.graph;
    let completed: Vec<String> = parse_checkboxes(state)
        .into_iter()
        .filter(|line| line.state == CheckboxState::Completed)
        .map(|line| line.slug)
        .collect();
    let receipts = completion_receipts(&wf.audit_text());
    let mut direct_reasons: Vec<(String, Vec<String>)> = Vec::new();
    let mut edits: Vec<(String, Vec<String>)> = Vec::new();
    let mut warnings = Vec::new();
    let mut unavailable: Vec<String> = Vec::new();
    for (slug, previous) in &receipts.latest_known {
        let Some(stage) = stages.iter().find(|stage| stage.slug == *slug) else {
            continue;
        };
        let current = match capture(wf, stage, state) {
            Ok(text) => match parse_basis(Some(&text)) {
                Some(current) => current,
                None => continue,
            },
            Err(err) => {
                unavailable.push(slug.clone());
                let message = trim(&focus_js::re::replace(
                    "[\r\n\u{2028}\u{2029}]+",
                    "g",
                    err.message(),
                    " ",
                ))
                .to_owned();
                warnings.push(format!(
                    "Validity inspection unavailable for stage \"{slug}\": {message}"
                ));
                continue;
            }
        };
        let recorded = basis_in_current_form(previous);
        let edited = content_only_changes(&recorded, &current);
        let content_only: Vec<String> = if accept_content_changes {
            edited.clone()
        } else {
            Vec::new()
        };
        let changes: Vec<String> = diff_basis(&recorded, &current)
            .into_iter()
            .filter(|change| {
                (change != "project-type" || work_depends_on_project_type(stage))
                    && !content_only.contains(change)
            })
            .collect();
        if !changes.is_empty() {
            direct_reasons.push((slug.clone(), changes));
        }
        edits.push((slug.clone(), edited));
    }
    let issues = propagate(stages, &completed, &direct_reasons, &receipts.latest_known)
        .into_iter()
        .map(|issue| {
            let edited = issue.direct
                && issue.reasons.iter().all(|reason| {
                    reason.starts_with("output:")
                        && edits
                            .iter()
                            .find(|(slug, _)| *slug == issue.stage)
                            .is_some_and(|(_, edited)| edited.contains(reason))
                });
            Issue { edited, ..issue }
        })
        .collect();
    let untracked = stages
        .iter()
        .filter(|stage| {
            completed.contains(&stage.slug)
                && (!receipts.current.iter().any(|(slug, _)| *slug == stage.slug)
                    || unavailable.contains(&stage.slug))
        })
        .map(|stage| stage.slug.clone())
        .collect();
    Inspection {
        issues,
        untracked,
        warnings,
    }
}

/// `staleStageLine`.
#[must_use]
pub fn stale_stage_line(name: &str) -> String {
    format!(
        "Something {name} used changed after it finished. I'm carrying on with it as it is. Do you want me to redo {name} with the change?"
    )
}

/// `ownDocumentChangedLine`.
#[must_use]
pub fn own_document_changed_line(name: &str, files: &[String]) -> String {
    let several = files.len() > 1;
    let listed = if several {
        format!(
            "{} and {}",
            files[..files.len() - 1].join(", "),
            files[files.len() - 1]
        )
    } else {
        files.first().cloned().unwrap_or_default()
    };
    format!(
        "{listed} changed after {name} finished. I'm carrying on with {} as {} {}. Do you want me to redo {name} with your {} instead?",
        if several { "them" } else { "it" },
        if several { "they" } else { "it" },
        if several { "are" } else { "is" },
        if several { "changes" } else { "change" }
    )
}

/// `codeArrivedStageLine`.
#[must_use]
pub fn code_arrived_stage_line(name: &str) -> String {
    format!(
        "{name} ran before the code was here. I'm carrying on with it as it is. Do you want me to redo {name} with the code?"
    )
}

/// `staleStageNote`: what the person hears about a finished stage that is
/// behind, and the one way to act on it.
#[must_use]
pub fn stale_stage_note(name: &str, issue: &Issue, state: &str) -> String {
    if issue.reasons.iter().any(|reason| reason == "project-type")
        && project_type(state) == Some("brownfield")
    {
        return code_arrived_stage_line(name);
    }
    if issue.edited && !issue.reasons.is_empty() {
        let files: Vec<String> = issue
            .reasons
            .iter()
            .map(|reason| artifact_filename(reason.strip_prefix("output:").unwrap_or(reason)))
            .collect();
        return own_document_changed_line(name, &files);
    }
    stale_stage_line(name)
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

    #[test]
    fn a_receipt_reads_back_only_in_its_own_shape() {
        let entry = r#"{"artifact":"a","producer":"p","required":true,"instanceCount":1,"presentCount":1,"structureHash":"s","contentHash":"c"}"#;
        let text = |schema: &str, kind: &str, inputs: &str| {
            format!(
                r#"{{"schema":{schema},"graphContract":"g","projectType":{kind},"inputs":[{inputs}],"outputs":[{entry}]}}"#
            )
        };
        let read = parse_basis(Some(&text("3", "null", entry))).unwrap();
        assert_eq!(read.project_type, None);
        assert_eq!(read.inputs[0].artifact, "a");
        assert!(parse_basis(Some(&text("3.0", "\"brownfield\"", ""))).is_some());
        assert!(parse_basis(Some(&text("2", "null", ""))).is_none());
        assert!(parse_basis(Some(&text("3", "\"mixed\"", ""))).is_none());
        assert!(parse_basis(Some(&text("3", "null", "{\"artifact\":\"a\"}"))).is_none());
        assert!(
            parse_basis(Some(&text(
                "3",
                "null",
                &entry.replace("\"presentCount\":1", "\"presentCount\":2")
            )))
            .is_none()
        );
        assert!(
            parse_basis(Some(&text(
                "3",
                "null",
                &entry.replace("\"instanceCount\":1", "\"instanceCount\":1.5")
            )))
            .is_none()
        );
        assert!(parse_basis(Some("[]")).is_none());
        assert!(parse_basis(Some("{nope")).is_none());
        assert!(parse_basis(Some("")).is_none());
        assert!(parse_basis(None).is_none());
        // Entries come back in key order.
        let two = format!(
            "{},{}",
            entry.replace("\"artifact\":\"a\"", "\"artifact\":\"z\""),
            entry
        );
        let sorted = parse_basis(Some(&text("3", "null", &two))).unwrap();
        assert_eq!(
            sorted
                .inputs
                .iter()
                .map(|b| b.artifact.as_str())
                .collect::<Vec<_>>(),
            ["a", "z"]
        );
    }

    fn entry(artifact: &str, producer: &str, content: &str) -> ArtifactBasis {
        ArtifactBasis {
            artifact: artifact.to_owned(),
            producer: producer.to_owned(),
            required: true,
            instance_count: 1,
            present_count: 1,
            structure_hash: "s".to_owned(),
            content_hash: content.to_owned(),
        }
    }

    fn basis(inputs: Vec<ArtifactBasis>, outputs: Vec<ArtifactBasis>) -> Basis {
        Basis {
            graph_contract: "g".to_owned(),
            project_type: None,
            inputs,
            outputs,
        }
    }

    #[test]
    fn changes_are_named_and_an_edit_alone_is_told_apart() {
        let before = basis(
            vec![entry("in", "p", "1")],
            vec![entry("out", "s", "1"), entry("gone", "s", "1")],
        );
        let mut after = basis(
            vec![entry("in", "p", "2")],
            vec![entry("out", "s", "1"), entry("new", "s", "1")],
        );
        after.graph_contract = "h".to_owned();
        after.project_type = Some("greenfield".to_owned());
        assert_eq!(
            diff_basis(&before, &after),
            [
                "graph-contract",
                "project-type",
                "input:in",
                "output:gone",
                "output:new"
            ]
        );
        assert_eq!(content_only_changes(&before, &after), ["input:in"]);
        // A change of shape that also changes the content is not an edit.
        let mut reshaped = after.clone();
        reshaped.inputs[0].instance_count = 2;
        assert!(content_only_changes(&before, &reshaped).is_empty());
        assert!(diff_basis(&before, &before).is_empty());
        // The same artifact from two producers: an edit to one and a reshape of
        // the other name the same change, which is then no edit.
        let twice = basis(vec![entry("x", "p1", "1"), entry("x", "p2", "1")], vec![]);
        let mut twice_after = basis(vec![entry("x", "p1", "2"), entry("x", "p2", "1")], vec![]);
        assert_eq!(content_only_changes(&twice, &twice_after), ["input:x"]);
        twice_after.inputs[1].instance_count = 2;
        assert!(content_only_changes(&twice, &twice_after).is_empty());
    }

    #[test]
    fn staleness_propagates_through_the_dependencies_the_receipts_observed() {
        let graph = focus_graph::runtime::load_from_value(&serde_json::json!([
            {"slug": "a", "phase": "ideation"},
            {"slug": "b", "phase": "ideation"},
            {"slug": "c", "phase": "ideation"},
            {"slug": "d", "phase": "ideation"},
        ]));
        let bases = vec![
            ("b".to_owned(), basis(vec![entry("x", "a", "1")], vec![])),
            (
                "c".to_owned(),
                basis(vec![entry("y", "b", "1"), entry("x", "a", "1")], vec![]),
            ),
            ("d".to_owned(), basis(vec![entry("z", "c", "1")], vec![])),
        ];
        let completed = ["a", "b", "d"].map(str::to_owned);
        let direct = vec![(
            "a".to_owned(),
            vec!["output:x".to_owned(), "output:x".to_owned()],
        )];
        let issues = propagate(&graph, &completed, &direct, &bases);
        let summary: Vec<(&str, bool, &[String], &[String])> = issues
            .iter()
            .map(|i| {
                (
                    i.stage.as_str(),
                    i.direct,
                    i.reasons.as_slice(),
                    i.roots.as_slice(),
                )
            })
            .collect();
        assert_eq!(
            summary[0],
            (
                "a",
                true,
                &["output:x".to_owned()][..],
                &["a".to_owned()][..]
            )
        );
        assert_eq!(summary[1].0, "b");
        assert!(!summary[1].1);
        assert_eq!(
            summary[1].2,
            &["depends on stale stage \"a\" via artifact:x".to_owned()][..]
        );
        // c is not completed: no issue of its own, but the path through it reaches d.
        assert_eq!(summary.len(), 3);
        assert_eq!(summary[2].0, "d");
        assert_eq!(
            summary[2].2,
            &["depends on stale stage \"c\" via artifact:z".to_owned()][..]
        );
        assert_eq!(summary[2].3, &["a".to_owned()][..]);
        // A stage stale on its own keeps its own reasons when a root reaches it too.
        let both = vec![
            ("a".to_owned(), vec!["output:x".to_owned()]),
            ("b".to_owned(), vec!["input:q".to_owned()]),
        ];
        let b = &propagate(&graph, &completed, &both, &bases)[1];
        assert!(b.direct);
        assert_eq!(b.reasons, ["input:q"]);
        assert_eq!(b.roots, ["a", "b"]);
        // A root that is not completed still invalidates what depended on it.
        let open_root = propagate(&graph, &["b".to_owned()], &direct, &bases);
        assert_eq!(open_root.len(), 1);
        assert_eq!(open_root[0].stage, "b");
        assert!(propagate(&graph, &completed, &[], &bases).is_empty());
    }

    /// An installed project with the first stage finished and its receipt on
    /// the ledger, opened as a workflow.
    fn finished(name: &str) -> (Workflow, std::path::PathBuf) {
        use focus_harness::Harness;
        use focus_workspace::Layout;
        use focus_workspace::records::{Invoker, Selection};
        fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_dir(&entry.path(), &target);
                } else {
                    std::fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        let root =
            std::env::temp_dir().join(format!("focus-validity-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/upstream/installed/claude");
        copy_dir(&fixtures, &root);
        let record = root.join("focus/spaces/default/intents/260105-add-login");
        std::fs::create_dir_all(record.join("audit")).unwrap();
        std::fs::create_dir_all(record.join("ideation/intent-capture")).unwrap();
        std::fs::write(
            record.join("ideation/intent-capture/intent-statement.md"),
            "# Intent\r\nline\r\n",
        )
        .unwrap();
        std::fs::write(
            record.join("focus-state.md"),
            "- **Scope**: feature\n- **Project Type**: Greenfield\n- [x] intent-capture \u{2014} EXECUTE\n- [x] market-research \u{2014} EXECUTE\n",
        )
        .unwrap();
        std::fs::write(root.join("focus/.focus-clone-id"), "abc123\nlaptop\n").unwrap();
        let layout = Layout::new(root.clone(), Harness::Claude);
        let selection = Selection {
            space: "default".to_owned(),
            intent: Some("260105-add-login".to_owned()),
            session_id: None,
            binding: None,
        };
        let wf = Workflow::open(layout, selection, Invoker::default()).unwrap();
        let state = wf.read_state().unwrap();
        let stage = wf.stage("intent-capture").unwrap();
        let receipt = capture(&wf, stage, &state).unwrap();
        std::fs::write(
            record.join("audit/laptop-abc123.md"),
            format!("# Log\n\n## Stage Completion\n**Timestamp**: 2026-01-05T10:01:00Z\n**Event**: STAGE_COMPLETED\n**Stage**: intent-capture\n**Validation Basis**: {receipt}\n\n---\n"),
        )
        .unwrap();
        (wf, root)
    }

    #[test]
    fn a_document_edited_after_its_stage_finished_is_said_as_such() {
        let (wf, root) = finished("edited");
        let state = wf.read_state().unwrap();
        let record = root.join("focus/spaces/default/intents/260105-add-login");
        let statement = record.join("ideation/intent-capture/intent-statement.md");
        assert!(inspect(&wf, &state, false).issues.is_empty());
        // The same text with other line endings is the same document.
        std::fs::write(&statement, "# Intent\nline\n").unwrap();
        assert!(inspect(&wf, &state, false).issues.is_empty());
        // Other words: an edit, which a policy that accepts changes leaves alone.
        std::fs::write(&statement, "# Intent\nmore\n").unwrap();
        let inspection = inspect(&wf, &state, false);
        assert_eq!(inspection.issues.len(), 1);
        let issue = &inspection.issues[0];
        assert!(issue.direct && issue.edited);
        assert_eq!(issue.reasons, ["output:intent-statement"]);
        assert_eq!(
            stale_stage_note("Intent Capture", issue, &state),
            "intent-statement.md changed after Intent Capture finished. I'm carrying on with it as it is. Do you want me to redo Intent Capture with your change instead?"
        );
        assert!(inspect(&wf, &state, true).issues.is_empty());
        assert_eq!(inspection.untracked, ["market-research"]);
        // The stage started again: its receipt is no longer its current attempt's.
        let shard = record.join("audit/laptop-abc123.md");
        let text = std::fs::read_to_string(&shard).unwrap();
        std::fs::write(&shard, format!("{text}\n## Stage Start\n**Timestamp**: 2026-01-05T10:02:00Z\n**Event**: STAGE_STARTED\n**Stage**: intent-capture\n\n---\n")).unwrap();
        assert_eq!(
            inspect(&wf, &state, false).untracked,
            ["intent-capture", "market-research"]
        );
        std::fs::write(&shard, &text).unwrap();
        // The document gone: not an edit.
        std::fs::remove_file(&statement).unwrap();
        let gone = inspect(&wf, &state, false);
        assert!(gone.issues[0].direct && !gone.issues[0].edited);
        assert_eq!(
            stale_stage_note("Intent Capture", &gone.issues[0], &state),
            stale_stage_line("Intent Capture")
        );
        assert!(!inspect(&wf, &state, true).issues.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_receipt_taken_over_raw_line_endings_reads_as_the_same_document() {
        let (wf, root) = finished("raw");
        let state = wf.read_state().unwrap();
        let record = root.join("focus/spaces/default/intents/260105-add-login");
        let shard = record.join("audit/laptop-abc123.md");
        // The receipt as an older release wrote it: the content hashed over the raw CRLF bytes.
        let raw_entry = r#"[{"path":"focus/spaces/default/intents/260105-add-login/ideation/intent-capture/intent-statement.md","present":true,"sha256":"sha256:HASH"}]"#
            .replace("HASH", &sha256_hex(b"# Intent\r\nline\r\n"));
        let raw_hash = format!("sha256:{}", sha256_hex(raw_entry.as_bytes()));
        let text = std::fs::read_to_string(&shard).unwrap();
        let receipt = parse_basis(Some(
            text.split("**Validation Basis**: ")
                .nth(1)
                .unwrap()
                .lines()
                .next()
                .unwrap(),
        ))
        .unwrap();
        let current_hash = receipt
            .outputs
            .iter()
            .find(|o| o.artifact == "intent-statement")
            .unwrap()
            .content_hash
            .clone();
        assert_ne!(current_hash, raw_hash);
        std::fs::write(&shard, text.replace(&current_hash, &raw_hash)).unwrap();
        assert!(inspect(&wf, &state, false).issues.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_lines_a_person_hears_name_the_documents() {
        assert_eq!(
            own_document_changed_line(
                "X",
                &["a.md".to_owned(), "b.md".to_owned(), "c.md".to_owned()]
            ),
            "a.md, b.md and c.md changed after X finished. I'm carrying on with them as they are. Do you want me to redo X with your changes instead?"
        );
        assert_eq!(
            code_arrived_stage_line("X"),
            "X ran before the code was here. I'm carrying on with it as it is. Do you want me to redo X with the code?"
        );
        let issue = Issue {
            stage: "x".to_owned(),
            direct: true,
            reasons: vec!["project-type".to_owned()],
            roots: vec!["x".to_owned()],
            edited: false,
        };
        assert_eq!(
            stale_stage_note("X", &issue, "- **Project Type**: Brownfield\n"),
            code_arrived_stage_line("X")
        );
        assert_eq!(
            stale_stage_note("X", &issue, "- **Project Type**: Greenfield\n"),
            stale_stage_line("X")
        );
    }
}

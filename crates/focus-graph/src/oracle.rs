//! The oracle protocol.
//!
//! Parity with upstream is established by running the same inputs through both
//! implementations and comparing their answers as text. A case is one JSON
//! object naming a `kind` and its inputs; [`run_case`] answers it in the same
//! shape `tools/oracle/driver.ts` produces from the TypeScript original.
//!
//! The corpus under `fixtures/oracle/` holds cases with the answers upstream
//! gave, so the comparison runs in `cargo test` with no Bun installed;
//! `tools/oracle/refresh.sh` regenerates it from an upstream checkout.

use focus_frontmatter as fm;
use focus_js::num;
use focus_js::order::locale_compare;
use serde_json::{Map, Value, json};

use crate::composed;
use crate::guard_memory;
use crate::proposal;
use crate::query;
use crate::rules;
use crate::runtime;
use crate::sensors;
use crate::stage_schema::{self, ValidationContext};

/// The kinds of case this crate answers. Each kind in the corpus belongs to
/// exactly one crate, and each crate replays its own.
pub const KINDS: [&str; 13] = [
    "stage",
    "sensor",
    "rule",
    "scalar",
    "list",
    "collate",
    "num",
    "cycles",
    "record",
    "foldback",
    "settings",
    "guard",
    "guardwild",
];

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    match text("kind") {
        "stage" => stage(case, text("raw")),
        "sensor" => sensor(text("raw"), text("filename_id")),
        "rule" => rule(text("raw")),
        "scalar" => json!({ "value": fm::scalar_field(text("fm"), text("key")) }),
        "list" => json!({ "value": fm::list_field(text("fm"), text("key")) }),
        "collate" => json!({ "sign": locale_compare(text("a"), text("b")) as i8 }),
        "num" => number(
            text("text"),
            case.get("digits").and_then(Value::as_u64).unwrap_or(0),
        ),
        "cycles" => cycles(case),
        "record" => record(text("body")),
        "foldback" => foldback(case),
        "settings" => {
            let (settings, errors) =
                proposal::validate_scope_settings(case.get("raw").unwrap_or(&Value::Null));
            json!({ "settings": settings, "errors": errors })
        }
        "guard" | "guardwild" => match guard_memory::declared_mode(text("content")) {
            Ok(Some((heading, raw))) => json!({ "heading": heading, "mode": raw }),
            Ok(None) => json!({ "heading": null, "mode": null }),
            Err(reason) => json!({ "refused": reason }),
        },
        other => json!({ "unsupported": other }),
    }
}

fn number(text: &str, digits: u64) -> Value {
    let x = num::from_str(text);
    let fixed = if x.is_finite() {
        Value::from(num::to_fixed(x, usize::try_from(digits).unwrap_or(0)))
    } else {
        Value::Null
    };
    let mut wrapped = Map::new();
    wrapped.insert("x".to_owned(), Value::from(x));
    json!({
        "string": num::to_string(x),
        "fixed": fixed,
        "round": num::to_string(num::round(x)),
        "json": focus_js::json::stringify(&Value::Object(wrapped)),
    })
}

fn cycles(case: &Value) -> Value {
    let stages = runtime::load_from_value(case.get("stages").unwrap_or(&Value::Null));
    let refs: Vec<&runtime::Stage> = stages.iter().collect();
    let topo = match query::topo_sort(&refs) {
        Ok(order) => json!(order),
        Err(err) => json!({ "error": err.message() }),
    };
    json!({ "cycles": query::find_cycles(&refs), "topo": topo })
}

fn record(body: &str) -> Value {
    match composed::parse_record(body, "FILE") {
        // Text the JSON parser wrote is the runtime's, not the engine's: both
        // sides keep the engine's sentence and replace what follows it.
        Err(err) => {
            let marker = "has an unparseable grid fence: ";
            let message = match err.message().find(marker) {
                Some(at) => format!("{}<runtime>", &err.message()[..at + marker.len()]),
                None => err.message().to_owned(),
            };
            json!({ "error": message })
        }
        Ok(parsed) => {
            let rendered = match composed::render_record(&parsed.identity, &parsed.stages, None) {
                Ok(text) => json!(text),
                Err(err) => json!({ "error": err.message() }),
            };
            json!({
                "name": parsed.name,
                "identity": parsed.identity,
                "stages": parsed.stages,
                "rendered": rendered,
            })
        }
    }
}

fn foldback(case: &Value) -> Value {
    let names = |key: &str| -> Vec<String> {
        case.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let records: Vec<composed::Record> = case
        .get("records")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, stages)| composed::Record {
            name: name.clone(),
            identity: String::new(),
            stages: stages.as_object().cloned().unwrap_or_default(),
        })
        .collect();
    let (stock, installed) = (names("stock"), names("installed"));
    let stock: Vec<&str> = stock.iter().map(String::as_str).collect();
    let installed: Vec<&str> = installed.iter().map(String::as_str).collect();
    let folded = composed::fold_back(
        &records,
        case.get("on_disk").and_then(Value::as_str),
        &stock,
        &installed,
    );
    let fresh = case
        .get("fresh")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let merged = composed::merge(focus_js::order::js_key_order(fresh), &folded);
    json!({
        "json": focus_js::json::stringify(&Value::Object(folded.grid.clone())),
        "names": folded.names,
        "recordNames": folded.record_names,
        "gridOnlyNames": folded.grid_only_names,
        "merged": focus_js::json::stringify(&Value::Object(merged)),
    })
}

fn stage(case: &Value, raw: &str) -> Value {
    let agents: Option<Vec<String>> = case.get("agents").and_then(Value::as_array).map(|list| {
        list.iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    });
    let parsed = match stage_schema::parse(raw) {
        Ok(parsed) => Value::Object(parsed),
        Err(err) => return json!({ "error": err.message() }),
    };
    let errors = stage_schema::validate(
        &parsed,
        ValidationContext {
            agents: agents.as_deref(),
        },
    )
    .err()
    .unwrap_or_default();
    json!({ "parsed": parsed, "valid": errors.is_empty(), "errors": errors })
}

fn sensor(raw: &str, filename_id: &str) -> Value {
    let manifest = match sensors::parse_manifest(raw) {
        Ok(manifest) => manifest,
        Err(err) => return json!({ "error": err.message() }),
    };
    // Keys in the order upstream's parser assigns them.
    let mut out = Map::new();
    let mut put = |key: &str, value: &Option<String>| {
        if let Some(value) = value {
            out.insert(key.to_owned(), Value::String(value.clone()));
        }
    };
    put("id", &manifest.id);
    put("kind", &manifest.kind);
    put("command", &manifest.command);
    put("default_severity", &manifest.default_severity);
    put("description", &manifest.description);
    put("category", &manifest.category);
    put("fire_on", &Some(manifest.fire_on.clone()));
    put("matches", &manifest.matches);
    if let Some(timeout) = manifest.timeout_seconds {
        out.insert("timeout_seconds".to_owned(), Value::Number(timeout.into()));
    }
    let invalid = sensors::validate_manifest(&manifest, "FILE", filename_id).err();
    json!({ "manifest": out, "invalid": invalid.map(|err| err.message().to_owned()) })
}

fn rule(raw: &str) -> Value {
    let frontmatter = rules::parse_frontmatter(raw);
    let mut out = Map::new();
    let fields = [
        ("pairing", &frontmatter.pairing),
        ("status", &frontmatter.status),
        ("stale_after", &frontmatter.stale_after),
    ];
    for (key, value) in fields {
        if let Some(value) = value {
            out.insert(key.to_owned(), Value::String(value.clone()));
        }
    }
    let invalid = rules::validate_frontmatter(&frontmatter, "FILE").err();
    json!({ "frontmatter": out, "invalid": invalid.map(|err| err.message().to_owned()) })
}

//! The designer export: the graph, the scopes, the artifacts and the agents
//! in one JSON document.
//!
//! A data snapshot for a visual workflow designer, not a versioned contract:
//! it reshapes whenever its inputs do. Mirrors `exportBundle` and
//! `canonicalExportJson` in `aidlc-graph.ts` (L2061).

use focus_js::json;
use serde_json::{Map, Value};

use crate::agents::AgentMetadata;
use crate::compile::FIELD_ORDER;
use crate::query::artifacts;
use crate::runtime::{ScopeDefinition, Stage};

/// The export's bytes. Stage keys follow [`FIELD_ORDER`], whatever order the
/// compiled file had them in; scopes keep the order given; artifacts are
/// sorted; agents come sorted by slug from their loader.
#[must_use]
pub fn export_json(
    graph: &[Stage],
    scopes: &[ScopeDefinition],
    agents: &[AgentMetadata],
) -> String {
    let stages: Vec<Value> = graph
        .iter()
        .map(|stage| {
            let ordered: Map<String, Value> = FIELD_ORDER
                .iter()
                .filter_map(|key| stage.raw.get(*key).map(|v| ((*key).to_owned(), v.clone())))
                .collect();
            Value::Object(ordered)
        })
        .collect();
    let scopes: Map<String, Value> = scopes
        .iter()
        .map(|scope| (scope.meta.name.clone(), scope.to_json()))
        .collect();
    let agents: Vec<Value> = agents
        .iter()
        .map(|agent| {
            let mut out = Map::new();
            out.insert("slug".to_owned(), Value::from(agent.slug.clone()));
            out.insert(
                "display_name".to_owned(),
                Value::from(agent.display_name.clone()),
            );
            out.insert(
                "examples".to_owned(),
                Value::Array(agent.examples.iter().cloned().map(Value::String).collect()),
            );
            Value::Object(out)
        })
        .collect();

    let mut bundle = Map::new();
    bundle.insert("stages".to_owned(), Value::Array(stages));
    bundle.insert(
        "scopes".to_owned(),
        Value::Object(focus_js::order::js_key_order(scopes)),
    );
    bundle.insert(
        "artifacts".to_owned(),
        Value::Array(artifacts(graph).into_iter().map(Value::from).collect()),
    );
    bundle.insert("agents".to_owned(), Value::Array(agents));
    format!("{}\n", json::stringify_pretty(&Value::Object(bundle)))
}

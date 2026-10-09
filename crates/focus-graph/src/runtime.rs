//! The compiled files as the engine reads them at runtime.
//!
//! At runtime nothing walks the authored tree: `stage-graph.json` and
//! `scope-grid.json` are the only structural truth, joined with the scope
//! identity files for each scope's defaults. Mirrors the loaders in
//! `aidlc-lib.ts` (`loadStageGraphAll`, `loadStageGraph`, `loadScopeMapping`,
//! `validScopes`) and `loadScopeGrid` in `aidlc-graph.ts`.

use std::path::Path;

use focus_js::order::cmp_utf16;
use focus_js::{EngineError, Result, bail, json};
use focus_workspace::names::CORE_PLUGIN;
use serde_json::{Map, Value};

use crate::scopes::{CEREMONY_KEYS, GuardPolicy, ScopeMetadata, is_scope_name};

/// One `consumes:` entry of a compiled stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consume {
    pub artifact: String,
    pub required: bool,
    /// `brownfield` or `greenfield` when the input only applies to one kind
    /// of project.
    pub conditional_on: Option<String>,
}

/// A stage as compiled. The typed fields are the ones the engine decides on;
/// `raw` is the whole node, for output that passes it through.
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub slug: String,
    pub number: String,
    pub phase: String,
    pub plugin: Option<String>,
    /// `false` when the install's plugin selection disabled this stage.
    pub enabled: bool,
    pub requires_stage: Vec<String>,
    pub produces: Vec<String>,
    pub optional_produces: Vec<String>,
    pub consumes: Vec<Consume>,
    /// The scopes this stage declares itself part of.
    pub scopes: Vec<String>,
    pub for_each: Option<String>,
    /// The stage's own screening prior, when it carries one.
    pub ars: Option<Value>,
    pub raw: Map<String, Value>,
}

impl Stage {
    /// The plugin that owns this stage; core when the node names none.
    #[must_use]
    pub fn plugin_owner(&self) -> &str {
        self.plugin.as_deref().unwrap_or(CORE_PLUGIN)
    }

    /// Whether this stage produces `artifact`, required or optional.
    #[must_use]
    pub fn produces_artifact(&self, artifact: &str) -> bool {
        self.produces
            .iter()
            .chain(&self.optional_produces)
            .any(|name| name == artifact)
    }

    fn from_json(row: &Value) -> Self {
        let text = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_owned);
        let strings = |key: &str| -> Vec<String> {
            row.get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        };
        Self {
            slug: text("slug").unwrap_or_default(),
            number: text("number").unwrap_or_default(),
            phase: text("phase").unwrap_or_default(),
            plugin: text("plugin"),
            enabled: row.get("enabled") != Some(&Value::Bool(false)),
            requires_stage: strings("requires_stage"),
            produces: strings("produces"),
            optional_produces: strings("optional_produces"),
            consumes: row
                .get("consumes")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|c| Consume {
                    artifact: c
                        .get("artifact")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    required: c.get("required").is_some_and(truthy),
                    conditional_on: c
                        .get("conditional_on")
                        .and_then(Value::as_str)
                        .filter(|on| !on.is_empty())
                        .map(str::to_owned),
                })
                .collect(),
            scopes: strings("scopes"),
            for_each: text("for_each"),
            ars: row.get("ars").cloned(),
            raw: row.as_object().cloned().unwrap_or_default(),
        }
    }
}

/// JS truthiness of a JSON value.
#[must_use]
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0 && !x.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Every stage in the compiled graph, including those a plugin selection
/// disabled, in file order.
///
/// # Errors
///
/// The file is missing, is not JSON, or is not a list.
pub fn load_stages_all(path: &Path) -> Result<Vec<Stage>> {
    let shown = path.display();
    let bytes = std::fs::read(path).map_err(|err| {
        EngineError::new(format!(
            "Stage graph not readable at {shown}: {err}. Reinstall the framework or re-run setup to restore the data file."
        ))
    })?;
    let parsed = json::parse(&String::from_utf8_lossy(&bytes)).map_err(|err| {
        EngineError::new(format!("Stage graph at {shown} is not valid JSON: {err}"))
    })?;
    let Value::Array(rows) = parsed else {
        bail!("Stage graph at {shown} is not a list of stages");
    };
    Ok(rows.iter().map(Stage::from_json).collect())
}

/// Stages from an already parsed graph. Rows that are not objects read as
/// empty stages, as they would to upstream's untyped reads.
#[must_use]
pub fn load_from_value(parsed: &Value) -> Vec<Stage> {
    parsed
        .as_array()
        .into_iter()
        .flatten()
        .map(Stage::from_json)
        .collect()
}

/// The runtime graph: the stages the plugin selection left enabled.
#[must_use]
pub fn enabled(all: &[Stage]) -> Vec<Stage> {
    all.iter().filter(|stage| stage.enabled).cloned().collect()
}

/// The EXECUTE/SKIP grid derived from stage `scopes:` lists, for when no
/// compiled grid can be read. `initialization_always` applies the compile's
/// rule that initialization stages run under every scope.
fn transpose(graph: &[Stage], initialization_always: bool) -> Map<String, Value> {
    let mut names: Vec<&str> = Vec::new();
    for name in graph.iter().flat_map(|stage| &stage.scopes) {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    names.sort_by(|a, b| cmp_utf16(a, b));
    let mut grid = Map::new();
    for scope in names {
        let cells: Map<String, Value> = graph
            .iter()
            .map(|stage| {
                let runs = (initialization_always && stage.phase == "initialization")
                    || stage.scopes.iter().any(|name| name == scope);
                (
                    stage.slug.clone(),
                    Value::String(if runs { "EXECUTE" } else { "SKIP" }.to_owned()),
                )
            })
            .collect();
        let mut entry = Map::new();
        entry.insert("stages".to_owned(), Value::Object(cells));
        grid.insert(scope.to_owned(), Value::Object(entry));
    }
    focus_js::order::js_key_order(grid)
}

fn read_grid(path: &Path) -> Option<Map<String, Value>> {
    let bytes = std::fs::read(path).ok()?;
    match json::parse(&String::from_utf8_lossy(&bytes)).ok()? {
        Value::Object(grid) => Some(grid),
        // Valid JSON of another shape has no columns.
        _ => Some(Map::new()),
    }
}

/// Whether the compiled grid at `path` can be read. When it cannot, the grid
/// is derived from the stage graph, so the graph has to load first.
#[must_use]
pub fn grid_readable(path: &Path) -> bool {
    read_grid(path).is_some()
}

/// The compiled scope grid: scope name to `{ "stages": { slug: action } }`.
/// When the file cannot be read it is derived from `graph`, so a caller never
/// fails on an artifact that can be recomputed.
#[must_use]
pub fn load_scope_grid(path: &Path, graph: &[Stage]) -> Map<String, Value> {
    read_grid(path).unwrap_or_else(|| transpose(graph, true))
}

/// The grid as `loadScopeMapping` reads it. It differs from
/// [`load_scope_grid`] only in its fallback: upstream has two derivations,
/// and this one does not force initialization stages to EXECUTE.
#[must_use]
pub fn load_scope_grid_for_mapping(path: &Path, graph: &[Stage]) -> Map<String, Value> {
    read_grid(path).unwrap_or_else(|| transpose(graph, false))
}

/// A scope's column: stage slug to action. Empty when the scope has no
/// column, which is a valid scope that runs nothing yet.
#[must_use]
pub fn column<'a>(grid: &'a Map<String, Value>, scope: &str) -> Option<&'a Map<String, Value>> {
    grid.get(scope)?.get("stages")?.as_object()
}

/// A scope as the runtime sees it: its identity file's settings joined with
/// its grid column.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeDefinition {
    pub meta: ScopeMetadata,
    pub stages: Map<String, Value>,
}

impl ScopeDefinition {
    /// The scope's Guard Policy default: what its file declares, else off.
    #[must_use]
    pub fn guard_policy(&self) -> GuardPolicy {
        self.meta.guard_policy.unwrap_or(GuardPolicy::Off)
    }

    /// The definition as the designer export prints it.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let meta = &self.meta;
        let mut out = Map::new();
        out.insert("depth".to_owned(), Value::from(meta.depth.clone()));
        out.insert("stages".to_owned(), Value::Object(self.stages.clone()));
        out.insert(
            "keywords".to_owned(),
            Value::Array(meta.keywords.iter().cloned().map(Value::String).collect()),
        );
        out.insert(
            "description".to_owned(),
            Value::from(meta.description.clone()),
        );
        if let Some(strategy) = &meta.test_strategy {
            out.insert("testStrategy".to_owned(), Value::from(strategy.clone()));
        }
        if let Some(plugin) = &meta.plugin {
            out.insert("plugin".to_owned(), Value::from(plugin.clone()));
        }
        if let Some(runner) = meta.runner {
            out.insert("runner".to_owned(), Value::Bool(runner));
        }
        out.insert("skeleton".to_owned(), Value::Bool(meta.skeleton));
        if meta.existing_code {
            out.insert("existingCode".to_owned(), Value::Bool(true));
        }
        if let Some(policy) = meta.guard_policy {
            out.insert("guardPolicy".to_owned(), Value::from(policy.as_str()));
        }
        if !meta.ceremony.is_empty() {
            let ceremony: Map<String, Value> = CEREMONY_KEYS
                .iter()
                .filter_map(|key| {
                    meta.ceremony
                        .iter()
                        .find(|(k, _)| k == key)
                        .map(|(k, v)| ((*k).to_owned(), Value::from(v.clone())))
                })
                .collect();
            out.insert("ceremony".to_owned(), Value::Object(ceremony));
        }
        Value::Object(out)
    }
}

/// Every enabled scope with its grid column, in the scopes' own order. The
/// scope set is the identity files present; the grid only supplies columns.
#[must_use]
pub fn scope_mapping(
    enabled_scopes: &[ScopeMetadata],
    grid: &Map<String, Value>,
) -> Vec<ScopeDefinition> {
    enabled_scopes
        .iter()
        .map(|meta| ScopeDefinition {
            meta: meta.clone(),
            stages: column(grid, &meta.name).cloned().unwrap_or_default(),
        })
        .collect()
}

/// The scope names a command accepts, sorted. Validity comes from the scope
/// identity files, not from the grid: a scope with a file and no column is
/// valid and runs nothing.
#[must_use]
pub fn valid_scopes(enabled_scopes: &[ScopeMetadata]) -> Vec<String> {
    let mut names: Vec<String> = enabled_scopes
        .iter()
        .map(|scope| scope.name.clone())
        .filter(|name| is_scope_name(name))
        .collect();
    names.sort_by(|a, b| cmp_utf16(a, b));
    names
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_stage_reads_leniently_and_keeps_its_node() {
        let row = json!({
            "slug": "build", "number": "3.1", "phase": "construction",
            "enabled": false,
            "produces": ["code"], "optional_produces": ["notes"],
            "consumes": [
                {"artifact": "plan", "required": true, "conditional_on": "brownfield"},
                {"artifact": "hints", "required": false},
            ],
            "requires_stage": ["plan-stage"],
            "scopes": ["feature"],
        });
        let stage = Stage::from_json(&row);
        assert!(!stage.enabled);
        assert_eq!(stage.plugin_owner(), "focus");
        assert!(stage.produces_artifact("notes") && !stage.produces_artifact("plan"));
        assert_eq!(
            stage.consumes[0].conditional_on.as_deref(),
            Some("brownfield")
        );
        assert!(!stage.consumes[1].required);
        assert_eq!(stage.raw, *row.as_object().unwrap());
    }

    #[test]
    fn the_two_grid_fallbacks_differ_on_initialization() {
        let graph: Vec<Stage> = [
            json!({"slug": "init", "number": "0.1", "phase": "initialization", "scopes": []}),
            json!({"slug": "build", "number": "3.1", "phase": "construction", "scopes": ["feature"]}),
        ]
        .iter()
        .map(Stage::from_json)
        .collect();
        let missing = Path::new("/nonexistent/scope-grid.json");
        let for_graph = load_scope_grid(missing, &graph);
        let for_mapping = load_scope_grid_for_mapping(missing, &graph);
        assert_eq!(for_graph["feature"]["stages"]["init"], "EXECUTE");
        assert_eq!(for_mapping["feature"]["stages"]["init"], "SKIP");
        assert_eq!(for_mapping["feature"]["stages"]["build"], "EXECUTE");
    }
}

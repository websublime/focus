//! The compile: authored Markdown in, `stage-graph.json` and `scope-grid.json`
//! out.
//!
//! This is the boundary between what people author (stage, scope, agent,
//! sensor and method files) and what the engine reads at runtime. Everything
//! downstream (the orchestrator, the guards, the hooks) reads the two compiled
//! files and never walks the authored tree, so the compile is where authoring
//! mistakes are caught and where determinism is decided: the same sources must
//! yield the same bytes on every machine.
//!
//! Two inputs besides the authored files shape the result:
//!
//! - the **plugin selection** (`harness.json` `plugins`): a stage owned by a
//!   plugin the install did not select stays in the graph, marked
//!   `enabled: false`, and leaves the grid;
//! - **composed scopes** ([`crate::composed`]): columns with no stage
//!   frontmatter behind them, folded back in from their records.
//!
//! Mirrors `compileStageGraph` in `aidlc-graph.ts` (L2472).

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use focus_harness::Harness;
use focus_js::order::{cmp_utf16, js_key_order, locale_compare};
use focus_js::text::parse_int;
use focus_js::{EngineError, Result, bail, json, re};
use focus_workspace::Layout;
use focus_workspace::manifest::HarnessManifest;
use focus_workspace::names::CORE_PLUGIN;
use serde_json::{Map, Value};

use crate::agents::load_agents;
use crate::composed::{self, FoldBack};
use crate::fsx::{read_dir_sorted, read_text};
use crate::rules::{RuleFile, load_rules, resolve_for_phase};
use crate::scopes;
use crate::sensors::{SensorFile, load_sensors, resolve_for_stage};
use crate::stage_schema::{self, Consume, PHASES, StageFrontmatter, ValidationContext};

/// Where the compile reads from.
#[derive(Debug, Clone)]
pub struct Sources {
    pub harness: Harness,
    /// One directory per phase, one `<slug>.md` per stage.
    pub stages_dir: PathBuf,
    pub agents_dir: PathBuf,
    pub sensors_dir: PathBuf,
    pub scopes_dir: PathBuf,
    /// The method ("memory") tree: `org.md`, `team.md`, `project.md`, `phases/`.
    pub rules_dir: PathBuf,
    /// An installed tree's `tools/data`, holding the previous compile's output
    /// and `harness.json`. `None` for a source checkout, which has neither.
    pub data_dir: Option<PathBuf>,
    /// The project's composed-scope records (`focus/scopes/`), when installed.
    pub composed_scopes_dir: Option<PathBuf>,
}

impl Sources {
    /// A source tree: a `core/` directory of hand-authored Markdown. Nothing
    /// is pinned, so every stage number is derived from the stage files.
    #[must_use]
    pub fn from_core(core: &Path, harness: Harness) -> Self {
        Self {
            harness,
            stages_dir: core.join("focus-common").join("stages"),
            agents_dir: core.join("agents"),
            sensors_dir: core.join("sensors"),
            scopes_dir: core.join("scopes"),
            rules_dir: core.join("memory"),
            data_dir: None,
            composed_scopes_dir: None,
        }
    }

    /// A harness installed in a project. The method tree and the composed
    /// scope records live in the project's workspace, shared by every harness;
    /// the compile always reads the default space's method tree, because the
    /// rule paths it writes into the graph name that space.
    #[must_use]
    pub fn from_layout(layout: &Layout) -> Self {
        Self {
            harness: layout.harness,
            stages_dir: layout.stages_dir(),
            agents_dir: layout.agents_dir(),
            sensors_dir: layout.sensors_dir(),
            scopes_dir: layout.scopes_dir(),
            rules_dir: layout.memory_dir(focus_workspace::names::DEFAULT_SPACE),
            data_dir: Some(layout.data_dir()),
            composed_scopes_dir: Some(layout.composed_scopes_dir()),
        }
    }

    /// An installed harness directory (`<project>/.claude` and the like),
    /// named directly. The project root is its parent.
    #[must_use]
    pub fn from_installed(harness_root: &Path, harness: Harness) -> Self {
        let project = harness_root.parent().unwrap_or(harness_root);
        let mut sources = Self::from_layout(&Layout::new(project, harness));
        // The directory may not carry the harness's usual name (a staged copy
        // under another name), so every harness-tree path is taken from it.
        sources.stages_dir = harness_root.join("focus-common").join("stages");
        sources.agents_dir = harness_root.join("agents");
        sources.sensors_dir = harness_root.join("sensors");
        sources.scopes_dir = harness_root.join("scopes");
        sources.data_dir = Some(harness_root.join("tools").join("data"));
        sources
    }

    fn stage_graph_path(&self) -> Option<PathBuf> {
        self.data_dir
            .as_ref()
            .map(|dir| dir.join("stage-graph.json"))
    }

    fn scope_grid_path(&self) -> Option<PathBuf> {
        self.data_dir
            .as_ref()
            .map(|dir| dir.join("scope-grid.json"))
    }
}

/// The key order of a compiled stage, pinned so the emitted bytes never depend
/// on how a node was built. [`GraphStage::to_json`] writes in this order and
/// the designer export re-projects compiled nodes through it.
pub const FIELD_ORDER: [&str; 30] = [
    "slug",
    "number",
    "name",
    "plugin",
    "enabled",
    "phase",
    "execution",
    "condition",
    "lead_agent",
    "support_agents",
    "mode",
    "for_each",
    "workspace_requires",
    "produces",
    "optional_produces",
    "produces_kinds",
    "consumes",
    "requires_stage",
    "sensors",
    "scopes",
    "ars",
    "reviewer",
    "review_artifact",
    "reviewer_max_iterations",
    "review_class",
    "summary_confirmation",
    "inputs",
    "outputs",
    "rules_in_context",
    "sensors_applicable",
];

/// A rule file as it appears on a compiled stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleResolution {
    pub path: String,
    pub scope: &'static str,
}

/// A sensor import as it appears on a compiled stage: the manifest's dispatch
/// fields, snapshotted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorResolution {
    pub id: String,
    pub path: String,
    pub fire_on: String,
    pub default_severity: String,
    pub category: Option<String>,
    pub matches: Option<String>,
}

/// One node of the compiled stage graph.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphStage {
    /// The validated frontmatter this node was built from. `requires_stage`
    /// here is deduplicated; `reviewer_max_iterations` and `review_class`
    /// carry their defaults when a reviewer is declared.
    pub frontmatter: StageFrontmatter,
    /// `<phase index>.<index in phase>`, assigned by the engine.
    pub number: String,
    pub name: String,
    pub rules_in_context: Vec<RuleResolution>,
    pub sensors_applicable: Vec<SensorResolution>,
    /// `false` when the install's plugin selection leaves this stage's plugin
    /// out. A disabled stage stays in the compiled file, so its number is
    /// still taken when the plugin is selected again, but it is not part of
    /// the runtime graph.
    pub enabled: bool,
}

impl GraphStage {
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.frontmatter.slug
    }

    /// The plugin that owns this stage; core when the stage names none.
    #[must_use]
    pub fn plugin_owner(&self) -> &str {
        self.frontmatter.plugin.as_deref().unwrap_or(CORE_PLUGIN)
    }

    /// The node as JSON, keys in the pinned order every reader of
    /// `stage-graph.json` sees (`FIELD_ORDER` upstream). Optional fields are
    /// omitted when absent, so a stage that does not use a feature compiles to
    /// the same bytes as before the feature existed.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let f = &self.frontmatter;
        let mut out = Map::new();
        let mut put = |key: &str, value: Value| {
            out.insert(key.to_owned(), value);
        };
        let strings =
            |items: &[String]| Value::Array(items.iter().cloned().map(Value::String).collect());

        put("slug", Value::String(f.slug.clone()));
        put("number", Value::String(self.number.clone()));
        put("name", Value::String(self.name.clone()));
        if let Some(plugin) = &f.plugin {
            put("plugin", Value::String(plugin.clone()));
        }
        // Present only when false, so an install with no selection compiles to
        // the same bytes as before selections existed.
        if !self.enabled {
            put("enabled", Value::Bool(false));
        }
        put("phase", Value::String(f.phase.clone()));
        put("execution", Value::String(f.execution.clone()));
        put("condition", Value::String(f.condition.clone()));
        put("lead_agent", Value::String(f.lead_agent.clone()));
        put("support_agents", strings(&f.support_agents));
        put("mode", Value::String(f.mode.clone()));
        if let Some(for_each) = &f.for_each {
            put("for_each", Value::String(for_each.clone()));
        }
        if let Some(workspace_requires) = f.workspace_requires {
            put("workspace_requires", Value::Bool(workspace_requires));
        }
        put("produces", strings(&f.produces));
        if let Some(optional) = &f.optional_produces {
            put("optional_produces", strings(optional));
        }
        if let Some(kinds) = &f.produces_kinds {
            put("produces_kinds", Value::Object(kinds.clone()));
        }
        put(
            "consumes",
            Value::Array(f.consumes.iter().map(consume_json).collect()),
        );
        put("requires_stage", strings(&f.requires_stage));
        if let Some(sensors) = &f.sensors {
            put("sensors", strings(sensors));
        }
        if let Some(scopes) = &f.scopes {
            put("scopes", strings(scopes));
        }
        if let Some(ars) = &f.ars {
            put("ars", ars.clone());
        }
        if let Some(reviewer) = &f.reviewer {
            put("reviewer", Value::String(reviewer.clone()));
            if let Some(artifact) = &f.review_artifact {
                put("review_artifact", Value::String(artifact.clone()));
            }
            if let Some(cap) = &f.reviewer_max_iterations {
                put("reviewer_max_iterations", Value::Number(cap.clone()));
            }
            if let Some(class) = &f.review_class {
                put("review_class", Value::String(class.clone()));
            }
        }
        if let Some(confirmation) = &f.summary_confirmation {
            put("summary_confirmation", Value::String(confirmation.clone()));
        }
        put("inputs", Value::String(f.inputs.clone()));
        put("outputs", Value::String(f.outputs.clone()));
        put(
            "rules_in_context",
            Value::Array(self.rules_in_context.iter().map(rule_json).collect()),
        );
        put(
            "sensors_applicable",
            Value::Array(self.sensors_applicable.iter().map(sensor_json).collect()),
        );
        Value::Object(out)
    }
}

fn consume_json(consume: &Consume) -> Value {
    let mut out = Map::new();
    out.insert(
        "artifact".to_owned(),
        Value::String(consume.artifact.clone()),
    );
    out.insert("required".to_owned(), Value::Bool(consume.required));
    if let Some(on) = &consume.conditional_on {
        out.insert("conditional_on".to_owned(), Value::String(on.clone()));
    }
    Value::Object(out)
}

fn rule_json(rule: &RuleResolution) -> Value {
    let mut out = Map::new();
    out.insert("path".to_owned(), Value::String(rule.path.clone()));
    out.insert("scope".to_owned(), Value::String(rule.scope.to_owned()));
    Value::Object(out)
}

fn sensor_json(sensor: &SensorResolution) -> Value {
    let mut out = Map::new();
    out.insert("id".to_owned(), Value::String(sensor.id.clone()));
    out.insert("path".to_owned(), Value::String(sensor.path.clone()));
    out.insert("fire_on".to_owned(), Value::String(sensor.fire_on.clone()));
    out.insert(
        "default_severity".to_owned(),
        Value::String(sensor.default_severity.clone()),
    );
    if let Some(category) = &sensor.category {
        out.insert("category".to_owned(), Value::String(category.clone()));
    }
    if let Some(matches) = &sensor.matches {
        out.insert("matches".to_owned(), Value::String(matches.clone()));
    }
    Value::Object(out)
}

/// What a compile produces.
#[derive(Debug, Clone)]
pub struct Compiled {
    /// The exact bytes of `stage-graph.json`.
    pub stage_graph_json: String,
    /// The exact bytes of `scope-grid.json`.
    pub scope_grid_json: String,
    /// The compiled stages, in numeric order.
    pub stages: Vec<GraphStage>,
    /// Advisories for the author. They never change the emitted bytes.
    pub advisories: Vec<String>,
    /// The composed scopes the grid carries, by where each came from. The
    /// write side gives a record to those that had only a grid column.
    pub composed: FoldBack,
}

/// The sole writer of `stage-graph.json` bytes. One emitter is what makes the
/// byte comparison in `compile --check` meaningful.
#[must_use]
pub fn canonical_stage_graph_json(stages: &[GraphStage]) -> String {
    let nodes = Value::Array(stages.iter().map(GraphStage::to_json).collect());
    format!("{}\n", json::stringify_pretty(&nodes))
}

/// Total order over stage numbers (`<phase>.<index>`), phase first.
///
/// Callers pass engine-assigned or shape-checked numbers, so both parts parse.
#[must_use]
pub fn numeric_stage_order(a: &str, b: &str) -> Ordering {
    let parts = |number: &str| -> (f64, f64) {
        let mut it = number
            .split('.')
            .map(|part| parse_int(part).unwrap_or(f64::NAN));
        (it.next().unwrap_or(f64::NAN), it.next().unwrap_or(f64::NAN))
    };
    let ((a_phase, a_index), (b_phase, b_index)) = (parts(a), parts(b));
    let by = |x: f64, y: f64| x.partial_cmp(&y).unwrap_or(Ordering::Equal);
    by(a_phase, b_phase).then(by(a_index, b_index))
}

/// The default display name for a newly seeded stage: `my-stage` -> `My Stage`.
fn title_case_slug(slug: &str) -> String {
    slug.split('-')
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A stage seen for the first time: it has no pinned number yet.
struct NewStage {
    data: StageFrontmatter,
    phase_dir: String,
    name: String,
}

/// Compile the stage graph and the scope grid from `sources`.
///
/// # Errors
///
/// Any authoring error: malformed or invalid frontmatter, a file whose name
/// and slug disagree, a duplicate slug, a consumed artifact with two
/// producers, an unknown agent or sensor, a dependency cycle or an edge that
/// points forward in the numeric order; a malformed plugin selection or one
/// that disables the only producer of a required input; a composed scope
/// record that does not parse.
pub fn compile(sources: &Sources) -> Result<Compiled> {
    // Loaded first so scope authoring errors fail the compile.
    let scopes_all = scopes::load_all(&sources.scopes_dir)?;
    let manifest = match &sources.data_dir {
        Some(dir) => HarnessManifest::read(&dir.join("harness.json"))?,
        None => HarnessManifest::default(),
    };
    let scopes_enabled = scopes::enabled(&scopes_all, &manifest);
    scopes::check_single_freeform_default(&scopes_enabled)?;

    // A slug already in the installed graph keeps its number and name: stage
    // numbers appear in an in-flight workflow's state, so they are computed
    // once and stable thereafter. A new slug is seeded below.
    let pinned = load_pinned(sources)?;
    let pinned_of = |slug: &str| pinned.iter().find(|row| row.slug == slug);
    let mut max_index: Vec<(usize, f64)> = Vec::new();
    for row in &pinned {
        let mut parts = row.number.split('.').map(parse_int);
        if let (Some(Some(prefix)), Some(Some(index))) = (parts.next(), parts.next()) {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            bump_max(&mut max_index, prefix as usize, index);
        }
    }

    let known_agents: Vec<String> = load_agents(&sources.agents_dir)?
        .into_iter()
        .map(|a| a.slug)
        .collect();

    let mut stages: Vec<GraphStage> = Vec::new();
    let mut new_by_prefix: Vec<(usize, Vec<NewStage>)> = Vec::new();
    let mut slug_to_file: Vec<(String, String)> = Vec::new();
    let mut producers: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let mut consumers: Vec<(String, Vec<(String, String)>)> = Vec::new();

    for phase_dir in phase_dirs(&sources.stages_dir)? {
        let dir = sources.stages_dir.join(&phase_dir);
        let is_dir = std::fs::metadata(&dir)
            .map_err(|err| EngineError::new(format!("{}: {err}", dir.display())))?
            .is_dir();
        if !is_dir {
            continue;
        }
        for file in read_dir_sorted(&dir)?
            .into_iter()
            .filter(|f| f.ends_with(".md"))
        {
            let path = dir.join(&file);
            let shown = path.display().to_string();
            let raw = read_text(&path)?;
            let parsed = stage_schema::parse(&raw).map_err(|err| err.prefixed(&shown))?;
            let data = stage_schema::validate(
                &Value::Object(parsed),
                ValidationContext {
                    agents: Some(&known_agents),
                },
            )
            .map_err(|errors| {
                EngineError::new(format!(
                    "{shown}: schema validation failed: {}",
                    errors.join("; ")
                ))
            })?;
            let slug = data.slug.clone();

            if let Some(plugin) = &data.plugin {
                check_plugin_ownership(&shown, &slug, plugin)?;
            }
            let stem = file.strip_suffix(".md").unwrap_or(&file);
            if stem != slug {
                bail!(
                    "{shown}: stage filename stem \"{stem}\" does not match frontmatter slug \"{slug}\". Rename the file or fix the slug."
                );
            }
            if let Some((_, previous)) = slug_to_file.iter().find(|(s, _)| *s == slug) {
                bail!(
                    "Duplicate stage slug \"{slug}\" in {shown} — already declared in {previous}. Rename one of them."
                );
            }
            slug_to_file.push((slug.clone(), shown.clone()));

            // Required and optional outputs share one producer namespace.
            let declaration = (shown.clone(), slug.clone());
            let outputs = data
                .produces
                .iter()
                .chain(data.optional_produces.iter().flatten());
            for artifact in unique(outputs) {
                record(&mut producers, artifact, declaration.clone());
            }
            for artifact in unique(data.consumes.iter().map(|c| &c.artifact)) {
                record(&mut consumers, artifact, declaration.clone());
            }

            let Some(prefix) = PHASES.iter().position(|phase| *phase == phase_dir) else {
                bail!(
                    "Stage \"{slug}\" ({shown}) is in an unknown phase directory \"{phase_dir}\". Stage phase directories must be one of: {}.",
                    PHASES.join(", ")
                );
            };
            let pinned_row = pinned_of(&slug);
            let name = pinned_row
                .and_then(|row| row.name.clone())
                .or_else(|| data.name.clone())
                .unwrap_or_else(|| title_case_slug(&slug));
            match pinned_row
                .map(|row| row.number.clone())
                .filter(|number| !number.is_empty())
            {
                Some(number) => stages.push(build_stage(data, number, name)),
                None => {
                    let seed = NewStage {
                        data,
                        phase_dir: phase_dir.clone(),
                        name,
                    };
                    match new_by_prefix.iter_mut().find(|(p, _)| *p == prefix) {
                        Some((_, batch)) => batch.push(seed),
                        None => new_by_prefix.push((prefix, vec![seed])),
                    }
                }
            }
        }
    }

    // An artifact name may be shared while nobody consumes it (several stages
    // produce `traceability`); once consumed it needs exactly one producer.
    for (artifact, declared_by) in &producers {
        if declared_by.len() < 2 {
            continue;
        }
        let Some((consumer_file, consumer_slug)) = consumers
            .iter()
            .find(|(a, _)| a == artifact)
            .and_then(|(_, list)| list.first())
        else {
            continue;
        };
        let producer_list: Vec<String> = declared_by
            .iter()
            .map(|(file, slug)| format!("{file} (stage \"{slug}\")"))
            .collect();
        bail!(
            "Duplicate producers for consumed artifact \"{artifact}\" in {} — consumed by stage \"{consumer_slug}\" in {consumer_file}. Rename one produced artifact or update the consumer.",
            producer_list.join(", ")
        );
    }

    new_by_prefix.sort_by_key(|(prefix, _)| *prefix);
    for (prefix, batch) in new_by_prefix {
        for seed in seed_order(batch)? {
            let next = max_index
                .iter()
                .find(|(p, _)| *p == prefix)
                .map_or(0.0, |(_, max)| *max)
                + 1.0;
            bump_max(&mut max_index, prefix, next);
            stages.push(build_stage(
                seed.data,
                format!("{prefix}.{next}"),
                seed.name,
            ));
        }
    }
    stages.sort_by(|a, b| numeric_stage_order(&a.number, &b.number));

    let rules = load_rules(&sources.rules_dir)?;
    let sensors = load_sensors(&sources.sensors_dir, sources.harness.dir())?;
    for stage in &mut stages {
        stage.rules_in_context = resolve_rules(&stage.frontmatter.phase, &rules);
        let imports = stage.frontmatter.sensors.clone().unwrap_or_default();
        stage.sensors_applicable = resolve_sensors(stage.slug(), &imports, &sensors)?;
    }

    // Initialization always runs; every other stage follows its plugin.
    for stage in &mut stages {
        stage.enabled = stage.frontmatter.phase == "initialization"
            || manifest.plugin_enabled(stage.plugin_owner());
    }
    check_selection_closure(&stages)?;

    check_edges_point_backwards(&stages)?;
    let advisories = swarm_advisories(&stages);

    // The transpose covers only the scopes stage frontmatter declares.
    // Composed scopes have no frontmatter behind them, so they are folded back
    // in: a recompile must never destroy a scope a person approved.
    let stock_scopes: Vec<&str> = unique(
        stages
            .iter()
            .flat_map(|s| s.frontmatter.scopes.iter().flatten()),
    );
    let on_disk_grid = sources
        .scope_grid_path()
        .and_then(|path| std::fs::read(path).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let records = match &sources.composed_scopes_dir {
        Some(dir) => composed::load_records(dir)?,
        None => Vec::new(),
    };
    let installed_scopes: Vec<&str> = scopes_all.iter().map(|s| s.name.as_str()).collect();
    let composed = composed::fold_back(
        &records,
        on_disk_grid.as_deref(),
        &stock_scopes,
        &installed_scopes,
    );

    // Under a plugin selection the grid is exactly the enabled scopes (each
    // gets a column even when no enabled stage names it) plus the composed
    // ones. Without a selection it is every scope a stage names.
    let selected: Option<Vec<&str>> = manifest.plugins.as_ref().map(|_| {
        let mut names: Vec<&str> = scopes_enabled.iter().map(|s| s.name.as_str()).collect();
        names.sort_by(|a, b| cmp_utf16(a, b));
        names
    });
    let seeded: Option<Vec<&str>> = selected.as_ref().map(|names| {
        names
            .iter()
            .copied()
            .filter(|name| !composed.names.iter().any(|c| c == name))
            .collect()
    });
    let enabled_stages: Vec<&GraphStage> = stages.iter().filter(|s| s.enabled).collect();
    let mut grid = composed::merge(
        transpose_scope_grid(&enabled_stages, seeded.as_deref()),
        &composed,
    );
    if let Some(selected) = &selected {
        grid.retain(|name, _| {
            selected.contains(&name.as_str()) || composed.names.iter().any(|c| c == name)
        });
    }

    Ok(Compiled {
        stage_graph_json: canonical_stage_graph_json(&stages),
        scope_grid_json: canonical_scope_grid_json(&grid),
        stages,
        advisories,
        composed,
    })
}

/// A plugin selection must not starve an enabled stage: if a stage that still
/// runs requires an artifact, and every stage that produces it was disabled,
/// the selection is refused.
///
/// An ordering edge (`requires_stage`) to a disabled stage is not an error:
/// it is vacuous, and a plugin-only install legitimately keeps plugin stages
/// that name core stages.
fn check_selection_closure(stages: &[GraphStage]) -> Result<()> {
    for stage in stages.iter().filter(|s| s.enabled) {
        for consume in stage.frontmatter.consumes.iter().filter(|c| c.required) {
            let producers: Vec<&GraphStage> = stages
                .iter()
                .filter(|p| {
                    p.frontmatter
                        .produces
                        .iter()
                        .chain(p.frontmatter.optional_produces.iter().flatten())
                        .any(|artifact| *artifact == consume.artifact)
                })
                .collect();
            if producers.is_empty() || producers.iter().any(|p| p.enabled) {
                continue;
            }
            // A stage that lists the artifact under both produces and
            // optional_produces is named once per listing, as upstream names it.
            let mut producer_list: Vec<String> = Vec::new();
            for producer in &producers {
                let listings = producer
                    .frontmatter
                    .produces
                    .iter()
                    .chain(producer.frontmatter.optional_produces.iter().flatten())
                    .filter(|artifact| **artifact == consume.artifact)
                    .count();
                for _ in 0..listings {
                    producer_list.push(format!(
                        "{} ({})",
                        producer.slug(),
                        producer.plugin_owner()
                    ));
                }
            }
            producer_list.sort_by(|a, b| cmp_utf16(a, b));
            let mut plugins: Vec<&str> = unique_strs(producers.iter().map(|p| p.plugin_owner()));
            plugins.sort_by(|a, b| cmp_utf16(a, b));
            bail!(
                "Plugin selection closure failed: enabled stage \"{}\" consumes required artifact \"{}\", but its only producer(s) are disabled: {}. Enable plugin(s) {} or disable the consuming stage.",
                stage.slug(),
                consume.artifact,
                producer_list.join(", "),
                plugins.join(", ")
            );
        }
    }
    Ok(())
}

fn unique_strs<'a>(items: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// The entries of the stages directory in the order the compile walks them:
/// the five phases in lifecycle order, then anything else by name.
///
/// Upstream walks them in whatever order the filesystem lists them. The
/// compiled files do not depend on that order, but two error messages do
/// (which of two files a duplicate slug is "already declared in", and which
/// stage is named as the consumer of a doubly produced artifact), so upstream
/// words them differently from one machine to the next. Walking in lifecycle
/// order makes both messages deterministic and names the earliest stage.
fn phase_dirs(stages_dir: &Path) -> Result<Vec<String>> {
    let mut entries = read_dir_sorted(stages_dir)?;
    let rank = |name: &str| {
        PHASES
            .iter()
            .position(|phase| *phase == name)
            .unwrap_or(PHASES.len())
    };
    entries.sort_by_key(|name| rank(name));
    Ok(entries)
}

/// A row of the previously compiled graph: the identity a recompile preserves.
struct PinnedRow {
    slug: String,
    number: String,
    name: Option<String>,
}

fn load_pinned(sources: &Sources) -> Result<Vec<PinnedRow>> {
    let Some(path) = sources.stage_graph_path().filter(|p| p.exists()) else {
        return Ok(Vec::new());
    };
    let shown = path.display();
    let raw = std::fs::read_to_string(&path).map_err(|err| {
        EngineError::new(format!(
            "Stage graph not readable at {shown}: {err}. Reinstall the framework or re-run setup to restore the data file."
        ))
    })?;
    let parsed: Value = serde_json::from_str(&raw).map_err(|err| {
        EngineError::new(format!("Stage graph at {shown} is not valid JSON: {err}"))
    })?;
    let Value::Array(rows) = parsed else {
        bail!("Stage graph at {shown} is not a list of stages");
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let text = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_owned);
        let (Some(slug), Some(number)) = (text("slug"), text("number")) else {
            bail!("Stage graph at {shown} has a row without a slug or number");
        };
        // Numbers order the whole graph; one that does not parse would make
        // that order undefined, so it is refused here.
        if !re::test(r"^\d+\.\d+$", "", &number) {
            bail!(
                "Stage graph at {shown} pins stage \"{slug}\" to \"{number}\", which is not <phase>.<index>"
            );
        }
        out.push(PinnedRow {
            slug,
            number,
            name: text("name"),
        });
    }
    Ok(out)
}

fn bump_max(max_index: &mut Vec<(usize, f64)>, prefix: usize, index: f64) {
    match max_index.iter_mut().find(|(p, _)| *p == prefix) {
        Some((_, max)) => *max = max.max(index),
        None => max_index.push((prefix, index)),
    }
}

/// `focus-` is core's namespace in runner paths, and a plugin's stages carry
/// the plugin's name as their slug prefix.
fn check_plugin_ownership(shown: &str, slug: &str, plugin: &str) -> Result<()> {
    if plugin == "focus" {
        bail!("{shown}: stage \"{slug}\" declares plugin \"focus\"; omit plugin for core stages.");
    }
    if plugin.starts_with("focus-") {
        bail!(
            "{shown}: stage \"{slug}\" declares plugin \"{plugin}\"; the \"focus-\" prefix is reserved for core (a plugin named focus-<x> collides with core runner paths). Rename the plugin."
        );
    }
    if !slug.starts_with(&format!("{plugin}-")) {
        bail!(
            "{shown}: stage \"{slug}\" declares plugin \"{plugin}\", but plugin-owned stage slugs must start with \"{plugin}-\". Rename the slug or fix the plugin field."
        );
    }
    Ok(())
}

fn build_stage(mut data: StageFrontmatter, number: String, name: String) -> GraphStage {
    // Dependency edges are a set: a copy-pasted duplicate must not change any
    // consumer's indegree arithmetic.
    data.requires_stage = unique(data.requires_stage.iter())
        .into_iter()
        .map(str::to_owned)
        .collect();
    if data.reviewer.is_some() {
        // A declared reviewer always carries a cap and a class in the graph.
        data.reviewer_max_iterations.get_or_insert_with(|| 2.into());
        data.review_class = Some(
            if data.review_class.as_deref() == Some("advisory") {
                "advisory"
            } else {
                "adversarial"
            }
            .to_owned(),
        );
    }
    GraphStage {
        frontmatter: data,
        number,
        name,
        rules_in_context: Vec::new(),
        sensors_applicable: Vec::new(),
        enabled: true,
    }
}

/// Order one phase's new stages for numbering: by their own dependency edges
/// (Kahn), ties broken by the authored `number:` hint, then by slug.
///
/// Numbers are assigned by the engine and never claimed by authors, so two
/// plugins written without coordination cannot collide, and a batch whose file
/// order contradicts its flow order still numbers validly.
fn seed_order(batch: Vec<NewStage>) -> Result<Vec<NewStage>> {
    let in_batch = |slug: &str| batch.iter().any(|e| e.data.slug == slug);
    let mut indegree: Vec<usize> = batch
        .iter()
        .map(|e| {
            unique(e.data.requires_stage.iter())
                .into_iter()
                .filter(|dep| in_batch(dep))
                .count()
        })
        .collect();
    // Only the index segment of an authored number is read, and only to order
    // independent stages; its absolute value never reaches the graph.
    let hint = |e: &NewStage| -> f64 {
        e.data
            .number
            .as_deref()
            .and_then(|n| n.split('.').nth(1))
            .and_then(parse_int)
            .unwrap_or(f64::INFINITY)
    };
    let by_hint_then_slug = |a: &usize, b: &usize| {
        let (a, b) = (&batch[*a], &batch[*b]);
        hint(a)
            .partial_cmp(&hint(b))
            .unwrap_or(Ordering::Equal)
            .then_with(|| locale_compare(&a.data.slug, &b.data.slug))
    };

    let mut ready: Vec<usize> = (0..batch.len()).filter(|i| indegree[*i] == 0).collect();
    ready.sort_by(by_hint_then_slug);
    let mut order: Vec<usize> = Vec::with_capacity(batch.len());
    while !ready.is_empty() {
        let current = ready.remove(0);
        order.push(current);
        for other in 0..batch.len() {
            if indegree[other] == 0
                || !batch[other]
                    .data
                    .requires_stage
                    .contains(&batch[current].data.slug)
            {
                continue;
            }
            indegree[other] -= 1;
            if indegree[other] == 0 {
                ready.push(other);
                ready.sort_by(by_hint_then_slug);
            }
        }
    }
    if order.len() < batch.len() {
        // The unseeded set is the cycle plus everything downstream of it.
        let stuck: Vec<&str> = (0..batch.len())
            .filter(|i| !order.contains(i))
            .map(|i| batch[i].data.slug.as_str())
            .collect();
        bail!(
            "Cannot seed stage numbers for phase \"{}\": requires_stage cycle among new stages (stuck: {}). Break the cycle.",
            batch[0].phase_dir,
            stuck.join(", ")
        );
    }

    let mut slots: Vec<Option<NewStage>> = batch.into_iter().map(Some).collect();
    Ok(order.into_iter().filter_map(|i| slots[i].take()).collect())
}

fn resolve_rules(phase: &str, rules: &[RuleFile]) -> Vec<RuleResolution> {
    resolve_for_phase(phase, rules)
        .into_iter()
        .map(|rule| RuleResolution {
            path: rule.path.clone(),
            scope: rule.scope.as_str(),
        })
        .collect()
}

fn resolve_sensors(
    slug: &str,
    imports: &[String],
    registry: &[SensorFile],
) -> Result<Vec<SensorResolution>> {
    Ok(resolve_for_stage(slug, imports, registry)?
        .into_iter()
        .map(|sensor| SensorResolution {
            id: sensor.id.clone(),
            path: sensor.path.clone(),
            fire_on: sensor.manifest.fire_on.clone(),
            default_severity: sensor.manifest.default_severity.clone().unwrap_or_default(),
            category: sensor.manifest.category.clone(),
            matches: sensor.manifest.matches.clone(),
        })
        .collect())
}

/// Every dependency must be numbered before its dependent. The serial runtime
/// walks stages in numeric order, so this is what makes numeric order a valid
/// topological order of the graph.
fn check_edges_point_backwards(stages: &[GraphStage]) -> Result<()> {
    for stage in stages {
        for dep in &stage.frontmatter.requires_stage {
            let Some(dep_stage) = stages.iter().find(|s| s.slug() == dep) else {
                bail!(
                    "Unknown requires_stage: \"{dep}\" on stage \"{}\". Every requires_stage entry must reference a known stage slug.",
                    stage.slug()
                );
            };
            if numeric_stage_order(&dep_stage.number, &stage.number) != Ordering::Less {
                bail!(
                    "Compile invariant violated: stage \"{}\" ({}) requires \"{dep}\" ({}) — dependency must be lower-numbered. Fix: either renumber in stage-graph.json to match the dependency direction, or remove the offending requires_stage edge.",
                    stage.slug(),
                    stage.number,
                    dep_stage.number
                );
            }
        }
    }
    Ok(())
}

/// The autonomous Construction swarm fires on `for_each: unit-of-work` with
/// `mode: subagent`. A per-unit build stage in any other mode builds its units
/// serially: legal, but easy to cause by accident while retuning modes.
fn swarm_advisories(stages: &[GraphStage]) -> Vec<String> {
    stages
        .iter()
        .map(|stage| &stage.frontmatter)
        .filter(|f| {
            f.phase == "construction"
                && f.for_each.as_deref() == Some("unit-of-work")
                && f.workspace_requires == Some(true)
                && f.mode != "subagent"
        })
        .map(|f| {
            format!(
                "[advisory] stage \"{}\" is the per-unit build stage (for_each: unit-of-work + workspace_requires) but mode is \"{}\", not \"subagent\" — the autonomous Construction swarm will NOT fire for it; units build serially.",
                f.slug, f.mode
            )
        })
        .collect()
}

/// The scope grid: for each scope, which of `stages` run under it.
///
/// A pure transpose of the stages' `scopes:` lists, with one rule on top:
/// initialization stages run under every scope. With `allowed`, the columns
/// are exactly those names (a stage naming another scope adds nothing, and an
/// allowed scope no stage names still gets a column). Scopes are ordered as a
/// JS object built from sorted names enumerates them (see `js_key_order`);
/// stages keep the order given.
#[must_use]
pub fn transpose_scope_grid(
    stages: &[&GraphStage],
    allowed: Option<&[&str]>,
) -> Map<String, Value> {
    let mut scope_names: Vec<&str> = unique(
        stages
            .iter()
            .flat_map(|s| s.frontmatter.scopes.iter().flatten()),
    );
    if let Some(allowed) = allowed {
        scope_names.retain(|name| allowed.contains(name));
        for name in allowed {
            if !scope_names.contains(name) {
                scope_names.push(name);
            }
        }
    }
    scope_names.sort_by(|a, b| cmp_utf16(a, b));

    let mut grid = Map::new();
    for scope in scope_names {
        let mut cells = Map::new();
        for stage in stages {
            let f = &stage.frontmatter;
            let runs =
                f.phase == "initialization" || f.scopes.iter().flatten().any(|name| name == scope);
            cells.insert(
                f.slug.clone(),
                Value::String(if runs { "EXECUTE" } else { "SKIP" }.to_owned()),
            );
        }
        let mut entry = Map::new();
        entry.insert("stages".to_owned(), Value::Object(js_key_order(cells)));
        grid.insert(scope.to_owned(), Value::Object(entry));
    }
    js_key_order(grid)
}

/// The sole writer of `scope-grid.json` bytes.
#[must_use]
pub fn canonical_scope_grid_json(grid: &Map<String, Value>) -> String {
    format!("{}\n", json::stringify_pretty(&Value::Object(grid.clone())))
}

/// Distinct items in first-seen order (`[...new Set(items)]`).
fn unique<'a>(items: impl IntoIterator<Item = &'a String>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for item in items {
        if !out.contains(&item.as_str()) {
            out.push(item);
        }
    }
    out
}

fn record(
    index: &mut Vec<(String, Vec<(String, String)>)>,
    artifact: &str,
    declaration: (String, String),
) {
    match index.iter_mut().find(|(a, _)| a == artifact) {
        Some((_, list)) => list.push(declaration),
        None => index.push((artifact.to_owned(), vec![declaration])),
    }
}

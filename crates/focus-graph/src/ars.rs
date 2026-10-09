//! ARS (Autonomy Risk Score): the arithmetic behind a composed plan.
//!
//! When someone asks for a plan tailored to a piece of work, the composer
//! agent scores five kinds of uncertainty from evidence. That is the judgment
//! half. Everything downstream is arithmetic and lives here, so the same
//! scores always produce the same proposal and no model ever multiplies: the
//! weighted composite and its band, a per-stage screen (does this stage reduce
//! an uncertainty that is high enough to be worth the stage's cost?), the
//! stock scopes nearest to the screened plan, and the two tables shown at the
//! approval gate, pre-rendered.
//!
//! Every constant comes from `tools/data/ars-priors.json`. The composite is
//! advisory: nothing in the engine routes on it.
//!
//! Mirrors `loadArsPriors` and `computeArs` in `aidlc-graph.ts` (L3132, L3250).

use std::path::Path;

use focus_js::{EngineError, Result, bail, json, num};
use serde_json::{Map, Value};

use crate::query::{Nearest, nearest_stock_scopes};
use crate::runtime::Stage;
use crate::stage_schema::{
    ARS_COMPONENT_KEYS, ARS_COST_MAX, ARS_COST_MIN, ARS_PROJECT_TYPE_KEYS, ARS_ROLES,
};

/// The composite is normalised at this many decimals before rounding. Summing
/// the weighted terms in binary can land a hair under an exact half
/// (0.75 + 12.45 + 7.3 evaluates to 20.499999999999996), which would round
/// into the band below the one the documented formula gives.
const RAW_PRECISION: usize = 9;

/// A stage's screening prior, from the shipped table or the stage itself.
#[derive(Debug, Clone, PartialEq)]
struct StagePrior {
    /// The uncertainty components this stage reduces.
    targets: Vec<String>,
    /// How expensive the stage is, 1 to 5; `None` when it cannot be screened
    /// numerically.
    cost: Option<f64>,
    role: Option<String>,
    /// Present only when the stage's condition restricts it to one kind of
    /// project.
    project_types: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq)]
struct Band {
    min: f64,
    max: f64,
    label: String,
    shape: String,
}

/// The validated contents of `ars-priors.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Priors {
    weights: [f64; 5],
    names: [String; 5],
    low_max: f64,
    med_max: f64,
    composite_bands: Vec<Band>,
    /// Stage cost (as text) to the score a target must exceed.
    ev_thresholds: Map<String, Value>,
    stages: Vec<(String, StagePrior)>,
}

/// `String(value)` for a JSON value that may be absent.
fn js_string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_owned(),
        Some(Value::Null) => "null".to_owned(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => num::to_string(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_owned(),
    }
}

/// `typeof value` for a JSON value that may be absent.
fn js_typeof(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Null | Value::Array(_) | Value::Object(_)) => "object",
    }
}

/// A list of distinct strings drawn from `allowed`, or `None`.
fn symbol_list(value: Option<&Value>, allowed: &[&str]) -> Option<Vec<String>> {
    let items = value?.as_array()?;
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        let symbol = item.as_str().filter(|s| allowed.contains(s))?;
        if out.iter().any(|seen| seen == symbol) {
            return None;
        }
        out.push(symbol.to_owned());
    }
    Some(out)
}

/// Read and validate the priors file.
///
/// # Errors
///
/// Any violation of the file's schema. There is no fallback table: a silent
/// default would bring back the unauditable arithmetic this file removes.
pub fn load_priors(path: &Path) -> Result<Priors> {
    let shown = path.display();
    let parsed = std::fs::read(path)
        .map_err(|err| err.to_string())
        .and_then(|bytes| {
            json::parse(&String::from_utf8_lossy(&bytes)).map_err(|err| err.to_string())
        })
        .map_err(|err| EngineError::new(format!("cannot read ars priors at {shown}: {err}")))?;
    let Value::Object(priors) = &parsed else {
        bail!("ars priors at {shown}: not a JSON object.");
    };
    let version = priors.get("schemaVersion");
    if version.and_then(Value::as_f64) != Some(1.0) {
        bail!(
            "ars priors at {shown}: unsupported schemaVersion {} (expected 1).",
            js_string(version)
        );
    }

    let mut weights = [0.0_f64; 5];
    let mut names: [String; 5] = Default::default();
    let mut weight_sum = 0.0_f64;
    for (idx, component) in ARS_COMPONENT_KEYS.iter().enumerate() {
        let weight = priors
            .get("weights")
            .and_then(|w| w.get(component))
            .and_then(Value::as_f64)
            .filter(|w| (0.0..=1.0).contains(w));
        let Some(weight) = weight else {
            bail!("ars priors: weights.{component} must be a number in [0,1].");
        };
        weights[idx] = weight;
        weight_sum += weight;
        let name = priors
            .get("componentInfo")
            .and_then(|info| info.get(component))
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str);
        let Some(name) = name else {
            bail!("ars priors: componentInfo.{component}.name is missing.");
        };
        names[idx] = name.to_owned();
    }
    if (weight_sum - 1.0).abs() > 1e-9 {
        bail!(
            "ars priors: weights must sum to 1.0 (got {}).",
            num::to_string(weight_sum)
        );
    }

    let band_edge = |key: &str| {
        priors
            .get("componentBands")
            .and_then(|bands| bands.get(key))
            .and_then(Value::as_f64)
    };
    let (low_max, med_max) = match (band_edge("lowMax"), band_edge("medMax")) {
        (Some(low), Some(med)) if 0.0 < low && low < med && med <= 1.0 => (low, med),
        _ => bail!("ars priors: componentBands must satisfy 0 < lowMax < medMax <= 1."),
    };

    let bands_raw = priors
        .get("compositeBands")
        .and_then(Value::as_array)
        .filter(|bands| !bands.is_empty());
    let Some(bands_raw) = bands_raw else {
        bail!("ars priors: compositeBands must be a non-empty array.");
    };
    let mut composite_bands: Vec<Band> = Vec::with_capacity(bands_raw.len());
    let mut expect_min = 0.0_f64;
    for band in bands_raw {
        let number = |key: &str| band.get(key).and_then(Value::as_f64);
        let text = |key: &str| band.get(key).and_then(Value::as_str);
        match (number("min"), number("max"), text("label"), text("shape")) {
            (Some(min), Some(max), Some(label), Some(shape)) if min == expect_min && max >= min => {
                composite_bands.push(Band {
                    min,
                    max,
                    label: label.to_owned(),
                    shape: shape.to_owned(),
                });
                expect_min = max + 1.0;
            }
            _ => bail!(
                "ars priors: compositeBands must tile 0..100 contiguously with label + shape."
            ),
        }
    }
    if expect_min != 101.0 {
        bail!("ars priors: compositeBands must end at 100.");
    }

    let ev_thresholds: Map<String, Value> = match priors.get("evThresholds") {
        Some(Value::Object(map)) => map.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(idx, item)| (idx.to_string(), item.clone()))
            .collect(),
        _ => Map::new(),
    };
    for (key, threshold) in &ev_thresholds {
        if !threshold.as_f64().is_some_and(|t| (0.0..=1.0).contains(&t)) {
            bail!("ars priors: evThresholds[\"{key}\"] must be a number in [0,1].");
        }
    }

    // Each entry follows the rules of a stage's own `ars:` block, so the two
    // sources of a prior cannot drift apart.
    let mut stages: Vec<(String, StagePrior)> = Vec::new();
    let empty = Map::new();
    let stages_raw = priors
        .get("stages")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    for (slug, entry) in stages_raw {
        let Some(targets) = symbol_list(entry.get("targets"), &ARS_COMPONENT_KEYS) else {
            bail!(
                "ars priors: stages.{slug}.targets must be a subset of {{{}}}, without repeats.",
                ARS_COMPONENT_KEYS.join(", ")
            );
        };
        let role = entry.get("role");
        if role.is_some()
            && !role
                .and_then(Value::as_str)
                .is_some_and(|r| ARS_ROLES.contains(&r))
        {
            bail!(
                "ars priors: stages.{slug}.role must be one of {{{}}}.",
                ARS_ROLES.join(", ")
            );
        }
        let cost_raw = entry.get("cost");
        let cost = match cost_raw {
            Some(Value::Null) => None,
            Some(Value::Number(n)) => {
                let cost = n.as_f64().unwrap_or(f64::NAN);
                #[allow(clippy::cast_precision_loss)]
                let in_range = num::is_integer(cost)
                    && cost >= ARS_COST_MIN as f64
                    && cost <= ARS_COST_MAX as f64;
                if !in_range {
                    bail!(
                        "ars priors: stages.{slug}.cost must be null or an integer {ARS_COST_MIN}..{ARS_COST_MAX} (got {}).",
                        num::to_string(cost)
                    );
                }
                if !ev_thresholds.contains_key(&num::to_string(cost)) {
                    bail!(
                        "ars priors: stages.{slug}.cost {} has no evThresholds entry.",
                        num::to_string(cost)
                    );
                }
                Some(cost)
            }
            other => bail!(
                "ars priors: stages.{slug}.cost must be a number or null (got {}).",
                js_typeof(other)
            ),
        };
        let project_types = match entry.get("projectTypes") {
            None => None,
            types => match symbol_list(types, &ARS_PROJECT_TYPE_KEYS).filter(|t| !t.is_empty()) {
                Some(types) => Some(types),
                None => bail!(
                    "ars priors: stages.{slug}.projectTypes must be a non-empty subset of {{{}}}, without repeats.",
                    ARS_PROJECT_TYPE_KEYS.join(", ")
                ),
            },
        };
        stages.push((
            slug.clone(),
            StagePrior {
                targets,
                cost,
                role: role.and_then(Value::as_str).map(str::to_owned),
                project_types,
            },
        ));
    }

    Ok(Priors {
        weights,
        names,
        low_max,
        med_max,
        composite_bands,
        ev_thresholds,
        stages,
    })
}

/// What the caller knows about the work being planned.
#[derive(Debug, Clone, Default)]
pub struct ArsContext<'a> {
    /// Stages that already ran (a plan reshaped in flight). They stay in the
    /// plan whatever the screen says.
    pub completed: &'a [String],
    /// `brownfield` or `greenfield`, when known.
    pub project_type: Option<&'a str>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Decision {
    Execute,
    Skip,
    Completed,
}

impl Decision {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Execute => "EXECUTE",
            Self::Skip => "SKIP",
            Self::Completed => "COMPLETED",
        }
    }
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().cloned().map(Value::String).collect())
}

fn number_or_null(value: Option<f64>) -> Value {
    value.map_or(Value::Null, Value::from)
}

/// Compute the whole result from five scores, each in `[0, 1]` with at most
/// two decimals, in the order of [`ARS_COMPONENT_KEYS`].
///
/// `graph` is the runtime graph; `all_stages` also includes the stages a
/// plugin selection disabled, so a prior for a disabled core stage is not
/// mistaken for stale data.
///
/// # Errors
///
/// A score out of range or too fine, a completed stage or a prior naming a
/// stage the graph does not have, or a stage-side prior whose cost has no
/// threshold.
pub fn compute(
    scores: [f64; 5],
    context: &ArsContext<'_>,
    priors: &Priors,
    graph: &[Stage],
    all_stages: &[Stage],
    grid: &Map<String, Value>,
) -> Result<Value> {
    for (idx, component) in ARS_COMPONENT_KEYS.iter().enumerate() {
        let score = scores[idx];
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            bail!(
                "--{component} must be a number in [0.00, 1.00] (got {}).",
                num::to_string(score)
            );
        }
        // Banding reads the exact value and the tables print two decimals, so
        // a finer score could print "0.30 | LOW" against a LOW bound of 0.30.
        if num::from_str(&num::to_fixed(score, 2)) != score {
            bail!(
                "--{component} must have at most two decimals (got {}).",
                num::to_string(score)
            );
        }
    }
    for slug in context.completed {
        if !graph.iter().any(|stage| stage.slug == *slug) {
            bail!("--completed names unknown stage \"{slug}\" - not in the compiled stage graph.");
        }
    }
    for (slug, _) in &priors.stages {
        if !all_stages.iter().any(|stage| stage.slug == *slug) {
            bail!("ars priors: stages.{slug} is not in the compiled stage graph.");
        }
    }

    let score_of = |component: &str| {
        ARS_COMPONENT_KEYS
            .iter()
            .position(|key| *key == component)
            .map_or(f64::NAN, |idx| scores[idx])
    };
    let band = |score: f64| {
        if score < priors.low_max {
            "LOW"
        } else if score < priors.med_max {
            "MED"
        } else {
            "HIGH"
        }
    };
    let fmt = |score: f64| num::to_fixed(score, 2);
    let threshold_of = |cost: f64| {
        priors
            .ev_thresholds
            .get(&num::to_string(cost))
            .and_then(Value::as_f64)
    };

    let weighted = (0..5).fold(0.0_f64, |acc, idx| acc + priors.weights[idx] * scores[idx]);
    let raw = num::from_str(&num::to_fixed(100.0 * weighted, RAW_PRECISION));
    let total = num::round(raw);
    let Some(composite_band) = priors
        .composite_bands
        .iter()
        .find(|band| total >= band.min && total <= band.max)
    else {
        bail!(
            "composite {} falls outside the compositeBands coverage.",
            num::to_string(total)
        );
    };

    // The shipped prior wins when a stage also carries its own, so screening
    // of core stages never changes under a stage-side edit.
    let mut prior_of: Vec<Option<(StagePrior, &'static str)>> = Vec::with_capacity(graph.len());
    for stage in graph {
        if let Some((_, shipped)) = priors.stages.iter().find(|(slug, _)| *slug == stage.slug) {
            prior_of.push(Some((shipped.clone(), "shipped")));
            continue;
        }
        let Some(own) = &stage.ars else {
            prior_of.push(None);
            continue;
        };
        let cost_raw = own.get("cost");
        if !matches!(cost_raw, Some(Value::Null))
            && !priors.ev_thresholds.contains_key(&js_string(cost_raw))
        {
            bail!(
                "stage {}: ars.cost {} has no evThresholds entry in ars-priors.json.",
                stage.slug,
                js_string(cost_raw)
            );
        }
        let list = |key: &str| -> Option<Vec<String>> {
            own.get(key).and_then(Value::as_array).map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
        };
        prior_of.push(Some((
            StagePrior {
                targets: list("targets").unwrap_or_default(),
                cost: cost_raw.and_then(Value::as_f64),
                role: own.get("role").and_then(Value::as_str).map(str::to_owned),
                project_types: list("project_types"),
            },
            "stage",
        )));
    }

    // A stage restricted to the other kind of project is decided by that
    // restriction, not by the arithmetic.
    let off_project_type = |prior: Option<&StagePrior>| match (
        context.project_type,
        prior.and_then(|p| p.project_types.as_ref()),
    ) {
        (Some(project_type), Some(types)) => !types.iter().any(|t| t == project_type),
        _ => false,
    };
    let role_of = |idx: usize| {
        prior_of[idx]
            .as_ref()
            .and_then(|(prior, _)| prior.role.as_deref())
    };
    let max_target = |prior: &StagePrior| {
        prior
            .targets
            .iter()
            .map(|target| score_of(target))
            .fold(f64::NEG_INFINITY, f64::max)
    };

    // Pass 1: every stage except the phase gates, which depend on the rest.
    let mut decisions: Vec<Option<Decision>> = Vec::with_capacity(graph.len());
    for (idx, stage) in graph.iter().enumerate() {
        let prior = prior_of[idx].as_ref().map(|(prior, _)| prior);
        let decision = if context.completed.contains(&stage.slug) {
            Some(Decision::Completed)
        } else if off_project_type(prior) {
            Some(Decision::Skip)
        } else if role_of(idx) == Some("phase-gate") {
            None
        } else if matches!(role_of(idx), Some("initialization" | "core")) {
            Some(Decision::Execute)
        } else {
            match prior {
                Some(prior) if prior.role.as_deref() != Some("structural") => {
                    match prior.cost.and_then(&threshold_of) {
                        Some(threshold) => {
                            let best = if prior.targets.is_empty() {
                                0.0
                            } else {
                                max_target(prior)
                            };
                            Some(if best > threshold {
                                Decision::Execute
                            } else {
                                Decision::Skip
                            })
                        }
                        None => Some(Decision::Skip),
                    }
                }
                _ => Some(Decision::Skip),
            }
        };
        decisions.push(decision);
    }
    // Pass 2: a phase gate exists only when other work in its phase runs or
    // already ran. Gates do not count as work, so the gates of an otherwise
    // empty phase cannot keep each other alive.
    let active_phases: Vec<&str> = graph
        .iter()
        .enumerate()
        .filter(|(idx, _)| {
            role_of(*idx) != Some("phase-gate")
                && matches!(
                    decisions[*idx],
                    Some(Decision::Execute | Decision::Completed)
                )
        })
        .map(|(_, stage)| stage.phase.as_str())
        .collect();
    let decisions: Vec<Decision> = decisions
        .into_iter()
        .enumerate()
        .map(|(idx, decision)| {
            decision.unwrap_or(if active_phases.contains(&graph[idx].phase.as_str()) {
                Decision::Execute
            } else {
                Decision::Skip
            })
        })
        .collect();

    // Pass 3: one row per stage, with the reasoning the gate table shows.
    let mut rows: Vec<Value> = Vec::with_capacity(graph.len());
    let mut decision_lines: Vec<String> = Vec::with_capacity(graph.len());
    for (idx, stage) in graph.iter().enumerate() {
        let resolved = prior_of[idx].as_ref();
        let prior = resolved.map(|(prior, _)| prior);
        let decision = decisions[idx];
        let mut max_target_score: Option<f64> = None;
        let mut threshold: Option<f64> = None;
        let (screen, reason) = match (decision, prior) {
            (Decision::Completed, _) => (
                "completed",
                "completed - in-flight evidence; kept as EXECUTE in the derived grid".to_owned(),
            ),
            (_, None) => (
                "no-prior",
                "no entry in ars-priors.json and no ars: block on the stage - not screenable"
                    .to_owned(),
            ),
            (_, Some(prior)) if off_project_type(Some(prior)) => (
                "project-type",
                format!(
                    "project is {} - the stage's compiled condition restricts it to {} projects",
                    context.project_type.unwrap_or_default(),
                    prior.project_types.clone().unwrap_or_default().join("/")
                ),
            ),
            (_, Some(prior)) => match prior.role.as_deref() {
                Some("initialization") => {
                    ("initialization", "initialization - always runs".to_owned())
                }
                Some("core") => (
                    "core",
                    "spine - always (core implementation / verification)".to_owned(),
                ),
                Some("phase-gate") => (
                    "phase-gate",
                    if decision == Decision::Execute {
                        format!(
                            "phase gate - other {} stages execute, so the boundary exists",
                            stage.phase
                        )
                    } else {
                        format!(
                            "phase gate - every other {} stage folds away, so the boundary does not exist",
                            stage.phase
                        )
                    },
                ),
                Some("structural") => (
                    "structural",
                    "structural (decomposition) - not numerically screenable; mechanical default SKIP, human judgment at the gate".to_owned(),
                ),
                _ => match prior.cost {
                    None => (
                        "no-cost-prior",
                        format!(
                            "{} - not numerically screenable; human judgment at the gate",
                            if resolved.is_some_and(|(_, source)| *source == "stage") {
                                "the stage's ars: block declares no cost"
                            } else {
                                "no cost prior in the shipped table"
                            }
                        ),
                    ),
                    Some(cost) => {
                        threshold = threshold_of(cost);
                        let shown_threshold = threshold.map_or_else(
                            || "undefined".to_owned(),
                            num::to_string,
                        );
                        let shown_cost = num::to_string(cost);
                        if prior.targets.is_empty() {
                            // A costed prior naming no component can never
                            // clear its threshold.
                            (
                                "component",
                                format!(
                                    "no target component - nothing can clear threshold {shown_threshold} (cost {shown_cost}); mechanical default SKIP, human judgment at the gate"
                                ),
                            )
                        } else {
                            // The first of equal maxima names the row.
                            let best = prior.targets.iter().fold(&prior.targets[0], |a, b| {
                                if score_of(a) >= score_of(b) { a } else { b }
                            });
                            let best_score = score_of(best);
                            max_target_score = Some(best_score);
                            let symbol = best.to_uppercase();
                            (
                                "component",
                                if decision == Decision::Execute {
                                    format!(
                                        "reduces {symbol}={} > threshold {shown_threshold} (cost {shown_cost})",
                                        fmt(best_score)
                                    )
                                } else {
                                    format!(
                                        "max target {symbol}={} <= threshold {shown_threshold} (cost {shown_cost})",
                                        fmt(best_score)
                                    )
                                },
                            )
                        }
                    }
                },
            },
        };
        let mut row = Map::new();
        row.insert("stage".to_owned(), Value::from(stage.slug.clone()));
        row.insert("number".to_owned(), Value::from(stage.number.clone()));
        row.insert("decision".to_owned(), Value::from(decision.as_str()));
        row.insert(
            "priorSource".to_owned(),
            resolved.map_or(Value::Null, |(_, source)| Value::from(*source)),
        );
        row.insert(
            "targets".to_owned(),
            strings(prior.map_or(&[][..], |p| &p.targets)),
        );
        row.insert(
            "cost".to_owned(),
            number_or_null(prior.and_then(|p| p.cost)),
        );
        row.insert(
            "maxTargetScore".to_owned(),
            number_or_null(max_target_score),
        );
        row.insert("threshold".to_owned(), number_or_null(threshold));
        row.insert("screen".to_owned(), Value::from(screen));
        row.insert("reason".to_owned(), Value::from(reason.clone()));
        rows.push(Value::Object(row));
        decision_lines.push(format!(
            "| {} | {} | {} | {reason} |",
            stage.number,
            stage.slug,
            decision.as_str()
        ));
    }

    let screen_grid: Map<String, Value> = graph
        .iter()
        .zip(&decisions)
        .map(|(stage, decision)| {
            (
                stage.slug.clone(),
                Value::from(if *decision == Decision::Skip {
                    "SKIP"
                } else {
                    "EXECUTE"
                }),
            )
        })
        .collect();
    let nearest: Vec<Value> = nearest_stock_scopes(&screen_grid, graph, grid)
        .iter()
        .map(Nearest::to_json)
        .collect();

    let mut components = Map::new();
    let mut score_lines = vec![
        "| Component | Symbol | Score | Band |".to_owned(),
        "|-----------|--------|-------|------|".to_owned(),
    ];
    for (idx, component) in ARS_COMPONENT_KEYS.iter().enumerate() {
        let mut entry = Map::new();
        entry.insert("name".to_owned(), Value::from(priors.names[idx].clone()));
        entry.insert("score".to_owned(), Value::from(scores[idx]));
        entry.insert("band".to_owned(), Value::from(band(scores[idx])));
        components.insert((*component).to_owned(), Value::Object(entry));
        score_lines.push(format!(
            "| {} | {} | {} | {} |",
            priors.names[idx],
            component.to_uppercase(),
            fmt(scores[idx]),
            band(scores[idx])
        ));
    }
    score_lines.push(format!(
        "| **Composite ARS (advisory)** | - | **{} / 100** | **{}** |",
        num::to_string(total),
        composite_band.label
    ));
    let mut stage_lines = vec![
        "| # | Stage | Decision | Reasoning |".to_owned(),
        "|---|-------|----------|-----------|".to_owned(),
    ];
    stage_lines.extend(decision_lines);

    let mut composite = Map::new();
    composite.insert("raw".to_owned(), Value::from(raw));
    composite.insert("total".to_owned(), Value::from(total));
    composite.insert(
        "label".to_owned(),
        Value::from(composite_band.label.clone()),
    );
    composite.insert(
        "shape".to_owned(),
        Value::from(composite_band.shape.clone()),
    );
    let mut tables = Map::new();
    tables.insert("arsScores".to_owned(), Value::from(score_lines.join("\n")));
    tables.insert(
        "stageDecisions".to_owned(),
        Value::from(stage_lines.join("\n")),
    );

    let mut result = Map::new();
    result.insert("schemaVersion".to_owned(), Value::from(1));
    result.insert("components".to_owned(), Value::Object(components));
    result.insert("composite".to_owned(), Value::Object(composite));
    result.insert("evScreen".to_owned(), Value::Array(rows));
    result.insert("screenGrid".to_owned(), Value::Object(screen_grid));
    result.insert("nearestScopes".to_owned(), Value::Array(nearest));
    result.insert("completed".to_owned(), strings(context.completed));
    result.insert(
        "projectType".to_owned(),
        context.project_type.map_or(Value::Null, Value::from),
    );
    result.insert("tables".to_owned(), Value::Object(tables));
    Ok(Value::Object(result))
}

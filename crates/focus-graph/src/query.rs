//! Questions about a compiled stage graph.
//!
//! Every function here takes the loaded graph and grid as arguments and
//! touches no file, so a caller decides once what to load and the same
//! function serves the whole graph, a scope's sub-graph or a test fixture.
//! Mirrors the library half of `aidlc-graph.ts`.

use std::cmp::Ordering;

use focus_js::order::{cmp_utf16, locale_compare};
use focus_js::{Result, bail};
use serde_json::{Map, Value};

use crate::compile::numeric_stage_order;
use crate::runtime::{Stage, column};

/// The marker on a stage that runs once per unit of work.
pub const PER_UNIT_FOR_EACH: &str = "unit-of-work";
/// The stages that run per unit even if their marker was mistyped: a
/// defensive cross-check upstream keeps beside the marker.
pub const KNOWN_PER_UNIT_STAGES: [&str; 5] = [
    "nfr-requirements",
    "nfr-design",
    "functional-design",
    "infrastructure-design",
    "code-generation",
];

/// Whether a stage runs once per unit of work.
#[must_use]
pub fn is_per_unit(stage: &Stage) -> bool {
    stage.for_each.as_deref() == Some(PER_UNIT_FOR_EACH)
        || KNOWN_PER_UNIT_STAGES.contains(&stage.slug.as_str())
}

/// Every artifact name any stage produces, required or optional, sorted.
#[must_use]
pub fn artifacts(graph: &[Stage]) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for name in graph
        .iter()
        .flat_map(|n| n.produces.iter().chain(&n.optional_produces))
    {
        if !names.contains(&name.as_str()) {
            names.push(name);
        }
    }
    names.sort_by(|a, b| cmp_utf16(a, b));
    names
}

/// The stages that produce `artifact`, in graph order.
#[must_use]
pub fn producers_of<'a>(graph: &'a [Stage], artifact: &str) -> Vec<&'a Stage> {
    graph
        .iter()
        .filter(|n| n.produces_artifact(artifact))
        .collect()
}

/// The stages that consume `artifact`, in graph order.
#[must_use]
pub fn consumers_of<'a>(graph: &'a [Stage], artifact: &str) -> Vec<&'a Stage> {
    graph
        .iter()
        .filter(|n| n.consumes.iter().any(|c| c.artifact == artifact))
        .collect()
}

/// A topological order of `graph`, preferring lower stage numbers among the
/// stages that are ready. Edges to stages outside `graph` are ignored, so this
/// works on a scope's sub-graph too.
///
/// # Errors
///
/// The graph has a dependency cycle.
pub fn topo_sort<'a>(graph: &[&'a Stage]) -> Result<Vec<&'a str>> {
    let position = |slug: &str| graph.iter().position(|n| n.slug == slug);
    let mut indegree: Vec<usize> = graph
        .iter()
        .map(|n| {
            n.requires_stage
                .iter()
                .filter(|dep| position(dep).is_some())
                .count()
        })
        .collect();
    let by_number =
        |a: &usize, b: &usize| numeric_stage_order(&graph[*a].number, &graph[*b].number);

    let mut ready: Vec<usize> = (0..graph.len()).filter(|i| indegree[*i] == 0).collect();
    ready.sort_by(by_number);
    let mut order: Vec<&str> = Vec::with_capacity(graph.len());
    while !ready.is_empty() {
        let next = ready.remove(0);
        order.push(&graph[next].slug);
        for other in 0..graph.len() {
            if indegree[other] == 0 || !graph[other].requires_stage.contains(&graph[next].slug) {
                continue;
            }
            indegree[other] -= 1;
            if indegree[other] == 0 {
                let at = ready
                    .iter()
                    .position(|r| by_number(r, &other) != Ordering::Less)
                    .unwrap_or(ready.len());
                ready.insert(at, other);
            }
        }
    }
    if order.len() != graph.len() {
        bail!(
            "topoSort: cycle detected. Processed {} of {} nodes. Use findCycles() to enumerate.",
            order.len(),
            graph.len()
        );
    }
    Ok(order)
}

/// The dependency cycles in `graph`: every group of two or more stages that
/// depend on each other, and every stage that requires itself. Edges to
/// stages outside `graph` are ignored.
///
/// Tarjan's algorithm, visiting stages and their dependencies in the order
/// given, so the result (and the order within each cycle) is the same as
/// upstream's for the same graph.
#[must_use]
pub fn find_cycles<'a>(graph: &[&'a Stage]) -> Vec<Vec<&'a str>> {
    struct Walk<'g, 'a> {
        graph: &'g [&'a Stage],
        index: Vec<Option<usize>>,
        lowlink: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        cycles: Vec<Vec<&'a str>>,
    }

    impl Walk<'_, '_> {
        fn visit(&mut self, v: usize) {
            self.index[v] = Some(self.next);
            self.lowlink[v] = self.next;
            self.next += 1;
            self.stack.push(v);
            self.on_stack[v] = true;

            let deps: Vec<usize> = self.graph[v]
                .requires_stage
                .iter()
                .filter_map(|dep| self.graph.iter().position(|n| n.slug == *dep))
                .collect();
            for w in deps {
                match self.index[w] {
                    None => {
                        self.visit(w);
                        self.lowlink[v] = self.lowlink[v].min(self.lowlink[w]);
                    }
                    Some(index_w) if self.on_stack[w] => {
                        self.lowlink[v] = self.lowlink[v].min(index_w);
                    }
                    Some(_) => {}
                }
            }

            if Some(self.lowlink[v]) == self.index[v] {
                let mut component: Vec<usize> = Vec::new();
                while let Some(w) = self.stack.pop() {
                    self.on_stack[w] = false;
                    component.push(w);
                    if w == v {
                        break;
                    }
                }
                let self_loop = component.len() == 1
                    && self.graph[v].requires_stage.contains(&self.graph[v].slug);
                if component.len() >= 2 || self_loop {
                    self.cycles.push(
                        component
                            .into_iter()
                            .map(|w| self.graph[w].slug.as_str())
                            .collect(),
                    );
                }
            }
        }
    }

    let mut walk = Walk {
        graph,
        index: vec![None; graph.len()],
        lowlink: vec![0; graph.len()],
        on_stack: vec![false; graph.len()],
        stack: Vec::new(),
        next: 0,
        cycles: Vec::new(),
    };
    for v in 0..graph.len() {
        if walk.index[v].is_none() {
            walk.visit(v);
        }
    }
    walk.cycles
}

fn unknown_scope(scope: &str, valid_scopes: &[String]) -> focus_js::EngineError {
    focus_js::EngineError::new(format!(
        "Unknown scope: \"{scope}\". Valid scopes: {}",
        valid_scopes.join(", ")
    ))
}

/// Fail unless `scope` is a valid scope.
///
/// # Errors
///
/// The scope is unknown; the message lists the valid ones.
pub fn require_scope(scope: &str, valid_scopes: &[String]) -> Result<()> {
    if valid_scopes.iter().any(|name| name == scope) {
        Ok(())
    } else {
        Err(unknown_scope(scope, valid_scopes))
    }
}

/// The stages `scope` executes, in numeric order.
///
/// Numeric order is a topological order of the whole graph (the compile
/// guarantees it), and therefore of any subset, so nothing is sorted by
/// dependency here. A valid scope with no column executes nothing.
///
/// # Errors
///
/// `scope` is not a valid scope.
pub fn subgraph_for_scope<'a>(
    scope: &str,
    valid_scopes: &[String],
    grid: &Map<String, Value>,
    graph: &'a [Stage],
) -> Result<Vec<&'a Stage>> {
    if !valid_scopes.iter().any(|name| name == scope) {
        return Err(unknown_scope(scope, valid_scopes));
    }
    let executes = |slug: &str| {
        column(grid, scope)
            .and_then(|cells| cells.get(slug))
            .is_some_and(|action| action == "EXECUTE")
    };
    let mut stages: Vec<&Stage> = graph.iter().filter(|s| executes(&s.slug)).collect();
    stages.sort_by(|a, b| numeric_stage_order(&a.number, &b.number));
    Ok(stages)
}

/// One step of a scope's plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub slug: String,
    pub phase: String,
    pub execute: bool,
}

/// A scope's plan: every stage of the graph in numeric order, each marked
/// EXECUTE or SKIP.
///
/// # Errors
///
/// `scope` is not a valid scope.
pub fn resolve_plan_for_scope(
    scope: &str,
    valid_scopes: &[String],
    grid: &Map<String, Value>,
    graph: &[Stage],
) -> Result<Vec<PlanStep>> {
    if !valid_scopes.iter().any(|name| name == scope) {
        return Err(unknown_scope(scope, valid_scopes));
    }
    let cells = column(grid, scope);
    let mut stages: Vec<&Stage> = graph.iter().collect();
    stages.sort_by(|a, b| numeric_stage_order(&a.number, &b.number));
    Ok(stages
        .into_iter()
        .map(|stage| PlanStep {
            slug: stage.slug.clone(),
            phase: stage.phase.clone(),
            execute: cells
                .and_then(|cells| cells.get(&stage.slug))
                .is_some_and(|action| action == "EXECUTE"),
        })
        .collect())
}

/// How far a grid is from one stock scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nearest {
    pub scope: String,
    /// How many stages differ.
    pub diff: usize,
    /// Which ones, in the stock column's order and then the grid's.
    pub differs: Vec<String>,
}

impl Nearest {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("scope".to_owned(), Value::from(self.scope.clone()));
        out.insert("diff".to_owned(), Value::from(self.diff));
        out.insert(
            "differs".to_owned(),
            Value::Array(self.differs.iter().cloned().map(Value::String).collect()),
        );
        Value::Object(out)
    }
}

/// Rank every stock scope by its distance from `proposal` (stage slug to
/// action): nearest first, ties by name.
///
/// A stock scope is one some stage declares in its `scopes:`. Composed scopes
/// sit in the same grid but are never candidates: a plan someone saved must
/// not become the "nearest standard plan" for unrelated later work. Distance
/// counts the union of both sides' stages, so a stage missing from one side
/// is a difference, never an invisible overlap.
#[must_use]
pub fn nearest_stock_scopes(
    proposal: &Map<String, Value>,
    graph: &[Stage],
    grid: &Map<String, Value>,
) -> Vec<Nearest> {
    let is_stock = |scope: &str| {
        graph
            .iter()
            .any(|stage| stage.scopes.iter().any(|s| s == scope))
    };
    let mut ranked: Vec<Nearest> = grid
        .iter()
        .filter(|(scope, _)| is_stock(scope))
        .map(|(scope, entry)| {
            let empty = Map::new();
            let cells = entry
                .get("stages")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            let mut slugs: Vec<&String> = cells.keys().collect();
            for slug in proposal.keys() {
                if !cells.contains_key(slug) {
                    slugs.push(slug);
                }
            }
            let differs: Vec<String> = slugs
                .into_iter()
                .filter(|slug| proposal.get(*slug) != cells.get(*slug))
                .cloned()
                .collect();
            Nearest {
                scope: scope.clone(),
                diff: differs.len(),
                differs,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        a.diff
            .cmp(&b.diff)
            .then_with(|| locale_compare(&a.scope, &b.scope))
    });
    ranked
}

/// The ceremony a grid carries: how many stages, gates and per-unit stages.
/// Every line a person reads about a plan's size comes from these numbers, so
/// the composer relays the validator's count and never recounts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostSummary {
    pub total: usize,
    pub execute: usize,
    pub skip: usize,
    /// EXECUTE stages outside initialization: each ends at an approval gate.
    pub gates: usize,
    /// EXECUTE stages outside initialization: the stages a run shows.
    pub shown: usize,
    /// EXECUTE stages that repeat per unit of work. Zero unless
    /// `units-generation` executes, because without it there are no units.
    pub per_unit_stages: usize,
    /// The ceremonies the plan's settings switch off, as labels.
    pub off: Vec<String>,
}

impl CostSummary {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("total".to_owned(), Value::from(self.total));
        out.insert("execute".to_owned(), Value::from(self.execute));
        out.insert("skip".to_owned(), Value::from(self.skip));
        out.insert("gates".to_owned(), Value::from(self.gates));
        out.insert("shown".to_owned(), Value::from(self.shown));
        out.insert(
            "perUnitStages".to_owned(),
            Value::from(self.per_unit_stages),
        );
        out.insert(
            "off".to_owned(),
            Value::Array(self.off.iter().cloned().map(Value::String).collect()),
        );
        Value::Object(out)
    }
}

/// Count the ceremony of `proposal`. A slug the graph does not know counts
/// towards the totals only.
#[must_use]
pub fn grid_cost_summary(proposal: &Map<String, Value>, graph: &[Stage]) -> CostSummary {
    let has_units = proposal
        .get("units-generation")
        .is_some_and(|a| a == "EXECUTE");
    let mut summary = CostSummary {
        total: proposal.len(),
        execute: 0,
        skip: 0,
        gates: 0,
        shown: 0,
        per_unit_stages: 0,
        off: Vec::new(),
    };
    for (slug, action) in proposal {
        if action != "EXECUTE" {
            continue;
        }
        summary.execute += 1;
        let Some(stage) = graph.iter().find(|stage| stage.slug == *slug) else {
            continue;
        };
        if stage.phase != "initialization" {
            summary.gates += 1;
            summary.shown += 1;
        }
        if has_units && is_per_unit(stage) {
            summary.per_unit_stages += 1;
        }
    }
    summary.skip = summary.total - summary.execute;
    summary
}

/// What validating a grid found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridValidation {
    /// Hard problems: the plan cannot run as proposed.
    pub errors: Vec<String>,
    /// Things the plan's author is responsible for.
    pub advisories: Vec<String>,
    pub summary: CostSummary,
    pub nearest_stock: Vec<Nearest>,
}

/// How to validate a grid.
#[derive(Debug, Clone, Copy, Default)]
pub struct GridOptions<'a> {
    /// `brownfield` or `greenfield`: inputs that apply only to the other kind
    /// of project are not checked.
    pub project_type: Option<&'a str>,
    /// Reject a required input whose producer is not in the plan, instead of
    /// advising. Used when reshaping work already in flight, where nobody
    /// vouches for the skipped upstream stage.
    pub strict: bool,
    /// What to call the grid in messages.
    pub label: Option<&'a str>,
}

/// Validate an arbitrary grid (stage slug to action) against the graph.
///
/// Errors, in both modes: a slug the graph does not know (a typo must never
/// pass as an implicit SKIP), an action that is neither EXECUTE nor SKIP, a
/// compiled stage the grid leaves out, and a required input no stage in the
/// graph produces. A required input whose producer exists but is not in the
/// plan is an advisory, or an error under `strict`. An optional input is
/// never reported.
#[must_use]
pub fn validate_grid(
    proposal: &Map<String, Value>,
    options: GridOptions<'_>,
    graph: &[Stage],
    grid: &Map<String, Value>,
) -> GridValidation {
    let label = options.label.unwrap_or("proposed grid");
    let known = |slug: &str| graph.iter().any(|stage| stage.slug == slug);
    let mut errors: Vec<String> = Vec::new();
    let mut advisories: Vec<String> = Vec::new();

    for (slug, action) in proposal {
        if !known(slug) {
            errors.push(format!(
                "Grid names unknown stage \"{slug}\" - not in the compiled stage graph."
            ));
        }
        if action != "EXECUTE" && action != "SKIP" {
            errors.push(format!(
                "Grid entry \"{slug}\" has invalid action \"{}\" (expected EXECUTE or SKIP).",
                action.as_str().unwrap_or_default()
            ));
        }
    }
    let missing: Vec<&str> = graph
        .iter()
        .map(|stage| stage.slug.as_str())
        .filter(|slug| !proposal.contains_key(*slug))
        .collect();
    if !missing.is_empty() {
        errors.push(format!(
            "Grid is missing {} compiled stage entr{}: {}. Every compiled stage must be explicitly EXECUTE or SKIP.",
            missing.len(),
            if missing.len() == 1 { "y" } else { "ies" },
            missing.join(", ")
        ));
    }

    let on_path = |slug: &str| proposal.get(slug).is_some_and(|action| action == "EXECUTE");
    let mut planned: Vec<&Stage> = graph.iter().filter(|stage| on_path(&stage.slug)).collect();
    planned.sort_by(|a, b| numeric_stage_order(&a.number, &b.number));

    for stage in planned {
        for consume in stage.consumes.iter().filter(|c| c.required) {
            if let (Some(on), Some(project_type)) = (&consume.conditional_on, options.project_type)
                && on != project_type
            {
                continue;
            }
            let producers = producers_of(graph, &consume.artifact);
            if producers.is_empty() {
                errors.push(format!(
                    "Stage \"{}\" requires artifact \"{}\" but no stage in the graph produces it.",
                    stage.slug, consume.artifact
                ));
                continue;
            }
            if producers.iter().any(|p| on_path(&p.slug)) {
                continue;
            }
            let slugs: Vec<&str> = producers.iter().map(|p| p.slug.as_str()).collect();
            let message = format!(
                "Stage \"{}\" requires artifact \"{}\" whose producer(s) [{}] are not on the \"{label}\" path.",
                stage.slug,
                consume.artifact,
                slugs.join(", ")
            );
            if options.strict {
                errors.push(format!(
                    "{message} Strict (recompose) mode rejects a starved required input."
                ));
            } else {
                advisories.push(format!("{message} Ensure existing artifact is current."));
            }
        }
    }

    // Creating work on a greenfield project skips reverse-engineering, so a
    // greenfield count leaves it out too and matches what creation prints.
    let summary = if options.project_type == Some("greenfield")
        && proposal
            .get("reverse-engineering")
            .is_some_and(|a| a == "EXECUTE")
    {
        let mut adjusted = proposal.clone();
        adjusted.insert("reverse-engineering".to_owned(), Value::from("SKIP"));
        grid_cost_summary(&adjusted, graph)
    } else {
        grid_cost_summary(proposal, graph)
    };

    GridValidation {
        errors,
        advisories,
        summary,
        nearest_stock: nearest_stock_scopes(proposal, graph, grid),
    }
}

/// Validate a named scope: its own column, checked leniently.
///
/// # Errors
///
/// `scope` is not a valid scope.
pub fn validate_scope(
    scope: &str,
    project_type: Option<&str>,
    valid_scopes: &[String],
    graph: &[Stage],
    grid: &Map<String, Value>,
) -> Result<GridValidation> {
    let subgraph = subgraph_for_scope(scope, valid_scopes, grid, graph)?;
    let mut proposal: Map<String, Value> = graph
        .iter()
        .map(|stage| (stage.slug.clone(), Value::from("SKIP")))
        .collect();
    for stage in subgraph {
        proposal.insert(stage.slug.clone(), Value::from("EXECUTE"));
    }
    Ok(validate_grid(
        &proposal,
        GridOptions {
            project_type,
            strict: false,
            label: Some(scope),
        },
        graph,
        grid,
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn stage(slug: &str, number: &str, requires: &[&str]) -> Stage {
        let rows = crate::runtime::load_from_value(&json!([{
            "slug": slug, "number": number, "phase": "construction", "requires_stage": requires,
        }]));
        rows.into_iter().next().unwrap()
    }

    fn refs(graph: &[Stage]) -> Vec<&Stage> {
        graph.iter().collect()
    }

    #[test]
    fn topo_prefers_lower_numbers_among_ready_stages() {
        let graph = [
            stage("c", "1.3", &["a"]),
            stage("b", "1.2", &[]),
            stage("a", "1.1", &[]),
            stage("d", "1.10", &["b", "c"]),
        ];
        assert_eq!(topo_sort(&refs(&graph)).unwrap(), ["a", "b", "c", "d"]);
    }

    #[test]
    fn topo_ignores_edges_leaving_the_subset_and_reports_cycles() {
        let subset = [stage("b", "1.2", &["outside"])];
        assert_eq!(topo_sort(&refs(&subset)).unwrap(), ["b"]);
        let cyclic = [
            stage("a", "1.1", &["b"]),
            stage("b", "1.2", &["a"]),
            stage("c", "1.3", &[]),
        ];
        assert_eq!(
            topo_sort(&refs(&cyclic)).unwrap_err().message(),
            "topoSort: cycle detected. Processed 1 of 3 nodes. Use findCycles() to enumerate."
        );
    }

    #[test]
    fn cycles_are_strongly_connected_groups_and_self_loops() {
        let graph = [
            stage("a", "1.1", &["b"]),
            stage("b", "1.2", &["c"]),
            stage("c", "1.3", &["a"]),
            stage("solo", "1.4", &["solo", "outside"]),
            stage("free", "1.5", &["a"]),
        ];
        // Captured from upstream's findCycles on the same graph.
        assert_eq!(
            find_cycles(&refs(&graph)),
            [vec!["c", "b", "a"], vec!["solo"]]
        );
        assert!(find_cycles(&refs(&graph[4..])).is_empty());
    }
}

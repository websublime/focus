//! The plan a new piece of work runs: its scope's grid, with the stage
//! changes composed for it, and what follows from it.
//!
//! Mirrors `splitSlugList`, `planWithChanges`, `composedPlanLabel`,
//! `PLAN_NAME_PATTERN`, `nextInScopeStage` and `stagesInScope` in
//! `aidlc-lib.ts`, and `phasesWithExecuteStages` and
//! `determineFirstPostInitStage` in `aidlc-utility.ts`.

use focus_graph::runtime::{ScopeDefinition, Stage, column};
use focus_js::re;
use focus_js::text::trim;
use focus_state::progress::{CheckboxState, parse_checkboxes, parse_stage_suffixes};
use serde_json::{Map, Value};

/// The five phases, in order.
pub const PHASES: [&str; 5] = [
    "initialization",
    "ideation",
    "inception",
    "construction",
    "operation",
];

/// The state field that marks a workflow running a plan composed for it.
pub const PLAN_FIELD: &str = "Plan";

/// A stage's action in a plan.
pub type Action = &'static str;

/// A plan: each stage's action, in graph order.
pub type Plan = Vec<(String, Action)>;

/// The action `plan` gives `slug`; a stage the plan does not name is skipped.
#[must_use]
pub fn action_of(plan: &[(String, Action)], slug: &str) -> Action {
    plan.iter()
        .find(|(known, _)| known == slug)
        .map_or("SKIP", |(_, action)| action)
}

/// The action a scope's grid column gives `slug`: `EXECUTE` when it says so.
#[must_use]
pub fn grid_action(definition: &ScopeDefinition, slug: &str) -> Action {
    if definition.stages.get(slug).and_then(Value::as_str) == Some("EXECUTE") {
        "EXECUTE"
    } else {
        "SKIP"
    }
}

/// `splitSlugList`: `a, b,,c` as `[a, b, c]`.
#[must_use]
pub fn split_slug_list(raw: Option<&str>) -> Vec<String> {
    raw.map(|raw| {
        raw.split(',')
            .map(trim)
            .filter(|slug| !slug.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

/// The pattern a tailored plan's name matches.
const PLAN_NAME_PATTERN: &str = r"^[a-z0-9][a-z0-9-]{0,62}$";

/// `PLAN_NAME_PATTERN.test(name)`.
#[must_use]
pub fn is_plan_name(name: &str) -> bool {
    re::test(PLAN_NAME_PATTERN, "", name)
}

/// `composedPlanLabel`: the Plan field of a plan tailored to this work: its
/// name, or `tailored plan`.
#[must_use]
pub fn composed_plan_label(name: Option<&str>) -> String {
    match name {
        Some(name) if is_plan_name(name) => name.to_owned(),
        _ => "tailored plan".to_owned(),
    }
}

/// The stage changes asked for: slugs to skip and slugs to add.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub skip: Vec<String>,
    pub add: Vec<String>,
}

impl Changes {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.skip.is_empty() && self.add.is_empty()
    }
}

/// `planWithChanges`: `scope`'s grid with `changes` applied, and every
/// reason a change is refused: a slug that is not a stage, an initialization
/// stage (those always run), a stage named on both lists, and a change the
/// scope already makes, so a typo never passes as a no-op.
#[must_use]
pub fn plan_with_changes(
    definition: &ScopeDefinition,
    graph: &[Stage],
    changes: &Changes,
) -> (Plan, Vec<String>) {
    let scope = &definition.meta.name;
    let mut plan: Plan = graph
        .iter()
        .map(|stage| (stage.slug.clone(), grid_action(definition, &stage.slug)))
        .collect();
    let mut errors = Vec::new();
    let both: Vec<&String> = changes
        .skip
        .iter()
        .filter(|slug| changes.add.contains(slug))
        .collect();
    let mut seen: Vec<&String> = Vec::new();
    for slug in &both {
        if !seen.contains(slug) {
            seen.push(slug);
            errors.push(format!("\"{slug}\" is named by both --skip and --add."));
        }
    }
    for (flag, list, to) in [
        ("--skip", &changes.skip, "SKIP"),
        ("--add", &changes.add, "EXECUTE"),
    ] {
        for slug in list {
            if both.contains(&slug) {
                continue;
            }
            let Some(stage) = graph.iter().find(|stage| stage.slug == *slug) else {
                errors.push(format!("{flag} names \"{slug}\", which is not a stage."));
                continue;
            };
            if stage.phase == "initialization" {
                errors.push(format!(
                    "{flag} names \"{slug}\", an initialization stage; those always run."
                ));
            } else if action_of(&plan, slug) == to {
                errors.push(format!(
                    "{flag} names \"{slug}\", which scope {scope} already {}.",
                    if to == "SKIP" { "skips" } else { "runs" }
                ));
            } else if let Some(known) = plan.iter_mut().find(|(known, _)| known == slug) {
                known.1 = to;
            }
        }
    }
    (plan, errors)
}

/// `phasesWithExecuteStages`: the phases `plan` runs, those holding at
/// least one EXECUTE stage, in phase order.
#[must_use]
pub fn phases_with_execute_stages(plan: &[(String, Action)], graph: &[Stage]) -> Vec<&'static str> {
    PHASES
        .into_iter()
        .filter(|phase| {
            graph
                .iter()
                .any(|stage| stage.phase == *phase && action_of(plan, &stage.slug) == "EXECUTE")
        })
        .collect()
}

/// `determineFirstPostInitStage`: the first stage past initialization the
/// plan runs; `intent-capture` when it runs none.
#[must_use]
pub fn first_post_init_stage(plan: &[(String, Action)], graph: &[Stage]) -> String {
    graph
        .iter()
        .filter(|stage| stage.phase != "initialization")
        .find(|stage| action_of(plan, &stage.slug) == "EXECUTE")
        .map_or_else(|| "intent-capture".to_owned(), |stage| stage.slug.clone())
}

/// `nextInScopeStage`: the stage after `after_slug` that the workflow runs,
/// walking the graph forward: a stage the state has completed or skipped is
/// passed; the state's own action for a stage wins over the scope's grid.
/// `None` when `after_slug` is not a stage or nothing follows.
#[must_use]
pub fn next_in_scope_stage<'a>(
    after_slug: &str,
    definition: &ScopeDefinition,
    graph: &'a [Stage],
    state: Option<&str>,
) -> Option<&'a Stage> {
    let overrides = state.map(parse_stage_suffixes);
    let boxes = state.map(parse_checkboxes).unwrap_or_default();
    let current = graph.iter().position(|stage| stage.slug == after_slug)?;
    graph[current + 1..].iter().find(|stage| {
        if boxes.iter().any(|line| {
            line.slug == stage.slug
                && matches!(
                    line.state,
                    CheckboxState::Completed | CheckboxState::Skipped
                )
        }) {
            return false;
        }
        let action = overrides
            .as_ref()
            .and_then(|actions| {
                actions
                    .iter()
                    .find(|(slug, _)| *slug == stage.slug)
                    .map(|(_, action)| *action)
            })
            .or_else(|| {
                definition
                    .stages
                    .get(&stage.slug)
                    .and_then(Value::as_str)
                    .map(|action| {
                        if action == "EXECUTE" {
                            "EXECUTE"
                        } else {
                            "SKIP"
                        }
                    })
            });
        action == Some("EXECUTE")
    })
}

/// `stagesInScope`: every stage of the graph with `EXECUTE` when the scope's
/// column of the compiled grid runs it.
#[must_use]
pub fn stages_in_scope(grid: &Map<String, Value>, scope: &str, graph: &[Stage]) -> Plan {
    let cells = column(grid, scope);
    graph
        .iter()
        .map(|stage| {
            let runs = cells
                .and_then(|cells| cells.get(&stage.slug))
                .is_some_and(|action| action == "EXECUTE");
            (stage.slug.clone(), if runs { "EXECUTE" } else { "SKIP" })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_lists_and_plan_names_are_read_as_upstream_reads_them() {
        assert_eq!(split_slug_list(Some(" a, b,,c ")), ["a", "b", "c"]);
        assert_eq!(split_slug_list(None), Vec::<String>::new());
        assert_eq!(split_slug_list(Some(",")), Vec::<String>::new());
        assert!(is_plan_name("lean-login"));
        assert!(is_plan_name("9"));
        assert!(!is_plan_name("Lean"));
        assert!(!is_plan_name("-a"));
        assert!(!is_plan_name(&"a".repeat(64)));
        assert!(is_plan_name(&"a".repeat(63)));
        assert_eq!(composed_plan_label(Some("lean-login")), "lean-login");
        assert_eq!(composed_plan_label(Some("Lean Login")), "tailored plan");
        assert_eq!(composed_plan_label(None), "tailored plan");
    }

    fn graph() -> Vec<Stage> {
        focus_graph::runtime::load_from_value(&serde_json::json!([
            {"slug": "workspace-scaffold", "number": "0.1", "phase": "initialization"},
            {"slug": "intent-capture", "number": "1.1", "phase": "ideation"},
            {"slug": "market-research", "number": "1.2", "phase": "ideation"},
            {"slug": "feasibility", "number": "1.3", "phase": "ideation"},
            {"slug": "code-generation", "number": "3.5", "phase": "construction"},
        ]))
    }

    fn definition(stages: &[(&str, &str)]) -> ScopeDefinition {
        ScopeDefinition {
            meta: focus_graph::scopes::ScopeMetadata {
                name: "feature".to_owned(),
                plugin: None,
                depth: "Standard".to_owned(),
                description: String::new(),
                keywords: Vec::new(),
                test_strategy: None,
                runner: None,
                skeleton: false,
                existing_code: false,
                review_cap: None,
                freeform_default: false,
                guard_policy: None,
                ceremony: Vec::new(),
            },
            stages: stages
                .iter()
                .map(|(slug, action)| ((*slug).to_owned(), Value::from(*action)))
                .collect(),
        }
    }

    #[test]
    fn the_next_stage_follows_the_state_over_the_grid() {
        let graph = graph();
        let def = definition(&[
            ("intent-capture", "EXECUTE"),
            ("market-research", "EXECUTE"),
            ("code-generation", "EXECUTE"),
        ]);
        let next = |after: &str, state: Option<&str>| {
            next_in_scope_stage(after, &def, &graph, state).map(|stage| stage.slug.as_str())
        };
        assert_eq!(next("intent-capture", None), Some("market-research"));
        // A completed or skipped stage is passed; a state's SKIP drops a
        // stage and its EXECUTE promotes one.
        let state = "- [x] market-research \u{2014} EXECUTE\n- [ ] feasibility \u{2014} EXECUTE\n";
        assert_eq!(next("intent-capture", Some(state)), Some("feasibility"));
        let state = "- [S] market-research \u{2014} EXECUTE\n- [ ] code-generation \u{2014} SKIP\n";
        assert_eq!(next("intent-capture", Some(state)), None);
        assert_eq!(next("nope", None), None);
        assert_eq!(next("code-generation", None), None);
        assert_eq!(
            first_post_init_stage(
                &stages_in_scope(&serde_json::Map::new(), "feature", &graph),
                &graph
            ),
            "intent-capture"
        );
        let (plan, errors) = plan_with_changes(
            &def,
            &graph,
            &Changes {
                skip: vec!["market-research".into()],
                add: vec!["feasibility".into()],
            },
        );
        assert!(errors.is_empty());
        assert_eq!(first_post_init_stage(&plan, &graph), "intent-capture");
        assert_eq!(
            phases_with_execute_stages(&plan, &graph),
            ["ideation", "construction"]
        );
    }
}

//! `focus engine state lookup`: questions about the stage graph and the
//! scopes of the install, answered without touching the work
//! (`handleLookup`, `aidlc-state.ts` L8 353).
//!
//! Each answer is one line: a word, or a JSON document. Only `next-stage`
//! reads the state file, and only to walk past what the work has finished
//! or dropped; with no state it answers from the scope alone.

use focus_graph::query::subgraph_for_scope;
use focus_graph::stage_schema::PHASES;
use focus_intent::plan::{next_in_scope_stage, stages_in_scope};
use focus_js::{EngineError, Result, json};
use serde_json::{Value, json as value};

use crate::jump::{PHASE_NUMBERS, resolve_stage};
use crate::stage;
use crate::workflow::Workflow;

const VALID: &str = "phase-of, next-stage, agent-for, number-of, stages-in-scope, first-in-phase, validate-stage, validate-phase";

fn usage(of: &str) -> EngineError {
    EngineError::new(format!("Usage: lookup {of}"))
}

/// `handleLookup`.
///
/// # Errors
///
/// A subcommand that is none, one with too few arguments, a stage or a
/// scope the install does not have.
pub fn lookup(wf: &Workflow, args: &[String]) -> Result<String> {
    let Some(sub) = args.first() else {
        return Err(EngineError::new(
            "Usage: focus engine state lookup <subcommand> [args...]",
        ));
    };
    let args = &args[1..];
    let first = |of: &str| args.first().ok_or_else(|| usage(of));
    let known = |asked: &String| {
        resolve_stage(wf, asked).ok_or_else(|| EngineError::new(format!("Unknown stage: {asked}")))
    };
    let install = &wf.install;
    match sub.as_str() {
        "phase-of" => Ok(known(first("phase-of <slug>")?)?.phase.clone()),
        "agent-for" => Ok(stage::lead_agent(known(first("agent-for <slug>")?)?).to_owned()),
        "number-of" => Ok(known(first("number-of <slug>")?)?.number.clone()),
        "next-stage" => {
            let [slug, scope, ..] = args else {
                return Err(usage("next-stage <slug> <scope>"));
            };
            // The state, when there is one, so the walk passes what the work
            // has completed or skipped and honours a recomposed plan.
            let state = wf.read_state().ok();
            let next = install.definition(scope).and_then(|definition| {
                next_in_scope_stage(slug, definition, &install.graph, state.as_deref())
            });
            Ok(next.map_or("none", |stage| stage.slug.as_str()).to_owned())
        }
        "stages-in-scope" => {
            let scope = first("stages-in-scope <scope>")?;
            if install.definition(scope).is_none() {
                return Err(EngineError::new(format!("Unknown scope: {scope}")));
            }
            let stages: Vec<Value> = stages_in_scope(&install.grid, scope, &install.graph)
                .iter()
                .zip(&install.graph)
                .map(|((slug, action), stage)| {
                    value!({ "slug": slug, "phase": stage.phase, "action": action })
                })
                .collect();
            Ok(json::stringify(&Value::Array(stages)))
        }
        "first-in-phase" => {
            let [phase, scope, ..] = args else {
                return Err(usage("first-in-phase <phase> <scope>"));
            };
            let phase = phase.to_lowercase();
            // (A scope the install does not have has no stages to walk.)
            let found =
                subgraph_for_scope(scope, &install.valid_scopes, &install.grid, &install.graph)
                    .ok()
                    .and_then(|path| path.into_iter().find(|stage| stage.phase == phase));
            Ok(found.map_or("none", |stage| stage.slug.as_str()).to_owned())
        }
        "validate-stage" => {
            let asked = first("validate-stage <slug-or-number>")?;
            Ok(json::stringify(&match resolve_stage(wf, asked) {
                None => value!({ "valid": false, "input": asked }),
                Some(stage) => value!({
                    "valid": true,
                    "slug": stage.slug,
                    "number": stage.number,
                    "name": stage::name(stage),
                    "phase": stage.phase,
                    "lead_agent": stage::lead_agent(stage),
                }),
            }))
        }
        "validate-phase" => {
            let asked = first("validate-phase <phase-or-number>")?;
            let input = asked.to_lowercase();
            let by_number = PHASE_NUMBERS.iter().find(|(number, _)| *number == input);
            let phase = by_number
                .map(|(_, phase)| *phase)
                .or_else(|| PHASES.iter().copied().find(|phase| *phase == input));
            Ok(json::stringify(&match phase {
                None => value!({ "valid": false, "input": asked }),
                Some(phase) => {
                    let number = PHASE_NUMBERS
                        .iter()
                        .find(|(_, of)| *of == phase)
                        .map(|(number, _)| *number);
                    value!({
                        "valid": true,
                        "canonical": phase,
                        "number": number,
                        "display": phase.to_uppercase(),
                    })
                }
            }))
        }
        other => Err(EngineError::new(format!(
            "Unknown lookup subcommand: {other}. Valid: {VALID}"
        ))),
    }
}

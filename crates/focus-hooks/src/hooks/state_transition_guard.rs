//! PreToolUse: the state-transition fence. A conductor may not call the
//! state tool's transitions directly (the engine owns stage pinning,
//! evidence checks and transition selection), and a delegated agent may not
//! change stage status or routing at all. Mirrors
//! `aidlc-state-transition-guard.ts`; the readers of the command are
//! `focus_guards::state_transition`.
//!
//! The harness trust boundary comes first and consults nothing: a tool call
//! that would invoke a hook, edit a runtime record or write the audit trail
//! is refused before the fence is read. The fence then stands aside when it
//! is lowered for this piece of work (the guard policy word, or the person's
//! own `guard.state-transition off` switch), with one line and one audit
//! row instead of a refusal; a conversation that has not joined the
//! workflow keeps the fence up, and a main session outside the workflow is
//! left to the state tool's own check.

use focus_guards::Fence;
use focus_guards::authority::Decision;
use focus_guards::integrity::{self, Integrity};
use focus_guards::state_transition::{delegated_lifecycle_command, direct_state_transition};
use focus_guards::workflow::{Guards, fence_switch_sentence};
use focus_js::text::trim;
use focus_workspace::Layout;
use focus_workspace::records::Selector;
use serde_json::Value;

use super::{is_refusal, open, record_stood_aside, refused, stood_aside_output};
use crate::health::cursor_selection;
use crate::{Outcome, context, payload};

const HOOK: &str = "state-transition-guard";

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let Some(parsed) = payload::parse(input) else {
        return Outcome::exit(0);
    };
    // The harness trust boundary, not a fence: never consult policy, memory,
    // session presence or a bypass before enforcing it.
    if let Some(violation) =
        integrity::violation(&Integrity::current(), &Value::Object(parsed.clone()))
    {
        return Outcome {
            code: 2,
            stdout: String::new(),
            stderr: violation.refusal(),
        };
    }
    if payload::text(Some(&parsed), "tool_name") != "Bash" {
        return Outcome::exit(0);
    }
    let tool_input = parsed.get("tool_input").and_then(Value::as_object);
    let agent_type = match payload::text_field(Some(&parsed), "agent_type", HOOK) {
        Ok(agent) => trim(agent).to_owned(),
        Err(err) => return refused(&err),
    };
    let subagent_type = match payload::text_field(tool_input, "subagent_type", HOOK) {
        Ok(agent) => trim(agent).to_owned(),
        Err(err) => return refused(&err),
    };
    let agent = if agent_type.is_empty() {
        subagent_type
    } else {
        agent_type
    };
    let command = match payload::text_field(tool_input, "command", HOOK) {
        Ok(command) => command.to_owned(),
        Err(err) => return refused(&err),
    };
    let session = payload::text(Some(&parsed), "session_id").to_owned();
    let session = (!session.is_empty()).then_some(session);
    let actor = Guards::actor_from(
        parsed.get("agent_type").and_then(Value::as_str),
        tool_input
            .and_then(|input| input.get("subagent_type"))
            .and_then(Value::as_str),
    );

    if let Some(verb) = direct_state_transition(&command) {
        let detail = format!("focus-state {verb}");
        match stand_aside(layout, session.as_deref(), actor, &detail) {
            Aside::Stood(outcome) | Aside::Refused(outcome) => return outcome,
            Aside::Held => {}
        }
        // A conversation that has not joined the workflow: the state tool's
        // own check reads the same Guard Policy for the work it changes, and
        // refuses with the same words where the check holds, so this hook
        // does not stop it with an offer to turn off a check that may
        // already be off.
        let workflow = match context::enter(layout, session.as_deref()) {
            Ok(workflow) => workflow,
            Err(err) => return refused(&err),
        };
        if agent.is_empty() && workflow.outside_gate() {
            return Outcome::exit(0);
        }
        let switch = if agent.is_empty() {
            let guards = Guards::read(layout, &Selector::default(), &workflow.invoker);
            if let Err(err) = &guards
                && is_refusal(err)
            {
                return refused(err);
            }
            fence_switch_sentence(Fence::StateTransition, &guards)
        } else {
            String::new()
        };
        return Outcome {
            code: 2,
            stdout: String::new(),
            stderr: format!(
                "Stage status cannot be changed with `focus engine state {verb}` because that bypasses the workflow's completion and approval checks. Use `focus engine orchestrate report` --stage <slug> --result <awaiting-approval|approved|rejected|revised|completed|skipped>; use `focus engine orchestrate park` to pause, and next/jump to move through the workflow. {switch}\n"
            ),
        };
    }

    if agent.is_empty() {
        return Outcome::exit(0);
    }
    let Some(delegated) = delegated_lifecycle_command(&command) else {
        return Outcome::exit(0);
    };
    match stand_aside(layout, session.as_deref(), actor, &delegated) {
        Aside::Stood(outcome) | Aside::Refused(outcome) => return outcome,
        Aside::Held => {}
    }
    Outcome {
        code: 2,
        stdout: String::new(),
        stderr: format!(
            "Delegated agent \"{agent}\" cannot run {delegated} because only the main workflow session can change stage status or routing. Return the artifact, contribution, or review verdict to the main session without parking, resuming, reporting, routing, or presenting an approval question.\n"
        ),
    }
}

enum Aside {
    /// The fence stood aside: the hook's whole outcome (exit 0).
    Stood(Outcome),
    /// Input Focus does not port yet, said by name.
    Refused(Outcome),
    /// The fence stays up.
    Held,
}

/// `standAside`: whether the fence is lowered for this piece of work. A
/// lowered fence belongs to the workflow that lowered it, so a
/// conversation that has not joined that workflow keeps the fence up; an
/// unreadable policy is the strictest policy.
fn stand_aside(
    layout: &Layout,
    session: Option<&str>,
    actor: focus_guards::authority::Actor,
    detail: &str,
) -> Aside {
    let workflow = match context::enter(layout, session) {
        Ok(workflow) => workflow,
        Err(err) => return Aside::Refused(refused(&err)),
    };
    if workflow.outside_gate() {
        return Aside::Held;
    }
    let decided = match Guards::read(layout, &Selector::default(), &workflow.invoker) {
        Err(err) if is_refusal(&err) => return Aside::Refused(refused(&err)),
        Err(_) => return Aside::Held,
        Ok(guards) => guards.decide_for(Fence::StateTransition, actor),
    };
    if decided.decision != Decision::StandAside {
        return Aside::Held;
    }
    let stdout = stood_aside_output(layout, &decided, detail);
    if let Some(selection) = workflow
        .selection
        .clone()
        .or_else(|| cursor_selection(layout))
        && let Ok(wf) = open(layout, &selection, &workflow.invoker)
    {
        record_stood_aside(&wf, &decided, None, "Bash", detail);
    }
    Aside::Stood(Outcome {
        code: 0,
        stdout,
        stderr: String::new(),
    })
}

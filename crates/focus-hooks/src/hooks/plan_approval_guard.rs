//! PreToolUse: the Plan Approval guard. While Code Generation is the
//! current stage, nothing a plan governs is built or changed before the
//! person has approved the plan: the developer is not dispatched, the
//! project's files are not written, and a command whose writes cannot be
//! seen does not run. Mirrors `aidlc-plan-approval-guard.ts`; the readers
//! of the call are `focus_guards::mutation_intent`.
//!
//! What the plan never governs passes at any point and under every Guard
//! Policy: a read, a command the lexer read whole that names no file it
//! writes and is not the engine's own, a write outside the project, and a
//! file-tool write of the composer's proposal, a person's answer text or
//! the stage's own diary.
//!
//! Everything else is decided from the engine's last step (the
//! active-directive marker) and the approval behind it (the plan's
//! fingerprint and its receipt): M5's. Until then a call that reaches that
//! decision finds no current directive and is refused as upstream refuses
//! it without a marker, with the `next` that issues the step; a marker on
//! disk is refused by name. A lowered fence does not supply a missing
//! directive: it changes only the refusal's words.
//!
//! The harness trust boundary comes first and consults nothing. A
//! conversation that has not joined the workflow is not held to its Plan
//! Approval, but may not dispatch its developer. The off switch
//! `FOCUS_DISABLE_PLAN_APPROVAL_GUARD=1` lets every call through and says
//! so once in the ledger.

use std::time::SystemTime;

use focus_audit::read::block_field;
use focus_audit::time::iso_timestamp;
use focus_guards::Fence;
use focus_guards::authority::Decision;
use focus_guards::integrity::{self, Integrity};
use focus_guards::mutation_intent::{
    MutationIntent, PlanWait, Project, WRITE_TOOLS, code_generation_gate_held,
};
use focus_guards::plan_approval::{
    DISPATCH_TOOLS, GUARDED_AGENT, GUARDED_STAGE, NO_CURRENT_DIRECTIVE, authority_block_reason,
    authority_remedy, next_on_its_own, normalize_stage_name, prompt_stage_markers,
    prompt_unit_markers,
};
use focus_guards::posture::prompt_testing_contract_markers;
use focus_guards::shell::Host;
use focus_guards::workflow::Guards;
use focus_js::EngineError;
use focus_js::text::trim;
use focus_lifecycle::presence::person_spoke_since_gate;
use focus_lifecycle::workflow::Workflow;
use focus_workspace::Layout;
use focus_workspace::records::Selector;
use serde_json::{Map, Value};

use super::{append, is_refusal, open, refused};
use crate::engine::refuse_directive_marker;
use crate::health::{cursor_selection, hooks_health_dir, record_drop, write_status_file};
use crate::{Outcome, context, payload};

const HOOK: &str = "plan-approval-guard";

/// `SAFE_READ_TOOLS`: tools that change nothing.
const SAFE_READ_TOOLS: [&str; 12] = [
    "Read",
    "Glob",
    "Grep",
    "WebFetch",
    "WebSearch",
    "TodoRead",
    "TaskOutput",
    "AskUserQuestion",
    "fs_read",
    "file_search",
    "grep_search",
    "thinking",
];

fn refusal(text: String) -> Outcome {
    Outcome {
        code: 2,
        stdout: String::new(),
        stderr: text,
    }
}

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let Some(parsed) = payload::parse(input) else {
        return Outcome::exit(0);
    };
    let session = payload::text(Some(&parsed), "session_id").to_owned();
    let workflow = match context::enter(layout, (!session.is_empty()).then_some(&session)) {
        Ok(workflow) => workflow,
        Err(err) => return refused(&err),
    };
    // Runtime integrity is not a fence and cannot be disabled with this hook.
    if let Some(violation) =
        integrity::violation(&Integrity::current(), &Value::Object(parsed.clone()))
    {
        return refusal(violation.refusal());
    }
    let tool = payload::text(Some(&parsed), "tool_name").to_owned();
    let tool_input = parsed.get("tool_input").and_then(Value::as_object);
    let subagent = tool_input
        .and_then(|input| input.get("subagent_type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let guarded_dispatch = DISPATCH_TOOLS.contains(&tool.as_str()) && subagent == GUARDED_AGENT;

    // A conversation that has not joined the selected workflow is not held
    // to its Plan Approval: its ordinary edits pass as in a workspace with
    // no workflow. Dispatching that workflow's developer is joining it
    // without saying so. `next` asks which work this is, so the record's
    // name stays out of the refusal.
    if workflow.outside_gate() {
        if !guarded_dispatch {
            return Outcome::exit(0);
        }
        return refusal(format!(
            "AI-DLC: this conversation has not joined the selected workflow, so it cannot dispatch that workflow's developer. Run {}: it asks the person which piece of work this conversation is for. Dispatch again once it has joined.\n",
            next_on_its_own()
        ));
    }

    // The heartbeat says the host ran this hook, so it comes before the off
    // switch: a fence switched off never looks like a host running no hooks.
    let selection = workflow
        .selection
        .clone()
        .or_else(|| cursor_selection(layout));
    if let Some(selection) = &selection {
        write_status_file(
            &hooks_health_dir(layout, selection),
            "plan-approval-guard.last",
            &iso_timestamp(SystemTime::now()),
            false,
        );
    }
    // The deterministic off switch, recorded once; a settings file the
    // resolution refuses is upstream's thrown read, which ends the hook.
    match focus_workspace::settings::Places::of(&layout.project).flag(
        &focus_workspace::env::full_name("DISABLE_PLAN_APPROVAL_GUARD"),
        &|name| std::env::var(name).ok(),
    ) {
        Ok(Some(on)) if on == "1" => {
            if let Some(wf) = workflow
                .selection
                .as_ref()
                .and_then(|selection| open(layout, selection, &workflow.invoker).ok())
            {
                record_guard_disabled(&wf, &tool);
            }
            return Outcome::exit(0);
        }
        Ok(_) => {}
        Err(err) => {
            return Outcome {
                code: 1,
                stdout: String::new(),
                stderr: format!("focus: {err}\n"),
            };
        }
    }

    if SAFE_READ_TOOLS.contains(&tool.as_str()) {
        return Outcome::exit(0);
    }
    // Any tool with a name that is not a dispatch could change something:
    // the shell and the file tools among them.
    let mutation_capable = !DISPATCH_TOOLS.contains(&tool.as_str()) && !tool.is_empty();
    if !guarded_dispatch && !mutation_capable {
        return Outcome::exit(0);
    }
    let call = Call {
        parsed: &parsed,
        tool: &tool,
        tool_input,
        guarded_dispatch,
    };
    let failure = match evaluate(layout, &workflow, &call) {
        Ok(Verdict::Passes) => return Outcome::exit(0),
        Ok(Verdict::NoDirective) => NO_CURRENT_DIRECTIVE.to_owned(),
        Err(err) if is_refusal(&err) => return refused(&err),
        Err(err) => {
            if let Some(selection) = &selection {
                record_drop(layout, selection, HOOK, &err.to_string());
            }
            format!("Plan Approval authority evaluation failed closed: {err}")
        }
    };

    // The fence stands aside when it is lowered for this piece of work. A
    // lowered fence lets changed content through after an approval; it never
    // supplies a missing directive, so the refusal names the step that ends
    // it, with the fence on or off.
    let wf = workflow
        .selection
        .as_ref()
        .and_then(|selection| open(layout, selection, &workflow.invoker).ok());
    let actor = Guards::actor_from(
        parsed.get("agent_type").and_then(Value::as_str),
        tool_input
            .and_then(|input| input.get("subagent_type"))
            .and_then(Value::as_str),
    );
    match Guards::read(layout, &Selector::default(), &workflow.invoker) {
        Err(err) if is_refusal(&err) => return refused(&err),
        Err(err) => {
            if let Some(selection) = &selection {
                record_drop(layout, selection, HOOK, &err.to_string());
            }
        }
        Ok(guards) => {
            if guards.decide_for(Fence::PlanApproval, actor).decision == Decision::StandAside {
                return refusal(format!(
                    "Code Generation cannot start: {} The plan-approval setting is unchanged.\n",
                    one_sentence(&authority_remedy(&failure))
                ));
            }
        }
    }

    // The refusal goes in the run's record, so it shows when the ordering
    // bit. Best effort: the block is already decided.
    if let Some(wf) = &wf
        && wf
            .ledger()
            .audit_file(&wf.selector(), &wf.invoker)
            .is_ok_and(|shard| shard.exists())
    {
        let fields = [
            ("Tool", tool.clone()),
            (
                "Target",
                if guarded_dispatch {
                    subagent.clone()
                } else {
                    String::new()
                },
            ),
            ("Stage", GUARDED_STAGE.to_owned()),
            ("Unit", "(missing marker)".to_owned()),
        ];
        // A lock that cannot be had is a dropped row; a row that cannot be
        // written is advisory, and says nothing.
        match wf.lock("appending PLAN_APPROVAL_BLOCKED") {
            Ok(held) => {
                let _ = wf.emit_plain(&held, "PLAN_APPROVAL_BLOCKED", &fields);
            }
            Err(_) => {
                if let Some(selection) = &selection {
                    record_drop(
                        layout,
                        selection,
                        HOOK,
                        "audit lock contended; PLAN_APPROVAL_BLOCKED row dropped (block still enforced)",
                    );
                }
            }
        }
    }
    refusal(format!("{}\n", authority_block_reason(&failure)))
}

/// The tool call under judgement.
struct Call<'a> {
    parsed: &'a Map<String, Value>,
    tool: &'a str,
    tool_input: Option<&'a Map<String, Value>>,
    guarded_dispatch: bool,
}

enum Verdict {
    /// The plan does not govern this call.
    Passes,
    /// The call waits for the engine's step, and none is current.
    NoDirective,
}

/// The part of upstream's `evaluate` that reads the workflow: an error here
/// fails closed.
fn evaluate(
    layout: &Layout,
    workflow: &context::HookWorkflow,
    call: &Call<'_>,
) -> Result<Verdict, EngineError> {
    let invoker = &workflow.invoker;
    let selection = match &workflow.selection {
        Some(selection) => selection.clone(),
        // The resolution upstream's path helpers would throw again.
        None => focus_workspace::records::resolve_selection(layout, &Selector::default(), invoker)?,
    };
    let wf = open(layout, &selection, invoker)?;
    if !wf.state_path().exists() {
        // No workflow: fail open.
        return Ok(Verdict::Passes);
    }
    let state = wf.read_state()?;
    refuse_directive_marker(layout, &selection, HOOK)?;
    let durable_stage =
        normalize_stage_name(focus_state::get_field(&state, "Current Stage").unwrap_or_default());
    let dispatch_prompt = ["prompt", "description"]
        .iter()
        .filter_map(|key| call.tool_input?.get(*key)?.as_str())
        .collect::<Vec<&str>>()
        .join("\n");
    let explicit_plan_dispatch = !prompt_unit_markers(&dispatch_prompt).is_empty()
        || !prompt_stage_markers(&dispatch_prompt).is_empty()
        || !prompt_testing_contract_markers(&dispatch_prompt).is_empty();
    if durable_stage != GUARDED_STAGE && !(call.guarded_dispatch && explicit_plan_dispatch) {
        return Ok(Verdict::Passes);
    }
    if call.guarded_dispatch {
        return Ok(Verdict::NoDirective);
    }

    let host = Host::current();
    let said = |path: std::path::PathBuf| path.to_string_lossy().into_owned();
    let dir = said(layout.project.clone());
    let spaces_root = said(layout.workspace_root().join("spaces"));
    let docs_root = said(selection.docs_root(layout));
    let composer_proposal = said(layout.composer_proposal(&layout.active_space()));
    let project = Project {
        host: &host,
        dir: &dir,
        harness_dir: layout.harness.dir(),
        spaces_root: &spaces_root,
        docs_root: Some(&docs_root),
        composer_proposal: &composer_proposal,
    };
    let known = call.tool == "Bash" || WRITE_TOOLS.contains(&call.tool);
    let mutation = if known {
        let cwd = call
            .parsed
            .get("cwd")
            .and_then(Value::as_str)
            .unwrap_or(&dir);
        let wait = PlanWait {
            project,
            gate_held: code_generation_gate_held(&state),
            person_spoke: &|| person_spoke_since_gate(&wf, false, true, false),
            person_asked: &|| person_asked_since_gate(&wf),
            last_step_admits: &|| last_step_admits_persons_moves(&wf),
        };
        wait.mutation_intent(
            call.tool,
            call.tool_input,
            cwd,
            // Set by the adapter that ran the tool, outside the agent's input.
            call.parsed.get("focus_shell").and_then(Value::as_str) == Some("powershell"),
        )
    } else {
        // A tool this guard does not know could change anything.
        MutationIntent {
            opaque_shell: true,
            readable: true,
            ..MutationIntent::default()
        }
    };
    if mutation.targets.is_empty() && !mutation.opaque_shell {
        return Ok(Verdict::Passes);
    }
    // What the plan never governs: a command the lexer read whole that names
    // no file it writes and is not the engine's own, and a write whose every
    // target is outside the project.
    if known
        && !mutation.runs_engine
        && mutation.readable
        && mutation
            .targets
            .iter()
            .all(|target| project.is_outside(target))
    {
        return Ok(Verdict::Passes);
    }
    // A file-tool write of the composer's proposal alone, of a person's
    // answer text, or of the stage's own diary passes in every Plan Approval
    // state. A shell write, or one that also names another file, does not.
    // (A call with no target that is not opaque has passed above.)
    let every = |judge: &dyn Fn(&str) -> bool| {
        WRITE_TOOLS.contains(&call.tool)
            && !mutation.opaque_shell
            && mutation.targets.iter().all(|target| judge(target))
    };
    if every(&|target| project.is_composer_proposal_target(target))
        || every(&|target| project.is_answer_text_target(target))
        || every(&|target| project.is_stage_diary_target(target))
    {
        return Ok(Verdict::Passes);
    }
    Ok(Verdict::NoDirective)
}

/// `personAskedSinceGate`: the person asked for what the agent runs now,
/// and no unattended driver stands in for them.
fn person_asked_since_gate(wf: &Workflow) -> bool {
    focus_workspace::env::var("UNATTENDED").as_deref() != Some("1")
        && person_spoke_since_gate(wf, false, true, true)
}

/// `lastStepAdmitsPersonsMoves`: parked work's last step is the park, which
/// no issued step outlives. Otherwise the engine's last step decides, and
/// without its marker there is none.
fn last_step_admits_persons_moves(wf: &Workflow) -> bool {
    wf.read_state().is_ok_and(|state| {
        !trim(focus_state::get_field(&state, "Parked").unwrap_or_default()).is_empty()
    })
}

/// `recordGuardDisabled`: the off switch is not silent. While a workflow
/// exists, the first tool call that passes under it appends one
/// `GUARD_DISABLED` row, and consecutive calls append nothing until some
/// other row lands in this clone's shard. Every failure here still allows
/// the call.
fn record_guard_disabled(wf: &Workflow, tool: &str) {
    if !wf.state_path().exists() {
        return;
    }
    if let Ok(shard) = wf.ledger().audit_file(&wf.selector(), &wf.invoker)
        && let Ok(bytes) = std::fs::read(&shard)
    {
        let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
        let blocks: Vec<&str> = text.split("\n---\n").collect();
        if let Some(last) = blocks
            .iter()
            .rev()
            .find(|block| block_field(block, "Event").is_some())
            && block_field(last, "Event") == Some("GUARD_DISABLED")
            && block_field(last, "Guard") == Some(HOOK)
        {
            return;
        }
    }
    let tool = if tool.is_empty() { "(unknown)" } else { tool };
    let _ = append(
        wf,
        "GUARD_DISABLED",
        &[("Guard", HOOK.to_owned()), ("Tool", tool.to_owned())],
    );
}

/// The refusal as one plain sentence: some hosts show it to the person as
/// written. Control characters become a space, and it ends in one full stop.
fn one_sentence(text: &str) -> String {
    let plain = focus_js::re::replace(r"[\u0000-\u001f\u007f-\u009f\u2028\u2029]+", "g", text, " ");
    format!("{}.", trim(&plain).trim_end_matches('.'))
}

#[cfg(test)]
mod tests {
    use super::one_sentence;

    #[test]
    fn a_refusal_is_one_plain_sentence() {
        assert_eq!(one_sentence("as it is."), "as it is.");
        assert_eq!(one_sentence("  no stop  "), "no stop.");
        assert_eq!(one_sentence("three stops..."), "three stops.");
        // A reason that quotes a path or a command stays on the one line.
        assert_eq!(
            one_sentence("read\r\nfailed:\u{2028}x\u{9f}\ty"),
            "read failed: x y."
        );
    }
}

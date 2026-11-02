//! PreToolUse: the per-unit reviewer read-scope bound. A reviewer dispatched
//! for one unit must not read other units' `construction/<other>/` content
//! through any tool; the conductor's dispatch record
//! (`<record>/.focus-engine/reviewer-dispatch.json`) says which reviewer,
//! which unit and which files beyond it. Mirrors `aidlc-reviewer-scope.ts`;
//! the matcher is `focus_guards::reviewer_scope`.
//!
//! Fails open when its evidence is unavailable: no record, a stale one
//! (older than six hours, janitored), a malformed payload or record, an
//! unknown tool, an agent other than the dispatched reviewer. The off switch
//! `FOCUS_DISABLE_REVIEWER_SCOPE_HOOK=1` disables the read-scope check. A
//! claimed checkout's write bound (a team-owned workflow's unit scope stamp)
//! is units' territory: Focus refuses it by name.

use std::time::{Duration, SystemTime};

use focus_audit::time::iso_timestamp;
use focus_guards::Fence;
use focus_guards::authority::Decision;
use focus_guards::reviewer_scope::{
    ScopeContext, block_reason, evaluate_reviewer_scope, parse_dispatch_record,
    touches_construction,
};
use focus_guards::workflow::Guards;
use focus_js::EngineError;
use focus_js::text::trim;
use focus_state::fields::get_field;
use focus_workspace::Layout;
use focus_workspace::records::{Selection, Selector};
use serde_json::Value;

use super::{append, is_refusal, open, record_stood_aside, refused, stood_aside_output};
use crate::health::{cursor_selection, hooks_health_dir, record_drop, write_status_file};
use crate::{Outcome, context, payload};

const HOOK: &str = "reviewer-scope";
/// `REVIEWER_DISPATCH_TTL_MS`: the record's freshness window.
const DISPATCH_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const TOOLS: [&str; 10] = [
    "Read",
    "NotebookRead",
    "Edit",
    "MultiEdit",
    "Write",
    "NotebookEdit",
    "LS",
    "Glob",
    "Grep",
    "Bash",
];

/// `REVIEW_AGENT_RE`: the two shipped review-only agents, for the advisory.
fn is_review_agent(agent: &str) -> bool {
    focus_js::re::test(
        r"^focus-(architecture-reviewer|product-lead)-agent$",
        "",
        agent,
    )
}

fn thrown(err: &EngineError) -> Outcome {
    Outcome {
        code: 1,
        stdout: String::new(),
        stderr: format!("focus: {err}\n"),
    }
}

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let parsed = payload::parse(input);
    let session = parsed
        .as_ref()
        .and_then(|map| map.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let workflow = match context::enter(layout, session.as_deref()) {
        Ok(workflow) => workflow,
        Err(err) => return refused(&err),
    };
    // Outside the selected workflow the claimed-checkout bound still
    // applies; its bookkeeping and the reviewer read scope do not.
    let outside = workflow.outside_gate();
    let selection = workflow
        .selection
        .clone()
        .or_else(|| cursor_selection(layout));
    if !outside && let Some(selection) = &selection {
        write_status_file(
            &hooks_health_dir(layout, selection),
            "reviewer-scope.last",
            &iso_timestamp(SystemTime::now()),
            false,
        );
    }
    let Some(parsed) = parsed else {
        return Outcome::exit(0);
    };
    let tool = payload::text(Some(&parsed), "tool_name").to_owned();
    let tool_input = parsed.get("tool_input").and_then(Value::as_object);
    if !TOOLS.contains(&tool.as_str()) {
        return Outcome::exit(0);
    }
    // The state, for the claimed-checkout bound and the advisory; none
    // without a workflow.
    let wf = selection
        .as_ref()
        .and_then(|selection| open(layout, selection, &workflow.invoker).ok());
    let state = wf.as_ref().and_then(|wf| wf.read_state().ok());
    // A team-owned workflow's unit scope stamp names the Unit this checkout
    // claimed, and holds its writes to it (the units, M4).
    if state
        .as_deref()
        .is_some_and(|state| get_field(state, "Unit Ownership").map(trim) == Some("team"))
    {
        let stamp = layout.workspace_root().join(".focus-unit-scope.json");
        if stamp.exists() {
            return refused(&EngineError::new(format!(
                "{} records the Unit this checkout claimed, which bounds its writes to that Unit's construction tree: Focus does not work with unit claims yet.",
                stamp.display()
            )));
        }
    }
    if outside {
        return Outcome::exit(0);
    }
    // The off switch applies to the read scope only; a settings file the
    // resolution refuses is upstream's thrown read.
    match focus_workspace::settings::Places::of(&layout.project).flag(
        &focus_workspace::env::full_name("DISABLE_REVIEWER_SCOPE_HOOK"),
        &|name| std::env::var(name).ok(),
    ) {
        Ok(Some(on)) if on == "1" => return Outcome::exit(0),
        Ok(_) => {}
        Err(err) => return thrown(&err),
    }
    let Some(selection) = selection.as_ref() else {
        return Outcome::exit(0);
    };
    let record_path = selection.engine_dir(layout).join("reviewer-dispatch.json");
    let agent = payload::text(Some(&parsed), "agent_type").to_owned();
    if !record_path.exists() {
        // No review in flight. One advisory, once per ten minutes: a
        // review-only agent touching construction/ with no record suggests
        // the conductor skipped the dispatch write. Whether a per-unit
        // review was owed is the active directive's word (M5).
        if is_review_agent(&agent) && touches_construction(&tool, tool_input) && state.is_some() {
            let marker = selection.engine_dir(layout).join("active-directive.json");
            if marker.exists() {
                return refused(&EngineError::new(format!(
                    "{} is the orchestrator's directive marker, which says whether a per-unit review was owed here (the advisory a review agent earns by reading construction/ with no dispatch record): Focus does not read it yet.",
                    marker.display()
                )));
            }
        }
        return Outcome::exit(0);
    }
    let age = std::fs::metadata(&record_path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
    let Some(age) = age else {
        record_drop(
            layout,
            selection,
            HOOK,
            "reviewer dispatch record could not be read",
        );
        return Outcome::exit(0);
    };
    if age > DISPATCH_TTL {
        // An orphan (a session that crashed between dispatch and verdict):
        // ignored and janitored, so a stale window cannot keep refusing.
        let _ = std::fs::remove_file(&record_path);
        record_drop(
            layout,
            selection,
            HOOK,
            "ignoring an orphaned reviewer dispatch record (older than the freshness window); cleaned it up",
        );
        return Outcome::exit(0);
    }
    let dispatch = match std::fs::read(&record_path) {
        Ok(bytes) => parse_dispatch_record(&String::from_utf8_lossy(&bytes)),
        Err(err) => {
            record_drop(layout, selection, HOOK, &err.to_string());
            return Outcome::exit(0);
        }
    };
    let Some(dispatch) = dispatch else {
        record_drop(
            layout,
            selection,
            HOOK,
            "reviewer dispatch record is malformed; enforcement skipped",
        );
        return Outcome::exit(0);
    };
    // Identity: only the dispatched reviewer is held. Claude Code names the
    // active subagent in `agent_type`; a scoped registration asserts it.
    let dispatched = if agent.is_empty() {
        parsed.get("scoped_registration") == Some(&Value::Bool(true))
    } else {
        agent == dispatch.reviewer
    };
    if !dispatched {
        return Outcome::exit(0);
    }
    let cwd = parsed
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .map_or_else(
            || layout.project.to_string_lossy().into_owned(),
            str::to_owned,
        );
    let record_root = selection.docs_root(layout).to_string_lossy().into_owned();
    let verdict = evaluate_reviewer_scope(
        &tool,
        tool_input,
        &dispatch.unit,
        &dispatch.exempt,
        &ScopeContext {
            record_root: Some(record_root),
            cwd: Some(cwd),
        },
    );
    if !verdict.block {
        return Outcome::exit(0);
    }
    let target = verdict.target.clone().unwrap_or_default();
    let Some(wf) = wf.as_ref() else {
        return Outcome::exit(0);
    };
    // The fence stands aside when it is lowered for this piece of work;
    // `relaxed` keeps this one up.
    let actor = Guards::actor_from(
        parsed.get("agent_type").and_then(Value::as_str),
        tool_input
            .and_then(|input| input.get("subagent_type"))
            .and_then(Value::as_str),
    );
    match Guards::read(layout, &Selector::default(), &wf.invoker) {
        Err(err) if is_refusal(&err) => return refused(&err),
        Err(err) => record_drop(layout, selection, HOOK, &err.to_string()),
        Ok(guards) => {
            let decided = guards.decide_for(Fence::ReviewerScope, actor);
            if decided.decision == Decision::StandAside {
                let detail = format!("{target} (unit {})", dispatch.unit);
                let stdout = stood_aside_output(layout, &decided, &detail);
                record_stood_aside(wf, &decided, Some(&dispatch.stage), &tool, &detail);
                return Outcome {
                    code: 0,
                    stdout,
                    stderr: String::new(),
                };
            }
        }
    }
    emit_blocked(
        layout,
        selection,
        wf,
        &tool,
        &target,
        &dispatch.stage,
        &dispatch.unit,
    );
    Outcome {
        code: 2,
        stdout: String::new(),
        stderr: format!(
            "{}\n",
            block_reason(&target, &dispatch.unit, verdict.defaulted)
        ),
    }
}

/// `emitReviewerScopeBlocked`: the row, best effort: an audit failure never
/// changes the decision, and a contended lock drops the row.
fn emit_blocked(
    layout: &Layout,
    selection: &Selection,
    wf: &focus_lifecycle::workflow::Workflow,
    tool: &str,
    target: &str,
    stage: &str,
    unit: &str,
) {
    let shard = wf.ledger().audit_file(&wf.selector(), &wf.invoker);
    if !shard.is_ok_and(|shard| shard.exists()) {
        return;
    }
    let fields = [
        ("Tool", tool.to_owned()),
        ("Target", target.to_owned()),
        ("Stage", stage.to_owned()),
        ("Unit", unit.to_owned()),
    ];
    if append(wf, "REVIEWER_SCOPE_BLOCKED", &fields).is_err() {
        record_drop(
            layout,
            selection,
            HOOK,
            "audit lock contended; REVIEWER_SCOPE_BLOCKED row dropped (block still enforced)",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_review_agents_are_named() {
        assert!(is_review_agent("focus-architecture-reviewer-agent"));
        assert!(is_review_agent("focus-product-lead-agent"));
        assert!(!is_review_agent("focus-product-agent"));
        assert!(!is_review_agent(""));
    }
}

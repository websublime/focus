//! `focus engine state <verb>`.
//!
//! The state file of the active record, as data: `get` prints one field,
//! `set` writes fields, `checkbox` sets the box of stages in Stage Progress
//! and `count` counts them. The transitions that move a workflow through
//! its stages (`advance`, `finalize`, `complete-workflow`, `skip`, and
//! `resume` to read where it stands) and the gate verbs (`gate-start`,
//! `approve`, `reject`, `revise`) are `focus-lifecycle`'s.
//!
//! Two things stand before a write. The workflow must not be archived. And
//! lifecycle transitions and generic state writes are engine-owned: one the
//! orchestrator did not make (`FOCUS_STATE_TRANSITION_OWNER` naming this
//! process's parent, or `FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS=1`) passes
//! the state-transition fence first, and goes through only when the fence
//! is down for this piece of work, with one line on standard error and one
//! `GUARD_STOOD_ASIDE` row in the ledger.
//!
//! A verb that refuses prints `{"error": "<why>"}` on standard error, exits
//! 1, and, when the record has a state file, appends an `ERROR_LOGGED` row
//! to the ledger naming the tool, the command and the error, with the
//! project's path redacted from both (as the row's renderer redacts every
//! field, the row reads the same either way). The refusal of a selection
//! Focus cannot resolve finds no record to write beside, and is printed
//! alone, as upstream prints it.
//!
//! Mirrors `main`, `handleGet`, `handleSet`, `handleCheckbox`, `handleCount`,
//! `assertWorkflowNotArchived`, `getFlagValue` and `exitWithError` in
//! `aidlc-state.ts` (the transition handlers' argument reading included),
//! and `emitError` in `aidlc-lib.ts`.

use std::path::PathBuf;

use focus_audit::ledger::{Ledger, Machine};
use focus_audit::{Entry, Fields};
use focus_guards::authority::Decision;
use focus_guards::fences::Fence;
use focus_guards::workflow::Guards;
use focus_js::json;
use focus_js::text::{parse_int, trim};
use focus_lifecycle::gates::{self, GateStart, Reject};
use focus_lifecycle::transitions;
use focus_lifecycle::workflow::Workflow;
use focus_state::fields::{get_field, set_field};
use focus_state::file::{read_state_file, write_state_file};
use focus_state::progress::{CheckboxState, count_checkboxes, set_checkbox};
use focus_workspace::records::{self, Invoker, Selection, Selector};
use focus_workspace::{Layout, WorkspaceLock, env};
use serde_json::json;

use crate::Failure;
use crate::args::Args;
use crate::report::{Context, Refusal, state_path};

const HELP: &str = "\
Usage: focus engine state <verb> [--project-dir <dir>]

The state file of the active record, as data.

  get <field>              Print one bulleted field
  set <field=value> ...    Set fields; NOW is the time, +1 and -1 count
  checkbox <slug=state> ...
                           Set the box of stages in Stage Progress, and the
                           Completed count with it
  count <state>            Count the stages in one state

  advance <completed> [<next>]
                           Close a stage and open the next in-scope one
  finalize <completed>     Close a stage and move the cursor, opening none
  complete-workflow <completed> [--reason <text>]
                           Close the last stage
  skip <slug> [--reason <text>] [--route]
                           Pass a stage; with --route, move on from it
  resume                   Print where the work stands
  gate-start <slug> [--artifacts <csv>] [--recovered] [--approves-together <csv>]
                           Open a stage's approval gate
  approve <slug> [--user-input <text>]
                           Record the person's approval and move on
  reject <slug> --feedback <text>
                           Record the person's request for changes
  revise <slug>            Re-enter the gate after a revision

A state is one of pending, in-progress, awaiting-approval, revising,
completed, skipped. Writes are the engine's: set FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS=1
to make one by hand.";

const USAGE: &str = "Usage: focus engine state <subcommand>. Valid: get, set, checkbox, count, advance, finalize, complete-workflow, gate-start, approve, reject, revise, skip, resume. Run with --help for detail.";

/// The verbs Focus has.
const VERBS: [&str; 13] = [
    "get",
    "set",
    "checkbox",
    "count",
    "advance",
    "finalize",
    "complete-workflow",
    "gate-start",
    "approve",
    "reject",
    "revise",
    "skip",
    "resume",
];

/// `engineOwnedTransitions`: the verbs that pass the state-transition fence.
const ENGINE_OWNED: [&str; 10] = [
    "set",
    "checkbox",
    "advance",
    "finalize",
    "complete-workflow",
    "gate-start",
    "approve",
    "reject",
    "revise",
    "skip",
];

/// The verbs upstream has that Focus does not port, each refused by name.
const NOT_PORTED: [(&str, &str); 20] = [
    ("park", "parks the workflow"),
    ("unpark", "unparks the workflow"),
    (
        "set-skeleton-stance",
        "sets the Construction skeleton stance",
    ),
    (
        "set-construction-iteration",
        "sets the Construction iteration",
    ),
    (
        "set-construction-checkpoints",
        "sets the Construction checkpoints",
    ),
    (
        "set-construction-execution",
        "sets the Construction execution",
    ),
    (
        "set-construction-verification-command",
        "sets the Construction verification command",
    ),
    ("set-unit-ownership", "sets the units' ownership"),
    ("set-unit-gate-rhythm", "sets the units' gate rhythm"),
    ("refresh-unit-progress", "refreshes the units' progress"),
    ("sync-unit-scope-stage", "syncs a unit's scope stage"),
    ("fold-unit-merge", "folds a unit's merge into the state"),
    (
        "acknowledge-compaction",
        "acknowledges a context compaction",
    ),
    ("reuse-artifact", "reuses an artifact of another record"),
    ("lookup", "looks a stage up in the stage graph"),
    ("practices-event", "records a practices event"),
    ("practices-promote", "promotes a practice"),
    ("fork", "forks the state for a bolt"),
    ("merge", "merges a bolt's state"),
    ("unit", "runs a unit of work's step"),
];

/// What a verb answered: text to print, or nothing.
type Verb = Result<Option<String>, Refusal>;

pub fn run(args: &mut Args) -> Result<(), Failure> {
    if matches!(args.peek(), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    // The command as the ledger names it: the tool and its own arguments.
    let command = format!("focus-state {}", args.remaining().join(" "));
    let command = trim(&command).to_owned();
    let project_dir = match args.value("--project-dir") {
        Ok(project_dir) => project_dir,
        Err(message) => return Err(Failure::Message(message)),
    };
    let harness = match args.value("--harness") {
        Ok(harness) => harness,
        Err(message) => return Err(Failure::Message(message)),
    };
    let context = Context {
        project_dir,
        harness,
        tool: "focus-state",
        command,
        selector: Selector::default(),
    };
    match answer(args, &context) {
        Ok(Some(text)) => {
            println!("{text}");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(refusal) => Err(context.refuse(&refusal)),
    }
}

/// The record the command acts on.
struct Record {
    layout: Layout,
    selection: Selection,
    invoker: Invoker,
}

impl Record {
    fn find(context: &Context) -> Result<Self, Refusal> {
        let layout = context.workspace()?;
        let invoker = Invoker::from_env();
        let selection = records::resolve_selection(&layout, &Selector::default(), &invoker)?;
        Ok(Self {
            layout,
            selection,
            invoker,
        })
    }

    fn state_path(&self) -> PathBuf {
        state_path(&self.layout, &self.selection)
    }

    fn read_state(&self) -> Result<String, Refusal> {
        Ok(read_state_file(&self.state_path())?)
    }

    fn lock(&self) -> Result<WorkspaceLock, Refusal> {
        Ok(WorkspaceLock::acquire(
            &self.layout,
            "writing the state file",
        )?)
    }
}

fn answer(args: &mut Args, context: &Context) -> Verb {
    let Some(verb) = args.next_positional() else {
        return Err(Refusal::of(USAGE));
    };
    if let Some((_, does)) = NOT_PORTED.iter().find(|(name, _)| *name == verb) {
        return Err(Refusal::of(format!(
            "{verb} {does}, and Focus does not do that yet."
        )));
    }
    if !VERBS.contains(&verb.as_str()) {
        return Err(Refusal::of(format!(
            "Unknown subcommand: {verb}. Run focus engine state --help for usage."
        )));
    }
    let record = Record::find(context)?;
    if ENGINE_OWNED.contains(&verb.as_str()) {
        admit_write(&record, &verb)?;
    }
    // `archivedProtectedCommands`: every verb but the readers.
    if !matches!(verb.as_str(), "get" | "count") {
        let content = record.read_state()?;
        if get_field(&content, "Status") == Some("Archived") {
            return Err(Refusal::of(format!(
                "Workflow is Archived, so `focus engine state {verb}` is refused. Bring it back first with `/focus intent unarchive <name>`."
            )));
        }
    }
    let rest = args.take_rest();
    match verb.as_str() {
        "get" => get(&record, &rest),
        "set" => set(&record, &rest),
        "checkbox" => checkbox(&record, &rest),
        "count" => count(&record, &rest),
        _ => transition(record, &verb, &rest),
    }
}

/// `getFlagValue`: the value after `flag`, which must be there and not be
/// another flag.
fn flag_value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, Refusal> {
    let Some(at) = args.iter().position(|arg| arg == flag) else {
        return Ok(None);
    };
    let Some(value) = args.get(at + 1) else {
        return Err(Refusal::of(format!(
            "{flag} expects a value, got end of arguments."
        )));
    };
    if value.starts_with("--") {
        return Err(Refusal::of(format!(
            "{flag} expects a value, got another flag: \"{value}\". Did you forget the value?"
        )));
    }
    Ok(Some(value))
}

/// `getTextFlagValue`: `--flag=<text>` (the text may start with `--`), or
/// the separated form; not both.
fn text_flag_value<'a>(args: &'a [String], flag: &str) -> Result<Option<&'a str>, Refusal> {
    let prefix = format!("{flag}=");
    if let Some(inline) = args.iter().find(|arg| arg.starts_with(&prefix)) {
        if args.iter().any(|arg| arg == flag) {
            return Err(Refusal::of(format!("{flag} may be specified only once.")));
        }
        return Ok(Some(&inline[prefix.len()..]));
    }
    flag_value(args, flag)
}

/// `getFlagValues`: every value of a repeated flag.
fn flag_values(args: &[String], flag: &str) -> Result<Vec<String>, Refusal> {
    let mut values = Vec::new();
    let mut at = 0;
    while at < args.len() {
        if args[at] != flag {
            at += 1;
            continue;
        }
        let Some(value) = args.get(at + 1) else {
            return Err(Refusal::of(format!(
                "{flag} expects a value, got end of arguments."
            )));
        };
        if value.starts_with("--") {
            return Err(Refusal::of(format!(
                "{flag} expects a value, got another flag: \"{value}\". Did you forget the value?"
            )));
        }
        values.push(value.clone());
        at += 2;
    }
    Ok(values)
}

/// The lifecycle verbs, each reading its arguments as upstream's handler
/// does, then handed to `focus-lifecycle`.
fn transition(record: Record, verb: &str, args: &[String]) -> Verb {
    let operation = format!("`focus engine state {verb}`");
    let wf = Workflow::open(record.layout, record.selection, record.invoker)?;
    // Positional arguments are the ones that are not flags; a flag's value
    // is one of them unless the verb takes the flag.
    let positional: Vec<&String> = args.iter().filter(|arg| !arg.starts_with("--")).collect();
    let answer = match verb {
        "advance" => {
            let Some(completed) = positional.first() else {
                return Err(Refusal::of(
                    "Usage: focus engine state advance <completed-slug> [<next-slug>]",
                ));
            };
            transitions::advance(
                &wf,
                completed,
                positional.get(1).map(|s| s.as_str()),
                &operation,
            )?
        }
        "finalize" => {
            let Some(completed) = positional.first() else {
                return Err(Refusal::of(
                    "Usage: focus engine state finalize <completed-slug>",
                ));
            };
            transitions::finalize(&wf, completed, &operation)?
        }
        "complete-workflow" => {
            let reason_at = args.iter().position(|arg| arg == "--reason");
            let positional: Vec<&String> = args
                .iter()
                .enumerate()
                .filter(|(index, arg)| {
                    !arg.starts_with("--") && reason_at.is_none_or(|at| *index != at + 1)
                })
                .map(|(_, arg)| arg)
                .collect();
            let Some(completed) = positional.first() else {
                return Err(Refusal::of(
                    "Usage: focus engine state complete-workflow <completed-slug> [--reason <text>]",
                ));
            };
            let reason = reason_at
                .and_then(|at| args.get(at + 1))
                .map(String::as_str);
            transitions::complete_workflow(&wf, completed, reason, &operation)?
        }
        "gate-start" => {
            let Some(slug) = args.first() else {
                return Err(Refusal::of(
                    "Usage: focus engine state gate-start <slug> [--artifacts <csv>] [--recovered [--person-approves]] [--override-blocking-sensors] [--user-input <choice>] [--approves-together <csv>]",
                ));
            };
            let rest = &args[1..];
            let artifacts = args
                .iter()
                .position(|arg| arg == "--artifacts")
                .and_then(|at| args.get(at + 1))
                .map(String::as_str);
            let ask = GateStart {
                artifacts,
                recovered: args.iter().any(|arg| arg == "--recovered"),
                person_approves: args.iter().any(|arg| arg == "--person-approves"),
                override_blocking_sensors: args
                    .iter()
                    .any(|arg| arg == "--override-blocking-sensors"),
                user_input: flag_value(rest, "--user-input")?,
                approves_together: flag_value(rest, "--approves-together")?,
                unit: flag_value(rest, "--unit")?.map(trim),
            };
            gates::gate_start(&wf, slug, &ask, &operation)?
        }
        "approve" => {
            let Some(slug) = args.first() else {
                return Err(Refusal::of(
                    "Usage: focus engine state approve <slug> [--user-input <text>]",
                ));
            };
            let rest = &args[1..];
            let user_input = flag_value(rest, "--user-input")?;
            let unit = flag_value(rest, "--unit")?.map(trim);
            gates::approve(&wf, slug, user_input, unit, &operation)?
        }
        "reject" => {
            let Some(slug) = args.first() else {
                return Err(Refusal::of(
                    "Usage: focus engine state reject <slug> [--user-input \"Request Changes\"] [--feedback <text>] [--reject-finding <review-artifact>#R-NN=<human reason>]... [--reopen-finding <review-artifact>#R-NN=<human reason>]...",
                ));
            };
            let rest = &args[1..];
            let feedback = match text_flag_value(rest, "--feedback")? {
                Some(feedback) => Some(feedback),
                None => text_flag_value(rest, "--reason")?,
            };
            let ask = Reject {
                feedback,
                reject_findings: flag_values(rest, "--reject-finding")?,
                reopen_findings: flag_values(rest, "--reopen-finding")?,
                unit: flag_value(rest, "--unit")?.map(trim),
            };
            gates::reject(&wf, slug, &ask, &operation)?
        }
        "revise" => {
            let Some(slug) = args.first() else {
                return Err(Refusal::of(
                    "Usage: focus engine state revise <slug> [--override-blocking-sensors] [--user-input <choice>] [--approves-together <csv>]",
                ));
            };
            let rest = &args[1..];
            let ask = GateStart {
                artifacts: None,
                recovered: false,
                person_approves: false,
                override_blocking_sensors: args
                    .iter()
                    .any(|arg| arg == "--override-blocking-sensors"),
                user_input: flag_value(rest, "--user-input")?,
                approves_together: flag_value(rest, "--approves-together")?,
                unit: flag_value(rest, "--unit")?.map(trim),
            };
            gates::revise(&wf, slug, &ask, &operation)?
        }
        "skip" => {
            let Some(slug) = args.first() else {
                return Err(Refusal::of(
                    "Usage: focus engine state skip <slug> [--reason <text>] [--route | --unit <name>]",
                ));
            };
            let reason = flag_value(&args[1..], "--reason")?;
            let route = args.iter().any(|arg| arg == "--route");
            let unit = flag_value(&args[1..], "--unit")?;
            if route && unit.is_some() {
                return Err(Refusal::of(
                    "focus engine state skip takes --route or --unit, not both.",
                ));
            }
            if let Some(unit) = unit {
                return Err(Refusal::of(format!(
                    "skip --unit skips one unit of work's step (\"{unit}\"), which needs the unit DAG and the units' lifecycle: Focus does not run units yet."
                )));
            }
            transitions::skip(&wf, slug, reason, route)?
        }
        _ => transitions::resume(&wf)?,
    };
    Ok(Some(json::stringify(&answer)))
}

/// The engine-owned gate on a generic state write. The orchestrator passes
/// by naming this process's parent; a person by allowing direct
/// transitions; anyone else by the state-transition fence being down.
fn admit_write(record: &Record, verb: &str) -> Result<(), Refusal> {
    if let Some(owner) = env::var("STATE_TRANSITION_OWNER") {
        let Some(parent) = parent_process_id() else {
            return Err(Refusal::of(format!(
                "{} names the process that owns this transition, and Focus cannot tell its parent process on this platform yet. Unset it, or set {}=1.",
                env::full_name("STATE_TRANSITION_OWNER"),
                env::full_name("ALLOW_DIRECT_STATE_TRANSITIONS")
            )));
        };
        if owner == format!("orchestrate:{parent}") {
            return Ok(());
        }
    }
    if env::var("ALLOW_DIRECT_STATE_TRANSITIONS").as_deref() == Some("1") {
        return Ok(());
    }
    // An unreadable state or policy cannot lower the fence.
    let decided = record
        .read_state()
        .ok()
        .and_then(|state| {
            Guards::read(&record.layout, &Selector::default(), &record.invoker)
                .map(|guards| guards.with_state(&state))
                .ok()
        })
        .map(|guards| guards.decide(Fence::StateTransition));
    match decided {
        Some(decided) if decided.decision == Decision::StandAside => {
            let detail = format!("focus-state {verb}");
            if decided.speaks() {
                eprintln!("{}", decided.stood_aside_line(Some(&detail), true));
            }
            record_stood_aside(record, &decided, &detail);
            Ok(())
        }
        _ => Err(Refusal::of(format!(
            "Stage status cannot be changed with `focus engine state {verb}` because that bypasses the workflow's completion and approval checks. Use `focus engine orchestrate report` --stage <slug> --result <awaiting-approval|approved|rejected|revised|completed|skipped>; use `focus engine orchestrate park` to pause, and next/jump to move through the workflow. {}",
            fence_switch_sentence(record)
        ))),
    }
}

/// `recordGuardStoodAside`: one `GUARD_STOOD_ASIDE` row, when this clone's
/// shard exists. Best effort.
fn record_stood_aside(record: &Record, decided: &focus_guards::FenceDecision, detail: &str) {
    let ledger = Ledger::new(&record.layout, Machine::this());
    let Ok(shard) = ledger.audit_file(&Selector::default(), &record.invoker) else {
        return;
    };
    if !shard.exists() {
        return;
    }
    let Ok(held) = record.lock() else {
        return;
    };
    let mut fields = Fields::new();
    fields.set("Guard", decided.fence.name());
    fields.set("Authority", decided.authority.covered.as_str());
    fields.set("Grant", decided.authority.grant_source());
    fields.set("Actor", decided.authority.actor.as_str());
    fields.set("Tool", "focus-state");
    fields.set("Details", detail);
    let _ = ledger.append(
        &held,
        &[Entry::new("GUARD_STOOD_ASIDE", fields)],
        &Selector::default(),
        &record.invoker,
    );
}

/// `fenceSwitchSentence` for the state-transition fence: how the person
/// turns the check off, or why they cannot.
fn fence_switch_sentence(record: &Record) -> String {
    let fence = Fence::StateTransition.name();
    let guards = Guards::read(&record.layout, &Selector::default(), &record.invoker);
    match guards.as_ref().map(|guards| guards.policy(false)) {
        Ok(Ok(policy)) => match policy.memory_strict {
            None => format!(
                "If the person meant to do this now, offer to turn the {fence} check off for this piece of work. When they say so, run `focus engine config set guard.{fence} off` yourself and tell them in one line that it is off for this piece of work, comes back on for the next one, and that they can ask you to turn it back on."
            ),
            Some(strict) => format!(
                "Your team set Guard Policy to strict in {}, so the {fence} check stays on for everyone on this repo. Changing that line there changes it.",
                strict.path.display()
            ),
        },
        _ => {
            let state_line_only = guards
                .as_ref()
                .is_ok_and(|guards| guards.policy(true).is_ok());
            format!(
                "Guard Policy (how closely Focus checks changes to what you approved) could not be read, so the {fence} check cannot be turned off from chat. {}",
                if state_line_only {
                    "Do you want me to set it again, to strict, relaxed or off, and then try again?"
                } else {
                    "A Guard Policy line in this space's org.md, team.md or project.md cannot be read: correct it there (Mode: strict, relaxed or off), then try again."
                }
            )
        }
    }
}

#[cfg(unix)]
fn parent_process_id() -> Option<u32> {
    Some(std::os::unix::process::parent_id())
}

#[cfg(not(unix))]
fn parent_process_id() -> Option<u32> {
    None
}

fn get(record: &Record, args: &[String]) -> Verb {
    if args.is_empty() {
        return Err(Refusal::of("Usage: focus engine state get <field>"));
    }
    let field = args.join(" ");
    let content = record.read_state()?;
    match get_field(&content, &field) {
        Some(value) => Ok(Some(value.to_owned())),
        None => Err(Refusal::of(format!("Field not found: {field}"))),
    }
}

/// The setters a field has, that `set` must not stand in for.
const TYPED_SETTERS: [(&str, &str); 4] = [
    (
        "Construction Checkpoints",
        "set-construction-checkpoints <enabled|disabled>",
    ),
    (
        "Construction Execution",
        "set-construction-execution <serial|swarm>",
    ),
    (
        "Construction Iteration",
        "set-construction-iteration <unit-major|stage-major>",
    ),
    (
        "Construction Verification Command",
        "set-construction-verification-command --command-file verification-command.txt",
    ),
];

fn set(record: &Record, args: &[String]) -> Verb {
    if args.is_empty() {
        return Err(Refusal::of(
            "Usage: focus engine state set <field=value> ...",
        ));
    }
    // The whole batch is checked before any field changes.
    let mut pairs = Vec::new();
    for pair in args {
        let Some((field, value)) = pair.split_once('=').filter(|(field, _)| !field.is_empty())
        else {
            return Err(Refusal::of(format!("Invalid field=value pair: {pair}")));
        };
        if let Some((_, setter)) = TYPED_SETTERS.iter().find(|(name, _)| *name == field) {
            return Err(Refusal::of(format!(
                "{field} cannot be changed with `focus engine state set`. Use `focus engine state {setter}`."
            )));
        }
        pairs.push((field, value));
    }
    let _held = record.lock()?;
    let mut content = record.read_state()?;
    for (field, value) in &pairs {
        // NOW is the time; +1 and -1 count from the field as it is, as
        // `parseInt` reads it.
        let value = match *value {
            "NOW" => focus_audit::time::iso_timestamp(std::time::SystemTime::now()),
            "+1" | "-1" => {
                let current = get_field(&content, field).filter(|current| !current.is_empty());
                let number = current.map_or(0.0, |current| parse_int(current).unwrap_or(f64::NAN));
                // `Math.max(0, NaN)` is NaN, where Rust's `max` would say 0.
                let next = if *value == "+1" {
                    number + 1.0
                } else if number.is_nan() {
                    f64::NAN
                } else {
                    (number - 1.0).max(0.0)
                };
                focus_js::num::to_string(next)
            }
            other => other.to_owned(),
        };
        content = set_field(&content, field, &value);
    }
    write_state_file(&record.state_path(), &content)?;
    Ok(Some(json::stringify(
        &json!({ "updated": true, "fields": args.len() }),
    )))
}

fn checkbox_state(name: &str) -> Result<CheckboxState, Refusal> {
    CheckboxState::parse(name).ok_or_else(|| {
        Refusal::of(format!(
            "Invalid state: {name}. Valid: {}",
            CheckboxState::ALL
                .iter()
                .map(|state| state.name())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

fn checkbox(record: &Record, args: &[String]) -> Verb {
    if args.is_empty() {
        return Err(Refusal::of(
            "Usage: focus engine state checkbox <slug=state> ...",
        ));
    }
    let mut changes = Vec::new();
    for pair in args {
        let Some((slug, state)) = pair.split_once('=').filter(|(slug, _)| !slug.is_empty()) else {
            return Err(Refusal::of(format!("Invalid slug=state pair: {pair}")));
        };
        changes.push((slug, checkbox_state(state)?));
    }
    let _held = record.lock()?;
    let mut content = record.read_state()?;
    for (slug, state) in &changes {
        content = set_checkbox(&content, slug, *state);
    }
    // The Completed counter follows the boxes.
    let completed = count_checkboxes(&content, CheckboxState::Completed);
    content = set_field(&content, "Completed", &completed.to_string());
    write_state_file(&record.state_path(), &content)?;
    Ok(Some(json::stringify(&json!({
        "updated": true,
        "checkboxes": changes.len(),
        "completed_count": completed,
    }))))
}

fn count(record: &Record, args: &[String]) -> Verb {
    let Some(name) = args.first() else {
        return Err(Refusal::of("Usage: focus engine state count <state>"));
    };
    let state = checkbox_state(name)?;
    let content = record.read_state()?;
    Ok(Some(count_checkboxes(&content, state).to_string()))
}

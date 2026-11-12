//! `focus engine log <verb>`: `decision` records a question the agent asks
//! the person. `answer`, `answers`, `link` and `review` are not ported yet,
//! and are refused by name.
//!
//! Mirrors `main` and `error` in `aidlc-log.ts`.

use focus_js::text::trim;
use focus_lifecycle::workflow::Workflow;
use focus_log::flags::{self, Interaction};
use focus_workspace::names::STATE_FILE;
use focus_workspace::records::{self, Invoker, Selector};

use crate::Failure;
use crate::args::Args;
use crate::report::{Context, Refusal};

const HELP: &str = "\
Usage: focus engine log <verb> [--project-dir <dir>]

The interactions of the active record's work, written to its ledger.

  decision --stage <slug> --decision <text> [--options <a,b>] [--rationale <text>] [--single]
           [--message <id>] [--session <id>]
                           A question the agent asks the person; --message names the
                           message of theirs that already answered it
  answer, answers, link, review
                           Not ported yet
";

pub fn run(args: &mut Args) -> Result<(), Failure> {
    if matches!(args.peek(), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    let raw = args.take_rest();
    let command = trim(&format!("focus-log {}", raw.join(" "))).to_owned();
    // Upstream takes each `--project-dir <dir>` pair out wherever it is; a
    // trailing one with no value is left as an argument of the verb.
    let mut project_dir = None;
    let mut harness = None;
    let mut filtered = Vec::new();
    let mut raw_for_verb = Vec::new();
    let mut index = 0;
    while index < raw.len() {
        let arg = &raw[index];
        index += 1;
        if arg == "--harness" && index < raw.len() {
            // Focus's own flag, which upstream's command line never holds.
            harness = Some(raw[index].clone());
            index += 1;
            continue;
        }
        raw_for_verb.push(arg.clone());
        if arg == "--project-dir" && index < raw.len() {
            project_dir = Some(raw[index].clone());
            raw_for_verb.push(raw[index].clone());
            index += 1;
        } else {
            filtered.push(arg.clone());
        }
    }
    let context = Context {
        project_dir,
        harness,
        tool: "focus-log",
        command,
        selector: Selector::default(),
    };
    match answer(&filtered, &raw_for_verb, &context) {
        Ok(text) => {
            println!("{text}");
            Ok(())
        }
        Err(refusal) => Err(context.refuse(&refusal)),
    }
}

/// `resolveActiveProjectDir`: the piece of work an interaction is recorded
/// for. With none selected the row would land in the bare record root of
/// the space, where no ledger ever lives.
fn open(context: &Context) -> focus_js::Result<Workflow> {
    let layout = context.workspace().map_err(focus_js::EngineError::new)?;
    let invoker = Invoker::from_env();
    let selection = records::resolve_selection(&layout, &Selector::default(), &invoker)?;
    if !selection.docs_root(&layout).join(STATE_FILE).exists() {
        return Err(focus_js::EngineError::new(
            "No active workflow is selected, so this interaction cannot be recorded. Start one by describing what to build (/focus \"build the auth service\"), or switch to an existing one with /focus intent <name>.",
        ));
    }
    Workflow::open(layout, selection, invoker)
}

fn answer(filtered: &[String], raw: &[String], context: &Context) -> Result<String, Refusal> {
    let verb = filtered.first().map(String::as_str);
    let rest = filtered.get(1..).unwrap_or_default();
    let answered = match verb {
        Some("decision") => {
            flags::refuse_split_values(Interaction::Decision, raw)?;
            focus_log::decision::decision(&|| open(context), rest)?
        }
        Some(verb @ ("answer" | "answers" | "link" | "review")) => {
            return Err(Refusal::of(format!(
                "log {verb} is not ported yet: Focus records the questions of a stage (log decision), and not yet their answers, a pipeline's handoffs or the reviews."
            )));
        }
        other => {
            return Err(Refusal::of(format!(
                "Unknown subcommand: {}. Valid: decision, answer, answers, link, review",
                other.unwrap_or("undefined")
            )));
        }
    };
    Ok(focus_js::json::stringify(&answered))
}

//! `focus engine review-brief <command> --stage <slug>`: what a reviewer or
//! the person at a gate is shown of a stage's reviews.
//!
//! `context` prints the list of findings each scope of the stage stands at,
//! as the next reviewer reads it: what is open to check again, and what was
//! decided. `review` prints the brief the conductor shows at the gate, and
//! keeps it as printed beside the review's record. `summary` prints the
//! brief of a summary confirmation.
//!
//! Mirrors `main` and `parseCliFlags` in `aidlc-review-brief.ts`. The tool
//! reads and writes no row of the ledger: a refusal is printed, and nothing
//! else happens.

use focus_evidence::review_shown::{self, Audience};
use focus_js::EngineError;
use focus_lifecycle::review_brief::{self, Why};
use focus_lifecycle::review_derived;
use focus_lifecycle::workflow::Workflow;
use focus_workspace::records::{self, Invoker, Selector};

use crate::Failure;
use crate::args::Args;
use crate::report::Context;

const HELP: &str = "\
Usage: focus engine review-brief <command> --stage <slug> [--unit <name>] [--project-dir <dir>]

What is shown of a stage's reviews.

  context                  The findings the stage's reviews stand at, for the next reviewer
  review --why <first|revision|stale> [--fallback-finding <text>]
                           The brief shown at the gate, kept beside the review's record
  summary --questions-file <path>
                           The brief of a summary confirmation
";

pub fn run(args: &mut Args) -> Result<(), Failure> {
    if matches!(args.peek(), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    match answer(&args.take_rest()) {
        Ok(text) => {
            println!("{text}");
            Ok(())
        }
        Err(err) => {
            eprintln!(
                "{}",
                focus_js::json::stringify(&serde_json::json!({ "error": err.message() }))
            );
            Err(Failure::Reported)
        }
    }
}

/// `parseCliFlags`: every argument after the command is a `--flag` and its
/// value.
fn flags(args: &[String]) -> Result<Vec<(String, String)>, EngineError> {
    let mut flags: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    while at < args.len() {
        let flag = &args[at];
        if !flag.starts_with("--") || at + 1 >= args.len() {
            return Err(EngineError::new(format!(
                "Expected --flag value, got {}.",
                focus_js::json::stringify(&serde_json::json!(flag))
            )));
        }
        let name = flag[2..].to_owned();
        let value = args[at + 1].clone();
        match flags.iter_mut().find(|(known, _)| *known == name) {
            Some(slot) => slot.1 = value,
            None => flags.push((name, value)),
        }
        at += 2;
    }
    Ok(flags)
}

/// `main`: the command is the first argument whatever it is, and where the
/// project is one flag among the others, so neither is looked for before the
/// flags are read.
fn answer(rest: &[String]) -> Result<String, EngineError> {
    let command = rest.first().map(String::as_str);
    let flags = flags(rest.get(1..).unwrap_or_default())?;
    let flag = |name: &str| {
        flags
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
            // (An empty value is no value, as upstream's truthiness has it.)
            .filter(|value| !value.is_empty())
    };
    // (A unit given as nothing is still a unit given: upstream tells the two
    // apart where it asks whether one was named.)
    let given = |name: &str| {
        flags
            .iter()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.as_str())
    };
    let context = Context {
        project_dir: flag("project-dir").map(str::to_owned),
        // (Upstream's tool has no flag for the harness, and no path read
        // here depends on it.)
        harness: None,
        tool: "focus-review-brief",
        command: String::new(),
        selector: Selector::default(),
    };
    let layout = context.workspace().map_err(EngineError::new)?;
    let Some(slug) = flag("stage") else {
        return Err(EngineError::new("Missing --stage <slug>."));
    };
    let invoker = Invoker::from_env();
    let selection = records::resolve_selection(&layout, &Selector::default(), &invoker)?;
    let wf = Workflow::open(layout, selection, invoker)?;
    let stage = wf.stage(slug)?;
    match command {
        Some("context") => {
            let contexts = review_derived::contexts(&wf, stage, given("unit"))?;
            let shown: Vec<review_shown::Context<'_>> = contexts
                .iter()
                .map(|context| review_shown::Context {
                    artifact: &context.artifact,
                    findings: &context.findings,
                    findings_text: context.findings_text.as_deref(),
                })
                .collect();
            Ok(review_shown::findings_context(&shown, Audience::Reviewer))
        }
        Some("review") => {
            let Some(why) = given("why").and_then(Why::of) else {
                return Err(EngineError::new(
                    "Review brief requires --why <first|revision|stale>.",
                ));
            };
            let brief = review_brief::review_brief(
                &wf,
                stage,
                why,
                given("unit"),
                flag("fallback-finding"),
            )?;
            // What is kept is what is printed, to its last line break.
            review_brief::keep_gate_brief(&wf, stage, given("unit"), why, &format!("{brief}\n"));
            Ok(brief)
        }
        Some("summary") => {
            let Some(questions) = flag("questions-file") else {
                return Err(EngineError::new(
                    "Summary brief requires --questions-file <path>.",
                ));
            };
            review_brief::summary_brief(&wf, stage, questions, given("unit"))
        }
        other => Err(EngineError::new(format!(
            "Unknown subcommand: {}. Valid: review, context, summary.",
            // (No command leaves no flag either, and the stage was asked for
            // above.)
            other.unwrap_or_default()
        ))),
    }
}

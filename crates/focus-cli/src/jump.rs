//! `focus engine jump <verb>`: `resolve` says where a jump would land,
//! `execute` moves the work there, `reopen` starts a per-unit stage again
//! for named units (not ported: refused by name).
//!
//! Mirrors `main` and `error` in `aidlc-jump.ts`.

use focus_js::text::trim;
use focus_lifecycle::jump;
use focus_lifecycle::workflow::Workflow;
use focus_workspace::records::{self, Invoker, Selector};

use crate::Failure;
use crate::args::Args;
use crate::report::{Context, Refusal};

const HELP: &str = "\
Usage: focus engine jump <verb> [--project-dir <dir>]

Moving the active record's work to another stage.

  resolve --stage <slug|#> [--allow-skipped] | --phase <name|#> [--scope <scope>]
                           Where a jump would land, and what it passes over or resets
  execute --target <slug> --direction <forward|backward|redo> [--scope <scope>]
                           Move the work there, with the rows an advance would write
  reopen                   Start a per-unit stage again for named units (not ported)
";

pub fn run(args: &mut Args) -> Result<(), Failure> {
    if matches!(args.peek(), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    let command = trim(&format!("focus-jump {}", args.remaining().join(" "))).to_owned();
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
        tool: "focus-jump",
        command,
        selector: Selector::default(),
    };
    match answer(args, &context) {
        Ok(text) => {
            println!("{text}");
            Ok(())
        }
        Err(refusal) => Err(context.refuse(&refusal)),
    }
}

fn answer(args: &mut Args, context: &Context) -> Result<String, Refusal> {
    let rest = args.take_rest();
    let verb = rest.first().map(String::as_str);
    let open = || -> Result<Workflow, Refusal> {
        let layout = context.workspace()?;
        let invoker = Invoker::from_env();
        let selection = records::resolve_selection(&layout, &Selector::default(), &invoker)?;
        Ok(Workflow::open(layout, selection, invoker)?)
    };
    let answered = match verb {
        Some("resolve") => jump::resolve(&open()?, &rest[1..])?,
        Some("execute") => jump::execute(&open()?, &rest[1..])?,
        Some("reopen") => jump::reopen(&open()?, &rest[1..])?,
        other => {
            return Err(Refusal::of(format!(
                "Unknown subcommand: {}. Valid: resolve, execute, reopen",
                other.unwrap_or("undefined")
            )));
        }
    };
    Ok(focus_js::json::stringify(&answered))
}

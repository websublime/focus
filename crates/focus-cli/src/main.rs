//! The `focus` command.
//!
//! The command tree follows upstream's `aidlc` so that a hook registration or
//! a skill written for one reads the same for the other: engine internals live
//! under `engine`: `engine graph <verb>` is upstream's `aidlc-graph.ts`, and
//! `engine audit <verb>` its `aidlc-audit.ts`.
//! Arguments are parsed by hand, as upstream does: the engine's usage and
//! error text is part of what agents read, so it is written, not generated.

mod args;
mod audit;
mod graph;

use std::process::ExitCode;

use args::Args;

const USAGE: &str = "\
Usage: focus <command>

Commands:
  engine graph compile         Compile stage-graph.json and scope-grid.json
  engine graph topo            Print the stages in dependency order
  engine graph cycles          Report dependency cycles
  engine graph scope           Print the stages a scope executes
  engine graph artifacts       List every artifact any stage produces
  engine graph producers       List the stages that produce an artifact
  engine graph consumers       List the stages that consume an artifact
  engine graph validate-scope  Check a scope's stages have their inputs
  engine graph validate-grid   Check a proposed plan
  engine graph ars             Compute the arithmetic behind a tailored plan
  engine graph resolve         Write or print a scope's plan
  engine graph export          Print the graph, scopes and agents as one document
  engine audit append          Append a diagnostic event to the audit ledger
  engine audit append-batch    Append several events as one write
  engine audit append-raw      Append a free-form note
  engine audit history         Print the active record's events and notes

Run `focus engine graph --help` or `focus engine audit --help` for the options
of each.";

/// How a command failed.
pub enum Failure {
    /// A message to print as it is.
    Message(String),
    /// The command already printed everything it had to say.
    Reported,
}

impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

fn main() -> ExitCode {
    let mut args = match Args::from_env() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };
    let outcome: Result<(), Failure> = match args.next_positional().as_deref() {
        None | Some("--help" | "-h") => {
            println!("{USAGE}");
            Ok(())
        }
        Some("--version" | "-V") => {
            println!("focus {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("engine") => match args.next_positional().as_deref() {
            Some("graph") => graph::run(&mut args),
            Some("audit") => audit::run(&mut args),
            Some(other) => {
                Err(format!("Unknown engine command: {other}. Valid: audit, graph").into())
            }
            None => Err("Usage: focus engine <command>. Valid: audit, graph".into()),
        },
        // Development aid: answers oracle cases for the parity corpus.
        Some("dev") => match args.next_positional().as_deref() {
            Some("oracle") => graph::oracle(&mut args).map_err(Failure::from),
            _ => Err("Usage: focus dev oracle <cases.ndjson>".into()),
        },
        Some(other) => Err(format!("Unknown command: {other}. Run `focus --help`.").into()),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Message(message)) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
        Err(Failure::Reported) => ExitCode::FAILURE,
    }
}

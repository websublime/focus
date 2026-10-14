//! `focus engine utility <verb>`: upstream's `aidlc-utility.ts`.
//!
//! The utility tool is where a workflow is created and looked at from
//! outside: `detect` scans the workspace, `now` prints the engine's time,
//! and `intent-create` (M3's next slice) mints a record. The rest of
//! upstream's 34 verbs are refused by name, each with a line that says what
//! it does, until the milestone that brings it.
//!
//! Arguments are read as upstream reads them ([`Parsed`]): every input is a
//! flag, `--json` alone is `true`. A verb that refuses prints `{"error": …}`
//! on standard error, exits 1 and offers the ledger an `ERROR_LOGGED` row
//! ([`crate::report`]), beside the record an explicit `--intent` or
//! `--space` names, else the active one.
//!
//! Mirrors `main`, `die`, `handleDetect` and the `now` arm in
//! `aidlc-utility.ts`.

use std::time::SystemTime;

use focus_graph::runtime;
use focus_graph::scopes::{self, ScopeMetadata};
use focus_js::json;
use focus_js::text::trim;
use focus_workspace::manifest::HarnessManifest;
use focus_workspace::project;
use focus_workspace::records::Selector;
use focus_workspace::scan::{self, Scan};
use serde_json::{Map, Value, json};

use crate::Failure;
use crate::args::{Args, Parsed};
use crate::report::{Context, Refusal};

const HELP: &str = "\
Usage: focus engine utility <verb> [--project-dir <dir>]

The workflow from outside: what the workspace holds, and the engine's time.

  detect [--json]          Scan the workspace: project type, languages,
                           frameworks, build system, submodules; and where
                           the scopes live
  now                      Print the engine's time, as the ledger stamps it

Run with --help for this text.";

/// Upstream's other verbs: what each does, for the line that refuses it.
const NOT_PORTED: [(&str, &str); 32] = [
    ("help", "prints upstream's help"),
    ("version", "prints the engine's version"),
    ("status", "prints the workflow's status"),
    ("doctor", "checks the installation"),
    ("intent-create", "creates an intent record"),
    ("init", "is the retired name of intent-create"),
    ("intent", "lists, switches, archives and unarchives intents"),
    ("space", "lists, switches and creates spaces"),
    ("space-create", "creates a space"),
    (
        "codekb-path",
        "prints where the code knowledge base is written",
    ),
    ("codekb-snapshot", "snapshots the code knowledge base"),
    ("codekb-publish", "publishes the code knowledge base"),
    (
        "project-description",
        "prints the exact project description",
    ),
    ("document-input", "reads a document the person gave"),
    (
        "codekb-scope-diff",
        "compares the code knowledge base with the tree",
    ),
    ("reclassify", "records the project type the person declared"),
    ("select-plugins", "selects the plugins an install enables"),
    ("plugin-list", "lists the plugins"),
    ("plugin-sync", "syncs the plugins"),
    ("plugin-validate", "validates a plugin"),
    ("plugin-build", "builds a plugin"),
    ("recompose", "recomposes a workflow's plan"),
    ("scope-change", "changes a workflow's scope"),
    ("scope-save", "saves a composed scope"),
    ("config-change", "changes a workflow's settings"),
    ("config-get", "prints a workflow's setting"),
    ("config-list", "lists a workflow's settings"),
    ("set-status", "sets a workflow's status"),
    ("detect-scope", "detects a scope from the request"),
    (
        "resolve-env-scope",
        "resolves the default scope from the environment",
    ),
    ("scope-table", "prints the scope table"),
    ("stage-table", "prints the stage table"),
];

/// The verbs Focus has.
const VERBS: [&str; 2] = ["detect", "now"];

type Verb = Result<Option<String>, Refusal>;

pub fn run(args: &mut Args) -> Result<(), Failure> {
    if matches!(args.peek(), Some("--help" | "-h")) {
        println!("{HELP}");
        return Ok(());
    }
    let raw = args.take_rest();
    let parsed = Parsed::from_args(&raw);
    let command = trim(&format!("focus-utility {}", raw.join(" "))).to_owned();
    let context = Context {
        project_dir: parsed.valued("project-dir").map(str::to_owned),
        harness: parsed.valued("harness").map(str::to_owned),
        tool: "focus-utility",
        command,
        selector: Selector {
            space: parsed.valued("space").map(str::to_owned),
            intent: parsed.valued("intent").map(str::to_owned),
            session_id: None,
        },
    };
    match answer(&parsed, &context) {
        Ok(Some(text)) => {
            print!("{text}");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(refusal) => Err(context.refuse(&refusal)),
    }
}

fn answer(parsed: &Parsed, context: &Context) -> Verb {
    let Some(verb) = parsed.positional.first() else {
        return Err(Refusal::of(format!(
            "Usage: focus engine utility <verb>. Valid: {}. Run with --help for detail.",
            VERBS.join(", ")
        )));
    };
    if let Some((_, does)) = NOT_PORTED.iter().find(|(name, _)| name == verb) {
        return Err(Refusal::of(format!(
            "{verb} {does}, and Focus does not do that yet."
        )));
    }
    match verb.as_str() {
        "detect" => detect(parsed, context),
        "now" => Ok(Some(format!(
            "{}\n",
            focus_audit::time::iso_timestamp(SystemTime::now())
        ))),
        other => Err(Refusal::of(format!(
            "Unknown command \"{other}\". Run `focus engine utility --help` for what this tool can do.\n\nAvailable commands: {}\nCommon options: [--project-dir <path>] [--json]",
            VERBS.join(", ")
        ))),
    }
}

/// The scopes the install's plugin selection leaves enabled.
fn enabled_scopes(layout: &focus_workspace::Layout) -> Result<Vec<ScopeMetadata>, Refusal> {
    let all = scopes::load_all(&layout.scopes_dir())?;
    let manifest = HarnessManifest::read(&layout.manifest())?;
    Ok(scopes::enabled(&all, &manifest))
}

/// `detect`: the scan, and where the scope registry lives, so the composer
/// agent is told rather than left to derive it. Read-only.
fn detect(parsed: &Parsed, context: &Context) -> Verb {
    let layout = context.workspace()?;
    // Upstream names the scopes of the install the tool runs from, whatever
    // `--project-dir` says; Focus runs from nowhere, so the project named
    // must hold the install whose scopes are reported.
    if !project::installed_harnesses(&layout.project).contains(&layout.harness) {
        return Err(Refusal::of(format!(
            "{} is not installed in {}: `focus engine utility detect` scans the project it is installed in.",
            layout.harness.product_name(),
            layout.project.display()
        )));
    }
    let scan = scan::detect(&layout.project);
    let scopes_dir = layout.scopes_dir().display().to_string();
    let scope_grid = layout.scope_grid().display().to_string();
    let proposal = layout
        .composer_proposal(&layout.active_space())
        .strip_prefix(&layout.project)
        .map_or_else(
            |_| layout.composer_proposal(&layout.active_space()),
            std::path::Path::to_path_buf,
        )
        .display()
        .to_string()
        .replace('\\', "/");
    let scopes = runtime::valid_scopes(&enabled_scopes(&layout)?);
    if parsed.flag("json") == Some("true") {
        return Ok(Some(format!(
            "{}\n",
            json::stringify(&payload(
                &scan,
                &scopes_dir,
                &scope_grid,
                &proposal,
                &scopes
            ))
        )));
    }
    let uninitialized = scan
        .submodules
        .iter()
        .filter(|submodule| !submodule.initialized)
        .count();
    let mut text = format!(
        "Project type: {}\nLanguages: {}\nFrameworks: {}\nBuild system: {}\n",
        scan.project_type, scan.languages, scan.frameworks, scan.build_system
    );
    if let Some(nested) = &scan.nested_root {
        text.push_str(&format!("Nested root: {nested}\n"));
    }
    if !scan.submodules.is_empty() {
        text.push_str(&format!(
            "Submodules: {} declared, {uninitialized} uninitialized\n",
            scan.submodules.len()
        ));
    }
    text.push_str(&format!(
        "Scopes dir: {scopes_dir}\nScope grid: {scope_grid}\nProposal file: {proposal}\nValid scopes: {}\n",
        scopes.join(", ")
    ));
    Ok(Some(text))
}

fn payload(
    scan: &Scan,
    scopes_dir: &str,
    scope_grid: &str,
    proposal: &str,
    scopes: &[String],
) -> Value {
    let mut out = Map::new();
    out.insert("projectType".to_owned(), json!(scan.project_type));
    out.insert("languages".to_owned(), json!(scan.languages));
    out.insert("frameworks".to_owned(), json!(scan.frameworks));
    out.insert("buildSystem".to_owned(), json!(scan.build_system));
    if let Some(nested) = &scan.nested_root {
        out.insert("nestedRoot".to_owned(), json!(nested));
    }
    out.insert(
        "submodules".to_owned(),
        scan.submodules
            .iter()
            .map(|submodule| {
                json!({
                    "name": submodule.name,
                    "path": submodule.path,
                    "url": submodule.url,
                    "initialized": submodule.initialized,
                })
            })
            .collect(),
    );
    out.insert("scopesDir".to_owned(), json!(scopes_dir));
    out.insert("scopeGridPath".to_owned(), json!(scope_grid));
    out.insert("proposalPath".to_owned(), json!(proposal));
    out.insert("scopes".to_owned(), json!(scopes));
    Value::Object(out)
}

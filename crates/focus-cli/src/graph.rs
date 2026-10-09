//! `focus engine graph <verb>`.

use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};

use focus_graph::compile::canonical_scope_grid_json;
use focus_graph::proposal::{ProposalRequest, ProposalWorld};
use focus_graph::runtime::{self, ScopeDefinition, Stage};
use focus_graph::scopes::{self, ScopeMetadata};
use focus_graph::{
    Compiled, Sources, agents, ars, compile, export, guard_memory, oracle as cases, proposal,
    query, write,
};
use focus_harness::Harness;
use focus_js::order::{cmp_utf16, js_key_order};
use focus_js::text::trim;
use focus_js::{json, num};
use focus_workspace::atomic::write_atomic;
use focus_workspace::manifest::HarnessManifest;
use focus_workspace::names::DEFAULT_SPACE;
use focus_workspace::{Layout, WorkspaceLock, env, project};
use serde_json::{Map, Value};

use crate::Failure;
use crate::args::Args;

const VERBS: &str = "ars, artifacts, compile, consumers, cycles, export, producers, resolve, scope, topo, validate-grid, validate-scope";

const HELP: &str = "\
Usage: focus engine graph <verb> [--project-dir <dir>] [--harness <claude|cursor|copilot>]

Every verb acts on one harness installed in one project. The project is
--project-dir, else FOCUS_PROJECT_DIR, else the directory the host names, else
the nearest directory at or above the current one with a harness installed.
The harness is --harness, else FOCUS_HARNESS, else the only one installed.

  compile                      Recompile stage-graph.json and scope-grid.json in
                               place, under the workspace lock
  compile --check              Fail when the compiled files are out of date
  topo                         Stages in dependency order
  cycles [--scope <name>]      Dependency cycles; exit 1 when there are any
  scope <name>                 The stages a scope executes
  artifacts                    Every artifact any stage produces
  producers <artifact>         Stages that produce an artifact
  consumers <artifact>         Stages that consume an artifact
  validate-scope <name>        Check each stage of a scope has its inputs
  validate-grid [--proposal <path>] [--strict] [--project-type <brownfield|greenfield>]
                [--keywords <csv>] [--report] [--matched <stock-scope> | --custom]
                [--guard-policy <strict|relaxed|off>] [--space <name>] [--intent <name>]
                               Check a proposed plan (a JSON grid); prints the
                               verdict as JSON, exit 1 when invalid. Without
                               --proposal it reads the composer's proposal file;
                               `-` reads standard input.
  ars --iae <s> --csu <s> --ve <s> --r <s> --ua <s>
      [--completed <csv>] [--project-type <brownfield|greenfield>]
                               The arithmetic behind a tailored plan, as JSON
  resolve <scope> --stdout     A scope's plan (needs FOCUS_GRAPH_RESOLVE=1)
  export [--check <file>]      The graph, scopes, artifacts and agents as JSON

topo, artifacts, producers, consumers and cycles (without --scope) also accept
--graph <stage-graph.json> to read a compiled graph outside any project.

Compiling outside an installed project:

  compile --harness <name> (--core <dir> | --installed <dir>)
          (--out <dir> | --check [<dir>] | --print <stage-graph|scope-grid>)
          [--rules-dir <dir>]

      --core <dir>       Read a source tree: a core/ directory of authored Markdown.
      --installed <dir>  Read an installed harness directory (.claude, .cursor,
                         .focus). Stages already in its graph keep their numbers.
      --out <dir>        Write both files into <dir>.
      --check [<dir>]    Compare with the files in <dir> (default: the installed
                         tree's tools/data) and fail when they differ.
      --print <which>    Write one compiled file to standard output.
      --rules-dir <dir>  Read the method files from <dir>.";

/// How a verb stops early.
enum Stop {
    /// An error the engine raised; printed as `focus graph <verb>: <message>`.
    Raised(String),
    /// A usage message printed as it is.
    Said(String),
    /// Exit 1 with nothing more to print.
    Reported,
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Self::Raised(message)
    }
}

impl From<&str> for Stop {
    fn from(message: &str) -> Self {
        Self::Raised(message.to_owned())
    }
}

impl From<focus_js::EngineError> for Stop {
    fn from(error: focus_js::EngineError) -> Self {
        Self::Raised(error.to_string())
    }
}

impl From<std::io::Error> for Stop {
    fn from(error: std::io::Error) -> Self {
        Self::Raised(error.to_string())
    }
}

type Verb = Result<(), Stop>;

pub fn run(args: &mut Args) -> Result<(), Failure> {
    let Some(verb) = args.next_positional() else {
        return Err(format!(
            "Usage: focus engine graph <subcommand>. Valid: {VERBS}. Run with --help for detail."
        )
        .into());
    };
    let outcome = match verb.as_str() {
        "--help" | "-h" => {
            println!("{HELP}");
            Ok(())
        }
        "compile" => run_compile(args),
        "topo" | "artifacts" | "producers" | "consumers" => run_query(&verb, args),
        "cycles" => run_cycles(args),
        "scope" => run_scope(args),
        "validate-scope" => run_validate_scope(args),
        "validate-grid" => run_validate_grid(args),
        "ars" => run_ars(args),
        "resolve" => run_resolve(args),
        "export" => run_export(args),
        other => {
            return Err(format!("Unknown subcommand: {other}. Valid: {VERBS}").into());
        }
    };
    outcome.map_err(|stop| match stop {
        Stop::Raised(message) => Failure::Message(format!("focus graph {verb}: {message}")),
        Stop::Said(message) => Failure::Message(message),
        Stop::Reported => Failure::Reported,
    })
}

/// Which install a verb acts on.
struct Context {
    project_dir: Option<String>,
    harness: Option<String>,
    /// `--graph`: a compiled graph named directly, outside any project.
    graph: Option<PathBuf>,
}

impl Context {
    fn take(args: &mut Args) -> Result<Self, String> {
        Ok(Self {
            project_dir: args.value("--project-dir")?,
            harness: args.value("--harness")?,
            graph: args.value("--graph")?.map(PathBuf::from),
        })
    }

    fn layout(&self) -> Result<Layout, String> {
        if self.graph.is_some() {
            return Err(
                "--graph names a compiled graph outside a project; this verb needs an installed project. Drop --graph."
                    .to_owned(),
            );
        }
        let project = project::resolve_project_dir(self.project_dir.as_deref().map(Path::new))
            .map_err(|err| err.to_string())?;
        let harness = project::resolve_harness(&project, self.harness.as_deref())
            .map_err(|err| err.to_string())?;
        Ok(Layout::new(project, harness))
    }

    /// Every compiled stage, then the enabled ones.
    fn stages(&self) -> Result<(Vec<Stage>, Vec<Stage>), String> {
        let path = match &self.graph {
            Some(path) => path.clone(),
            None => self.layout()?.stage_graph(),
        };
        let all = runtime::load_stages_all(&path).map_err(|err| err.to_string())?;
        let enabled = runtime::enabled(&all);
        Ok((all, enabled))
    }
}

/// The scopes the install's plugin selection leaves enabled.
fn enabled_scopes(layout: &Layout) -> Result<Vec<ScopeMetadata>, String> {
    let all = scopes::load_all(&layout.scopes_dir()).map_err(|err| err.to_string())?;
    let manifest = HarnessManifest::read(&layout.manifest()).map_err(|err| err.to_string())?;
    let enabled = scopes::enabled(&all, &manifest);
    scopes::check_single_freeform_default(&enabled).map_err(|err| err.to_string())?;
    Ok(enabled)
}

/// The scope names a verb accepts. Scope validity comes from the scope files,
/// so an unknown scope is reported even when the stage graph cannot be read;
/// the graph is needed here only when there is no compiled grid to read.
fn scope_names(context: &Context, layout: &Layout) -> Result<Vec<String>, String> {
    if !runtime::grid_readable(&layout.scope_grid()) {
        context.stages()?;
    }
    Ok(runtime::valid_scopes(&enabled_scopes(layout)?))
}

fn print_lines<'a>(lines: impl IntoIterator<Item = &'a str>) -> Verb {
    let mut stdout = std::io::stdout().lock();
    for line in lines {
        writeln!(stdout, "{line}")?;
    }
    Ok(())
}

fn print_text(text: &str) -> Verb {
    std::io::stdout().write_all(text.as_bytes())?;
    Ok(())
}

fn run_query(verb: &str, args: &mut Args) -> Verb {
    let context = Context::take(args)?;
    let artifact = match verb {
        "producers" | "consumers" => Some(args.positional("artifact")?),
        _ => None,
    };
    args.finish()?;

    let (_, graph) = context.stages()?;
    let refs: Vec<&Stage> = graph.iter().collect();
    let lines: Vec<&str> = match (verb, artifact.as_deref()) {
        ("topo", _) => query::topo_sort(&refs)?,
        ("artifacts", _) => query::artifacts(&graph),
        ("producers", Some(artifact)) => query::producers_of(&graph, artifact)
            .iter()
            .map(|n| n.slug.as_str())
            .collect(),
        ("consumers", Some(artifact)) => query::consumers_of(&graph, artifact)
            .iter()
            .map(|n| n.slug.as_str())
            .collect(),
        _ => Vec::new(),
    };
    print_lines(lines)
}

fn run_cycles(args: &mut Args) -> Verb {
    let context = Context::take(args)?;
    // `--scope` with nothing after it checks the whole graph, as upstream does.
    let scope = args.following("--scope").flatten();
    args.finish()?;

    let cycles: Vec<String> = match scope {
        None => {
            let (_, graph) = context.stages()?;
            let refs: Vec<&Stage> = graph.iter().collect();
            query::find_cycles(&refs)
                .iter()
                .map(|cycle| cycle.join(" -> "))
                .collect()
        }
        Some(scope) => {
            let layout = context.layout()?;
            let valid = scope_names(&context, &layout)?;
            query::require_scope(&scope, &valid)?;
            let (_, graph) = context.stages()?;
            let grid = runtime::load_scope_grid(&layout.scope_grid(), &graph);
            let subgraph = query::subgraph_for_scope(&scope, &valid, &grid, &graph)?;
            query::find_cycles(&subgraph)
                .iter()
                .map(|cycle| cycle.join(" -> "))
                .collect()
        }
    };
    if cycles.is_empty() {
        return Ok(());
    }
    print_lines(cycles.iter().map(String::as_str))?;
    Err(Stop::Reported)
}

fn run_scope(args: &mut Args) -> Verb {
    let context = Context::take(args)?;
    let scope = args.positional("scope")?;
    args.finish()?;

    let layout = context.layout()?;
    let valid = scope_names(&context, &layout)?;
    query::require_scope(&scope, &valid)?;
    let (_, graph) = context.stages()?;
    let grid = runtime::load_scope_grid(&layout.scope_grid(), &graph);
    let subgraph = query::subgraph_for_scope(&scope, &valid, &grid, &graph)?;
    print_lines(subgraph.iter().map(|stage| stage.slug.as_str()))
}

fn run_validate_scope(args: &mut Args) -> Verb {
    let context = Context::take(args)?;
    let scope = args.positional("scope")?;
    args.finish()?;

    let layout = context.layout()?;
    let valid = scope_names(&context, &layout)?;
    query::require_scope(&scope, &valid)?;
    let (_, graph) = context.stages()?;
    let grid = runtime::load_scope_grid(&layout.scope_grid(), &graph);
    let checked = query::validate_scope(&scope, None, &valid, &graph, &grid)?;
    for advisory in &checked.advisories {
        eprintln!("[advisory] {advisory}");
    }
    if checked.errors.is_empty() {
        return Ok(());
    }
    for error in &checked.errors {
        eprintln!("[error] {error}");
    }
    Err(Stop::Reported)
}

/// `brownfield` or `greenfield`, in any letter case.
fn project_type(verb: &str, raw: &str) -> Result<&'static str, Stop> {
    match raw.to_lowercase().as_str() {
        "brownfield" => Ok("brownfield"),
        "greenfield" => Ok("greenfield"),
        _ => Err(Stop::Said(format!(
            "{verb}: --project-type must be brownfield or greenfield (got \"{raw}\")."
        ))),
    }
}

fn csv(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(trim)
        .filter(|part| !part.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A `--space` or `--intent` value that is one path segment. Anything else
/// (a `../`, a separator) is not a name.
fn is_one_segment(value: &str) -> bool {
    !(value == "."
        || value == ".."
        || value.contains(['/', '\0'])
        || (cfg!(windows) && value.contains('\\')))
}

/// The ceremony kill switches, by the setting each forces off.
const CEREMONY_SWITCHES: [(&str, &str); 5] = [
    ("sensors", "DISABLE_SENSORS"),
    ("learnings", "DISABLE_LEARNINGS"),
    ("summary_confirmation", "DISABLE_SUMMARY_CONFIRMATION"),
    ("plan_approval", "DISABLE_PLAN_APPROVAL_GUARD"),
    ("collaborators", "DISABLE_COLLABORATORS"),
];

const GUARD_POLICY_RENAME_NOTICE: &str = "Change Control is now Guard Policy (--guard-policy, config key guard-policy, scope key guard_policy, memory heading ## Guard Policy). The old names still work in this release and are removed in the next minor.";

#[allow(clippy::too_many_lines)]
fn run_validate_grid(args: &mut Args) -> Verb {
    let context = Context::take(args)?;
    let explicit_proposal = args.has("--proposal");
    let proposal_arg = args.value("--proposal")?;
    let strict = args.flag("--strict");
    let report = args.flag("--report");
    let custom = args.flag("--custom");

    let matched = match args.following("--matched") {
        Some(Some(value)) if !value.starts_with("--") => Some(value),
        Some(_) => {
            return Err(Stop::Said(
                "validate-grid: --matched requires <stock-scope>.".to_owned(),
            ));
        }
        None => None,
    };
    if matched.is_some() && custom {
        return Err(Stop::Said(
            "validate-grid: pass --matched <stock-scope> or --custom, not both.".to_owned(),
        ));
    }
    let keywords = match args.following("--keywords") {
        Some(Some(value)) if !value.starts_with("--") => Some(csv(&value)),
        Some(_) => {
            return Err(Stop::Said(
                "validate-grid: --keywords requires a comma-separated value.".to_owned(),
            ));
        }
        None => None,
    };
    // A trailing --project-type with no value reads as unset here (and as an
    // error in `ars`); both follow upstream.
    let project_type = match args.following("--project-type").flatten() {
        Some(raw) => Some(project_type("validate-grid", &raw)?),
        None => None,
    };
    let policy_flag = args
        .following("--guard-policy")
        .map(|value| ("--guard-policy", value))
        .or_else(|| {
            args.following("--change-control").map(|value| {
                eprintln!("{GUARD_POLICY_RENAME_NOTICE}");
                ("--change-control", value)
            })
        });
    let space = args.following("--space").flatten();
    let intent = args.following("--intent").flatten();
    args.finish()?;

    let layout = context.layout()?;
    let proposal_path = proposal_arg.unwrap_or_else(|| {
        layout
            .composer_proposal(&layout.active_space())
            .display()
            .to_string()
    });
    let read = if proposal_path == "-" || proposal_path == "/dev/stdin" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map(|_| text)
            .map_err(|err| err.to_string())
    } else {
        fs::read(&proposal_path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|err| err.to_string())
    };
    let document = read
        .and_then(|text| json::parse(&text).map_err(|err| err.to_string()))
        .map_err(|err| {
            Stop::Said(format!(
                "validate-grid: cannot read {proposal_path}: {err}{}",
                if explicit_proposal {
                    ""
                } else {
                    ". Write the grid to the proposalPath that `workspace detect --json` prints, or pass --proposal <path>."
                }
            ))
        })?;
    let Some(grid_proposed) = proposal::grid_of(&document) else {
        return Err(Stop::Said(
            "validate-grid: proposal must be a JSON object of {\"<stage-slug>\": \"EXECUTE\"|\"SKIP\"} (or {stages: {...}}).".to_owned(),
        ));
    };

    let (_, graph) = context.stages()?;
    let grid = runtime::load_scope_grid(&layout.scope_grid(), &graph);

    let guard_policy = match &policy_flag {
        Some((_, Some(value))) if !value.starts_with("--") => Some(value.as_str()),
        Some((flag, _)) => {
            return Err(Stop::Said(format!(
                "validate-grid: {flag} requires <strict|relaxed|off>."
            )));
        }
        None => None,
    };
    let mapping: Vec<ScopeDefinition> = runtime::scope_mapping(
        &enabled_scopes(&layout)?,
        &runtime::load_scope_grid_for_mapping(&layout.scope_grid(), &graph),
    );

    // The memory tree consulted is the named space's, else the active one. A
    // value that is not a single name falls back to the default space.
    let memory_space = if [&space, &intent]
        .into_iter()
        .flatten()
        .any(|value| !is_one_segment(value))
    {
        DEFAULT_SPACE.to_owned()
    } else {
        space.unwrap_or_else(|| layout.active_space())
    };
    let memory_dir = layout.memory_dir(&memory_space);
    let memory_strict = || -> focus_js::Result<Option<String>> {
        Ok(guard_memory::declarations(&memory_dir)?
            .iter()
            .find(|declaration| declaration.value == scopes::GuardPolicy::Strict)
            .map(guard_memory::Declaration::strict_refusal))
    };
    // Only the environment is read. A switch recorded in a settings file is
    // not seen until the settings reader exists.
    let kill_switch = |key: &str| -> Option<String> {
        let (_, name) = CEREMONY_SWITCHES.iter().find(|(k, _)| *k == key)?;
        env::is_on(name).then(|| env::full_name(name))
    };

    let (result, valid) = proposal::validate(
        &document,
        &grid_proposed,
        &ProposalRequest {
            strict,
            project_type,
            report,
            matched: matched.as_deref(),
            custom,
            keywords,
            guard_policy,
        },
        &ProposalWorld {
            graph: &graph,
            grid: &grid,
            mapping: &mapping,
            memory_strict: &memory_strict,
            kill_switch: &kill_switch,
        },
    )?;
    print_text(&format!("{}\n", json::stringify_pretty(&result)))?;
    if valid { Ok(()) } else { Err(Stop::Reported) }
}

fn run_ars(args: &mut Args) -> Verb {
    let context = Context::take(args)?;
    let mut scores = [0.0_f64; 5];
    for (idx, component) in focus_graph::stage_schema::ARS_COMPONENT_KEYS
        .iter()
        .enumerate()
    {
        let flag = format!("--{component}");
        let raw = args
            .value(&flag)?
            .ok_or_else(|| format!("Missing required flag: {flag} <value>"))?;
        let score = num::from_str(&raw);
        if trim(&raw).is_empty() || !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err(Stop::Said(format!(
                "ars: {flag} must be a number in [0.00, 1.00] (got \"{raw}\")."
            )));
        }
        if num::from_str(&num::to_fixed(score, 2)) != score {
            return Err(Stop::Said(format!(
                "ars: {flag} must have at most two decimals (got \"{raw}\")."
            )));
        }
        scores[idx] = score;
    }
    let completed = match args.following("--completed") {
        Some(Some(value)) if !value.starts_with("--") => csv(&value),
        Some(_) => {
            return Err(Stop::Said(
                "ars: --completed requires a comma-separated value.".to_owned(),
            ));
        }
        None => Vec::new(),
    };
    let project_type = match args.following("--project-type") {
        Some(Some(raw)) => Some(project_type("ars", &raw)?),
        Some(None) => {
            return Err(Stop::Said(
                "ars: --project-type requires a value (brownfield or greenfield).".to_owned(),
            ));
        }
        None => None,
    };
    args.finish()?;

    let layout = context.layout()?;
    let priors = ars::load_priors(&layout.ars_priors())?;
    let (all, graph) = context.stages()?;
    let grid = runtime::load_scope_grid(&layout.scope_grid(), &graph);
    let result = ars::compute(
        scores,
        &ars::ArsContext {
            completed: &completed,
            project_type,
        },
        &priors,
        &graph,
        &all,
        &grid,
    )?;
    print_text(&format!("{}\n", json::stringify_pretty(&result)))
}

fn run_resolve(args: &mut Args) -> Verb {
    if !env::is_on("GRAPH_RESOLVE") {
        return Err(Stop::Said(format!(
            "focus graph resolve is gated behind {}=1 (rollout flag).",
            env::full_name("GRAPH_RESOLVE")
        )));
    }
    let context = Context::take(args)?;
    let to_stdout = args.flag("--stdout");
    let scope = args.positional("scope")?;
    args.finish()?;

    let layout = context.layout()?;
    let valid = scope_names(&context, &layout)?;
    query::require_scope(&scope, &valid)?;
    let (_, graph) = context.stages()?;
    let grid = runtime::load_scope_grid(&layout.scope_grid(), &graph);
    let plan: Vec<Value> = query::resolve_plan_for_scope(&scope, &valid, &grid, &graph)?
        .into_iter()
        .map(|step| {
            let mut out = Map::new();
            out.insert("slug".to_owned(), Value::from(step.slug));
            out.insert("phase".to_owned(), Value::from(step.phase));
            out.insert(
                "action".to_owned(),
                Value::from(if step.execute { "EXECUTE" } else { "SKIP" }),
            );
            Value::Object(out)
        })
        .collect();
    let text = format!("{}\n", json::stringify_pretty(&Value::Array(plan)));
    if to_stdout {
        return print_text(&text);
    }
    // The default target is the active intent record's engine directory, and
    // finding that record is the state reader's job.
    let Some(path) = env::path("PLAN_PATH").map(PathBuf::from) else {
        return Err(format!(
            "writing the plan into the active intent record needs the state reader, which Focus does not have yet. Pass --stdout, or set {} to a file.",
            env::full_name("PLAN_PATH")
        )
        .into());
    };
    write_atomic(&path, text.as_bytes()).map_err(|err| format!("{}: {err}", path.display()))?;
    print_lines([path.display().to_string().as_str()])
}

fn run_export(args: &mut Args) -> Verb {
    let context = Context::take(args)?;
    let check = args.value("--check")?;
    args.finish()?;

    let layout = context.layout()?;
    let (_, graph) = context.stages()?;
    let mapping = runtime::scope_mapping(
        &enabled_scopes(&layout)?,
        &runtime::load_scope_grid_for_mapping(&layout.scope_grid(), &graph),
    );
    let agents = agents::load_agents(&layout.agents_dir())?;
    let text = export::export_json(&graph, &mapping, &agents);
    let Some(fixture) = check else {
        return print_text(&text);
    };
    let Ok(expected) = fs::read_to_string(&fixture) else {
        return Err(Stop::Said(format!(
            "export --check: fixture not found at {fixture}"
        )));
    };
    if text != expected {
        return Err(Stop::Said(format!(
            "export --check: bundle drift vs {fixture}. Regenerate with: focus engine graph export > {fixture}"
        )));
    }
    Ok(())
}

fn run_compile(args: &mut Args) -> Verb {
    if args.has("--core") || args.has("--installed") {
        return run_compile_outside(args);
    }
    let context = Context::take(args)?;
    let check = args.flag("--check");
    args.finish()?;

    let layout = context.layout()?;
    if check {
        let compiled = compile(&Sources::from_layout(&layout))?;
        report_advisories(&compiled);
        return check_compiled(&layout.data_dir(), &compiled);
    }
    // One lock around restore, compile, write and back-fill: a second compile
    // waits and then runs against the first one's output.
    write::require_installed(&layout)?;
    let lock = WorkspaceLock::acquire(&layout, "graph compile")?;
    write::write_compiled(&layout, &lock, report_advisories)?;
    Ok(())
}

fn report_advisories(compiled: &Compiled) {
    for advisory in &compiled.advisories {
        eprintln!("{advisory}");
    }
}

fn run_compile_outside(args: &mut Args) -> Verb {
    let harness_name = args
        .value("--harness")?
        .ok_or("Missing required flag: --harness <value>")?;
    let harness = Harness::from_name(&harness_name).ok_or_else(|| {
        let known: Vec<&str> = Harness::ALL.iter().map(|h| h.name()).collect();
        format!(
            "Unknown harness: {harness_name}. Valid: {}",
            known.join(", ")
        )
    })?;
    let core = args.value("--core")?.map(PathBuf::from);
    let installed = args.value("--installed")?.map(PathBuf::from);
    let rules_dir = args.value("--rules-dir")?.map(PathBuf::from);
    let out = args.value("--out")?.map(PathBuf::from);
    let print = args.value("--print")?;
    let check = args.optional_value("--check");
    args.finish()?;

    let mut sources = match (&core, &installed) {
        (Some(core), None) => Sources::from_core(core, harness),
        (None, Some(root)) => Sources::from_installed(root, harness),
        _ => return Err("Pass exactly one of --core <dir> or --installed <dir>".into()),
    };
    if let Some(rules_dir) = rules_dir {
        sources.rules_dir = rules_dir;
    }

    let compiled = compile(&sources)?;
    report_advisories(&compiled);

    match (out, check, print) {
        (Some(dir), None, None) => {
            // The tree being read, or any other installed tree: a harness
            // manifest marks the data directory of one.
            if dir.join(HARNESS_MANIFEST).exists()
                || sources
                    .data_dir
                    .as_deref()
                    .is_some_and(|data_dir| same_dir(&dir, data_dir))
            {
                return Err(IN_PLACE_REFUSAL.into());
            }
            write_compiled(&dir, &compiled)
        }
        (None, Some(dir), None) => {
            let dir = match (dir, &sources.data_dir) {
                (Some(dir), _) => PathBuf::from(dir),
                (None, Some(data_dir)) => data_dir.clone(),
                (None, None) => {
                    return Err("--check needs a directory when reading --core".into());
                }
            };
            check_compiled(&dir, &compiled)
        }
        (None, None, Some(which)) => {
            let text = match which.as_str() {
                "stage-graph" => &compiled.stage_graph_json,
                "scope-grid" => &compiled.scope_grid_json,
                other => {
                    return Err(format!(
                        "Unknown --print target: {other}. Valid: stage-graph, scope-grid"
                    )
                    .into());
                }
            };
            print_text(text)
        }
        (None, None, None) if installed.is_some() => Err(IN_PLACE_REFUSAL.into()),
        (None, None, None) => {
            Err("Pass one of --out <dir>, --check <dir> or --print <which>".into())
        }
        _ => Err("Pass only one of --out, --check and --print".into()),
    }
}

/// Writing an installed tree's compiled files happens under the workspace
/// lock, together with the composed-scope reconciliation. This form of the
/// command does neither, so it does not write there.
const HARNESS_MANIFEST: &str = "harness.json";

const IN_PLACE_REFUSAL: &str = "this form does not write into an installed tree. Run `focus engine graph compile --project-dir <project> --harness <name>`, which writes in place under the workspace lock, or pass --out <another dir>, --check or --print.";

/// Whether two paths name the same existing directory.
fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn write_compiled(dir: &Path, compiled: &Compiled) -> Verb {
    for (name, text) in [
        ("stage-graph.json", &compiled.stage_graph_json),
        ("scope-grid.json", &compiled.scope_grid_json),
    ] {
        let path = dir.join(name);
        write_atomic(&path, text.as_bytes()).map_err(|err| format!("{}: {err}", path.display()))?;
        println!("{}", path.display());
    }
    Ok(())
}

fn check_compiled(dir: &Path, compiled: &Compiled) -> Verb {
    let stale = |name: &str| {
        Stop::Said(format!(
            "{name} is out of date. Run `focus engine graph compile` to regenerate."
        ))
    };
    let on_disk = |name: &str| fs::read_to_string(dir.join(name)).unwrap_or_default();
    if on_disk("stage-graph.json") != compiled.stage_graph_json {
        return Err(stale("stage-graph.json"));
    }
    // The grid is compared after re-emitting the on-disk copy through the
    // canonical emitter with its scopes sorted: a column appended at the end
    // of the file is the same grid, and must not read as drift.
    let grid = on_disk("scope-grid.json");
    let normalised = match json::parse(&grid) {
        Ok(Value::Object(map)) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|(a, _), (b, _)| cmp_utf16(a, b));
            canonical_scope_grid_json(&js_key_order(entries.into_iter().collect::<Map<_, _>>()))
        }
        _ => grid,
    };
    if normalised != compiled.scope_grid_json {
        return Err(stale("scope-grid.json"));
    }
    Ok(())
}

/// `focus dev oracle <cases.ndjson>`: answer each case on its own line as
/// `{"id": ..., "out": ...}`, the shape `tools/oracle/driver.ts` prints.
pub fn oracle(args: &mut Args) -> Result<(), String> {
    let path = args.positional("cases.ndjson")?;
    args.finish()?;
    let file = fs::File::open(&path).map_err(|err| format!("{path}: {err}"))?;
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    for line in std::io::BufReader::new(file).lines() {
        let line = line.map_err(|err| format!("{path}: {err}"))?;
        if line.is_empty() {
            continue;
        }
        // Parsed as upstream's driver parses it, so nested objects are in the
        // order a JS object enumerates them.
        let case = json::parse(&line).map_err(|err| format!("{path}: {err}"))?;
        let mut answer = Map::new();
        answer.insert(
            "id".to_owned(),
            case.get("id").cloned().unwrap_or(Value::Null),
        );
        answer.insert("out".to_owned(), cases::run_case(&case));
        writeln!(stdout, "{}", json::stringify(&Value::Object(answer)))
            .map_err(|err| err.to_string())?;
    }
    stdout.flush().map_err(|err| err.to_string())
}

//! `focus engine runtime <verb>`.
//!
//! The runtime graph of the active record: `compile` reads the ledger, the
//! state file and the stages' diaries and writes `runtime-graph.json`; `read`
//! prints one stage's row of it; `summary` prints its numbers. The engine
//! compiles after every event that moves a workflow on, so calling `compile`
//! by hand is for looking, and it changes nothing a workflow decides.
//!
//! Compiling does write two things, both under the workspace lock: the graph,
//! and one `MEMORY_EMPTY` event for each stage approved with an empty diary,
//! once per approval.
//!
//! A verb that refuses says so in a line on standard error,
//! `focus runtime <verb>: <why>`, with exit code 1, as upstream's does. What
//! is wrong before a verb is reached (no verb, one that does not exist, a
//! flag without its value) is a line with no such prefix.
//!
//! The record's own files (the state file, a diary, the units file, the
//! compiled graph) are opened by name and read, links followed, because that
//! is how upstream reads them here and what they hold decides nothing: the
//! graph is a reading, and anything that carries authority is read back from
//! the ledger, whose shards are opened without trusting their paths.
//!
//! Mirrors `main`, `handleCompile`, `handleRead`, `handleSummary`,
//! `writeEmptyGraph`, `readMemory`, `computeBoltDag` and the locked section
//! of `compile` in `aidlc-runtime.ts`. `fragment-fork` and `fragment-merge`
//! copy the graph into a bolt's worktree and remove it again; they come with
//! bolts and worktrees, and are refused by name.

use std::fs;
use std::path::{Path, PathBuf};

use focus_audit::ledger::{Ledger, Machine};
use focus_audit::memory::{diary_path, parse_memory_headings};
use focus_audit::read::find_all_events;
use focus_audit::runtime_graph::{self, Place, Sources};
use focus_audit::units::parse_bolt_dag;
use focus_audit::{Entry, Fields, summary};
use focus_graph::runtime::{enabled, load_stages_all};
use focus_harness::Harness;
use focus_js::json;
use focus_js::order::cmp_utf16;
use focus_js::re;
use focus_workspace::atomic::write_atomic;
use focus_workspace::names::STATE_FILE;
use focus_workspace::records::{self, Invoker, Selection, Selector};
use focus_workspace::{Layout, WorkspaceLock, env, observer, project};
use serde_json::{Map, Value, json};

use crate::Failure;
use crate::args::Args;

const HELP: &str = "\
Usage: focus engine runtime <verb> [--project-dir <dir>]

The runtime graph of the active record: what the workflow has done so far,
read off its audit ledger.

  compile            Read the ledger, the state file and the stages' diaries,
                     and write runtime-graph.json
  read <stage-slug>  Print one stage's row of runtime-graph.json
  summary [--json]   Print the graph's numbers: stages by outcome and phase,
                     diary entries, sensors, learnings, duration

The engine compiles after every event that moves a workflow on; running
compile by hand is for looking.";

const USAGE: &str = "Usage: focus engine runtime <subcommand>. Valid: compile, read, summary. Run with --help for detail.";

/// What a verb answered with: a refusal already carries the verb's name.
type Verb = Result<(), String>;

pub fn run(args: &mut Args) -> Result<(), Failure> {
    let answer = (|| -> Verb {
        // `--project-dir <dir>` is taken wherever it stands, before the verb
        // is looked at.
        let project_dir = args.value("--project-dir")?;
        let harness = args.value("--harness")?;
        let Some(verb) = args.next_positional() else {
            return Err(USAGE.to_owned());
        };
        if verb == "--help" || verb == "-h" {
            println!("{HELP}");
            return Ok(());
        }
        let context = Context {
            project_dir,
            harness,
        };
        let named = |outcome: Verb| outcome.map_err(|why| format!("focus runtime {verb}: {why}"));
        match verb.as_str() {
            "compile" => named(compile(args, &context)),
            "read" => named(read(args, &context)),
            "summary" => named(summarize(args, &context)),
            "fragment-fork" | "fragment-merge" => Err(format!(
                "{verb} copies the runtime graph into a bolt's worktree or removes it again, and Focus does not work with bolts and worktrees yet."
            )),
            other => Err(format!(
                "Unknown subcommand: {other}. Run focus engine runtime --help for usage."
            )),
        }
    })();
    answer.map_err(Failure::from)
}

struct Context {
    project_dir: Option<String>,
    harness: Option<String>,
}

impl Context {
    fn project(&self) -> Result<PathBuf, String> {
        project::resolve_project_dir(self.project_dir.as_deref().map(Path::new))
            .map_err(|err| err.to_string())
    }

    /// The workspace of the project. No path of it depends on the harness,
    /// so a project with several installed, or none, is not asked which.
    fn workspace(&self) -> Result<Layout, String> {
        let project = self.project()?;
        let harness = if self.names_a_harness() {
            project::resolve_harness(&project, self.harness.as_deref())
                .map_err(|err| err.to_string())?
        } else {
            project::installed_harnesses(&project)
                .first()
                .copied()
                .unwrap_or(Harness::Claude)
        };
        Ok(Layout::new(project, harness))
    }

    fn names_a_harness(&self) -> bool {
        self.harness.is_some() || env::var("HARNESS").is_some_and(|harness| !harness.is_empty())
    }

    /// Where each enabled stage is in the compiled stage graph, which is the
    /// harness's own: here the harness has to be known. A phase is kept as
    /// the graph has it, text or not, for each verb to make of it what
    /// upstream does.
    fn places(&self) -> Result<Vec<(String, Placed)>, String> {
        let project = self.project()?;
        let harness = project::resolve_harness(&project, self.harness.as_deref())
            .map_err(|err| err.to_string())?;
        let all = load_stages_all(&Layout::new(project, harness).stage_graph())
            .map_err(|err| err.to_string())?;
        Ok(enabled(&all)
            .into_iter()
            .map(|stage| {
                let placed = Placed {
                    phase: stage.raw.get("phase").cloned(),
                    lead_agent: stage.raw.get("lead_agent").cloned(),
                };
                (stage.slug, placed)
            })
            .collect())
    }
}

/// A stage's row of the stage graph, as far as this command reads it.
struct Placed {
    phase: Option<Value>,
    lead_agent: Option<Value>,
}

/// The row of `slug`. A slug the graph has twice is the later row, as in
/// upstream's map of them.
fn placed<'a>(places: &'a [(String, Placed)], slug: &str) -> Option<&'a Placed> {
    places
        .iter()
        .rev()
        .find(|(known, _)| known == slug)
        .map(|(_, placed)| placed)
}

/// The record the command acts on, and where its files are.
struct Record {
    layout: Layout,
    selection: Selection,
    invoker: Invoker,
}

impl Record {
    fn find(context: &Context) -> Result<Self, String> {
        let layout = context.workspace()?;
        let invoker = Invoker::from_env();
        let selection = records::resolve_selection(&layout, &Selector::default(), &invoker)
            .map_err(|err| err.to_string())?;
        Ok(Self {
            layout,
            selection,
            invoker,
        })
    }

    fn file(&self, name: &str) -> PathBuf {
        self.selection.docs_root(&self.layout).join(name)
    }

    fn graph(&self) -> PathBuf {
        self.file("runtime-graph.json")
    }
}

/// A file of the record as text, as upstream reads one: bytes that are not
/// UTF-8 become U+FFFD.
fn read_text(path: &Path) -> Result<String, String> {
    fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|err| format!("{}: {err}", path.display()))
}

fn graph_text(graph: &Map<String, Value>) -> String {
    format!(
        "{}\n",
        json::stringify_pretty(&Value::Object(graph.clone()))
    )
}

fn write_graph(path: &Path, graph: &Map<String, Value>) -> Result<(), String> {
    observer::refuse_write(observer::WRITE_FILE_ATOMIC).map_err(|err| err.to_string())?;
    write_atomic(path, graph_text(graph).as_bytes())
        .map_err(|err| format!("{}: {err}", path.display()))
}

fn compile(args: &mut Args, context: &Context) -> Verb {
    args.finish()?;
    let record = Record::find(context)?;
    let state_path = record.file(STATE_FILE);
    if !state_path.exists() {
        eprintln!("focus runtime: no {STATE_FILE}, skipping (likely pre-init)");
        return Ok(());
    }
    let ledger = Ledger::new(&record.layout, Machine::this());
    let selector = Selector::default();
    let audit = ledger
        .read_all(&selector, &record.invoker)
        .map_err(|err| err.to_string())?;
    let state = || {
        if state_path.exists() {
            read_text(&state_path)
        } else {
            Err(format!("State file not found: {}", state_path.display()))
        }
    };
    let lock = || {
        WorkspaceLock::acquire(&record.layout, "compiling the runtime graph")
            .map_err(|err| err.to_string())
    };
    let written = || {
        println!(
            "{}",
            json::stringify(&json!({ "written": record.graph().display().to_string() }))
        );
        Ok(())
    };
    // A state file and nothing in the ledger yet, or no workflow in it: the
    // graph says only the scope, so that a reader need not treat "no graph"
    // as a case of its own.
    let Some(workflow) = runtime_graph::workflow(&audit) else {
        let graph = runtime_graph::empty_graph(&state()?);
        let _held = lock()?;
        write_graph(&record.graph(), &graph)?;
        return written();
    };
    let state = state()?;
    let places = context.places()?;
    let docs = record.selection.docs_root(&record.layout);
    let record_prefix = record.selection.relative_record_dir();
    let reading = runtime_graph::read(
        &audit,
        &workflow,
        &Sources {
            project: &record.layout.project,
            state: &state,
            record_prefix: record_prefix.as_deref(),
            place: &|slug| {
                placed(&places, slug).map(|placed| Place {
                    phase: placed
                        .phase
                        .as_ref()
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    lead_agent: placed.lead_agent.clone(),
                })
            },
            diary: &|phase, slug| {
                let path = diary_path(&docs, phase, slug);
                if !path.exists() {
                    return Ok(None);
                }
                let raw = read_text(&path).map_err(focus_js::EngineError::new)?;
                Ok(Some(parse_memory_headings(&raw)))
            },
        },
    )
    .map_err(|err| err.to_string())?;
    for remark in &reading.remarks {
        eprintln!("focus runtime: {remark}");
    }
    let mut graph = reading.graph;

    // The order the units can be built in, once Units Generation has written
    // it down. A block that cannot be read gives no order, and says so here:
    // the gate's own sensor is what stops a workflow on it.
    let dependencies = docs
        .join("inception")
        .join("units-generation")
        .join("unit-of-work-dependency.md");
    if dependencies.exists() {
        match parse_bolt_dag(&read_text(&dependencies)?) {
            Ok(dag) => {
                let units: Vec<Value> = dag
                    .units
                    .into_iter()
                    .map(|unit| {
                        let mut row = Map::new();
                        row.insert("name".to_owned(), json!(unit.name));
                        row.insert("depends_on".to_owned(), json!(unit.depends_on));
                        if let Some(kind) = unit.kind {
                            row.insert("kind".to_owned(), json!(kind));
                        }
                        Value::Object(row)
                    })
                    .collect();
                graph.insert(
                    "bolt_dag".to_owned(),
                    json!({ "units": units, "batches": dag.batches }),
                );
            }
            Err(unreadable) => eprintln!(
                "focus runtime: unit-of-work-dependency.md edge block {} ({}); bolt_dag node omitted",
                unreadable.reason, unreadable.detail
            ),
        }
    }

    // The events first, then the graph, in one hold of the lock. The ledger
    // is read again inside it: an empty diary is recorded once for each
    // approval, however many times the graph is compiled after it.
    let held = lock()?;
    let now = ledger
        .read_all(&selector, &record.invoker)
        .map_err(|err| err.to_string())?;
    let recorded = find_all_events(&now, "MEMORY_EMPTY", None);
    for empty in &reading.empty_diaries {
        let already = recorded.iter().any(|found| {
            re::find(r"^\*\*Stage\*\*:\s*(\S+)", "m", &found.block)
                .and_then(|stage| re::group(&found.block, &stage, 1))
                .is_some_and(|slug| slug == empty.slug)
                && cmp_utf16(&found.timestamp, &empty.completed_at).is_ge()
        });
        if already {
            continue;
        }
        let mut fields = Fields::new();
        fields.set("Stage", empty.slug.clone());
        ledger
            .append(
                &held,
                &[Entry::new("MEMORY_EMPTY", fields)],
                &selector,
                &record.invoker,
            )
            .map_err(|err| err.to_string())?;
    }
    write_graph(&record.graph(), &graph)?;
    written()
}

/// The compiled graph, parsed; `None` when the record has none.
fn compiled_graph(record: &Record) -> Result<Option<Value>, String> {
    let path = record.graph();
    if !path.exists() {
        return Ok(None);
    }
    json::parse(&read_text(&path)?)
        .map(Some)
        .map_err(|err| err.to_string())
}

fn read(args: &mut Args, context: &Context) -> Verb {
    let Some(slug) = args.next_positional().filter(|slug| !slug.is_empty()) else {
        return Err("stage slug required".to_owned());
    };
    args.finish()?;
    let record = Record::find(context)?;
    let no_row = || format!("no row for slug \"{slug}\"");
    let Some(graph) = compiled_graph(&record)? else {
        return Err(no_row());
    };
    let not_a_graph = |what: &str| {
        format!(
            "runtime-graph.json is not a runtime graph as the engine writes it: {what}. Run `focus engine runtime compile` to write it again."
        )
    };
    let Some(Value::Array(stages)) = graph.as_object().and_then(|graph| graph.get("stages")) else {
        return Err(not_a_graph("it has no list `stages`"));
    };
    for stage in stages {
        if stage.is_null() {
            return Err(not_a_graph("an entry of `stages` is null"));
        }
        if stage.get("stage_slug").and_then(Value::as_str) == Some(slug.as_str()) {
            println!("{}", json::stringify_pretty(stage));
            return Ok(());
        }
    }
    Err(no_row())
}

fn summarize(args: &mut Args, context: &Context) -> Verb {
    let as_json = args.flag("--json");
    args.finish()?;
    let record = Record::find(context)?;
    let Some(graph) = compiled_graph(&record)? else {
        return Err("no runtime-graph.json found \u{2014} run a workflow first".to_owned());
    };
    let places = context.places()?;
    let state_path = record.file(STATE_FILE);
    let overlay = if state_path.exists() {
        focus_state::completed_overlay(&read_text(&state_path)?)
    } else {
        None
    };
    let numbers = summary::summarize(
        &graph,
        // A stage the graph does not have, or has with no phase, is counted
        // under `unknown`. Upstream counts one whose phase is some other
        // value under that value's name as JavaScript prints it; that is
        // refused here, as a stage graph the compile did not write.
        &|slug| match placed(&places, slug).and_then(|placed| placed.phase.as_ref()) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(phase)) => Ok(Some(phase.clone())),
            Some(_) => Err(focus_js::EngineError::new(format!(
                "stage-graph.json gives the stage {slug} a phase that is not text. The stage graph is written by `focus engine graph compile`: compile it again."
            ))),
        },
        overlay.as_deref(),
    )
    .map_err(|err| err.to_string())?;
    if as_json {
        println!("{}", json::stringify_pretty(&Value::Object(numbers)));
    } else {
        println!("{}", summary::render(&numbers));
    }
    Ok(())
}

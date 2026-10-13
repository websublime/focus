//! The runtime graph: what a workflow has done so far, read off its ledger.
//!
//! `stage-graph.json` says which stages exist and how they depend on each
//! other. The runtime graph is its counterpart for one workflow: which stages
//! started and when, which were approved, who ran them, how many entries each
//! stage's diary holds, which sensors fired and how each ended, what was
//! learned at each gate, and, for a stage built by several bolts side by
//! side, one row per bolt. Gates, the lifecycle and the session summary read
//! it instead of walking the ledger themselves.
//!
//! Nothing in it is decided here. Every field is a reading of the ledger,
//! the state file and the diaries, and compiling the same ledger twice gives
//! the same bytes: "now" is never the clock, it is the latest timestamp the
//! ledger holds.
//!
//! This module is the reading. It touches no file: the caller hands it the
//! ledger as text and two ways to ask (where a stage is in the stage graph,
//! what a stage's diary holds), and writes what comes back.
//!
//! Mirrors `compile`, `pairStartedCompleted` and `buildWorkflowHeader` in
//! `aidlc-runtime.ts` (L319, L175, L242), and `resolveAuditProjectPath` in
//! `aidlc-lib.ts` (L14979).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::Path;

use focus_js::date::{self, Instant};
use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{Result, bail, re};
use focus_workspace::names::{DEFAULT_SPACE, WORKSPACE_DIR};
use serde_json::{Map, Value, json};

use crate::entry::{PROJECT_DIR_TOKEN, lexical_absolute_path};
use crate::memory::MemoryCounts;
use crate::read::{Found, bullet_field, find_all_events};

/// How long after it fired a sensor with no ending is called incomplete,
/// while its stage is still open. In a stage that has closed, the closing is
/// the cutoff.
const ORPHAN_CUTOFF_MS: i64 = 60_000;

/// Where a stage is in the stage graph: its phase (`None` when the graph has
/// none that is text), and the agent that leads it as the compiled graph has
/// it (absent when the graph names none).
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub phase: Option<String>,
    pub lead_agent: Option<Value>,
}

/// What the reading needs besides the ledger.
pub struct Sources<'a> {
    /// The project, for the worktree paths the ledger records relative to it.
    pub project: &'a Path,
    /// The state file's text.
    pub state: &'a str,
    /// The record directory relative to the project, with forward slashes;
    /// `None` while no intent resolves.
    pub record_prefix: Option<&'a str>,
    /// A stage's place in the stage graph; `None` for a slug it does not have.
    pub place: &'a dyn Fn(&str) -> Option<Place>,
    /// The counts of the diary of the stage `slug` of `phase`; `None` when
    /// the stage has no diary file, and an error when it has one that cannot
    /// be read.
    pub diary: &'a dyn Fn(&str, &str) -> Result<Option<MemoryCounts>>,
}

/// A stage approved with an empty diary: an event the ledger should carry,
/// once for each approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmptyDiary {
    pub slug: String,
    pub completed_at: String,
}

/// What a ledger reads as.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// The graph, without the units' order, which comes from another file.
    pub graph: Map<String, Value>,
    pub empty_diaries: Vec<EmptyDiary>,
    /// What upstream says on standard error while reading, without the
    /// name it prefixes each line with.
    pub remarks: Vec<String>,
}

fn before(a: &str, b: &str) -> bool {
    cmp_utf16(a, b) == Ordering::Less
}

/// Inside `[start, end)`; an open window has no end.
fn within(timestamp: &str, start: &str, end: Option<&str>) -> bool {
    !before(timestamp, start) && end.is_none_or(|end| before(timestamp, end))
}

/// The value of the first line of `block` that starts with the bold label
/// `name`, trimmed. Unlike the state file's reader this takes no bullet, and
/// a label that is there twice answers with the first.
fn field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let label = format!("**{name}**:");
    block
        .split('\n')
        .find_map(|line| line.strip_prefix(label.as_str()))
        .map(trim)
}

/// [`field`], when the label is there and has a value.
fn filled<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    field(block, name).filter(|value| !value.is_empty())
}

/// The first capture of `pattern` in `block`. The patterns are upstream's,
/// `\s*` after the label and all: a label with nothing after it reads its
/// value off the next line.
fn captured<'a>(block: &'a str, pattern: &str) -> Option<&'a str> {
    let found = re::find(pattern, "m", block)?;
    re::group(block, &found, 1)
}

/// A path the ledger recorded, as an absolute one: the project's own
/// location is recorded as a token, a relative path is relative to the
/// project, and an absolute one stands as written.
#[must_use]
pub fn resolve_recorded_path(project: &Path, recorded: &str) -> String {
    let resolve = |rest: &str| {
        lexical_absolute_path(&project.join(rest))
            .to_string_lossy()
            .into_owned()
    };
    if recorded == PROJECT_DIR_TOKEN {
        return resolve("");
    }
    if let Some(rest) = recorded
        .strip_prefix(PROJECT_DIR_TOKEN)
        .and_then(|rest| rest.strip_prefix(['/', '\\']))
    {
        return resolve(rest);
    }
    if Path::new(recorded).has_root() {
        recorded.to_owned()
    } else {
        resolve(recorded)
    }
}

/// The graph of a workflow nothing is known of yet beyond its scope: there
/// is a state file, and no ledger or no workflow in it.
#[must_use]
pub fn empty_graph(state: &str) -> Map<String, Value> {
    let scope = bullet_field(state, "Scope").unwrap_or_default();
    let mut graph = Map::new();
    graph.insert("workflow_id".to_owned(), json!(""));
    graph.insert("scope".to_owned(), json!(scope));
    graph.insert("started_at".to_owned(), json!(""));
    graph.insert("stages".to_owned(), json!([]));
    graph
}

/// A run of one stage alone (`--single`) commits its events under a workflow
/// id of its own. They belong to no workflow and never enter its graph.
fn single_stage(block: &str) -> bool {
    re::test(r"^\*\*Workflow\*\*:\s*single-stage:", "m", block)
}

struct Pairing {
    started_at: String,
    completed_at: Option<String>,
    agent: String,
    /// The order the stages last started in, to settle equal timestamps.
    started_index: usize,
}

/// Each stage's latest start and the completion after it. A stage that
/// starts again (a jump back) is a new entry: the row shows the current
/// attempt, and a completion counts once.
fn pair_started_completed(audit: &str, since: &str) -> Vec<(String, Pairing)> {
    let of_this_workflow = |event: &str| -> Vec<Found> {
        find_all_events(audit, event, None)
            .into_iter()
            .filter(|found| !before(&found.timestamp, since) && !single_stage(&found.block))
            .collect()
    };
    // One stream in time order. Starts go before completions of the same
    // instant, each kind in the order it was read; the numbers are
    // upstream's, so more than a hundred thousand starts tie as its do.
    let mut stream: Vec<(bool, usize, Found)> = Vec::new();
    for (index, found) in of_this_workflow("STAGE_STARTED").into_iter().enumerate() {
        stream.push((true, index, found));
    }
    for (index, found) in of_this_workflow("STAGE_COMPLETED").into_iter().enumerate() {
        stream.push((false, index + 100_000, found));
    }
    stream.sort_by(|a, b| cmp_utf16(&a.2.timestamp, &b.2.timestamp).then(a.1.cmp(&b.1)));

    let mut stages: HashMap<String, Pairing> = HashMap::new();
    let mut started_index = 0;
    for (started, _, found) in stream {
        let Some(slug) = captured(&found.block, r"^\*\*Stage\*\*:\s*(\S+)") else {
            continue;
        };
        if started {
            let agent = captured(&found.block, r"^\*\*Agent\*\*:\s*(.+)$").map_or("", trim);
            stages.insert(
                slug.to_owned(),
                Pairing {
                    started_at: found.timestamp,
                    completed_at: None,
                    agent: agent.to_owned(),
                    started_index,
                },
            );
            started_index += 1;
        } else if let Some(entry) = stages.get_mut(slug)
            && entry.completed_at.is_none()
        {
            entry.completed_at = Some(found.timestamp);
        }
    }
    let mut in_order: Vec<(String, Pairing)> = stages.into_iter().collect();
    // In the order of their latest starts. The stream was in time order, so
    // the count of starts is: upstream compares the timestamps first, and
    // that never decides anything the count would not.
    in_order.sort_by_key(|(_, entry)| entry.started_index);
    in_order
}

/// How a sensor's firing ended.
struct Terminal {
    result: &'static str,
    timestamp: String,
    detail_path: Option<String>,
}

/// One bolt's work on a stage: from the fork of its state to the merge.
struct Instance {
    bolt: String,
    worktree: String,
    started_at: String,
    completed_at: Option<String>,
    outcome: &'static str,
    firings: Vec<Value>,
}

struct Row {
    slug: String,
    started_at: String,
    completed_at: Option<String>,
    agent: Option<Value>,
    memory_path: String,
    diary: Option<MemoryCounts>,
    outcome: &'static str,
    learnings: Option<(u64, u64)>,
    firings: Vec<Value>,
    instances: Vec<Instance>,
}

fn nullable(text: Option<&str>) -> Value {
    text.map_or(Value::Null, Value::from)
}

impl Row {
    fn into_json(self) -> Value {
        let by_bolts = !self.instances.is_empty();
        let mut row = Map::new();
        row.insert("stage_slug".to_owned(), json!(self.slug));
        // A stage built by several bolts keeps its times, its agent and its
        // diary counts on the bolts' rows.
        let own = |value: Value| if by_bolts { Value::Null } else { value };
        row.insert("started_at".to_owned(), own(json!(self.started_at)));
        row.insert(
            "completed_at".to_owned(),
            own(nullable(self.completed_at.as_deref())),
        );
        if by_bolts {
            row.insert("agent".to_owned(), Value::Null);
        } else if let Some(agent) = self.agent {
            row.insert("agent".to_owned(), agent);
        }
        row.insert("memory_path".to_owned(), json!(self.memory_path));
        let diary = if by_bolts { None } else { self.diary };
        row.insert(
            "memory_entries".to_owned(),
            diary.map_or(Value::Null, |counts| json!(counts.total())),
        );
        row.insert(
            "memory_breakdown".to_owned(),
            diary.map_or(Value::Null, |counts| {
                json!({
                    "interpretations": counts.interpretations,
                    "deviations": counts.deviations,
                    "tradeoffs": counts.tradeoffs,
                    "open_questions": counts.open_questions,
                })
            }),
        );
        row.insert("sensor_firings".to_owned(), Value::Array(self.firings));
        row.insert("outcome".to_owned(), json!(self.outcome));
        row.insert(
            "learnings_captured".to_owned(),
            self.learnings.map_or(Value::Null, |(orchestrator, user)| {
                json!({ "from_orchestrator": orchestrator, "from_user_addition": user })
            }),
        );
        if by_bolts {
            let memory_path = self.memory_path;
            let instances = self.instances.into_iter().map(|instance| {
                json!({
                    "bolt": instance.bolt,
                    "worktree": instance.worktree,
                    "started_at": instance.started_at,
                    "completed_at": instance.completed_at,
                    "memory_path": memory_path,
                    "memory_entries": null,
                    "memory_breakdown": null,
                    "sensor_firings": instance.firings,
                    "outcome": instance.outcome,
                })
            });
            row.insert("instances".to_owned(), instances.collect());
        }
        Value::Object(row)
    }
}

/// The workflow a ledger is about: the latest one started. Starting over
/// appends to the same ledger, and the first row would name a dead workflow.
/// `None` when the ledger holds no `WORKFLOW_STARTED`: the graph is then
/// [`empty_graph`].
#[must_use]
pub fn workflow(audit: &str) -> Option<Found> {
    find_all_events(audit, "WORKFLOW_STARTED", None).pop()
}

/// Read `audit`, every shard of a record as one text, as the graph of
/// `workflow` (see [`workflow`]).
///
/// # Errors
///
/// A diary cannot be read. Or a sensor fired in a stage that is still open
/// and never ended, and how long ago that was cannot be told: its timestamp
/// or the latest one of the ledger has digits and is not an instant written
/// with its zone (see [`focus_js::date`]).
pub fn read(audit: &str, workflow: &Found, sources: &Sources) -> Result<Reading> {
    let events = |event: &str| find_all_events(audit, event, None);
    let scope = bullet_field(sources.state, "Scope")
        .filter(|scope| !scope.is_empty())
        .or_else(|| captured(&workflow.block, r"^\*\*Scope\*\*:\s*(.+)$").map(trim))
        .unwrap_or_default();

    let memory_prefix = sources.record_prefix.map_or_else(
        // The default space by name, whichever space is active: upstream's
        // fallback takes no space.
        || format!("{WORKSPACE_DIR}/spaces/{DEFAULT_SPACE}/intents"),
        str::to_owned,
    );
    let mut rows: Vec<Row> = Vec::new();
    let mut phases: Vec<String> = Vec::new();
    let mut empty_diaries = Vec::new();
    for (slug, entry) in pair_started_completed(audit, &workflow.timestamp) {
        // A slug the stage graph does not have is left out, not refused.
        let Some(place) = (sources.place)(&slug) else {
            continue;
        };
        // The phase is a directory of the stage's diary. Upstream fails here
        // too, with what its runtime says of joining a path that is no text.
        let Some(phase) = place.phase else {
            bail!(
                "stage-graph.json gives the stage {slug} no phase that is text, so where its diary is cannot be told. The stage graph is written by `focus engine graph compile`: compile it again."
            );
        };
        let diary = (sources.diary)(&phase, &slug)?;
        let approved = entry.completed_at.is_some();
        if let Some(completed_at) = &entry.completed_at
            && diary.is_some_and(|counts| counts.total() == 0)
        {
            empty_diaries.push(EmptyDiary {
                slug: slug.clone(),
                completed_at: completed_at.clone(),
            });
        }
        rows.push(Row {
            memory_path: format!("{memory_prefix}/{phase}/{slug}/memory.md"),
            agent: if entry.agent.is_empty() {
                place.lead_agent
            } else {
                Some(json!(entry.agent))
            },
            slug,
            started_at: entry.started_at,
            completed_at: entry.completed_at,
            diary,
            outcome: if approved { "approved" } else { "pending" },
            learnings: approved.then_some((0, 0)),
            firings: Vec::new(),
            instances: Vec::new(),
        });
        phases.push(phase);
    }

    // A Construction stage on which two or more bolts forked their state is
    // built by bolts: one row each, in name order. A fork counts for the
    // stage whose latest start it follows, so an earlier attempt's bolts do
    // not show in a later one. One bolt alone leaves the stage's row as it is.
    let (forked, merged, bolt_failed) = (
        events("STATE_FORKED"),
        events("STATE_MERGED"),
        events("BOLT_FAILED"),
    );
    for (row, phase) in rows.iter_mut().zip(&phases) {
        if phase != "construction" {
            continue;
        }
        let end = row.completed_at.as_deref();
        let mut forks: Vec<(&str, String, &str)> = Vec::new();
        for found in &forked {
            if !within(&found.timestamp, &row.started_at, end) {
                continue;
            }
            let Some(bolt) = filled(&found.block, "Bolt slug") else {
                continue;
            };
            let worktree = field(&found.block, "Worktree path")
                .map_or_else(String::new, |recorded| {
                    resolve_recorded_path(sources.project, recorded)
                });
            // The latest fork of a bolt is the one that counts.
            forks.retain(|(known, ..)| *known != bolt);
            forks.push((bolt, worktree, &found.timestamp));
        }
        if forks.len() < 2 {
            continue;
        }
        forks.sort_by(|a, b| cmp_utf16(a.0, b.0));
        for (bolt, worktree, started_at) in forks {
            let later = |rows: &'_ [Found]| -> Option<String> {
                rows.iter()
                    .find(|found| {
                        within(&found.timestamp, started_at, end)
                            && field(&found.block, "Bolt slug") == Some(bolt)
                    })
                    .map(|found| found.timestamp.clone())
            };
            let completed_at = later(&merged);
            let outcome = if completed_at.is_some() {
                "approved"
            } else if later(&bolt_failed).is_some() {
                "failed"
            } else {
                "pending"
            };
            row.instances.push(Instance {
                bolt: bolt.to_owned(),
                worktree,
                started_at: started_at.to_owned(),
                completed_at,
                outcome,
                firings: Vec::new(),
            });
        }
        // The stage is what its bolts are: failed if one failed, pending
        // while one is, approved when all are. Learnings are an approved
        // row's.
        let any = |outcome: &str| {
            row.instances
                .iter()
                .any(|instance| instance.outcome == outcome)
        };
        row.outcome = if any("failed") {
            "failed"
        } else if any("pending") {
            "pending"
        } else {
            "approved"
        };
        if row.outcome != "approved" {
            row.learnings = None;
        }
    }

    // Sensors. A firing and its ending carry the same `Fire id`: one write
    // sets several sensors off at once and their endings come back in any
    // order, so position pairs nothing.
    let fired = events("SENSOR_FIRED");
    let endings = [
        ("passed", events("SENSOR_PASSED")),
        ("failed", events("SENSOR_FAILED")),
        ("budget-override", events("SENSOR_BUDGET_OVERRIDE")),
    ];
    let mut remarks = Vec::new();
    let mut terminals: HashMap<&str, Terminal> = HashMap::new();
    for (result, rows) in &endings {
        for found in rows {
            let Some(fire_id) = filled(&found.block, "Fire id") else {
                continue;
            };
            // Two endings for one firing: the later one stands.
            if let Some(known) = terminals.get(fire_id) {
                remarks.push(format!(
                    "duplicate terminal for Fire id {fire_id} (keeping latest-ts)"
                ));
                if !before(&known.timestamp, &found.timestamp) {
                    continue;
                }
            }
            let detail_path = filled(&found.block, "Detail path").filter(|_| *result == "failed");
            terminals.insert(
                fire_id,
                Terminal {
                    result,
                    timestamp: found.timestamp.clone(),
                    detail_path: detail_path.map(str::to_owned),
                },
            );
        }
    }
    // The ledger's own "now": the latest timestamp of a start or of anything
    // a sensor wrote.
    let every_start = events("STAGE_STARTED");
    let latest = every_start
        .iter()
        .chain(&fired)
        .chain(endings.iter().flat_map(|(_, rows)| rows))
        .map(|found| found.timestamp.as_str())
        .fold("", |latest, timestamp| {
            if before(latest, timestamp) {
                timestamp
            } else {
                latest
            }
        });

    // The firings of `slug` inside a window, oldest first. `where_written`
    // narrows them by the path each wrote to, resolved.
    let firings = |slug: &str,
                   start: &str,
                   end: Option<&str>,
                   where_written: Option<&dyn Fn(&str) -> bool>|
     -> Result<Vec<Value>> {
        let mut found_in_window: Vec<Value> = Vec::new();
        for found in &fired {
            if !within(&found.timestamp, start, end)
                || field(&found.block, "Stage slug") != Some(slug)
            {
                continue;
            }
            if let Some(accepts) = where_written {
                let output = field(&found.block, "Output path").unwrap_or_default();
                if !accepts(&resolve_recorded_path(sources.project, output)) {
                    continue;
                }
            }
            let (Some(fire_id), Some(sensor)) = (
                filled(&found.block, "Fire id"),
                filled(&found.block, "Sensor ID"),
            ) else {
                continue;
            };
            let mut firing = Map::new();
            firing.insert("id".to_owned(), json!(sensor));
            firing.insert("fire_id".to_owned(), json!(fire_id));
            if let Some(terminal) = terminals.get(fire_id) {
                firing.insert("result".to_owned(), json!(terminal.result));
                firing.insert("ts".to_owned(), json!(found.timestamp));
                if let Some(detail_path) = &terminal.detail_path {
                    firing.insert("detail_path".to_owned(), json!(detail_path));
                }
            } else {
                // No ending. In a closed window the closing settles it. In an
                // open one it is incomplete once the ledger has moved a
                // minute past it, and until then it is not listed at all.
                if end.is_none() {
                    let age = match (date::parse(latest), date::parse(&found.timestamp)) {
                        (Instant::At(now), Instant::At(then)) => Some(now - then),
                        // Not a date on either side: no age, and so not
                        // old enough, whatever the other side is.
                        (Instant::NotADate, _) | (_, Instant::NotADate) => None,
                        (now, _) => {
                            let unread = if now == Instant::Unread {
                                latest
                            } else {
                                &found.timestamp
                            };
                            bail!(
                                "The sensor {sensor} fired in {slug}, which is still open, and has no ending in the audit ledger. Whether it counts as incomplete depends on how long ago it fired, and the timestamp {unread} is not an instant Focus reads: it wants the form 2026-01-05T10:00:00Z, with the zone written out, on a date the calendar has. Correct the timestamp in the ledger."
                            );
                        }
                    };
                    if age.is_none_or(|age| age < ORPHAN_CUTOFF_MS) {
                        continue;
                    }
                }
                firing.insert("result".to_owned(), json!("incomplete"));
                firing.insert("ts".to_owned(), json!(found.timestamp));
            }
            found_in_window.push(Value::Object(firing));
        }
        Ok(found_in_window)
    };

    let learned: Vec<Found> = events("RULE_LEARNED")
        .into_iter()
        .chain(events("SENSOR_PROPOSED"))
        .collect();
    for row in &mut rows {
        if row.instances.is_empty() {
            row.firings = firings(
                &row.slug,
                &row.started_at,
                row.completed_at.as_deref(),
                None,
            )?;
            if row.outcome == "approved" {
                let (mut orchestrator, mut user) = (0, 0);
                for found in &learned {
                    if within(
                        &found.timestamp,
                        &row.started_at,
                        row.completed_at.as_deref(),
                    ) && field(&found.block, "Stage") == Some(row.slug.as_str())
                    {
                        if field(&found.block, "Source") == Some("user_addition") {
                            user += 1;
                        } else {
                            orchestrator += 1;
                        }
                    }
                }
                row.learnings = Some((orchestrator, user));
            }
            continue;
        }
        // Each bolt has the firings that wrote under its worktree; the
        // stage keeps the ones that wrote under none, over the span from
        // the first fork to the last merge (open while a bolt is).
        let worktrees: Vec<String> = row
            .instances
            .iter()
            .map(|instance| instance.worktree.clone())
            .collect();
        let under = |path: &str, worktree: &str| !worktree.is_empty() && path.starts_with(worktree);
        for instance in &mut row.instances {
            let own = |path: &str| under(path, &instance.worktree);
            instance.firings = firings(
                &row.slug,
                &instance.started_at,
                instance.completed_at.as_deref(),
                Some(&own),
            )?;
        }
        let first = row
            .instances
            .iter()
            .map(|instance| instance.started_at.as_str())
            .reduce(|a, b| if before(a, b) { a } else { b })
            .unwrap_or_default();
        let last = row
            .instances
            .iter()
            .map(|instance| instance.completed_at.as_deref())
            .collect::<Option<Vec<&str>>>()
            .and_then(|ends| {
                ends.into_iter()
                    .reduce(|a, b| if before(b, a) { a } else { b })
            });
        let outside = |path: &str| !worktrees.iter().any(|worktree| under(path, worktree));
        row.firings = firings(&row.slug, first, last, Some(&outside))?;
    }

    let mut graph = Map::new();
    graph.insert("workflow_id".to_owned(), json!(workflow.timestamp));
    graph.insert("scope".to_owned(), json!(scope));
    graph.insert("started_at".to_owned(), json!(workflow.timestamp));
    graph.insert(
        "stages".to_owned(),
        rows.into_iter().map(Row::into_json).collect(),
    );
    Ok(Reading {
        graph,
        empty_diaries,
        remarks,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn row(event: &str, at: &str, fields: &[&str]) -> String {
        let mut block = format!("\n## Row\n**Timestamp**: {at}\n**Event**: {event}\n");
        for field in fields {
            block.push_str(field);
            block.push('\n');
        }
        block + "\n---\n"
    }

    fn t(minute: u32, second: u32) -> String {
        format!("2026-01-05T10:{minute:02}:{second:02}Z")
    }

    /// The graph of `audit`, with every stage placed in `construction` but
    /// `intent-capture`, and `unknown` not placed at all.
    fn graph(audit: &str) -> Value {
        graph_with(audit, "# State\n- **Scope**: feature\n", &|_, _| Ok(None)).unwrap()
    }

    fn graph_with(
        audit: &str,
        state: &str,
        diary: &dyn Fn(&str, &str) -> Result<Option<MemoryCounts>>,
    ) -> Result<Value> {
        let workflow = workflow(audit).expect("the ledger has a workflow");
        let sources = Sources {
            project: Path::new("/project"),
            state,
            record_prefix: Some("focus/spaces/default/intents/add-login-1a2b3c4d"),
            place: &|slug| {
                (slug != "unknown").then(|| Place {
                    phase: Some(
                        if slug == "intent-capture" {
                            "ideation"
                        } else {
                            "construction"
                        }
                        .to_owned(),
                    ),
                    lead_agent: Some(json!("lead")),
                })
            },
            diary,
        };
        read(audit, &workflow, &sources).map(|reading| Value::Object(reading.graph))
    }

    fn workflow_row() -> String {
        row("WORKFLOW_STARTED", &t(0, 0), &["**Scope**: bugfix"])
    }

    #[test]
    fn a_stage_row_is_its_latest_start_and_the_completion_after_it() {
        let audit = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture", "**Agent**:  someone  "])
            + &row("STAGE_COMPLETED", &t(2, 0), &["**Stage**: intent-capture"])
            + &row("STAGE_STARTED", &t(3, 0), &["**Stage**: unknown"])
            + &row("STAGE_STARTED", &t(4, 0), &["**Stage**: code-generation"])
            // A single-stage run is no part of the workflow.
            + &row("STAGE_COMPLETED", &t(5, 0), &["**Stage**: code-generation", "**Workflow**: single-stage:x"])
            // Started again: a new attempt, and the completion before it is gone.
            + &row("STAGE_STARTED", &t(6, 0), &["**Stage**: intent-capture"])
            // Before the workflow: not this workflow's.
            + &row("STAGE_STARTED", "2026-01-05T09:00:00Z", &["**Stage**: build-and-test"]);
        let graph = graph(&audit);
        assert_eq!(graph["workflow_id"], t(0, 0));
        assert_eq!(graph["scope"], "feature");
        let stages = graph["stages"].as_array().unwrap();
        let slugs: Vec<&str> = stages
            .iter()
            .map(|stage| stage["stage_slug"].as_str().unwrap())
            .collect();
        assert_eq!(slugs, ["code-generation", "intent-capture"]);
        assert_eq!(stages[0]["outcome"], "pending");
        assert_eq!(stages[0]["agent"], "lead");
        assert_eq!(stages[0]["learnings_captured"], Value::Null);
        assert_eq!(stages[1]["started_at"], t(6, 0));
        assert_eq!(stages[1]["completed_at"], Value::Null);
        assert_eq!(stages[1]["agent"], "lead");
        assert_eq!(
            stages[1]["memory_path"],
            "focus/spaces/default/intents/add-login-1a2b3c4d/ideation/intent-capture/memory.md"
        );
        let keys: Vec<&String> = stages[1].as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "stage_slug",
                "started_at",
                "completed_at",
                "agent",
                "memory_path",
                "memory_entries",
                "memory_breakdown",
                "sensor_firings",
                "outcome",
                "learnings_captured"
            ]
        );
    }

    #[test]
    fn rows_of_one_instant_are_read_starts_first() {
        let instant = t(1, 0);
        // Two starts and a completion, all at once, the completion written
        // between them: it still closes the stage that started after it.
        let audit = workflow_row()
            + &row("STAGE_STARTED", &instant, &["**Stage**: code-generation"])
            + &row("STAGE_COMPLETED", &instant, &["**Stage**: intent-capture"])
            + &row("STAGE_STARTED", &instant, &["**Stage**: intent-capture"])
            // A second completion changes nothing: the first one stands.
            + &row("STAGE_COMPLETED", &t(2, 0), &["**Stage**: intent-capture"]);
        let graph = graph(&audit);
        let stages = graph["stages"].as_array().unwrap();
        // In the order they started, which at one instant is the order read.
        assert_eq!(stages[0]["stage_slug"], "code-generation");
        assert_eq!(stages[1]["stage_slug"], "intent-capture");
        assert_eq!(stages[1]["completed_at"], instant);
        assert_eq!(stages[1]["outcome"], "approved");
    }

    #[test]
    fn the_agent_is_the_one_logged_or_else_the_stage_graphs() {
        let audit = workflow_row()
            + &row(
                "STAGE_STARTED",
                &t(1, 0),
                &["**Stage**: intent-capture", "**Agent**:  someone  "],
            )
            + &row("STAGE_COMPLETED", &t(2, 0), &["**Stage**: intent-capture"]);
        let stage = &graph(&audit)["stages"][0];
        assert_eq!(stage["agent"], "someone");
        assert_eq!(stage["outcome"], "approved");
        assert_eq!(
            stage["learnings_captured"],
            json!({"from_orchestrator": 0, "from_user_addition": 0})
        );
    }

    #[test]
    fn the_scope_is_the_state_files_and_failing_that_the_ledgers() {
        let audit = workflow_row() + &row("WORKFLOW_STARTED", &t(9, 0), &["**Scope**:  mvp "]);
        let scope =
            |state: &str| graph_with(&audit, state, &|_, _| Ok(None)).unwrap()["scope"].clone();
        assert_eq!(scope("- **Scope**: feature\n"), "feature");
        // The latest workflow's, when the state file has none or an empty one.
        assert_eq!(scope("# State\n"), "mvp");
        assert_eq!(scope("- **Scope**:\n"), "mvp");
        assert_eq!(empty_graph("- **Scope**: feature\n")["scope"], "feature");
        assert_eq!(
            Value::Object(empty_graph("# State\n")),
            json!({"workflow_id": "", "scope": "", "started_at": "", "stages": []})
        );
    }

    #[test]
    fn an_approved_stage_with_an_empty_diary_is_an_event_to_record() {
        let audit = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
            + &row("STAGE_COMPLETED", &t(2, 0), &["**Stage**: intent-capture"])
            + &row("STAGE_STARTED", &t(3, 0), &["**Stage**: code-generation"]);
        let workflow = workflow(&audit).unwrap();
        let counts = MemoryCounts {
            interpretations: 0,
            deviations: 2,
            tradeoffs: 0,
            open_questions: 1,
        };
        let reading = |diary: &dyn Fn(&str, &str) -> Result<Option<MemoryCounts>>| {
            let sources = Sources {
                project: Path::new("/project"),
                state: "",
                record_prefix: None,
                place: &|_| {
                    Some(Place {
                        phase: Some("ideation".to_owned()),
                        lead_agent: None,
                    })
                },
                diary,
            };
            read(&audit, &workflow, &sources).unwrap()
        };
        // An empty diary: recorded for the approved stage, not for the open one.
        let empty = reading(&|_, _| Ok(Some(MemoryCounts::default())));
        assert_eq!(
            empty.empty_diaries,
            [EmptyDiary {
                slug: "intent-capture".to_owned(),
                completed_at: t(2, 0)
            }]
        );
        assert_eq!(empty.graph["stages"][0]["memory_entries"], 0);
        // No diary at all, or one with entries: nothing to record.
        assert!(reading(&|_, _| Ok(None)).empty_diaries.is_empty());
        let filled = reading(&|_, _| Ok(Some(counts)));
        assert!(filled.empty_diaries.is_empty());
        assert_eq!(filled.graph["stages"][0]["memory_entries"], 3);
        assert_eq!(
            filled.graph["stages"][0]["memory_breakdown"],
            json!({"interpretations": 0, "deviations": 2, "tradeoffs": 0, "open_questions": 1})
        );
        // Without an intent the path is the default space's, and a stage
        // graph that names no agent leaves the key out.
        assert_eq!(
            filled.graph["stages"][0]["memory_path"],
            "focus/spaces/default/intents/ideation/intent-capture/memory.md"
        );
        assert!(filled.graph["stages"][0].get("agent").is_none());
    }

    fn fired(minute: u32, second: u32, stage: &str, fire: &str, more: &[&str]) -> String {
        let (stage, fire) = (
            format!("**Stage slug**: {stage}"),
            format!("**Fire id**: {fire}"),
        );
        let mut fields = vec![stage.as_str(), "**Sensor ID**: linter", fire.as_str()];
        fields.extend_from_slice(more);
        row("SENSOR_FIRED", &t(minute, second), &fields)
    }

    fn ended(kind: &str, minute: u32, second: u32, fire: &str, more: &[&str]) -> String {
        let fire = format!("**Fire id**: {fire}");
        let mut fields = vec![fire.as_str()];
        fields.extend_from_slice(more);
        row(kind, &t(minute, second), &fields)
    }

    #[test]
    fn a_firing_is_paired_with_its_ending_by_its_id() {
        let audit = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
            + &fired(1, 10, "intent-capture", "a", &[])
            + &fired(1, 20, "intent-capture", "b", &[])
            + &fired(1, 30, "intent-capture", "c", &[])
            + &fired(1, 40, "intent-capture", "d", &[])
            + &ended("SENSOR_FAILED", 1, 41, "b", &["**Detail path**: where"])
            + &ended("SENSOR_PASSED", 1, 42, "a", &["**Detail path**: not kept"])
            + &ended("SENSOR_BUDGET_OVERRIDE", 1, 43, "c", &[])
            // A second ending: the later one stands, and it is remarked on.
            + &ended("SENSOR_PASSED", 1, 50, "b", &[])
            + &row("STAGE_COMPLETED", &t(2, 0), &["**Stage**: intent-capture"])
            // After the stage closed: no firing of it.
            + &fired(2, 0, "intent-capture", "e", &[]);
        let workflow = workflow(&audit).unwrap();
        let sources = Sources {
            project: Path::new("/project"),
            state: "",
            record_prefix: None,
            place: &|_| {
                Some(Place {
                    phase: Some("ideation".to_owned()),
                    lead_agent: None,
                })
            },
            diary: &|_, _| Ok(None),
        };
        let reading = read(&audit, &workflow, &sources).unwrap();
        assert_eq!(
            reading.remarks,
            ["duplicate terminal for Fire id b (keeping latest-ts)"]
        );
        assert_eq!(
            reading.graph["stages"][0]["sensor_firings"],
            json!([
                {"id": "linter", "fire_id": "a", "result": "passed", "ts": t(1, 10)},
                {"id": "linter", "fire_id": "b", "result": "passed", "ts": t(1, 20)},
                {"id": "linter", "fire_id": "c", "result": "budget-override", "ts": t(1, 30)},
                // No ending, in a stage that closed.
                {"id": "linter", "fire_id": "d", "result": "incomplete", "ts": t(1, 40)},
            ])
        );
        // Two endings at one instant: the first that is read stands, and
        // endings are read kind by kind, those that passed first.
        let tied = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
            + &fired(1, 10, "intent-capture", "a", &[])
            + &fired(1, 11, "intent-capture", "b", &[])
            + &ended("SENSOR_FAILED", 1, 30, "a", &["**Detail path**: where"])
            + &ended("SENSOR_PASSED", 1, 30, "a", &[])
            + &ended("SENSOR_BUDGET_OVERRIDE", 1, 31, "b", &[])
            + &ended("SENSOR_FAILED", 1, 31, "b", &["**Detail path**: where"]);
        assert_eq!(
            graph(&tied)["stages"][0]["sensor_firings"],
            json!([
                {"id": "linter", "fire_id": "a", "result": "passed", "ts": t(1, 10)},
                {"id": "linter", "fire_id": "b", "result": "failed", "ts": t(1, 11), "detail_path": "where"},
            ])
        );
        // The earlier ending stands when it is the later one in time.
        let kept = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
            + &fired(1, 10, "intent-capture", "a", &[])
            + &ended("SENSOR_FAILED", 1, 30, "a", &["**Detail path**: where"])
            + &ended("SENSOR_PASSED", 1, 20, "a", &[])
            + &row("STAGE_COMPLETED", &t(2, 0), &["**Stage**: intent-capture"]);
        assert_eq!(
            graph(&kept)["stages"][0]["sensor_firings"],
            json!([{"id": "linter", "fire_id": "a", "result": "failed", "ts": t(1, 10), "detail_path": "where"}])
        );
    }

    #[test]
    fn in_an_open_stage_a_firing_with_no_ending_is_incomplete_after_a_minute() {
        let open = |fired_at: (u32, u32), latest: &str| {
            workflow_row()
                + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
                + &fired(fired_at.0, fired_at.1, "intent-capture", "a", &[])
                + &row(
                    "SENSOR_FIRED",
                    latest,
                    &[
                        "**Stage slug**: elsewhere",
                        "**Sensor ID**: x",
                        "**Fire id**: z",
                    ],
                )
        };
        let firings = |audit: &str| {
            graph(audit)["stages"][0]["sensor_firings"]
                .as_array()
                .unwrap()
                .len()
        };
        // "Now" is the ledger's latest timestamp, never the clock.
        assert_eq!(firings(&open((2, 0), &t(3, 0))), 1);
        assert_eq!(firings(&open((2, 1), &t(3, 0))), 0);
        // A word where the latest timestamp goes is no date: no age.
        assert_eq!(firings(&open((2, 0), "tomorrow")), 0);
    }

    #[test]
    fn the_ledgers_now_is_its_latest_start_or_sensor_row() {
        // A firing at 10:02:00 with no ending, in a stage still open.
        let listed = |later: &str| {
            let audit = workflow_row()
                + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
                + &fired(2, 0, "intent-capture", "a", &[])
                + later;
            graph(&audit)["stages"][0]["sensor_firings"]
                .as_array()
                .unwrap()
                .len()
        };
        assert_eq!(listed(""), 0);
        // A start moves the ledger's "now", whatever stage it is of, and so
        // does the ending of any firing.
        for moves in [
            row("STAGE_STARTED", &t(3, 0), &["**Stage**: unknown"]),
            ended("SENSOR_PASSED", 3, 0, "other", &[]),
            ended("SENSOR_FAILED", 3, 0, "other", &[]),
            ended("SENSOR_BUDGET_OVERRIDE", 3, 0, "other", &[]),
            // With no fire id it pairs nothing, and is a timestamp all the same.
            row("SENSOR_PASSED", &t(3, 0), &[]),
        ] {
            assert_eq!(listed(&moves), 1, "{moves}");
        }
        // Nothing else does: the firing stays too recent to list.
        for stays in [
            row("STAGE_COMPLETED", &t(3, 0), &["**Stage**: unknown"]),
            row("RULE_LEARNED", &t(3, 0), &["**Stage**: intent-capture"]),
            row("STATE_FORKED", &t(3, 0), &["**Bolt slug**: web"]),
            row("MEMORY_EMPTY", &t(3, 0), &["**Stage**: intent-capture"]),
            row("WORKFLOW_COMPLETED", &t(3, 0), &[]),
        ] {
            assert_eq!(listed(&stays), 0, "{stays}");
        }
        // A second of less than a minute is not enough.
        assert_eq!(
            listed(&row(
                "STAGE_STARTED",
                "2026-01-05T10:02:59.999Z",
                &["**Stage**: unknown"]
            )),
            0
        );
    }

    #[test]
    fn a_firing_with_no_ending_is_judged_by_the_window_it_is_in() {
        let bolt = |event: &str, minute: u32, bolt: &str, more: &[&str]| {
            let bolt = format!("**Bolt slug**: {bolt}");
            let mut fields = vec![bolt.as_str()];
            fields.extend_from_slice(more);
            row(event, &t(minute, 0), &fields)
        };
        let audit = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: code-generation"])
            + &bolt("STATE_FORKED", 2, "api", &["**Worktree path**: wt/api"])
            + &bolt("STATE_FORKED", 2, "web", &["**Worktree path**: wt/web"])
            // In a bolt that merged: its window closed, so it is incomplete.
            + &fired(3, 0, "code-generation", "closed", &["**Output path**: wt/api/a"])
            + &bolt("STATE_MERGED", 4, "api", &[])
            // In a bolt still open: incomplete once a minute old, not before.
            + &fired(5, 0, "code-generation", "old", &["**Output path**: wt/web/a"])
            + &fired(5, 30, "code-generation", "recent", &["**Output path**: wt/web/b"])
            // Under no bolt, while one is open: the stage's span is open too.
            + &fired(5, 0, "code-generation", "own-old", &["**Output path**: src/a"])
            + &fired(5, 30, "code-generation", "own-recent", &["**Output path**: src/b"])
            // The ledger's "now".
            + &ended("SENSOR_PASSED", 6, 0, "elsewhere", &[]);
        let ids = |firings: &Value| -> Vec<(String, String)> {
            firings
                .as_array()
                .unwrap()
                .iter()
                .map(|firing| {
                    (
                        firing["fire_id"].as_str().unwrap().to_owned(),
                        firing["result"].as_str().unwrap().to_owned(),
                    )
                })
                .collect()
        };
        let incomplete = |id: &str| (id.to_owned(), "incomplete".to_owned());
        let stage = graph(&audit)["stages"][0].clone();
        assert_eq!(
            ids(&stage["instances"][0]["sensor_firings"]),
            [incomplete("closed")]
        );
        assert_eq!(
            ids(&stage["instances"][1]["sensor_firings"]),
            [incomplete("old")]
        );
        assert_eq!(ids(&stage["sensor_firings"]), [incomplete("own-old")]);
        // With the last bolt merged every window is closed, and each firing
        // inside one is incomplete however recent.
        let merged = audit + &bolt("STATE_MERGED", 7, "web", &[]);
        let stage = graph(&merged)["stages"][0].clone();
        assert_eq!(
            ids(&stage["instances"][1]["sensor_firings"]),
            [incomplete("old"), incomplete("recent")]
        );
        assert_eq!(
            ids(&stage["sensor_firings"]),
            [incomplete("own-old"), incomplete("own-recent")]
        );
    }

    /// Deliberate deviation. Upstream asks JavaScript's `Date.parse` how old
    /// a firing is, which reads many forms by the runtime's own rules and a
    /// text with no zone in the machine's. Focus reads an instant written
    /// with its zone, and refuses the compile when the timestamp it needs is
    /// anything else with a digit in it.
    #[test]
    fn a_timestamp_focus_does_not_read_stops_the_compile_where_it_matters() {
        let audit = |fired_at: &str, latest: &str| {
            workflow_row()
                + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
                + &row(
                    "SENSOR_FIRED",
                    fired_at,
                    &[
                        "**Stage slug**: intent-capture",
                        "**Sensor ID**: linter",
                        "**Fire id**: a",
                    ],
                )
                + &row(
                    "SENSOR_FIRED",
                    latest,
                    &[
                        "**Stage slug**: elsewhere",
                        "**Sensor ID**: x",
                        "**Fire id**: z",
                    ],
                )
        };
        let refusal = |fired_at: &str, latest: &str| {
            graph_with(&audit(fired_at, latest), "", &|_, _| Ok(None))
                .unwrap_err()
                .message()
                .to_owned()
        };
        // Upstream: a graph, with the firing in it or not as its runtime reads the text.
        let no_zone = refusal("2026-01-05T10:02:00", "2026-01-05T10:30:00Z");
        assert!(
            no_zone.starts_with("The sensor linter fired in intent-capture, which is still open"),
            "{no_zone}"
        );
        assert!(
            no_zone.contains("the timestamp 2026-01-05T10:02:00 is not an instant Focus reads"),
            "{no_zone}"
        );
        assert!(
            refusal("2026-01-05T10:02:00Z", "2026-01-05T10:30:00")
                .contains("the timestamp 2026-01-05T10:30:00 is not")
        );
        assert!(
            refusal("2026-02-29T10:02:00Z", "2026-03-05T10:30:00Z")
                .contains("the timestamp 2026-02-29T10:02:00Z is not")
        );
        // Where no age is asked for, the same text is only text: a stage
        // that closed, or a firing that has its ending.
        let closed = audit("2026-01-05T10:02:00", "2026-01-05T10:30:00Z")
            + &row(
                "STAGE_COMPLETED",
                "2026-01-05T10:03:00Z",
                &["**Stage**: intent-capture"],
            );
        assert_eq!(
            graph(&closed)["stages"][0]["sensor_firings"][0]["result"],
            "incomplete"
        );
        let paired = audit("2026-01-05T10:02:00", "2026-01-05T10:30:00Z")
            + &row("SENSOR_PASSED", "2026-01-05T10:02:01", &["**Fire id**: a"]);
        assert_eq!(
            graph(&paired)["stages"][0]["sensor_firings"][0]["result"],
            "passed"
        );
    }

    #[test]
    fn what_was_learned_at_a_gate_is_counted_by_who_brought_it() {
        let learned = |minute: u32, event: &str, stage: &str, source: &str| {
            let stage = format!("**Stage**: {stage}");
            row(event, &t(minute, 0), &[stage.as_str(), source])
        };
        let audit = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
            + &learned(
                1,
                "RULE_LEARNED",
                "intent-capture",
                "**Source**: user_addition",
            )
            + &learned(
                2,
                "SENSOR_PROPOSED",
                "intent-capture",
                "**Source**: user_addition",
            )
            + &learned(
                2,
                "RULE_LEARNED",
                "intent-capture",
                "**Source**:  user_addition ",
            )
            + &learned(2, "RULE_LEARNED", "intent-capture", "")
            + &learned(
                3,
                "SENSOR_PROPOSED",
                "intent-capture",
                "**Source**: orchestrator",
            )
            + &learned(
                3,
                "RULE_LEARNED",
                "code-generation",
                "**Source**: user_addition",
            )
            + &row("STAGE_COMPLETED", &t(4, 0), &["**Stage**: intent-capture"])
            + &learned(
                4,
                "RULE_LEARNED",
                "intent-capture",
                "**Source**: user_addition",
            );
        assert_eq!(
            graph(&audit)["stages"][0]["learnings_captured"],
            json!({"from_orchestrator": 2, "from_user_addition": 3})
        );
    }

    #[test]
    fn a_construction_stage_two_bolts_forked_on_is_a_row_for_each() {
        let bolt = |event: &str, minute: u32, bolt: &str, more: &[&str]| {
            let bolt = format!("**Bolt slug**: {bolt}");
            let mut fields = vec![bolt.as_str()];
            fields.extend_from_slice(more);
            row(event, &t(minute, 0), &fields)
        };
        let stage = |rows: &str| {
            let audit = workflow_row()
                + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: code-generation"])
                + rows;
            graph(&audit)["stages"][0].clone()
        };
        let two = bolt(
            "STATE_FORKED",
            2,
            "web",
            &["**Worktree path**: <project-dir>/wt/web"],
        ) + &bolt("STATE_FORKED", 3, "api", &["**Worktree path**: wt/./api/"])
            + &fired(
                4,
                0,
                "code-generation",
                "a",
                &["**Output path**: wt/api/src/a.rs"],
            )
            + &fired(
                4,
                1,
                "code-generation",
                "b",
                &["**Output path**: <project-dir>/wt/web/b.rs"],
            )
            + &fired(
                4,
                2,
                "code-generation",
                "c",
                &["**Output path**: src/main.rs"],
            )
            + &fired(4, 3, "code-generation", "d", &[])
            + &ended("SENSOR_PASSED", 4, 4, "a", &[])
            + &ended("SENSOR_PASSED", 4, 5, "c", &[])
            + &bolt("STATE_MERGED", 5, "api", &[]);
        let built = stage(&two);
        // The stage's own times, agent and counts move to the bolts' rows.
        for key in [
            "started_at",
            "completed_at",
            "agent",
            "memory_entries",
            "memory_breakdown",
            "learnings_captured",
        ] {
            assert_eq!(built[key], Value::Null, "{key}");
        }
        assert_eq!(built["outcome"], "pending");
        let instances = built["instances"].as_array().unwrap();
        // In name order, each with the firings that wrote under its worktree.
        assert_eq!(instances[0]["bolt"], "api");
        if cfg!(unix) {
            assert_eq!(instances[0]["worktree"], "/project/wt/api");
            assert_eq!(instances[1]["worktree"], "/project/wt/web");
        }
        assert_eq!(instances[0]["completed_at"], t(5, 0));
        assert_eq!(instances[0]["outcome"], "approved");
        assert_eq!(instances[0]["sensor_firings"][0]["fire_id"], "a");
        assert_eq!(instances[0]["memory_path"], built["memory_path"]);
        assert_eq!(instances[1]["bolt"], "web");
        assert_eq!(instances[1]["outcome"], "pending");
        // `b` fired after the window of `web` opened and has no ending: in
        // an open window, too recent to list.
        assert_eq!(instances[1]["sensor_firings"], json!([]));
        // The stage keeps what wrote under no worktree.
        let own: Vec<&str> = built["sensor_firings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|firing| firing["fire_id"].as_str().unwrap())
            .collect();
        assert_eq!(own, ["c"]);

        // One failed: the stage failed. All merged: approved, and as the
        // stage itself never completed, with no learnings of its own.
        let failed = two.clone() + &bolt("BOLT_FAILED", 6, "web", &[]);
        assert_eq!(stage(&failed)["outcome"], "failed");
        let merged = two.clone() + &bolt("STATE_MERGED", 6, "web", &[]);
        assert_eq!(stage(&merged)["outcome"], "approved");
        assert_eq!(stage(&merged)["learnings_captured"], Value::Null);
        // A stage that completed while a bolt had not: no learnings of its
        // own either, and the diary's counts are the bolts' to carry.
        let audit = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: code-generation"])
            + &two
            + &row("STAGE_COMPLETED", &t(9, 0), &["**Stage**: code-generation"]);
        let two_entries = MemoryCounts {
            deviations: 2,
            ..MemoryCounts::default()
        };
        let counted =
            graph_with(&audit, "", &|_, _| Ok(Some(two_entries))).unwrap()["stages"][0].clone();
        assert_eq!(counted["outcome"], "pending");
        assert_eq!(counted["learnings_captured"], Value::Null);
        assert_eq!(counted["memory_entries"], Value::Null);
        assert_eq!(counted["memory_breakdown"], Value::Null);
        // Merged and failed both: the merge is what is asked first.
        let both = two.clone()
            + &bolt("BOLT_FAILED", 6, "api", &[])
            + &bolt("STATE_MERGED", 6, "web", &[])
            + &bolt("BOLT_FAILED", 7, "web", &[]);
        assert_eq!(stage(&both)["outcome"], "approved");
        // A merge at the instant of the fork is inside the bolt's window.
        let at_once = bolt("STATE_FORKED", 2, "web", &[])
            + &bolt("STATE_MERGED", 2, "web", &[])
            + &bolt("STATE_FORKED", 3, "api", &[])
            + &bolt("BOLT_FAILED", 3, "api", &[]);
        let instances = stage(&at_once)["instances"].clone();
        assert_eq!(instances[0]["outcome"], "failed");
        assert_eq!(instances[1]["completed_at"], t(2, 0));
        // One bolt alone leaves the stage's row as it is.
        let one = stage(&bolt("STATE_FORKED", 2, "web", &[]));
        assert!(one.get("instances").is_none());
        assert_eq!(one["started_at"], t(1, 0));
        // Outside Construction, forks make no rows.
        let ideation = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: intent-capture"])
            + &bolt("STATE_FORKED", 2, "web", &[])
            + &bolt("STATE_FORKED", 3, "api", &[]);
        assert!(graph(&ideation)["stages"][0].get("instances").is_none());
    }

    #[test]
    fn a_bolts_window_is_its_latest_fork_and_its_firings_are_under_its_worktree() {
        let bolt = |event: &str, minute: u32, bolt: &str, more: &[&str]| {
            let bolt = format!("**Bolt slug**: {bolt}");
            let mut fields = vec![bolt.as_str()];
            fields.extend_from_slice(more);
            row(event, &t(minute, 0), &fields)
        };
        let audit = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: code-generation"])
            + &bolt("STATE_FORKED", 2, "web", &["**Worktree path**: wt/old"])
            + &bolt("STATE_MERGED", 4, "web", &[])
            // No worktree recorded: nothing is under it.
            + &bolt("STATE_FORKED", 5, "api", &[])
            // After `api` forked and before `web` forked again: inside the
            // stage's span, which runs from the earliest fork that counts.
            + &fired(5, 30, "code-generation", "early", &["**Output path**: elsewhere/x"])
            + &ended("SENSOR_PASSED", 5, 31, "early", &[])
            + &bolt("STATE_FORKED", 6, "web", &["**Worktree path**: wt/new"])
            + &fired(7, 0, "code-generation", "new", &["**Output path**: wt/new/a"])
            + &ended("SENSOR_PASSED", 7, 1, "new", &[])
            + &fired(7, 2, "code-generation", "old", &["**Output path**: wt/old/a"])
            + &ended("SENSOR_PASSED", 7, 3, "old", &[])
            + &bolt("STATE_MERGED", 8, "api", &[])
            // Between the two merges: the span runs to the last one.
            + &fired(8, 30, "code-generation", "between", &["**Output path**: elsewhere/z"])
            + &ended("SENSOR_PASSED", 8, 31, "between", &[])
            + &bolt("STATE_MERGED", 9, "web", &[])
            // After the last merge: outside the stage's span.
            + &fired(9, 0, "code-generation", "late", &["**Output path**: elsewhere/y"])
            + &ended("SENSOR_PASSED", 9, 1, "late", &[]);
        let stage = graph(&audit)["stages"][0].clone();
        let ids = |firings: &Value| -> Vec<String> {
            firings
                .as_array()
                .unwrap()
                .iter()
                .map(|firing| firing["fire_id"].as_str().unwrap().to_owned())
                .collect()
        };
        let instances = stage["instances"].as_array().unwrap();
        assert_eq!(instances.len(), 2);
        // `api`, with no worktree, has no firings of its own.
        assert_eq!(instances[0]["bolt"], "api");
        assert_eq!(instances[0]["worktree"], "");
        assert_eq!(ids(&instances[0]["sensor_firings"]), Vec::<String>::new());
        // `web` is its second fork: the merge before it is not its own.
        assert_eq!(instances[1]["started_at"], t(6, 0));
        assert_eq!(instances[1]["completed_at"], t(9, 0));
        assert_eq!(ids(&instances[1]["sensor_firings"]), ["new"]);
        // The stage keeps what wrote under no bolt's worktree, between the
        // earliest fork that counts and the last merge.
        assert_eq!(ids(&stage["sensor_firings"]), ["early", "old", "between"]);
        assert_eq!(stage["outcome"], "approved");
    }

    #[test]
    fn bolts_and_timestamps_are_ordered_as_javascript_orders_text() {
        // By UTF-16 code unit: a character past the basic plane is two units
        // that start at U+D800, so it comes before U+E000 and U+FFFF, which
        // by code point it follows.
        let forked = |minute: u32, bolt: &str| {
            row(
                "STATE_FORKED",
                &t(minute, 0),
                &[&format!("**Bolt slug**: {bolt}")],
            )
        };
        let audit = workflow_row()
            + &row("STAGE_STARTED", &t(1, 0), &["**Stage**: code-generation"])
            + &forked(2, "\u{e000}")
            + &forked(3, "\u{1F600}")
            + &forked(4, "z")
            + &forked(5, "\u{ffff}");
        let stage = graph(&audit)["stages"][0].clone();
        let bolts: Vec<&str> = stage["instances"]
            .as_array()
            .unwrap()
            .iter()
            .map(|instance| instance["bolt"].as_str().unwrap())
            .collect();
        assert_eq!(bolts, ["z", "\u{1F600}", "\u{e000}", "\u{ffff}"]);
        // Timestamps too, as text: a row "after" another is so by its text,
        // whatever instants the two would be.
        assert!(before("2026-01-05T10:00:00Z", "2026-01-05T10:00:00z"));
        assert!(before("2026-01-05T10:00:00+01:00", "2026-01-05T10:00:00Z"));
        assert!(!before("same", "same"));
        assert!(before("\u{1F600}", "\u{e000}"));
    }

    // The paths are a Unix machine's; on Windows the project has a drive
    // and the separators are its own.
    #[cfg(unix)]
    #[test]
    fn a_recorded_path_is_resolved_against_the_project() {
        let project = Path::new("/home/me/project");
        let resolved = |recorded: &str| resolve_recorded_path(project, recorded);
        assert_eq!(resolved("<project-dir>"), "/home/me/project");
        assert_eq!(resolved("<project-dir>/wt/a"), "/home/me/project/wt/a");
        assert_eq!(resolved("wt/../x/./y/"), "/home/me/project/x/y");
        assert_eq!(resolved(""), "/home/me/project");
        // An absolute path stands as written, unsettled.
        assert_eq!(resolved("/abs/../one/"), "/abs/../one/");
        // The token is one only as a whole first segment.
        assert_eq!(
            resolved("<project-dir>x"),
            "/home/me/project/<project-dir>x"
        );
    }
}

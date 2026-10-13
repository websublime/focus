//! A workflow in numbers.
//!
//! The session skills (cost, replay, the outcomes pack) show how a workflow
//! went: how many stages were approved, failed or are pending, by phase; how
//! many diary entries of each kind; how the sensors ended; what was learned;
//! how long it took. Every one of those numbers comes from here, counted off
//! the runtime graph as it was last compiled, so that no model counts
//! anything.
//!
//! A stage built by several bolts counts once per bolt, and its own row not
//! at all. And once the state file says the workflow is complete, its
//! checkboxes have the last word on stages the plan executes: a stage ticked
//! there is approved, and one the graph never saw is counted too.
//!
//! Mirrors `summarize`, `unitsForStage`, `durationMinutes` and
//! `renderSummary` in `aidlc-runtime.ts` (L953, L886, L1062, L1069).

use std::cmp::Ordering;

use focus_js::Result;
use focus_js::date::{self, Instant};
use focus_js::order::{cmp_utf16, js_key_order};
use serde_json::{Map, Value, json};

#[derive(Default, Clone, Copy)]
struct Outcomes {
    total: u64,
    approved: u64,
    failed: u64,
    pending: u64,
}

impl Outcomes {
    fn count(&mut self, outcome: &str) {
        self.total += 1;
        match outcome {
            "approved" => self.approved += 1,
            "failed" => self.failed += 1,
            _ => self.pending += 1,
        }
    }

    fn json(self) -> Value {
        json!({ "total": self.total, "approved": self.approved, "failed": self.failed, "pending": self.pending })
    }
}

/// Why a file is not a runtime graph this module can count.
fn not_a_graph(what: &str) -> focus_js::EngineError {
    focus_js::EngineError::new(format!(
        "runtime-graph.json is not a runtime graph as the engine writes it: {what}. Run `focus engine runtime compile` to write it again."
    ))
}

fn text<'a>(holder: &'a Map<String, Value>, key: &str, what: &str) -> Result<&'a str> {
    holder
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| not_a_graph(&format!("{what} has no text `{key}`")))
}

/// A text, or nothing: `null` and an absent key both read as nothing.
fn optional_text<'a>(
    holder: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<Option<&'a str>> {
    match holder.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(not_a_graph(&format!(
            "`{key}` of {what} is neither text nor null"
        ))),
    }
}

/// The largest count a JavaScript number holds exactly. Upstream adds counts
/// as numbers, and past this its sums and Focus's would part.
const LARGEST_COUNT: u64 = 9_007_199_254_740_991;

fn count(holder: &Map<String, Value>, key: &str, what: &str) -> Result<u64> {
    holder
        .get(key)
        .and_then(Value::as_u64)
        .filter(|count| *count <= LARGEST_COUNT)
        .ok_or_else(|| not_a_graph(&format!("{what} has no count `{key}`")))
}

/// Add a count to a total that must stay a count.
fn add(total: &mut u64, by: u64) -> Result<()> {
    *total = total
        .checked_add(by)
        .filter(|sum| *sum <= LARGEST_COUNT)
        .ok_or_else(|| not_a_graph("its counts add up to more than a number holds exactly"))?;
    Ok(())
}

/// The object under `key`, when the row has one; `None` when the key is
/// absent or `null`. Anything else is not what the engine writes there.
fn optional_object<'a>(
    holder: &'a Map<String, Value>,
    key: &str,
    what: &str,
) -> Result<Option<&'a Map<String, Value>>> {
    match holder.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(object)) => Ok(Some(object)),
        Some(_) => Err(not_a_graph(&format!(
            "`{key}` of {what} is neither an object nor null"
        ))),
    }
}

fn outcome<'a>(holder: &'a Map<String, Value>, what: &str) -> Result<&'a str> {
    let outcome = text(holder, "outcome", what)?;
    if !["approved", "failed", "pending"].contains(&outcome) {
        return Err(not_a_graph(&format!("{what} has the outcome `{outcome}`")));
    }
    Ok(outcome)
}

/// The rows of a list under `key`; an absent or `null` list is empty when
/// `optional`.
fn rows<'a>(
    holder: &'a Map<String, Value>,
    key: &str,
    what: &str,
    optional: bool,
) -> Result<Vec<&'a Map<String, Value>>> {
    match holder.get(key) {
        None | Some(Value::Null) if optional => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_object().ok_or_else(|| {
                    not_a_graph(&format!("an entry of `{key}` of {what} is not an object"))
                })
            })
            .collect(),
        _ => Err(not_a_graph(&format!("{what} has no list `{key}`"))),
    }
}

/// The numbers of a runtime graph.
///
/// `phase_of` answers the phase of a stage in the stage graph; a stage it
/// does not have is counted under `unknown`. `overlay` is what the state
/// file of a complete workflow says of its stages, slug and outcome in the
/// order it lists them (`focus_state::completed_overlay`); `None` while the
/// workflow is not complete.
///
/// # Errors
///
/// The graph is not one the engine writes (a missing field, an outcome that
/// is none of the three, a count that is not one), or a timestamp the
/// duration depends on has digits and is not an instant written with its
/// zone (see [`focus_js::date`]).
pub fn summarize(
    graph: &Value,
    phase_of: &dyn Fn(&str) -> Result<Option<String>>,
    overlay: Option<&[(String, &'static str)]>,
) -> Result<Map<String, Value>> {
    let Some(graph) = graph.as_object() else {
        return Err(not_a_graph("it is not an object"));
    };
    let mut stages = Outcomes::default();
    let mut by_phase: Vec<(String, Outcomes)> = Vec::new();
    let mut memory = [0_u64; 5];
    let mut sensors = [0_u64; 5];
    let mut learnings = [0_u64; 2];
    let mut latest: Option<&str> = None;
    let mut seen: Vec<&str> = Vec::new();

    let mut tally = |phase: &str, outcome: &str, stages: &mut Outcomes| {
        let at = by_phase
            .iter()
            .position(|(known, _)| known == phase)
            .unwrap_or_else(|| {
                by_phase.push((phase.to_owned(), Outcomes::default()));
                by_phase.len() - 1
            });
        stages.count(outcome);
        by_phase[at].1.count(outcome);
    };
    let phase = |slug: &str| -> Result<String> {
        Ok(phase_of(slug)?.unwrap_or_else(|| "unknown".to_owned()))
    };

    for stage in rows(graph, "stages", "the graph", false)? {
        let slug = text(stage, "stage_slug", "a stage")?;
        let what = format!("the stage {slug}");
        seen.push(slug);
        let phase = phase(slug)?;
        if let Some(captured) = optional_object(stage, "learnings_captured", &what)? {
            add(
                &mut learnings[0],
                count(captured, "from_orchestrator", &what)?,
            )?;
            add(
                &mut learnings[1],
                count(captured, "from_user_addition", &what)?,
            )?;
        }
        let instances = rows(stage, "instances", &what, true)?;
        // A stage's own completion, or failing that its bolts' latest.
        let mut completed = optional_text(stage, "completed_at", &what)?;
        if completed.is_none() {
            for instance in &instances {
                let merged =
                    optional_text(instance, "completed_at", &what)?.filter(|at| !at.is_empty());
                if merged.is_some_and(|at| {
                    completed.is_none_or(|known| cmp_utf16(at, known) == Ordering::Greater)
                }) {
                    completed = merged;
                }
            }
        }
        if let Some(at) = completed.filter(|at| !at.is_empty())
            && latest.is_none_or(|known| cmp_utf16(at, known) == Ordering::Greater)
        {
            latest = Some(at);
        }
        let stated = overlay.and_then(|overlay| overlay.iter().find(|(known, _)| known == slug));
        // A stage built by bolts counts once per bolt, and what the state
        // file says of the stage does not reach them.
        let by_bolts = !instances.is_empty();
        let units = if by_bolts { instances } else { vec![stage] };
        for unit in units {
            let outcome = match stated {
                Some((_, stated)) if !by_bolts => stated,
                _ => outcome(unit, &what)?,
            };
            tally(&phase, outcome, &mut stages);
            if let Some(breakdown) = optional_object(unit, "memory_breakdown", &what)? {
                let entries = match unit.get("memory_entries") {
                    None | Some(Value::Null) => 0,
                    Some(_) => count(unit, "memory_entries", &what)?,
                };
                add(&mut memory[0], entries)?;
                for (at, key) in [
                    "interpretations",
                    "deviations",
                    "tradeoffs",
                    "open_questions",
                ]
                .iter()
                .enumerate()
                {
                    add(&mut memory[at + 1], count(breakdown, key, &what)?)?;
                }
            }
            for firing in rows(unit, "sensor_firings", &what, false)? {
                sensors[0] += 1;
                let ended = match text(firing, "result", &format!("a sensor firing of {what}"))? {
                    "passed" => 1,
                    "failed" => 2,
                    "budget-override" => 3,
                    _ => 4,
                };
                sensors[ended] += 1;
            }
        }
    }
    for (slug, outcome) in overlay.unwrap_or_default() {
        if !seen.contains(&slug.as_str()) {
            tally(&phase(slug)?, outcome, &mut stages);
        }
    }

    let started_at = text(graph, "started_at", "the graph")?;
    let duration = match latest {
        None => Value::Null,
        Some(end) => match (date::parse(end), date::parse(started_at)) {
            // Whole minutes, half a minute going up; none when it would run
            // backwards.
            (Instant::At(end), Instant::At(start)) if end >= start => {
                json!((end - start + 30_000) / 60_000)
            }
            (Instant::At(_), Instant::At(_)) | (Instant::NotADate, _) | (_, Instant::NotADate) => {
                Value::Null
            }
            (ended, _) => {
                let unread = if ended == Instant::Unread {
                    end
                } else {
                    started_at
                };
                return Err(focus_js::EngineError::new(format!(
                    "The workflow's duration runs from {started_at} to {end}, and the timestamp {unread} is not an instant Focus reads: it wants the form 2026-01-05T10:00:00Z, with the zone written out, on a date the calendar has. Correct the timestamp in the ledger and compile again."
                )));
            }
        },
    };

    let mut summary = Map::new();
    summary.insert(
        "workflow_id".to_owned(),
        json!(text(graph, "workflow_id", "the graph")?),
    );
    summary.insert(
        "scope".to_owned(),
        json!(text(graph, "scope", "the graph")?),
    );
    summary.insert("started_at".to_owned(), json!(started_at));
    summary.insert("duration_minutes".to_owned(), duration);
    summary.insert("stages".to_owned(), stages.json());
    let phases: Map<String, Value> = by_phase
        .into_iter()
        .map(|(phase, counts)| (phase, counts.json()))
        .collect();
    summary.insert("by_phase".to_owned(), Value::Object(js_key_order(phases)));
    summary.insert(
        "memory".to_owned(),
        json!({
            "total": memory[0],
            "interpretations": memory[1],
            "deviations": memory[2],
            "tradeoffs": memory[3],
            "open_questions": memory[4],
        }),
    );
    summary.insert(
        "sensors".to_owned(),
        json!({
            "total": sensors[0],
            "passed": sensors[1],
            "failed": sensors[2],
            "budget_override": sensors[3],
            "incomplete": sensors[4],
        }),
    );
    summary.insert(
        "learnings".to_owned(),
        json!({ "from_orchestrator": learnings[0], "from_user_addition": learnings[1] }),
    );
    Ok(summary)
}

/// A summary as a person reads it.
#[must_use]
pub fn render(summary: &Map<String, Value>) -> String {
    let at = |path: &[&str]| -> String {
        let mut value = summary.get(path[0]);
        for key in &path[1..] {
            value = value.and_then(|value| value.get(key));
        }
        match value {
            Some(Value::String(text)) => text.clone(),
            Some(other) => other.to_string(),
            None => String::new(),
        }
    };
    let duration = match summary.get("duration_minutes") {
        Some(Value::Null) | None => "in progress".to_owned(),
        Some(minutes) => format!("{minutes} min"),
    };
    let phases: Vec<String> = summary
        .get("by_phase")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(phase, counts)| {
            let of = |key: &str| counts.get(key).and_then(Value::as_u64).unwrap_or(0);
            // Padded to fourteen, counted as upstream counts a text's length.
            let pad = 14_usize.saturating_sub(phase.encode_utf16().count());
            let mut line = format!(
                "  {phase}{} {}/{} approved",
                " ".repeat(pad),
                of("approved"),
                of("total")
            );
            if of("failed") > 0 {
                line.push_str(&format!(", {} failed", of("failed")));
            }
            if of("pending") > 0 {
                line.push_str(&format!(", {} pending", of("pending")));
            }
            line
        })
        .collect();
    let phases = if phases.is_empty() {
        "  (none)".to_owned()
    } else {
        phases.join("\n")
    };
    format!(
        "Session Summary
===============

Workflow:   {}
Scope:      {}
Duration:   {duration}

Stages
  Total:      {}
  Approved:   {}
  Failed:     {}
  Pending:    {}

By phase
{phases}

Memory entries
  Total:            {}
  Interpretations:  {}
  Deviations:       {}
  Trade-offs:       {}
  Open questions:   {}

Sensors
  Fired:            {}
  Passed:           {}
  Failed:           {}
  Budget-override:  {}
  Incomplete:       {}

Learnings captured
  From orchestrator:    {}
  From user additions:  {}",
        at(&["workflow_id"]),
        at(&["scope"]),
        at(&["stages", "total"]),
        at(&["stages", "approved"]),
        at(&["stages", "failed"]),
        at(&["stages", "pending"]),
        at(&["memory", "total"]),
        at(&["memory", "interpretations"]),
        at(&["memory", "deviations"]),
        at(&["memory", "tradeoffs"]),
        at(&["memory", "open_questions"]),
        at(&["sensors", "total"]),
        at(&["sensors", "passed"]),
        at(&["sensors", "failed"]),
        at(&["sensors", "budget_override"]),
        at(&["sensors", "incomplete"]),
        at(&["learnings", "from_orchestrator"]),
        at(&["learnings", "from_user_addition"]),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn phase_of(slug: &str) -> Result<Option<String>> {
        Ok(match slug {
            "intent-capture" => Some("ideation".to_owned()),
            "code-generation" | "build-and-test" => Some("construction".to_owned()),
            "nine" => Some("9".to_owned()),
            "accented" => Some("constru\u{e7}\u{e3}o".to_owned()),
            _ => None,
        })
    }

    fn stage(slug: &str, outcome: &str, completed_at: Value) -> Value {
        json!({
            "stage_slug": slug,
            "started_at": "2026-01-05T10:01:00Z",
            "completed_at": completed_at,
            "memory_entries": 3,
            "memory_breakdown": {"interpretations": 1, "deviations": 2, "tradeoffs": 0, "open_questions": 0},
            "sensor_firings": [
                {"result": "passed"}, {"result": "failed"}, {"result": "budget-override"}, {"result": "incomplete"},
                {"result": "something-new"},
            ],
            "outcome": outcome,
            "learnings_captured": {"from_orchestrator": 2, "from_user_addition": 1},
        })
    }

    fn graph(stages: Value) -> Value {
        json!({"workflow_id": "w", "scope": "feature", "started_at": "2026-01-05T10:00:00Z", "stages": stages})
    }

    #[test]
    fn a_graph_is_counted_by_outcome_phase_diary_and_sensor() {
        let mut by_bolts = stage("code-generation", "pending", Value::Null);
        by_bolts["memory_breakdown"] = Value::Null;
        by_bolts["learnings_captured"] = Value::Null;
        let mut api = stage("x", "failed", json!("2026-01-05T10:40:00Z"));
        api["sensor_firings"] = json!([]);
        let mut web = stage("x", "pending", Value::Null);
        web["memory_breakdown"] = Value::Null;
        by_bolts["instances"] = json!([api, web]);
        let graph = graph(json!([
            stage("intent-capture", "approved", json!("2026-01-05T10:05:00Z")),
            by_bolts,
            stage("nine", "approved", json!("")),
            stage("made-up", "pending", Value::Null),
        ]));
        let summary = Value::Object(summarize(&graph, &phase_of, None).unwrap());
        // The stage built by bolts counts once for each of them, and its own
        // row not at all.
        assert_eq!(
            summary["stages"],
            json!({"total": 5, "approved": 2, "failed": 1, "pending": 2})
        );
        // A phase named like a number comes first, as a JavaScript object
        // lists it; the rest in the order they were met.
        let phases: Vec<&String> = summary["by_phase"].as_object().unwrap().keys().collect();
        assert_eq!(phases, ["9", "ideation", "construction", "unknown"]);
        assert_eq!(
            summary["by_phase"]["construction"],
            json!({"total": 2, "approved": 0, "failed": 1, "pending": 1})
        );
        // Diaries of the units that have a breakdown: three stages and one bolt.
        assert_eq!(
            summary["memory"],
            json!({"total": 12, "interpretations": 4, "deviations": 8, "tradeoffs": 0, "open_questions": 0})
        );
        // A result that is none of the three endings is incomplete.
        assert_eq!(
            summary["sensors"],
            json!({"total": 20, "passed": 4, "failed": 4, "budget_override": 4, "incomplete": 8})
        );
        assert_eq!(
            summary["learnings"],
            json!({"from_orchestrator": 6, "from_user_addition": 3})
        );
        // From the workflow's start to the latest completion, a bolt's included.
        assert_eq!(summary["duration_minutes"], 40);
        let keys: Vec<&String> = summary.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "workflow_id",
                "scope",
                "started_at",
                "duration_minutes",
                "stages",
                "by_phase",
                "memory",
                "sensors",
                "learnings"
            ]
        );
    }

    #[test]
    fn the_duration_is_whole_minutes_and_none_when_it_cannot_be() {
        let minutes = |completed_at: &str| {
            let graph = graph(json!([stage(
                "intent-capture",
                "approved",
                json!(completed_at)
            )]));
            summarize(&graph, &phase_of, None).unwrap()["duration_minutes"].clone()
        };
        assert_eq!(minutes("2026-01-05T10:00:29.999Z"), 0);
        assert_eq!(minutes("2026-01-05T10:00:00Z"), 0);
        // Half a minute goes up.
        assert_eq!(minutes("2026-01-05T10:00:30Z"), 1);
        assert_eq!(minutes("2026-01-05T12:01:30+02:00"), 2);
        // Ended before it started, or a word where the time goes.
        assert_eq!(minutes("2026-01-05T09:59:59Z"), Value::Null);
        assert_eq!(minutes("soon"), Value::Null);
        // Nothing completed: still running.
        let running = graph(json!([stage("intent-capture", "pending", Value::Null)]));
        assert_eq!(
            summarize(&running, &phase_of, None).unwrap()["duration_minutes"],
            Value::Null
        );
    }

    /// Deliberate deviation. Upstream's duration is a difference of two
    /// `Date.parse` answers, whatever its runtime makes of each text. Focus
    /// refuses a timestamp that has digits and is not an instant written
    /// with its zone, as the compile does.
    #[test]
    fn a_duration_between_timestamps_focus_does_not_read_is_refused() {
        let graph = graph(json!([stage(
            "intent-capture",
            "approved",
            json!("2026-01-05T10:05:00")
        )]));
        // Upstream: five minutes, in whatever zone the machine is in.
        let refused = summarize(&graph, &phase_of, None).unwrap_err();
        assert!(
            refused
                .message()
                .contains("the timestamp 2026-01-05T10:05:00 is not an instant Focus reads"),
            "{refused}"
        );
    }

    #[test]
    fn a_complete_workflows_state_file_has_the_last_word() {
        // What the state file of a complete workflow says of its stages
        // (`focus_state::completed_overlay`), in the order it lists them.
        let overlay = [
            ("intent-capture".to_owned(), "approved"),
            ("code-generation".to_owned(), "pending"),
            ("made-up".to_owned(), "approved"),
        ];
        let graph = graph(json!([
            stage("intent-capture", "pending", Value::Null),
            stage("code-generation", "approved", Value::Null),
        ]));
        let summary = summarize(&graph, &phase_of, Some(&overlay)).unwrap();
        // The two rows take the state file's word, and the stage the graph
        // never saw is counted too, under the phase the stage graph gives it.
        assert_eq!(
            summary["stages"],
            json!({"total": 3, "approved": 2, "failed": 0, "pending": 1})
        );
        assert_eq!(
            summary["by_phase"]["unknown"],
            json!({"total": 1, "approved": 1, "failed": 0, "pending": 0})
        );
        // A stage built by bolts is counted by its bolts, whatever the state
        // file says of it.
        let mut by_bolts = stage("code-generation", "pending", Value::Null);
        by_bolts["instances"] = json!([stage("x", "failed", Value::Null)]);
        let summary = summarize(
            &super::tests::graph(json!([by_bolts])),
            &phase_of,
            Some(&overlay),
        )
        .unwrap();
        assert_eq!(
            summary["stages"],
            json!({"total": 3, "approved": 2, "failed": 1, "pending": 0})
        );
    }

    /// Deliberate deviation. On a file that is not shaped as the engine
    /// writes it, upstream fails with what its runtime says of the value
    /// (`undefined is not an object`), or carries on and counts under a name
    /// that is no outcome. Focus says the file is not a runtime graph, what
    /// is wrong with it, and how to write it again.
    #[test]
    fn a_file_that_is_not_a_runtime_graph_is_refused() {
        let refusal = |graph: Value| {
            summarize(&graph, &phase_of, None)
                .unwrap_err()
                .message()
                .to_owned()
        };
        let why = |graph: Value| {
            let refusal = refusal(graph);
            let rest = refusal
                .strip_prefix("runtime-graph.json is not a runtime graph as the engine writes it: ")
                .unwrap_or_else(|| panic!("{refusal}"));
            rest.strip_suffix(". Run `focus engine runtime compile` to write it again.")
                .unwrap_or_else(|| panic!("{refusal}"))
                .to_owned()
        };
        assert_eq!(why(json!([])), "it is not an object");
        assert_eq!(why(json!({})), "the graph has no list `stages`");
        assert_eq!(
            why(graph(json!([null]))),
            "an entry of `stages` of the graph is not an object"
        );
        assert_eq!(
            why(graph(json!([{"outcome": "approved"}]))),
            "a stage has no text `stage_slug`"
        );
        // Upstream: a summary with `"odd": null` among the outcomes.
        assert_eq!(
            why(graph(json!([stage("intent-capture", "odd", Value::Null)]))),
            "the stage intent-capture has the outcome `odd`"
        );
        let mut no_firings = stage("intent-capture", "approved", Value::Null);
        no_firings.as_object_mut().unwrap().remove("sensor_firings");
        assert_eq!(
            why(graph(json!([no_firings]))),
            "the stage intent-capture has no list `sensor_firings`"
        );
        let mut counted = stage("intent-capture", "approved", Value::Null);
        counted["memory_entries"] = json!("3");
        assert_eq!(
            why(graph(json!([counted]))),
            "the stage intent-capture has no count `memory_entries`"
        );
        // Nor is a number past the largest a double holds exactly a count.
        let mut huge = stage("intent-capture", "approved", Value::Null);
        huge["memory_entries"] = json!(9_007_199_254_740_992_u64);
        assert_eq!(
            why(graph(json!([huge]))),
            "the stage intent-capture has no count `memory_entries`"
        );
        // Upstream reads these three without a word: a value that is no
        // object counts for nothing, and a sum past the largest exact
        // integer is printed rounded.
        for key in ["learnings_captured", "memory_breakdown"] {
            for wrong in [json!(true), json!("none"), json!(3), json!([])] {
                let mut odd = stage("intent-capture", "approved", Value::Null);
                odd[key] = wrong;
                assert_eq!(
                    why(graph(json!([odd]))),
                    format!("`{key}` of the stage intent-capture is neither an object nor null")
                );
            }
            let mut none = stage("intent-capture", "approved", Value::Null);
            none[key] = Value::Null;
            assert!(summarize(&graph(json!([none])), &phase_of, None).is_ok());
        }
        let learned = |count: u64| {
            let mut stage = stage("intent-capture", "approved", Value::Null);
            stage["learnings_captured"]["from_orchestrator"] = json!(count);
            stage
        };
        let largest = 9_007_199_254_740_991_u64;
        assert_eq!(
            why(graph(json!([learned(largest), learned(1)]))),
            "its counts add up to more than a number holds exactly"
        );
        let exact = summarize(
            &graph(json!([learned(largest - 1), learned(1)])),
            &phase_of,
            None,
        )
        .unwrap();
        assert_eq!(exact["learnings"]["from_orchestrator"], json!(largest));
    }

    #[test]
    fn a_summary_reads_as_a_page() {
        let graph = graph(json!([
            stage("intent-capture", "approved", json!("2026-01-05T10:05:00Z")),
            stage("code-generation", "failed", Value::Null),
            stage("build-and-test", "pending", Value::Null),
        ]));
        let page = render(&summarize(&graph, &phase_of, None).unwrap());
        assert!(page.starts_with("Session Summary\n===============\n\nWorkflow:   w\nScope:      feature\nDuration:   5 min\n"));
        assert!(page.contains("\nBy phase\n  ideation       1/1 approved\n  construction   0/2 approved, 1 failed, 1 pending\n\n"));
        assert!(
            page.ends_with(
                "Learnings captured\n  From orchestrator:    6\n  From user additions:  3"
            )
        );
        // Padded by the name's length as JavaScript counts it, not by bytes.
        let accented = super::tests::graph(json!([stage("accented", "approved", Value::Null)]));
        let page = render(&summarize(&accented, &phase_of, None).unwrap());
        assert!(
            page.contains("\n  constru\u{e7}\u{e3}o     1/1 approved\n"),
            "{page}"
        );
        let nothing = render(&summarize(&super::tests::graph(json!([])), &phase_of, None).unwrap());
        assert!(
            nothing.contains("Duration:   in progress\n")
                && nothing.contains("\nBy phase\n  (none)\n")
        );
    }
}

//! `log answers`: the questions of a stage and the answers they have, read
//! off the ledger (`handleAnswers` in `aidlc-log.ts`). Read only: it
//! writes nothing, and a refusal of its own leaves no row.
//!
//! A question is a `DECISION_RECORDED` row and an answer a
//! `QUESTION_ANSWERED` one; neither names the other. One reply answers
//! every question that was open when it arrived (a menu's questions are
//! logged, then one answer for the reply), and an answer with no open
//! question of its own that follows a closed exchange, with no question
//! logged in between, continues that exchange. Two rows in different
//! shards with the same instant have no order, so an answer that could be
//! one of several questions' is shown as ambiguous instead of being given
//! to whichever shard's name sorts first.
//!
//! # Tests that another test covers
//!
//! Two questions upstream asks change no answer here. Two rows are in one
//! scope when their stage and their scope are the same, and every row read
//! is of the one stage asked for. And a row is never asked whether it is
//! before itself, so a row of a shard is before another by a smaller
//! position or by one no greater alike.

use std::collections::HashMap;

use focus_audit::history::{Row, UNTRUSTED_AUDIT_NOTICE};
use focus_audit::ledger::{Ledger, Machine};
use focus_audit::read::block_field;
use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{Result, bail};
use focus_lifecycle::reply::is_non_answer;
use focus_workspace::Layout;
use focus_workspace::records::Invoker;
use serde_json::{Map, Value, json};

use crate::flags;

/// `interactionScope`: what a question and its answer share beside the
/// stage.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Scope {
    unit: Option<String>,
    attempt_generation: Option<String>,
    workflow: Option<String>,
}

fn scope(row: &Row) -> Scope {
    let field = |name: &str| block_field(&row.block, name).map(str::to_owned);
    Scope {
        unit: field("Unit"),
        attempt_generation: field("Attempt Generation"),
        workflow: field("Workflow"),
    }
}

/// The scope's fields on a view, where they are there.
fn put_scope(view: &mut Map<String, Value>, scope: &Scope) {
    for (key, value) in [
        ("unit", &scope.unit),
        ("attemptGeneration", &scope.attempt_generation),
        ("workflow", &scope.workflow),
    ] {
        if let Some(value) = value {
            view.insert(key.to_owned(), json!(value));
        }
    }
}

/// `sameInteractionScope`.
fn same_scope(a: &Row, b: &Row) -> bool {
    block_field(&a.block, "Stage") == block_field(&b.block, "Stage") && scope(a) == scope(b)
}

/// `attemptEventDefinitelyBefore`: by position within a shard, by instant
/// between shards. Two rows of different shards at one instant are neither
/// before the other.
fn definitely_before(earlier: &Row, later: &Row) -> bool {
    if earlier.shard == later.shard {
        return earlier.pos < later.pos;
    }
    cmp_utf16(&earlier.timestamp, &later.timestamp) == std::cmp::Ordering::Less
}

fn question_text(row: &Row) -> &str {
    block_field(&row.block, "Decision").unwrap_or("")
}

/// `questionView`.
fn question_view(row: &Row) -> Map<String, Value> {
    let options: Vec<&str> = block_field(&row.block, "Options")
        .unwrap_or("")
        .split(',')
        .map(trim)
        .filter(|option| !option.is_empty())
        .collect();
    let mut view = Map::new();
    view.insert("question".to_owned(), json!(question_text(row)));
    view.insert("options".to_owned(), json!(options));
    put_scope(&mut view, &scope(row));
    view.insert("askedAt".to_owned(), json!(row.timestamp));
    view
}

/// `handleAnswers`: the answered questions of the stage, the open ones,
/// and the answers that cannot be given to one question.
///
/// # Errors
///
/// The flags are not `--stage <slug> [--unit <unit>]`; no piece of work is
/// selected, or its state file or ledger cannot be read.
pub fn answers(layout: &Layout, invoker: &Invoker, args: &[String]) -> Result<Value> {
    let (positional, flags) = flags::parse(args)?;
    let Some(stage) = flags.given("stage") else {
        bail!("Missing --stage <slug>");
    };
    if !positional.is_empty() || flags.names().any(|name| name != "stage" && name != "unit") {
        bail!("Usage: focus-log answers --stage <slug> [--unit <unit>]");
    }
    let unit = flags.get("unit");
    let rows: Vec<Row> = Ledger::new(layout, Machine::this())
        .active_history(invoker)?
        .into_iter()
        // The history holds the notes too; a note is neither a question
        // nor an answer, so it takes no part below.
        .filter(|row| {
            block_field(&row.block, "Stage") == Some(stage)
                && block_field(&row.block, "Checkpoint").is_none()
                && unit.is_none_or(|unit| block_field(&row.block, "Unit") == Some(unit))
        })
        .collect();
    // Rows are named by their place in `rows`; each list keeps the order
    // its rows were put in, as upstream's sets do.
    let asked: Vec<usize> = (0..rows.len())
        .filter(|&at| {
            rows[at].event == "DECISION_RECORDED"
                && block_field(&rows[at].block, "Decision").is_some()
        })
        .collect();
    let mut questions = asked.clone();
    let mut pending: Vec<usize> = (0..rows.len())
        .filter(|&at| rows[at].event == "QUESTION_ANSWERED")
        .collect();
    let mut unanswered: Vec<usize> = Vec::new();
    let mut uncertain: Vec<usize> = Vec::new();
    let mut answered: Vec<Map<String, Value>> = Vec::new();
    let mut ambiguous: Vec<Value> = Vec::new();
    // The latest closed exchange of each scope: the answer that closed it,
    // and the answered entries it produced, so a further answer joins them.
    let mut exchanges: HashMap<(Option<String>, Scope), (usize, Vec<usize>)> = HashMap::new();
    let details = |at: usize| block_field(&rows[at].block, "Details");
    let could_own = |question: usize, answer: usize| {
        same_scope(&rows[question], &rows[answer])
            && !definitely_before(&rows[answer], &rows[question])
    };
    let scope_key = |at: usize| {
        (
            block_field(&rows[at].block, "Stage").map(str::to_owned),
            scope(&rows[at]),
        )
    };

    while !pending.is_empty() {
        // Only answers with no known predecessor: a tie between shards
        // cannot spend a question by whichever file name sorts first.
        let frontier: Vec<usize> = pending
            .iter()
            .copied()
            .filter(|&answer| {
                !pending.iter().any(|&other| {
                    other != answer
                        && same_scope(&rows[other], &rows[answer])
                        && definitely_before(&rows[other], &rows[answer])
                })
            })
            .collect();
        let cycle = frontier.is_empty();
        let round = if cycle { pending.clone() } else { frontier };
        let results: Vec<(usize, Vec<usize>, bool)> = round
            .into_iter()
            .map(|answer| {
                let candidates: Vec<usize> = questions
                    .iter()
                    .copied()
                    .filter(|&question| could_own(question, answer))
                    .collect();
                let paired = !cycle
                    && !candidates.is_empty()
                    && candidates.iter().all(|&question| {
                        !uncertain.contains(&question)
                            && definitely_before(&rows[question], &rows[answer])
                            // A tied cancellation carries no answer, so it
                            // never competes with one.
                            && !pending.iter().any(|&other| {
                                other != answer
                                    && !is_non_answer(details(other))
                                    && could_own(question, other)
                                    && !definitely_before(&rows[answer], &rows[other])
                            })
                    });
                (answer, candidates, paired)
            })
            .collect();
        for (answer, candidates, paired) in results {
            let said = details(answer).unwrap_or("");
            let non_answer = is_non_answer(Some(said));
            if paired {
                let mut entries = Vec::new();
                for &question in &candidates {
                    if non_answer {
                        if !unanswered.contains(&question) {
                            unanswered.push(question);
                        }
                    } else {
                        let mut entry = question_view(&rows[question]);
                        entry.insert("answer".to_owned(), json!(said));
                        entry.insert("answeredAt".to_owned(), json!(rows[answer].timestamp));
                        let message = block_field(&rows[answer].block, "Message Id")
                            .or_else(|| block_field(&rows[question].block, "Message Id"));
                        if let Some(message) = message {
                            entry.insert("messageId".to_owned(), json!(message));
                        }
                        entries.push(answered.len());
                        answered.push(entry);
                    }
                    questions.retain(|&open| open != question);
                }
                if !non_answer {
                    exchanges.insert(scope_key(answer), (answer, entries));
                }
            } else if !non_answer {
                let key = scope_key(answer);
                let continued = exchanges.get(&key).filter(|(closed, _)| {
                    candidates.is_empty()
                        && definitely_before(&rows[*closed], &rows[answer])
                        && !asked.iter().any(|&question| {
                            same_scope(&rows[question], &rows[answer])
                                && definitely_before(&rows[*closed], &rows[question])
                                && definitely_before(&rows[question], &rows[answer])
                        })
                });
                if let Some((_, entries)) = continued {
                    let entries = entries.clone();
                    for &entry in &entries {
                        let joined = format!(
                            "{}; {said}",
                            answered[entry]["answer"].as_str().unwrap_or_default()
                        );
                        answered[entry].insert("answer".to_owned(), json!(joined));
                        answered[entry]
                            .insert("answeredAt".to_owned(), json!(rows[answer].timestamp));
                    }
                    exchanges.insert(key, (answer, entries));
                } else {
                    let mut view = Map::new();
                    put_scope(&mut view, &scope(&rows[answer]));
                    view.insert("answer".to_owned(), json!(said));
                    view.insert("answeredAt".to_owned(), json!(rows[answer].timestamp));
                    view.insert(
                        "candidates".to_owned(),
                        json!(
                            candidates
                                .iter()
                                .map(|&question| question_text(&rows[question]))
                                .collect::<Vec<_>>()
                        ),
                    );
                    ambiguous.push(Value::Object(view));
                    // A later answer cannot resolve whether this one already
                    // spent a prompt.
                    for question in candidates {
                        if !uncertain.contains(&question) {
                            uncertain.push(question);
                        }
                    }
                }
            }
            // An unpaired non-answer carries no answer, so it spends no prompt.
            pending.retain(|&waiting| waiting != answer);
        }
    }
    // What is still open and what was cancelled, in the ledger's order.
    let mut open: Vec<usize> = questions.iter().chain(&unanswered).copied().collect();
    open.sort_by(|&a, &b| {
        let (a, b) = (&rows[a], &rows[b]);
        cmp_utf16(&a.timestamp, &b.timestamp).then_with(|| {
            if a.shard == b.shard {
                a.pos.cmp(&b.pos)
            } else {
                a.shard_index.cmp(&b.shard_index).then(a.pos.cmp(&b.pos))
            }
        })
    });
    let mut out = Map::new();
    out.insert("data_notice".to_owned(), json!(UNTRUSTED_AUDIT_NOTICE));
    out.insert("stage".to_owned(), json!(stage));
    out.insert("answered".to_owned(), json!(answered));
    out.insert(
        "open".to_owned(),
        json!(
            open.iter()
                .map(|&question| question_view(&rows[question]))
                .collect::<Vec<_>>()
        ),
    );
    out.insert("ambiguous".to_owned(), json!(ambiguous));
    Ok(Value::Object(out))
}

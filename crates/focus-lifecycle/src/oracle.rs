//! The oracle's cases this crate answers.
//!
//! The lifecycle's commands are compared with upstream's by the
//! differential. What is here is the lifecycle's share of the oracle corpus:
//! readings of a ledger that upstream has as functions and no command shows
//! by itself. A case needs an install to be read against (the graph, the
//! scope grid), so [`run_case`] takes the workflow it is answered in:
//! `focus dev oracle` is told where one is with `--installed`.

use std::collections::BTreeSet;

use focus_audit::shards::{ShardEvent, parse_shard_events};
use serde_json::{Value, json};

use crate::review_attempt;
use crate::workflow::Workflow;

/// The case kinds this crate answers.
pub const KINDS: [&str; 2] = ["review_window", "review_accounting"];

/// The ledger a case gives as its shards, read as a project's would be.
fn ledger(case: &Value) -> Vec<ShardEvent> {
    case["shards"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .flat_map(|(index, shard)| {
            parse_shard_events(
                shard["content"].as_str().unwrap_or_default(),
                shard["shard"].as_str().unwrap_or_default(),
                index,
                &BTreeSet::new(),
            )
        })
        .collect()
}

/// Answer one oracle case in `wf`'s install. An unknown `kind` answers
/// `{"unsupported": kind}`.
#[must_use]
pub fn run_case(wf: &Workflow, case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    match text("kind") {
        "review_window" => {
            let per_unit = case["per_unit"] == true;
            let state = text("state");
            let stage_level = per_unit
                && crate::artifacts::uses_stage_level_per_unit_artifacts(
                    wf,
                    focus_state::fields::get_field(state, "Scope").unwrap_or_default(),
                    state,
                );
            let view = review_attempt::window_over(
                ledger(case),
                state,
                text("slug"),
                per_unit,
                stage_level,
                &wf.install.graph,
            );
            json!({
                "events": view
                    .events
                    .iter()
                    .map(|row| format!("{}:{}", row.shard_index, row.pos))
                    .collect::<Vec<_>>(),
                "floor": view.floor_idx.map_or(-1, |floor| i64::try_from(floor).unwrap_or(i64::MAX)),
                "merged": view.merged_bolt_units,
                "open": view.open_bolt_units,
            })
        }
        "review_accounting" => review_accounting_case(wf, case),
        other => json!({ "unsupported": other }),
    }
}

/// The reviews of one attempt, counted, with the review records the case
/// names laid out in a record's folder of its own.
fn review_accounting_case(wf: &Workflow, case: &Value) -> Value {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let text = |key: &str| case.get(key).and_then(Value::as_str);
    let record = std::env::temp_dir().join(format!(
        "focus-oracle-reviews-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&record);
    std::fs::create_dir_all(&record).expect("a scratch record");
    for (rel, content) in case["files"].as_object().into_iter().flatten() {
        let path = record.join(rel);
        std::fs::create_dir_all(path.parent().expect("a file has a folder")).expect("a folder");
        std::fs::write(path, content.as_str().unwrap_or_default()).expect("a review record");
    }
    let per_unit = case["per_unit"] == true;
    let state = text("state").unwrap_or_default();
    let slug = text("slug").unwrap_or_default();
    let stage_level = per_unit
        && crate::artifacts::uses_stage_level_per_unit_artifacts(
            wf,
            focus_state::fields::get_field(state, "Scope").unwrap_or_default(),
            state,
        );
    let view = review_attempt::window_over(
        ledger(case),
        state,
        slug,
        per_unit,
        stage_level,
        &wf.install.graph,
    );
    let counted = review_attempt::accounting(
        &view,
        state,
        &review_attempt::Reviewed {
            slug,
            per_unit,
            workspace_requires: case["workspace_requires"] == true,
            reviewer: text("reviewer").unwrap_or_default(),
            unit: text("unit"),
            workflow: text("workflow"),
        },
        &wf.install.graph,
        &|named| focus_evidence::review_record::read(Some(&record), named),
    );
    let _ = std::fs::remove_dir_all(&record);
    match counted {
        Err(error) => json!({ "error": error }),
        Ok(count) => json!({
            "floor": count.floor,
            "requests": count.request_count,
            "budgeted": count.budget_count,
            "bolt": [count.bolt_started, count.bolt_batch, count.bolt_slug],
            "pending": count.pending_iterations,
            "waiting": count
                .pending_requests
                .iter()
                .map(|(pass, request)| {
                    json!([
                        pass,
                        request.binding.request_id,
                        request.binding.artifact_fingerprint,
                        request.retried,
                        request.own_review_file,
                    ])
                })
                .collect::<Vec<_>>(),
            "recovery": [json!(count.recovery_iteration), json!(count.recovery_spent)],
            "ambiguity": count.ambiguity,
        }),
    }
}

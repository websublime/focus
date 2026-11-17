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
pub const KINDS: [&str; 3] = ["review_window", "review_accounting", "review_snapshot"];

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
        "review_snapshot" => review_snapshot_case(wf, case),
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
        // A file may be longer than the case holds: its text, then spaces.
        let padding = case["pad"][rel].as_u64().unwrap_or_default();
        let mut bytes = content.as_str().unwrap_or_default().as_bytes().to_vec();
        bytes.resize(
            bytes.len() + usize::try_from(padding).unwrap_or_default(),
            b' ',
        );
        std::fs::write(path, bytes).expect("a review record");
    }
    // A name that is a link to another place of the record, and a second
    // name of a file of it. Only where the platform has links as files.
    #[cfg(unix)]
    for (rel, target) in case["links"].as_object().into_iter().flatten() {
        let path = record.join(rel);
        std::fs::create_dir_all(path.parent().expect("a link has a folder")).expect("a folder");
        std::os::unix::fs::symlink(record.join(target.as_str().unwrap_or_default()), path)
            .expect("a link");
    }
    for (rel, target) in case["hard"].as_object().into_iter().flatten() {
        std::fs::hard_link(
            record.join(target.as_str().unwrap_or_default()),
            record.join(rel),
        )
        .expect("a second name");
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

/// What a review is of, in a project of the case's own, read with `wf`'s
/// install: the fingerprint of the stage's declared artifacts, the snapshot
/// with its appendix view, and what each kind of request would be told of
/// that snapshot.
fn review_snapshot_case(wf: &Workflow, case: &Value) -> Value {
    use focus_evidence::review::{LegacyAppendix, RequestBinding};
    use focus_evidence::source::raw;
    use focus_workspace::records::{Invoker, Selector, resolve_selection};
    use sha2::{Digest, Sha256};

    use crate::review_artifacts::{self, Options};

    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "focus-oracle-snapshot-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let layout = focus_workspace::Layout::new(root.clone(), wf.layout.harness);
    let space = layout.space_dir("default");
    std::fs::create_dir_all(&space).expect("a scratch space");
    let within = |rel: &str| {
        let path = space.join(rel);
        std::fs::create_dir_all(path.parent().expect("a name has a folder")).expect("a folder");
        path
    };
    for rel in case["dirs"].as_array().into_iter().flatten() {
        std::fs::create_dir_all(space.join(rel.as_str().unwrap_or_default())).expect("a folder");
    }
    for (rel, content) in case["files"].as_object().into_iter().flatten() {
        std::fs::write(within(rel), content.as_str().unwrap_or_default()).expect("a file");
    }
    for (rel, bytes) in case["hex"].as_object().into_iter().flatten() {
        let bytes = focus_evidence::oracle::unhex(bytes.as_str().unwrap_or_default());
        std::fs::write(within(rel), bytes).expect("a file");
    }
    if let Some(state) = case["state"].as_str() {
        let record = format!(
            "intents/rec-1a2b3c4d/{}",
            focus_workspace::names::STATE_FILE
        );
        std::fs::write(within(&record), state).expect("a state file");
    }
    // A name that is a link to another place of the space, and a second
    // name of a file of it. Only where the platform has links as files.
    #[cfg(unix)]
    for (rel, target) in case["links"].as_object().into_iter().flatten() {
        std::os::unix::fs::symlink(space.join(target.as_str().unwrap_or_default()), within(rel))
            .expect("a link");
    }
    for (rel, target) in case["hard"].as_object().into_iter().flatten() {
        std::fs::hard_link(space.join(target.as_str().unwrap_or_default()), within(rel))
            .expect("a second name");
    }
    let answer = (|| -> Result<Value, String> {
        let selection = resolve_selection(&layout, &Selector::default(), &Invoker::default())
            .map_err(|refusal| refusal.to_string())?;
        let scratch = Workflow {
            layout: layout.clone(),
            selection,
            invoker: Invoker::default(),
            install: wf.install.clone(),
        };
        let slug = case["slug"].as_str().unwrap_or_default();
        let stage = wf.stage(slug).map_err(|refusal| refusal.to_string())?;
        let unit = case["unit"].as_str();
        let merged: Option<BTreeSet<String>> = case["merged"].as_array().map(|units| {
            units
                .iter()
                .filter_map(|unit| unit.as_str().map(str::to_owned))
                .collect()
        });
        let asked = Options {
            require_required_artifacts: case["require"] == true,
            merged_bolt_units: merged.as_ref(),
        };
        let snapshot = review_artifacts::snapshot(&scratch, stage, unit, asked);
        let given = case.get("given_state").map(Value::as_str);
        let fingerprint = review_artifacts::fingerprint(&scratch, stage, unit, given, asked);
        let Some(snapshot) = snapshot else {
            return Ok(json!({ "fingerprint": fingerprint, "snapshot": null }));
        };
        let other = format!("sha256:{}", "0".repeat(64));
        let bodies = &snapshot.body_fingerprints;
        let requested = [
            snapshot.fingerprint.clone(),
            bodies[0].clone(),
            bodies[bodies.len() - 1].clone(),
            other.clone(),
            raw::raw_form(&snapshot.fingerprint).unwrap_or_else(|| other.clone()),
            raw::raw_form(&bodies[0]).unwrap_or(other),
        ];
        let told: Vec<Vec<[bool; 2]>> = requested
            .iter()
            .map(|artifact_fingerprint| {
                [None, Some(false), Some(true)]
                    .into_iter()
                    .map(|prior| {
                        let binding = RequestBinding {
                            artifact_fingerprint: artifact_fingerprint.clone(),
                            request_id: None,
                            legacy_appendix: prior.map(|prior_appendix| LegacyAppendix {
                                artifact: snapshot.review_artifact.clone(),
                                offset: 0,
                                prior_digest: None,
                                prior_length: None,
                                challenge: None,
                                prior_appendix,
                            }),
                            source_fingerprint: None,
                            unit_source_fingerprint: None,
                            recovery_cause: None,
                        };
                        [
                            review_artifacts::request_artifacts_current(&binding, &snapshot),
                            review_artifacts::appended_after_request(&binding, &snapshot),
                        ]
                    })
                    .collect()
            })
            .collect();
        let appendix: String = Sha256::digest(&snapshot.appendix)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Ok(json!({
            "fingerprint": fingerprint,
            "snapshot": {
                "fingerprint": snapshot.fingerprint,
                "artifact": snapshot.review_artifact,
                "bodies": bodies,
                "appendix": [snapshot.appendix.len(), appendix],
                "raw": std::iter::once(&snapshot.fingerprint)
                    .chain(bodies)
                    .map(|value| raw::raw_form(value))
                    .collect::<Vec<_>>(),
            },
            "told": told,
        }))
    })();
    let _ = std::fs::remove_dir_all(&root);
    match answer {
        Ok(out) => out,
        Err(error) => json!({ "error": error }),
    }
}

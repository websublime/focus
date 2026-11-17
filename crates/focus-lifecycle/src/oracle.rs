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
    // The project's own source, beside the engine's folder.
    for (rel, content) in case["source"].as_object().into_iter().flatten() {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("a name has a folder")).expect("a folder");
        std::fs::write(path, content.as_str().unwrap_or_default()).expect("a file");
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
        // A stage no install has: the compiled one with what the case says
        // of it (`null` takes a field away).
        let mut stage = wf
            .stage(slug)
            .map_err(|refusal| refusal.to_string())?
            .clone();
        for (field, value) in case["patch"].as_object().into_iter().flatten() {
            if value.is_null() {
                stage.raw.remove(field);
            } else {
                stage.raw.insert(field.clone(), value.clone());
            }
        }
        // The two lists the compiled stage keeps apart from its node.
        let names = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        };
        if let Some(listed) = case["patch"].get("produces") {
            stage.produces = names(listed);
        }
        if let Some(listed) = case["patch"].get("optional_produces") {
            stage.optional_produces = names(listed);
        }
        let stage = &stage;
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
        let pending = pending_answers(
            &scratch,
            stage,
            unit,
            asked,
            snapshot.as_ref(),
            case.get("source").is_some(),
        );
        let Some(snapshot) = snapshot else {
            return Ok(json!({ "fingerprint": fingerprint, "snapshot": null, "pending": pending }));
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
            "pending": pending,
        }))
    })();
    let _ = std::fs::remove_dir_all(&root);
    match answer {
        Ok(out) => out,
        Err(error) => json!({ "error": error }),
    }
}

/// What can be done with a request that waits for its verdict, for each
/// kind of request: of these artifacts or of others, of this source, of
/// what an earlier walk gave for it, of another or of none, and of each
/// era.
fn pending_answers(
    wf: &Workflow,
    stage: &focus_graph::runtime::Stage,
    unit: Option<&str>,
    asked: crate::review_artifacts::Options<'_>,
    snapshot: Option<&crate::review_artifacts::Snapshot>,
    has_source: bool,
) -> Value {
    use focus_evidence::review::{LegacyAppendix, RequestBinding};
    use focus_evidence::source::state::{legacy_alias, workspace_source_state};
    use focus_evidence::source::walk::Options as WalkOptions;

    use crate::review_attempt::{Accounting, PendingRequest};
    use crate::review_pending::{Asked, pending_request_currency, pending_review_request_status};

    let writes = crate::stage::workspace_requires(stage);
    let binds = writes && unit.is_some() && crate::stage::has_unit_marker(stage);
    let source = writes
        .then(|| workspace_source_state(&wf.layout.project, &[], &WalkOptions::from_env()).ok())
        .flatten();
    let now = source.as_ref().map_or_else(
        || "unbindable".to_owned(),
        |source| source.fingerprint.clone(),
    );
    let elsewhere = format!("sha256:{}", "0".repeat(64));
    let bits = |flags: &[bool]| -> String {
        flags
            .iter()
            .map(|flag| if *flag { '1' } else { '0' })
            .collect()
    };
    let mut bindings: Vec<RequestBinding> = Vec::new();
    for artifact_fingerprint in [
        snapshot.map_or_else(
            || elsewhere.clone(),
            |snapshot| snapshot.fingerprint.clone(),
        ),
        elsewhere.clone(),
    ] {
        // (A stage that does not write the workspace reads no source: one
        // request stands for the four. And a project the case gives no
        // source to is asked of its source and of none.)
        let sources = if writes && !has_source {
            vec![None, Some(now.clone())]
        } else if writes {
            vec![
                None,
                Some(now.clone()),
                Some(legacy_alias(&now).unwrap_or_else(|| "1".repeat(40))),
                Some("2".repeat(40)),
            ]
        } else {
            vec![None]
        };
        for source_fingerprint in sources {
            for (request_id, legacy) in [
                (Some(format!("review:{}", "a".repeat(32))), None),
                (None, None),
                (None, Some((false, None))),
                (None, Some((true, Some(5)))),
            ] {
                bindings.push(RequestBinding {
                    artifact_fingerprint: artifact_fingerprint.clone(),
                    request_id,
                    legacy_appendix: legacy.map(|(prior_appendix, prior_length)| LegacyAppendix {
                        artifact: snapshot
                            .map(|snapshot| snapshot.review_artifact.clone())
                            .unwrap_or_default(),
                        offset: 0,
                        prior_digest: None,
                        prior_length,
                        challenge: None,
                        prior_appendix,
                    }),
                    source_fingerprint: source_fingerprint.clone(),
                    unit_source_fingerprint: None,
                    recovery_cause: None,
                });
            }
        }
    }
    // (A unit of a per-unit stage that writes the workspace also binds the
    // unit's source, which is asked of a stage run by itself: the other is
    // a later slice's.)
    let options = Asked {
        require_required_artifacts: asked.require_required_artifacts,
        merged_bolt_units: asked.merged_bolt_units,
        single: binds,
        source_state: None,
    };
    let attempt =
        |pending_iterations: Vec<f64>, pending_requests: Vec<(f64, PendingRequest)>| Accounting {
            floor: String::new(),
            request_count: 0,
            budget_count: 0,
            bolt_started: false,
            bolt_batch: None,
            bolt_slug: None,
            pending_iterations,
            pending_requests,
            recovery_iteration: None,
            recovery_spent: false,
            ambiguity: None,
        };
    let waiting = |binding: &RequestBinding, retried: bool| PendingRequest {
        binding: binding.clone(),
        retried,
        own_review_file: false,
    };
    let shown = |status: Option<crate::review_pending::Status>, pass: f64| -> String {
        status.map_or_else(
            || "!".to_owned(),
            |status| {
                format!(
                    "{}{}",
                    if (status.iteration - pass).abs() < f64::EPSILON {
                        ""
                    } else {
                        "?"
                    },
                    bits(&[
                        status.request_current,
                        status.retryable,
                        status.verdict_recordable,
                        status.replaceable,
                    ])
                )
            },
        )
    };
    let (mut currency, mut status) = (String::new(), String::new());
    // The source as the caller has already taken it, said to be unreadable,
    // and (for the first requests) left for the currency to take.
    let taken = if writes {
        Asked {
            source_state: Some(source.as_ref()),
            ..options
        }
    } else {
        options
    };
    for (index, binding) in bindings.iter().enumerate() {
        let unreadable = Asked {
            source_state: Some(None),
            ..options
        };
        let ways = if index < 8 {
            vec![taken, unreadable, options]
        } else {
            vec![taken, unreadable]
        };
        for given in ways {
            match pending_request_currency(wf, stage, unit, binding, given) {
                Ok(got) => currency.push_str(&bits(&[
                    got.request_current,
                    got.readable,
                    got.modern_verdict_binding,
                ])),
                Err(_) => currency.push('!'),
            }
        }
        let other = RequestBinding {
            artifact_fingerprint: elsewhere.clone(),
            ..binding.clone()
        };
        let asked_of = attempt(
            vec![10.0, 9.0],
            vec![
                (10.0, waiting(&other, false)),
                (9.0, waiting(binding, (index / 4) % 2 == 1)),
            ],
        );
        status.push_str(&shown(
            pending_review_request_status(wf, stage, unit, &asked_of, taken)
                .ok()
                .flatten(),
            9.0,
        ));
    }
    let none =
        pending_review_request_status(wf, stage, unit, &attempt(Vec::new(), Vec::new()), options)
            .ok()
            .flatten();
    // A pass that waits with nothing to hold a request to.
    let unbound = pending_review_request_status(
        wf,
        stage,
        unit,
        &attempt(vec![2.0, 1.0], vec![(2.0, waiting(&bindings[0], false))]),
        options,
    )
    .ok()
    .flatten();
    json!({
        "currency": currency,
        "status": status,
        "none": none.map(|status| status.iteration),
        "unbound": unbound.map(|status| format!(
            "{}:{}",
            status.iteration,
            bits(&[
                status.request_current,
                status.retryable,
                status.verdict_recordable,
                status.replaceable,
            ])
        )),
    })
}

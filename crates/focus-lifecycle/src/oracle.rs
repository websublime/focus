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
pub const KINDS: [&str; 7] = [
    "review_window",
    "review_accounting",
    "review_snapshot",
    "summary_evidence",
    "source_snapshot",
    "review_receipts",
    "review_rows",
];

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
        "summary_evidence" => summary_evidence_case(wf, case),
        "source_snapshot" => source_snapshot_case(wf, case),
        "review_receipts" => review_receipts_case(wf, case),
        "review_rows" => {
            // What a review's row and a gate's say, read alone.
            use crate::review_receipts::{gate_stages, terminal_review_verdict, unit_plain_name};
            let listed = |key: &str| case[key].as_array().cloned().unwrap_or_default();
            json!({
                "terminal": listed("verdicts")
                    .iter()
                    .map(|asked| terminal_review_verdict(
                        asked[0].as_str(),
                        asked[1].as_str(),
                        asked[2].as_str().unwrap_or_default(),
                        asked[3].as_f64().unwrap_or(2.0),
                    ))
                    .collect::<Vec<_>>(),
                "gates": listed("blocks")
                    .iter()
                    .map(|block| gate_stages(block.as_str().unwrap_or_default()))
                    .collect::<Vec<_>>(),
                "names": listed("units")
                    .iter()
                    .map(|unit| unit_plain_name(unit.as_str().unwrap_or_default()))
                    .collect::<Vec<_>>(),
            })
        }
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

/// A project of a case's own, under a scratch root: `files`, `hex`
/// (bytes), `dirs`, `links` and `hard` under the
/// space's folder, `source` beside the engine's folder, and `state` as the
/// record's state file.
fn lay_project(wf: &Workflow, case: &Value) -> (std::path::PathBuf, focus_workspace::Layout) {
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
    (root, layout)
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

    let (root, layout) = lay_project(wf, case);
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
    // (The last artifacts are the set without a review appended to the
    // reviewed file, asked with no source.)
    let whole = snapshot.map_or_else(
        || elsewhere.clone(),
        |snapshot| snapshot.fingerprint.clone(),
    );
    let body = snapshot.map_or_else(
        || elsewhere.clone(),
        |snapshot| snapshot.body_fingerprints[0].clone(),
    );
    for (at, artifact_fingerprint) in [whole, elsewhere.clone(), body].into_iter().enumerate() {
        // (A stage that does not write the workspace reads no source: one
        // request stands for them all. And a project the case gives no
        // source to is asked of its source, of none and of one that could
        // not be bound.)
        let unbindable = Some("unbindable".to_owned());
        let sources = if !writes || at == 2 {
            vec![None]
        } else if has_source {
            vec![
                None,
                Some(now.clone()),
                Some(legacy_alias(&now).unwrap_or_else(|| "1".repeat(40))),
                Some("2".repeat(40)),
                unbindable,
            ]
        } else {
            vec![None, Some(now.clone()), unbindable]
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

/// Whether a stage's outputs descend from a current summary confirmation,
/// in a project of the case's own. The ledger's rows name the root as
/// `{P}` and what was confirmed by its hash: of the questions file as it is
/// (`{NOW}`, `{NOWRAW}` over its bytes) and of the text the case says was
/// confirmed (`{THEN}`, `{THENRAW}`).
fn summary_evidence_case(wf: &Workflow, case: &Value) -> Value {
    use focus_evidence::summary::content_hash;
    use focus_workspace::records::{Invoker, Selector, resolve_selection};
    use sha2::{Digest, Sha256};

    use crate::summary_evidence::{Asked, Evidence, check};

    let hashes = |key: &str| -> (String, String) {
        let text = case[key].as_str().unwrap_or_default();
        let raw: String = Sha256::digest(text.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        // A file that cannot be hashed was never confirmed under a scope.
        (content_hash(text).unwrap_or_else(|_| "0".repeat(64)), raw)
    };
    let (now, now_raw) = hashes("questions");
    let (then, then_raw) = hashes("then");
    // The root is known once the project is laid out: the files that name
    // it are written after.
    let mut bare = case.clone();
    bare["files"] = json!({});
    let (root, layout) = lay_project(wf, &bare);
    let shown_root = root.to_string_lossy().into_owned();
    let space = layout.space_dir("default");
    for (rel, content) in case["files"].as_object().into_iter().flatten() {
        let path = space.join(rel);
        std::fs::create_dir_all(path.parent().expect("a name has a folder")).expect("a folder");
        let text = content
            .as_str()
            .unwrap_or_default()
            .replace("{P}", &shown_root)
            .replace("{NOWRAW}", &now_raw)
            .replace("{NOW}", &now)
            .replace("{THENRAW}", &then_raw)
            .replace("{THEN}", &then);
        std::fs::write(path, text).expect("a file");
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
        let mut stage = wf
            .stage(case["slug"].as_str().unwrap_or_default())
            .map_err(|refusal| refusal.to_string())?
            .clone();
        for (field, value) in case["patch"].as_object().into_iter().flatten() {
            if value.is_null() {
                stage.raw.remove(field);
            } else {
                stage.raw.insert(field.clone(), value.clone());
            }
        }
        // The list the compiled stage keeps apart from its node.
        if let Some(listed) = case["patch"]["optional_produces"].as_array() {
            stage.optional_produces = listed
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect();
        }
        let given = case["given"] == true;
        let asked = Asked {
            workflow: case["workflow"].as_str(),
            state: case["state"].as_str().filter(|_| given),
            scope: case["scope"].as_str(),
            unit: case["unit"].as_str(),
        };
        let spelled = |text: &str| text.replace(&shown_root, "{P}");
        match check(&scratch, &stage, asked).map_err(|refusal| refusal.to_string())? {
            Evidence::Ok {
                required,
                accepted_changes,
                change_control_read,
            } => Ok(json!({
                "ok": true,
                "required": required,
                "accepted": accepted_changes
                    .iter()
                    .map(|change| json!([
                        change.stage,
                        change.unit,
                        change.changed,
                        change.recorded,
                        change.current,
                        change.notice,
                    ]))
                    .collect::<Vec<_>>(),
                "read": change_control_read,
            })),
            Evidence::Refused {
                code,
                message,
                coverage,
                change_control_read,
            } => Ok(json!({
                "ok": false,
                // (The refusal's code is on its recovery question, which
                // upstream asks only with a state in hand.)
                "code": given.then_some(code),
                "message": spelled(&message),
                "coverage": coverage.name(),
                "read": change_control_read,
            })),
        }
    })();
    let _ = std::fs::remove_dir_all(&root);
    match answer {
        Ok(out) => out,
        Err(error) => json!({ "error": error }),
    }
}

/// The two snapshots of a stage in a project of the case's own: written,
/// altered as the case says, written again, and read back under the
/// fingerprints a row could name.
fn source_snapshot_case(wf: &Workflow, case: &Value) -> Value {
    use focus_evidence::oracle::pairs;
    use focus_evidence::source::listing::{Listing, render_changed_paths};
    use focus_evidence::source::snapshot;
    use focus_evidence::source::walk::WorkspaceSource;
    use focus_workspace::names::ENGINE_DIR;
    use focus_workspace::records::{Invoker, Selector, resolve_selection};
    use sha2::{Digest, Sha256};

    let (root, layout) = lay_project(wf, case);
    let record = layout.space_dir("default").join("intents/rec-1a2b3c4d");
    let dirs = [
        record.join(ENGINE_DIR).join("source-review"),
        record.join(".focus-source-review"),
    ];
    let slug = case["slug"].as_str().unwrap_or_default();
    // A path of the scratch project, as the case's answers name it. The
    // record is reached by its real path where a link is looked for.
    let real = std::fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    let spelled = |text: &str| {
        let text = text
            .replace(real.to_string_lossy().as_ref(), "{P}")
            .replace(root.to_string_lossy().as_ref(), "{P}");
        if cfg!(windows) {
            text.replace('\\', "/")
        } else {
            text
        }
    };
    // What a system says of a file is the host's; the engine's own
    // sentences are kept.
    let said = |refusal: &dyn std::fmt::Display| -> Value {
        let text = spelled(&refusal.to_string());
        let own = focus_js::re::test(
            r"^(?:Source snapshot|Source path keys|Cannot write|Invalid canonical|Invalid source-listing|\{P\}.* is a symlink)",
            "",
            &text,
        );
        json!({ "error": if own { text } else { "<host>".to_owned() } })
    };
    let kept = || -> Value {
        fn walk(
            dir: &std::path::Path,
            record: &std::path::Path,
            found: &mut serde_json::Map<String, Value>,
        ) {
            let Ok(listed) = std::fs::read_dir(dir) else {
                return;
            };
            let mut names: Vec<_> = listed.flatten().map(|entry| entry.file_name()).collect();
            names.sort();
            // (A folder with nothing in it is said: one made and left is
            // something a write did.)
            if names.is_empty() {
                let rel = dir.strip_prefix(record).unwrap_or(dir);
                found.insert(rel.to_string_lossy().replace('\\', "/"), json!("empty"));
            }
            for name in names {
                let path = dir.join(&name);
                let Ok(meta) = std::fs::symlink_metadata(&path) else {
                    continue;
                };
                let rel = path
                    .strip_prefix(record)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if meta.file_type().is_symlink() {
                    found.insert(rel, json!("link"));
                } else if meta.is_dir() {
                    walk(&path, record, found);
                } else {
                    let digest: String = Sha256::digest(std::fs::read(&path).unwrap_or_default())
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect();
                    found.insert(rel, json!(digest));
                }
            }
        }
        let mut found = serde_json::Map::new();
        for dir in &dirs {
            walk(dir, &record, &mut found);
        }
        Value::Object(found)
    };
    let listed = |listing: Option<Listing>| -> Value {
        listing.map_or(Value::Null, |listing| {
            listing
                .iter()
                .map(|(key, entry)| json!([key, entry]))
                .collect()
        })
    };
    let answer = (|| -> Result<Value, String> {
        let selection = resolve_selection(&layout, &Selector::default(), &Invoker::default())
            .map_err(|refusal| refusal.to_string())?;
        let fingerprint = case["recorded"]["fingerprint"].as_str().unwrap_or_default();
        let state = WorkspaceSource {
            fingerprint: fingerprint.to_owned(),
            listing: pairs(&case["recorded"]["listing"]),
            legacy: None,
            dotnet_output_seen: false,
        };
        let baseline = if case["baseline"].is_null() {
            state.listing.clone()
        } else {
            pairs(&case["baseline"])
        };
        let write = || -> Vec<Value> {
            vec![
                match snapshot::write_baseline(&layout, &selection, slug, &baseline) {
                    Ok(fingerprint) => json!(fingerprint),
                    Err(refusal) => said(&refusal),
                },
                match snapshot::write_workspace_snapshot(&layout, &selection, slug, &state) {
                    Ok(kept) => json!(kept),
                    Err(refusal) => said(&refusal),
                },
            ]
        };
        let first = write();
        let files = kept();
        let hash = first[0].as_str().map_or_else(
            || "0".repeat(64),
            |written| written["sha256:".len()..].to_owned(),
        );
        // (A fingerprint that is no hash names no file: its first twelve
        // characters are taken as upstream's driver takes them.)
        let head = |text: &str| text.chars().take(12).collect::<String>();
        let place = |which: &str, legacy: bool| {
            dirs[usize::from(legacy)]
                .join(slug)
                .join(if which == "baseline" {
                    format!("baseline-{}.tsv", head(&hash))
                } else {
                    format!("workspace-{}.tsv", head(fingerprint))
                })
        };
        for step in case["tamper"].as_array().into_iter().flatten() {
            let which = step[0].as_str().unwrap_or_default();
            let op = step[1].as_str().unwrap_or_default();
            let text = step[2]
                .as_str()
                .unwrap_or_default()
                .replace("{FP}", fingerprint);
            let path = place(which, false);
            let earlier = place(which, true);
            let folder = |path: &std::path::Path| {
                std::fs::create_dir_all(path.parent().expect("a name has a folder"))
                    .expect("a folder");
            };
            if op == "legacy-write" {
                folder(&earlier);
                std::fs::write(&earlier, text).expect("a file");
                continue;
            }
            // The folder of every stage's snapshots behind a link, with
            // this stage's folder not there.
            #[cfg(unix)]
            if op == "rootlink" {
                let linked = std::fs::symlink_metadata(&dirs[0])
                    .is_ok_and(|meta| meta.file_type().is_symlink());
                if dirs[0].exists() && !linked {
                    let moved = record.join("moved-root");
                    std::fs::rename(&dirs[0], &moved).expect("moved");
                    let _ = std::fs::remove_dir_all(moved.join(slug));
                    std::os::unix::fs::symlink(&moved, &dirs[0]).expect("a link");
                }
                continue;
            }
            // (A name made a folder is left one.)
            if !path.exists() || path.is_dir() {
                continue;
            }
            // (A name that was already made a link, or put behind one, is
            // left as it is.)
            let linked = |path: &std::path::Path| {
                std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
            };
            if matches!(op, "link" | "dirlink")
                && (linked(&path) || path.parent().is_some_and(linked))
            {
                continue;
            }
            let read =
                || String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()).into_owned();
            match op {
                "crlf" => std::fs::write(&path, read().replace('\n', "\r\n")).expect("a file"),
                "append" => std::fs::write(&path, read() + &text).expect("a file"),
                "write" => std::fs::write(&path, text).expect("a file"),
                "hex" => {
                    std::fs::write(&path, focus_evidence::oracle::unhex(&text)).expect("a file")
                }
                "bom" => {
                    let mut bytes = vec![0xef, 0xbb, 0xbf];
                    bytes.extend(std::fs::read(&path).unwrap_or_default());
                    std::fs::write(&path, bytes).expect("a file");
                }
                "remove" => std::fs::remove_file(&path).expect("gone"),
                "legacy" => {
                    folder(&earlier);
                    std::fs::rename(&path, &earlier).expect("moved");
                }
                "dir" => {
                    std::fs::remove_file(&path).expect("gone");
                    std::fs::create_dir(&path).expect("a folder");
                }
                #[cfg(unix)]
                "link" => {
                    let moved = record.join(format!("moved-{which}.tsv"));
                    std::fs::rename(&path, &moved).expect("moved");
                    std::os::unix::fs::symlink(&moved, &path).expect("a link");
                }
                #[cfg(unix)]
                "dirlink" => {
                    let folder = path.parent().expect("a name has a folder");
                    std::fs::rename(folder, record.join("moved")).expect("moved");
                    std::os::unix::fs::symlink(record.join("moved"), folder).expect("a link");
                }
                other => return Err(format!("no such alteration {other}")),
            }
        }
        let second = write();
        let after = kept();
        let more: Vec<String> = case["asks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|ask| ask.as_str().map(str::to_owned))
            .collect();
        let mut asked = vec![
            format!("sha256:{hash}"),
            hash.clone(),
            format!("sha256:{}", hash.to_uppercase()),
            format!("sha256:{}{}", head(&hash), "0".repeat(52)),
        ];
        asked.extend(more.iter().cloned());
        let mut fingerprints = vec![
            fingerprint.to_owned(),
            format!("{}{}", head(fingerprint), "0".repeat(52)),
            format!("sha256:{fingerprint}"),
        ];
        fingerprints.extend(more);
        let current = (!case["current"].is_null()).then(|| WorkspaceSource {
            fingerprint: "f".repeat(64),
            listing: pairs(&case["current"]),
            legacy: None,
            dotnet_output_seen: false,
        });
        let paths = |current: Option<&WorkspaceSource>| {
            snapshot::workspace_changed_paths(&layout, &selection, slug, fingerprint, current)
        };
        let changed = paths(current.as_ref());
        Ok(json!({
            "first": first,
            "files": files,
            "second": second,
            "after": after,
            "baseline": asked
                .iter()
                .map(|fingerprint| {
                    match snapshot::read_baseline(&layout, &selection, slug, fingerprint) {
                        Ok(listing) => listed(listing),
                        Err(refusal) => said(&refusal),
                    }
                })
                .collect::<Vec<_>>(),
            "workspace": fingerprints
                .iter()
                .map(|fingerprint| {
                    match snapshot::read_workspace_snapshot(&layout, &selection, slug, fingerprint) {
                        Ok(listing) => listed(listing),
                        Err(refusal) => said(&refusal),
                    }
                })
                .collect::<Vec<_>>(),
            "changed": match &changed {
                Ok(paths) => json!(paths),
                Err(refusal) => said(refusal),
            },
            "told": changed.ok().flatten().map(|paths| render_changed_paths(&paths)),
            "unasked": match paths(None) {
                Ok(paths) => json!(paths),
                Err(refusal) => said(&refusal),
            },
        }))
    })();
    let _ = std::fs::remove_dir_all(&root);
    answer.unwrap_or_else(|error| json!({ "error": error }))
}

/// The reviews of a stage that still stand, in a project of the case's own.
/// The ledger and the review records name what each engine computes for
/// itself: `{ART}` and `{ART:<unit>}` (the artifacts' fingerprint as laid
/// out), `{SRC}` (the source's) and `{DIGEST:<record>}` (a review
/// record's, once written). What the case changes `after` is done once
/// those are taken.
fn review_receipts_case(wf: &Workflow, case: &Value) -> Value {
    use focus_workspace::records::{Invoker, Selector, resolve_selection};
    use sha2::{Digest, Sha256};

    use crate::review_artifacts::{self, Options as Asked};
    use crate::review_receipts::{self, Baseline, Options, Pending, StaleProgress};

    let (root, layout) = lay_project(wf, case);
    let space = layout.space_dir("default");
    let record = space.join("intents/rec-1a2b3c4d");
    // The one variable a case sets is handed over, never set: the oracle
    // answers in a process that may be answering another case.
    let skip_source_freshness =
        case["env"][focus_workspace::env::full_name("SKIP_SOURCE_FRESHNESS").as_str()] == "1";
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
        // What the compiled stage keeps apart from its node.
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
        if let Some(marker) = case["patch"].get("for_each") {
            stage.for_each = marker.as_str().map(str::to_owned);
        }
        let state = case["state"].as_str().unwrap_or_default();
        let none = format!("sha256:{}", "0".repeat(64));
        let fingerprint = |unit: Option<&str>| {
            review_artifacts::fingerprint(
                &scratch,
                &stage,
                unit,
                Some(Some(state)),
                Asked::default(),
            )
            .unwrap_or_else(|| none.clone())
        };
        let mut filled: Vec<(String, String)> = Vec::new();
        for unit in case["units"].as_array().into_iter().flatten() {
            let unit = unit.as_str().unwrap_or_default();
            filled.push((format!("{{ART:{unit}}}"), fingerprint(Some(unit))));
        }
        filled.push(("{ART}".to_owned(), fingerprint(None)));
        let taken = crate::review_pending::current_source(&scratch)
            .ok()
            .flatten();
        // The listing behind the source's fingerprint, kept as a review
        // request keeps it.
        if case["keep"] == true
            && let Some(source) = &taken
        {
            let _ = focus_evidence::source::snapshot::write_workspace_snapshot(
                &scratch.layout,
                &scratch.selection,
                slug,
                source,
            );
        }
        filled.push((
            "{SRC}".to_owned(),
            taken.map_or_else(|| "unbindable".to_owned(), |source| source.fingerprint),
        ));
        let fill = |filled: &[(String, String)], text: &str| {
            filled.iter().fold(text.to_owned(), |out, (name, value)| {
                out.replace(name, value)
            })
        };
        let put = |path: &std::path::Path, text: &str| {
            std::fs::create_dir_all(path.parent().expect("a name has a folder")).expect("a folder");
            std::fs::write(path, text).expect("a file");
        };
        for (rel, text) in case["records"].as_object().into_iter().flatten() {
            let text = fill(&filled, text.as_str().unwrap_or_default());
            put(&record.join(rel), &text);
            let digest: String = Sha256::digest(text.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            filled.push((format!("{{DIGEST:{rel}}}"), format!("sha256:{digest}")));
        }
        for (name, text) in case["shards"].as_object().into_iter().flatten() {
            put(
                &record.join("audit").join(name),
                &fill(&filled, text.as_str().unwrap_or_default()),
            );
        }
        for (within, key) in [(&space, "files"), (&root, "source")] {
            for (rel, text) in case["after"][key].as_object().into_iter().flatten() {
                let path = within.join(rel);
                // (Whatever was at the name goes first: a file can take a
                // folder's place.)
                let _ = std::fs::remove_dir_all(&path);
                let _ = std::fs::remove_file(&path);
                if let Some(text) = text.as_str() {
                    put(&path, text);
                }
            }
        }
        // (A name of the space made a link to another, once everything is
        // written.)
        #[cfg(unix)]
        for (rel, target) in case["after"]["links"].as_object().into_iter().flatten() {
            let path = space.join(rel);
            let _ = std::fs::remove_dir_all(&path);
            let _ = std::fs::remove_file(&path);
            std::os::unix::fs::symlink(space.join(target.as_str().unwrap_or_default()), &path)
                .expect("a link");
        }
        // (And the source as it is once the case changed it: `{NOW}`.)
        filled.push((
            "{NOW}".to_owned(),
            crate::review_pending::current_source(&scratch)
                .ok()
                .flatten()
                .map_or_else(|| "unbindable".to_owned(), |source| source.fingerprint),
        ));
        // An answer names what each engine computed as the case does.
        let named = |value: &str| -> String {
            filled
                .iter()
                .fold(value.to_owned(), |out, (name, computed)| {
                    if *computed == none || computed == "unbindable" {
                        out
                    } else {
                        out.replace(computed, name)
                    }
                })
        };
        let got = review_receipts::fresh(
            &scratch,
            state,
            &stage,
            Options {
                review_class: case["class"].as_str(),
                skip_source_freshness: Some(skip_source_freshness),
                ..Options::default()
            },
        )
        .map_err(|refusal| refusal.to_string())?;
        let progress =
            |at: Option<&StaleProgress>| at.map(|at| json!([at.next_iteration, at.recovery_spent]));
        let pending = |at: Option<&Pending>| {
            at.map(|at| {
                json!([
                    at.state.as_str(),
                    at.iteration,
                    at.recovery,
                    at.did_not_finish,
                    at.verification_failed
                ])
            })
        };
        let keys =
            |names: &review_receipts::Names| names.keys().map(str::to_owned).collect::<Vec<_>>();
        Ok(json!({
            "stage": [
                got.stage_verdict,
                got.stage_stale,
                got.stage_iteration,
                progress(got.stage_stale_progress.as_ref()),
                pending(got.stage_pending.as_ref()),
            ],
            "units": {
                "verdicts": got.unit_verdicts.iter().map(|(unit, said)| json!([unit, said])).collect::<Vec<_>>(),
                "stale": keys(&got.unit_stale),
                "progress": got.unit_stale_progress.iter().map(|(unit, at)| json!([unit, progress(Some(at))])).collect::<Vec<_>>(),
                "iterations": got.unit_iterations.iter().map(|(unit, at)| json!([unit, at])).collect::<Vec<_>>(),
                "pending": got.unit_pending.iter().map(|(unit, at)| json!([unit, pending(Some(at))])).collect::<Vec<_>>(),
                "kept": keys(&got.unit_source_kept),
                "reopened": keys(&got.unit_recheck_reopened),
            },
            "source": [
                got.newest_source_fingerprint.as_deref().map(named),
                got.newest_source_unit,
                got.source_stale,
                got.source_stale_reason,
                progress(got.source_stale_progress.as_ref()),
                got.source_recovery_spent,
            ],
            "baseline": match &got.source_baseline {
                Baseline::Legacy => json!("legacy"),
                Baseline::Unbindable => json!("unbindable"),
                Baseline::Invalid => json!("invalid"),
                Baseline::Ready(listing) => json!([
                    "ready",
                    listing.iter().map(|(key, entry)| json!([key, entry])).collect::<Vec<_>>()
                ]),
            },
            "listing": got.current_source_listing.as_ref().map(focus_evidence::source::listing::Listing::len),
            "awaiting": keys(&got.awaiting_verdict),
            "unfinished": keys(&got.unfinished_verdicts),
            "bolts": [got.merged_bolt_units, got.open_bolt_units],
            "accepted": got
                .accepted_changes
                .iter()
                .map(|change| json!([
                    "review-receipt",
                    change.stage,
                    change.unit,
                    change.changed,
                    named(&change.recorded),
                    named(&change.current),
                    change.notice,
                ]))
                .collect::<Vec<_>>(),
            "read": got.change_control_read,
            // What only the reading of a unit's own source fills.
            "unit_source": [0, 0, 0],
        }))
    })();
    let shown_root = root.to_string_lossy().into_owned();
    let _ = std::fs::remove_dir_all(&root);
    answer.unwrap_or_else(|error| json!({ "error": error.replace(&shown_root, "{P}") }))
}

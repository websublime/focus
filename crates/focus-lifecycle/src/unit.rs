//! `state unit <start|pause|resume|complete>`: the receipts of a unit's
//! lifecycle in a stage that runs for each unit (`handleUnit`,
//! `aidlc-state.ts` L2 342).
//!
//! A completion is a receipt with evidence: it is recorded for the unit
//! that is open and in progress, once its review is not left waiting for a
//! verdict and every artifact the unit owes is on disk. The row carries
//! the floor of the stage's attempt ([`crate::run_floor`]), and the state
//! mirrors the latest checkpoint for cheap reads.
//!
//! # What is refused by name
//!
//! - **The route.** Before a start, a pause or a resumption upstream asks
//!   the orchestration engine which unit it routes now (it runs
//!   `orchestrate next` in a subprocess), so a serial receipt cannot land in
//!   a wave or on a unit the engine did not route. Focus has no
//!   orchestrator yet: the three refuse where that question is asked, after
//!   every refusal that comes before it.
//! - **A completion in a wave** (`--wave`), which settles an entry of the
//!   batch the engine routes.
//! - **A swarm.** With Construction built as a swarm, whether the swarm
//!   owns the units of a subagent's stage is the swarm referee's question.
//! - **A team's units**, which are claimed by a checkout.
//! - **A completion picked on a recovery ask**, which is read off the
//!   active-directive marker.

use std::time::SystemTime;

use focus_audit::read::has_unsafe_single_line_character;
use focus_audit::time::iso_timestamp;
use focus_audit::units::validate_unit_name;
use focus_graph::runtime::Stage;
use focus_graph::vocabulary::artifact_filename;
use focus_js::text::trim;
use focus_js::{Result, bail, num};
use focus_state::fields::{get_field, remove_field, set_field};
use serde_json::{Value, json};

use crate::admission::{artifact_guard_disabled, is_autonomous_mode, resolve_review_class};
use crate::artifacts::{self, Dag};
use crate::gates::guard_policy_accepts_changes;
use crate::guard_refusal::construction_checkpoints_apply;
use crate::review_attempt::{is_team_unit_ownership, unit_scoped_lifecycle_floors};
use crate::review_receipts::{self, PendingState};
use crate::unit_lifecycle::{self, Checkpoint};
use crate::workflow::Workflow;
use crate::{presence, review_artifacts, run_floor, stage as stages};

/// What the command line asks.
#[derive(Debug, Clone, Copy)]
pub struct Ask<'a> {
    pub action: Option<&'a str>,
    pub stage: Option<&'a str>,
    pub unit: Option<&'a str>,
    pub reason: Option<&'a str>,
    pub next_action: Option<&'a str>,
    pub set_aside_for: Option<&'a str>,
    pub wave: bool,
}

/// `validateStateLineValue`.
fn line_value(label: &str, value: Option<&str>) -> Result<()> {
    if value.is_some_and(has_unsafe_single_line_character) {
        bail!("{label} must be printable text on one physical line.");
    }
    Ok(())
}

/// `openUnitReview`: the unit's review is open with no verdict at all.
/// The iteration, and whether the request has to be made again.
fn open_review(
    wf: &Workflow,
    state: &str,
    stage: &Stage,
    unit: &str,
) -> Result<Option<(f64, bool)>> {
    // (A stage nobody reviews, and a class of review that is none, have no
    // receipts: upstream answers both before it reads them.)
    let class = resolve_review_class(wf, stage, state);
    let receipts = review_receipts::fresh(
        wf,
        state,
        stage,
        review_receipts::Options {
            review_class: Some(class),
            ..Default::default()
        },
    )?;
    let Some(pending) = receipts.unit_pending.get(unit) else {
        return Ok(None);
    };
    if receipts.awaiting_verdict.get(unit).is_none()
        || pending.verification_failed
        || (pending.recovery && guard_policy_accepts_changes(wf, state))
    {
        return Ok(None);
    }
    Ok(Some((
        pending.iteration,
        pending.state == PendingState::Outstanding,
    )))
}

/// `missingUnitArtifacts`: the artifacts the unit owes for the stage that
/// are not files under `<record>/construction/<unit>/<slug>/`. A stage that
/// maps kinds owes a unit only its kind's, when the unit DAG can be read.
fn missing_artifacts(wf: &Workflow, stage: &Stage, unit: &str, dag: &Dag) -> Vec<String> {
    // (With no record there is no ledger either, so no unit is open and
    // this is not asked: upstream's answer is kept all the same.)
    let Some(record) = wf.record_dir() else {
        return stage.produces.clone();
    };
    let required: Vec<&String> = match (stages::produces_kinds(stage), dag) {
        (
            Some(map),
            Dag::Ok {
                unit_kinds: Some(kinds),
                ..
            },
        ) => {
            let kind = kinds
                .iter()
                .find(|(name, _)| name == unit)
                .map(|(_, kind)| kind.as_str());
            artifacts::filter_produces_by_kind(Some(map), &stage.produces, kind)
        }
        _ => stage.produces.iter().collect(),
    };
    required
        .into_iter()
        .filter(|name| {
            let path = record
                .join("construction")
                .join(unit)
                .join(&stage.slug)
                .join(artifact_filename(name));
            !std::fs::metadata(&path).is_ok_and(|meta| meta.is_file())
        })
        .cloned()
        .collect()
}

/// The units whose completion stands (`unitCompletedReceipts`), and the
/// open ones (`unitOpenCheckpoints`).
fn lifecycle(wf: &Workflow, state: &str, stage: &Stage) -> (Vec<String>, Vec<Checkpoint>) {
    let rows = wf.events();
    let fingerprint = |unit: &str| {
        review_artifacts::fingerprint(
            wf,
            stage,
            Some(unit),
            None,
            // (All the unit owes, as upstream asks. A completion in a wave
            // always records the fingerprint of a whole set, so here it
            // only tells apart a row no engine writes.)
            review_artifacts::Options {
                require_required_artifacts: true,
                merged_bolt_units: None,
            },
        )
    };
    let snapshot = unit_lifecycle::snapshot(
        &rows,
        &stage.slug,
        state,
        &fingerprint,
        false,
        &wf.install.graph,
    );
    (snapshot.receipts, snapshot.open)
}

/// `handleUnit`.
///
/// # Errors
///
/// What upstream refuses, in its order and words; and what the module
/// docs name.
#[allow(clippy::too_many_lines)]
pub fn unit(wf: &Workflow, ask: Ask<'_>) -> Result<Value> {
    let Some(action) = ask
        .action
        .filter(|action| matches!(*action, "start" | "pause" | "resume" | "complete"))
    else {
        bail!(
            "Usage: focus engine state unit <start|pause|resume|complete> --stage <slug> --unit <name> [--reason <text>] [--next-action <text>] [--set-aside-for <unit>] [--wave]"
        );
    };
    let set_aside_for = ask.set_aside_for.map(trim);
    let reason = ask.reason.map(trim).filter(|reason| !reason.is_empty());
    let next_action = ask.next_action.map(trim).filter(|next| !next.is_empty());
    let Some(slug) = ask.stage.filter(|slug| !slug.is_empty()) else {
        bail!("Missing --stage <slug>");
    };
    let Some(unit) = ask.unit.filter(|unit| !unit.is_empty()) else {
        bail!("Missing --unit <name>");
    };
    if let Some(refusal) = validate_unit_name(unit) {
        bail!("{refusal}");
    }
    line_value("--reason", ask.reason)?;
    line_value("--next-action", ask.next_action)?;
    let Some(stage) = wf.install.stage(slug) else {
        bail!("Unknown stage: {slug}");
    };
    if !stages::has_unit_marker(stage) {
        bail!(
            "Stage \"{slug}\" is not per-unit (for_each: unit-of-work); unit receipts do not apply."
        );
    }
    // A unit set aside while already paused keeps its own reason and next
    // action, so they need not be given again.
    if action == "pause" && set_aside_for.is_none() {
        if reason.is_none() {
            bail!("unit pause requires --reason <text> (why the unit stopped).");
        }
        if next_action.is_none() {
            bail!("unit pause requires --next-action <text> (the exact next step on resume).");
        }
    }
    if ask.wave && action != "complete" {
        bail!("unit --wave is supported only with the complete action.");
    }
    if let Some(other) = set_aside_for {
        if action != "pause" {
            bail!("unit --set-aside-for is supported only with the pause action.");
        }
        if let Some(refusal) = validate_unit_name(other) {
            bail!("{refusal}");
        }
        if other == unit {
            bail!("unit --set-aside-for must name another unit.");
        }
    }

    // `validateLiveUnitScope`: without a team no checkout claims a unit.
    if is_team_unit_ownership(&wf.read_state()?) {
        bail!(
            "Unit Ownership is team, and a team's unit is worked in the checkout that claimed it, checked against the claim registry: Focus does not run a team's units."
        );
    }
    let held = wf.lock("recording a unit's step")?;
    let mut content = wf.read_state()?;
    let scope = get_field(&content, "Scope").unwrap_or_default();
    if artifacts::uses_stage_level_per_unit_artifacts(wf, scope, &content) {
        bail!(
            "Refusing unit {action} for \"{unit}\": the effective workflow plan skips Units Generation, so this stage runs once at stage level."
        );
    }

    // `autonomousSwarmOwnsStage`, as far as it is answered without a swarm.
    if stage.phase == "construction"
        && stages::mode(stage) == Some("subagent")
        && get_field(&content, "Construction Iteration").map(trim) != Some("unit-major")
    {
        // `isConstructionSwarmEnabled`.
        let swarm = if construction_checkpoints_apply(wf, &content) {
            get_field(&content, "Construction Execution") == Some("swarm")
        } else {
            is_autonomous_mode(&content)
        };
        if swarm {
            bail!(
                "Construction is built as a swarm here, and whether the swarm owns the units of \"{slug}\" (a subagent's stage) is the swarm referee's question: Focus runs no swarm yet."
            );
        }
    }

    let dag = artifacts::resolve_bolt_dag(wf);
    let units: &[String] = match &dag {
        Dag::Malformed { reason, detail } => bail!(
            "Refusing unit {action}: the authoritative unit DAG is {reason} ({detail}). Fix unit-of-work-dependency.md first."
        ),
        Dag::Ok { units, .. } => units,
        Dag::None => &[],
    };
    if !units.iter().any(|known| known == unit) {
        bail!("Refusing unit {action} for \"{unit}\": it is not in the authoritative unit DAG.");
    }
    if let Some(other) = set_aside_for
        && !units.iter().any(|known| known == other)
    {
        bail!(
            "Refusing unit pause for \"{unit}\": \"{other}\" is not in the authoritative unit DAG."
        );
    }

    let (completed, open) = lifecycle(wf, &content, stage);
    let checkpoint = open.first();

    if action != "complete" {
        bail!(
            "unit {action} asks the orchestration engine which unit it routes now, so that a receipt cannot land in a wave or on a unit the engine did not route: Focus has no orchestrator yet."
        );
    }
    if ask.wave {
        bail!(
            "unit complete --wave settles an entry of the wave the orchestration engine routes: Focus has no orchestrator yet."
        );
    }
    let Some(checkpoint) = checkpoint.filter(|open| open.unit == unit) else {
        // (`guardRecoveryAskSelected`: with no unit open, the person may
        // have picked recording this receipt on a recovery ask, which is
        // read off the active-directive marker.)
        if checkpoint.is_none() && focus_state::file::has_active_directive(&wf.state_path()) {
            bail!(
                "The engine has a step in hand for this record, and whether the person picked recording this completion on its recovery ask is read off that step's marker: Focus does not read it yet."
            );
        }
        // The refusal names the step that is true for this unit.
        if completed.iter().any(|done| done == unit) {
            bail!(
                "Refusing to complete unit \"{unit}\" for \"{slug}\": it is already completed. Nothing more to do for it; run `focus engine orchestrate next` for the next step."
            );
        }
        let active = checkpoint.map_or_else(
            || {
                format!(
                    " (no unit is active; start it first with `focus engine state unit start --stage {slug} --unit {unit}`)"
                )
            },
            |open| format!(" (active: \"{}\", {})", open.unit, open.state()),
        );
        bail!(
            "Refusing to complete unit \"{unit}\" for \"{slug}\": it is not the active unit{active}."
        );
    };
    if checkpoint.paused {
        let why = checkpoint
            .reason
            .as_deref()
            .filter(|reason| !reason.is_empty())
            .map_or_else(String::new, |reason| format!(" (reason: {reason})"));
        bail!(
            "Refusing to complete unit \"{unit}\" for \"{slug}\": it is paused{why}. Resume it first (focus engine state unit resume --stage {slug} --unit {unit}); a paused unit's work is not done."
        );
    }

    // The receipt waits for the unit's review: a request open with no
    // verdict means a finding is still owed. The person's words win.
    if let Some((iteration, request_again)) = open_review(wf, &content, stage, unit)?
        && !presence::person_spoke_since_gate(wf, false, true, false)
    {
        let reviewer = stages::reviewer(stage).unwrap_or_default();
        let iteration = num::to_string(iteration);
        let request = format!(
            "focus engine log review --stage {slug} --reviewer {reviewer} --unit {unit} --iteration {iteration}"
        );
        let theirs = " If the person said to go on without this review, run this completion again: their words let it record, and their approval (the Unit checkpoint's `verify --over-unfinished-review`, or the stage gate) goes over the review.";
        if request_again {
            bail!(
                "Refusing to complete unit \"{unit}\" for \"{slug}\": its review (iteration {iteration}) was requested before its outputs changed, so request it again with `{request}` and record the verdict with the same command plus `--verdict <READY|NOT-READY>`.{theirs}"
            );
        }
        bail!(
            "Refusing to complete unit \"{unit}\" for \"{slug}\": its review (iteration {iteration}) is still waiting for a verdict. Record it with `{request} --verdict <READY|NOT-READY>`; if the reviewer gave none, rerun that command with `--retry-pending` instead and dispatch the reviewer again.{theirs}"
        );
    }

    // The receipt commits only when every artifact the unit owes is there.
    if !artifact_guard_disabled(&wf.layout) {
        let missing = missing_artifacts(wf, stage, unit, &dag);
        if !missing.is_empty() {
            bail!(
                "Refusing to complete unit \"{unit}\" for \"{slug}\": required artifacts are missing on disk ({}). Write the unit's artifacts before completing it.",
                missing.join(", ")
            );
        }
    }

    let rows = wf.events();
    let floor = run_floor::unit_lifecycle(
        &rows,
        slug,
        unit_lifecycle::unit_major(&content),
        Some(unit),
        unit_scoped_lifecycle_floors(&content),
        &wf.install.graph,
    );
    wf.emit(
        &held,
        "UNIT_COMPLETED",
        &[
            ("Stage", slug.to_owned()),
            ("Unit", unit.to_owned()),
            ("Run floor", floor),
        ],
    )?;

    // The latest checkpoint, mirrored into the state for cheap reads: the
    // ledger stays the source.
    let timestamp = iso_timestamp(SystemTime::now());
    for field in [
        "Active Unit",
        "Unit Stage",
        "Unit State",
        "Unit Pause Reason",
        "Unit Next Action",
    ] {
        content = remove_field(&content, field);
    }
    content = set_field(&content, "Last Updated", &timestamp);
    wf.write_state(&content)?;
    Ok(json!({
        "emitted": "UNIT_COMPLETED",
        "stage": slug,
        "unit": unit,
        "timestamp": timestamp,
    }))
}

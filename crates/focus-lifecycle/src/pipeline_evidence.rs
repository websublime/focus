//! What a pipeline stage's chain is in this piece of work, the handoffs
//! of it that count, and the ones the stage still owes.
//!
//! A stage in `pipeline` mode hands its work from its lead agent to each
//! collaborator in turn, per repository when the piece of work records
//! any. `focus engine log link` records each return and reads this chain
//! to say whether a link is in order; a transition reads it to say which
//! handoffs are still owed before the stage is certified. The rows
//! themselves are read by [`focus_evidence::pipeline`]; this module adds
//! what needs the workflow: the collaborators the run gets, the registered
//! repositories, the Guard Policy, and the record a handoff file is in.
//!
//! Mirrors `effectiveSupportAgentsForProject`, `singleStageAttemptScope`,
//! `pipelineLinks` and `pipelineLinkEvidence` in `aidlc-lib.ts`.
//!
//! # Tests that change no answer
//!
//! - The ceremony's setting is `on` or `off` and nothing else, so asking
//!   for `off` and asking for anything but `on` are one question.
//! - Whether a receipt's file is still current is asked of the lead's
//!   receipts alone, and the chain asks it of no other: naming the lead
//!   there says who the question is about, and decides nothing.
//!
//! # What is not here
//!
//! What a stage run by itself owes. Upstream's reader then asks, of a
//! repository whose knowledge base the run kept, whether the store is
//! still current (`codekbStoreIsCurrent`); nothing asks that reader for a
//! stage run by itself until the orchestrator does (M5, slice 8), and
//! [`owed`] takes no such flag rather than answer without the store.

use std::path::PathBuf;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::pipeline::{self, Owed, Receipt};
use focus_graph::runtime::Stage;
use focus_js::Result;
use focus_state::fields::get_field;
use serde_json::Value;

use crate::workflow::Workflow;
use crate::{admission, gates, registry, stage};

/// `stage.support_agents`.
fn declared_supports(node: &Stage) -> Vec<String> {
    node.raw
        .get("support_agents")
        .and_then(Value::as_array)
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| agent.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `singleStageAttemptScope`: the scope the stage's own run recorded on its
/// start, the newest one.
fn single_stage_scope(events: &[ShardEvent], slug: &str) -> Option<String> {
    let workflow = format!("single-stage:{slug}");
    events
        .iter()
        .rfind(|row| {
            row.event == "STAGE_STARTED"
                && block_field(&row.block, "Stage") == Some(slug)
                && block_field(&row.block, "Workflow") == Some(workflow.as_str())
        })
        .and_then(|row| block_field(&row.block, "Scope").map(str::to_owned))
}

/// `effectiveSupportAgentsForProject`: the collaborators the stage gets in
/// this run. None when the `collaborators` ceremony is off (the kill
/// switch, the run's own setting, the scope's default); the declared ones
/// when the state cannot be read. A stage run by itself reads the scope its
/// attempt recorded and no state.
fn effective_supports(
    wf: &Workflow,
    node: &Stage,
    events: &[ShardEvent],
    single_run: bool,
) -> Vec<String> {
    let declared = declared_supports(node);
    if declared.is_empty() {
        return declared;
    }
    let (scope, state) = if single_run {
        (single_stage_scope(events, &node.slug), None)
    } else {
        match wf.read_state() {
            Ok(state) => (get_field(&state, "Scope").map(str::to_owned), Some(state)),
            Err(_) => return declared,
        }
    };
    let setting = admission::ceremony_setting_of(
        wf,
        scope.as_deref(),
        state.as_deref(),
        "collaborators",
        "Collaborators",
        "DISABLE_COLLABORATORS",
    );
    if setting == "off" {
        Vec::new()
    } else {
        declared
    }
}

/// `pipelineLinks`: the lead, then the collaborators this run gets.
/// `events` are in the order of [`pipeline::ordered`].
#[must_use]
pub fn links(wf: &Workflow, node: &Stage, events: &[ShardEvent], single_run: bool) -> Vec<String> {
    let mut links = vec![stage::lead_agent(node).to_owned()];
    links.extend(effective_supports(wf, node, events, single_run));
    links
}

/// The receipts of the stage's current attempt that count, chain by chain.
pub struct Counted {
    pub receipts: Vec<Receipt>,
    /// Whether the Guard Policy keeps changes: a rejection then keeps the
    /// receipts, and a handoff rewritten since its receipt still counts.
    pub accepts_changes: bool,
    /// The record a handoff file is in.
    pub record: Option<PathBuf>,
}

/// The receipts half of `pipelineLinkEvidence`: the attempt's receipts,
/// and of them the ones that make a chain in order. The developer's scan
/// (the lead of Reverse Engineering) counts while its file is the one its
/// receipt recorded.
#[must_use]
pub fn counted(
    wf: &Workflow,
    node: &Stage,
    events: &[ShardEvent],
    links: &[String],
    repos: &[String],
    single_run: bool,
) -> Counted {
    let state = wf.read_state().unwrap_or_default();
    let accepts_changes = gates::guard_policy_accepts_changes(wf, &state);
    let record = wf.record_dir();
    let raw = pipeline::current_receipts(
        events,
        &node.slug,
        single_run,
        accepts_changes,
        &wf.install.graph,
    );
    let is_lead_scan = node.slug == "reverse-engineering";
    let lead = stage::lead_agent(node);
    let receipts = pipeline::chain(&raw, links, repos, &|receipt| {
        !is_lead_scan
            || receipt.link != lead
            || pipeline::artifact_is_current(
                &wf.layout.project,
                record.as_deref(),
                receipt,
                accepts_changes,
            )
    });
    Counted {
        receipts,
        accepts_changes,
        record,
    }
}

/// What a pipeline stage still owes before it is certified.
pub struct Owing {
    /// Each link with no receipt that counts, by repository.
    pub missing: Vec<Owed>,
    /// The repositories the piece of work records.
    pub repos: Vec<String>,
}

/// `pipelineLinkEvidence`, for the stage in the piece of work: the
/// handoffs that are owed. A repository whose knowledge base this attempt
/// kept instead of rebuilding owes none.
///
/// # Errors
///
/// The registry's row for the piece of work names its repositories with
/// something that is not a list of names.
pub fn owed(wf: &Workflow, node: &Stage) -> Result<Owing> {
    let events = pipeline::ordered(wf.events());
    let links = links(wf, node, &events, false);
    let repos = registry::intent_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    let counted = counted(wf, node, &events, &links, &repos, false);
    let reused = pipeline::current_reuse(
        &events,
        &node.slug,
        false,
        counted.accepts_changes,
        &wf.install.graph,
    );
    Ok(Owing {
        missing: pipeline::missing(&links, &repos, &counted.receipts, &reused),
        repos,
    })
}

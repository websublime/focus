//! What can still be done with a review request that waits for its
//! verdict (`pendingRequestCurrency`, `pendingReviewRequestStatus`,
//! `aidlc-lib.ts` L18 999, L19 096).
//!
//! A request binds what it asked a review of: the stage's artifacts and,
//! for a stage that writes the workspace, the project's source. While the
//! binding still describes the bytes on disk the request is current: it
//! can be dispatched again once, and its verdict recorded. When the bytes
//! changed it can never finish, and a new request may take its pass. When
//! they cannot be read (an output is not there, the source cannot be
//! walked) nothing is proved either way: the request is not current, and
//! not one to replace, since putting the file back can make it current
//! again.
//!
//! # What is not here yet
//!
//! A unit of a per-unit stage that writes the workspace also binds the
//! unit's own source (`readUnitSourceManifest`, `unitSourceFingerprint`).
//! That is slice 5: until then such a request is refused by name, unless
//! the stage is run by itself, which binds no unit source.

use std::collections::BTreeSet;

use focus_evidence::review::{self, RequestBinding};
use focus_evidence::source::snapshot::UNBINDABLE;
use focus_evidence::source::state::{same_workspace_source, workspace_source_state};
use focus_evidence::source::walk::{Options, Stop, WorkspaceSource};
use focus_graph::runtime::Stage;
use focus_js::{Result, bail};

use crate::review_artifacts::{self, appended_after_request, request_artifacts_current};
use crate::review_attempt::Accounting;
use crate::stage;
use crate::workflow::Workflow;

/// What the currency of a request is asked with.
#[derive(Debug, Clone, Copy, Default)]
pub struct Asked<'a> {
    /// See [`review_artifacts::Options::require_required_artifacts`].
    pub require_required_artifacts: bool,
    /// See [`review_artifacts::Options::merged_bolt_units`].
    pub merged_bolt_units: Option<&'a BTreeSet<String>>,
    /// The stage is run by itself, which binds no unit source.
    pub single: bool,
    /// The project's source, when the caller has already taken it:
    /// `Some(None)` says it could not be read. `None` takes it here.
    pub source_state: Option<Option<&'a WorkspaceSource>>,
}

/// Whether a request's binding still describes the bytes on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Currency {
    pub request_current: bool,
    /// Everything the binding is compared with could be read.
    pub readable: bool,
    /// A verdict can be held to the binding.
    pub modern_verdict_binding: bool,
}

/// `workspaceSourceState(projectDir)`: the source of the selected piece of
/// work now; `None` when the walk stops.
///
/// # Errors
///
/// The record's repositories cannot be read, or the source is behind
/// something Focus does not read yet.
fn current_source(wf: &Workflow) -> Result<Option<WorkspaceSource>> {
    let repos = crate::registry::source_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    match workspace_source_state(&wf.layout.project, &repos, &Options::from_env()) {
        Ok(source) => Ok(Some(source)),
        Err(Stop::Failed(_)) => Ok(None),
        Err(Stop::NotPorted(what)) => bail!(
            "The source of this piece of work is behind {what}, and Focus does not read that yet: whether a review request still describes it cannot be told."
        ),
    }
}

/// `pendingRequestCurrency`: whether `binding` still describes the stage's
/// artifacts and, for a stage that writes the workspace, the source.
///
/// # Errors
///
/// As [`current_source`]; and the request is of a unit of a per-unit stage
/// that writes the workspace, whose unit source is not read yet.
pub fn pending_request_currency(
    wf: &Workflow,
    stage: &Stage,
    unit: Option<&str>,
    binding: &RequestBinding,
    asked: Asked<'_>,
) -> Result<Currency> {
    let Some(snapshot) = review_artifacts::snapshot(
        wf,
        stage,
        unit,
        review_artifacts::Options {
            require_required_artifacts: asked.require_required_artifacts,
            merged_bolt_units: asked.merged_bolt_units,
        },
    ) else {
        return Ok(Currency {
            request_current: false,
            readable: false,
            modern_verdict_binding: false,
        });
    };
    let writes_workspace = stage::workspace_requires(stage);
    let mut readable = true;
    let mut request_current =
        request_artifacts_current(binding, &snapshot) || appended_after_request(binding, &snapshot);
    let modern_verdict_binding = review::binding_is_modern(binding, writes_workspace);
    if writes_workspace {
        let taken;
        let source = match asked.source_state {
            Some(given) => given,
            None => {
                taken = current_source(wf)?;
                taken.as_ref()
            }
        };
        // A source that cannot be walked proves no change, so the request
        // is not one to replace; a walk that works again can make it
        // current.
        readable = source.is_some();
        let current = source.map_or(UNBINDABLE, |source| source.fingerprint.as_str());
        if let Some(recorded) = &binding.source_fingerprint
            && !same_workspace_source(Some(recorded), Some(current))
        {
            request_current = false;
        }
        if unit.is_some() && stage::has_unit_marker(stage) && !asked.single {
            bail!(
                "A review request of unit \"{}\" in \"{}\" binds the unit's own source, and Focus does not read that yet (units arrive with a later slice).",
                unit.unwrap_or_default(),
                stage.slug
            );
        }
    }
    Ok(Currency {
        request_current,
        readable,
        modern_verdict_binding,
    })
}

/// `PendingReviewRequestStatus`: what can be done with the oldest pending
/// request of an attempt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Status {
    /// The pass the request is of.
    pub iteration: f64,
    pub request_current: bool,
    /// It can be dispatched again against its own binding: once.
    pub retryable: bool,
    /// Its verdict can be recorded.
    pub verdict_recordable: bool,
    /// Its outputs or source changed, and all of them still read: it can
    /// never finish, and a new request may take its pass.
    pub replaceable: bool,
}

/// `pendingReviewRequestStatus`: the status of the pending request of the
/// lowest pass; `None` when no pass waits. A pass that waits with no
/// binding to hold a request to allows nothing.
///
/// # Errors
///
/// As [`pending_request_currency`].
pub fn pending_review_request_status(
    wf: &Workflow,
    stage: &Stage,
    unit: Option<&str>,
    attempt: &Accounting,
    asked: Asked<'_>,
) -> Result<Option<Status>> {
    let Some(iteration) = attempt
        .pending_iterations
        .iter()
        .copied()
        .min_by(f64::total_cmp)
    else {
        return Ok(None);
    };
    let Some((_, pending)) = attempt
        .pending_requests
        .iter()
        .find(|(pass, _)| *pass == iteration)
    else {
        return Ok(Some(Status {
            iteration,
            request_current: false,
            retryable: false,
            verdict_recordable: false,
            replaceable: false,
        }));
    };
    let currency = pending_request_currency(wf, stage, unit, &pending.binding, asked)?;
    Ok(Some(Status {
        iteration,
        request_current: currency.request_current,
        retryable: currency.request_current && !pending.retried,
        verdict_recordable: currency.request_current && currency.modern_verdict_binding,
        replaceable: currency.readable && !currency.request_current,
    }))
}

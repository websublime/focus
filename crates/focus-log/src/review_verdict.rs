//! `focus engine log review --verdict`: the verdict that closes a review
//! request.
//!
//! Port of the second half of `handleReview` (`aidlc-log.ts`). The request
//! opened a slot and bound it to the bytes the reviewer was given; the
//! verdict reads the review out of that slot, checks it says what the
//! command says, joins its findings to the stage's list, and writes the
//! review's record, the `REVIEW_COMPLETED` row that names it, and a copy a
//! person can read beside the artifact.
//!
//! # The three reviews a verdict can close
//!
//! The review file its request opened, which `--review-file` may name but
//! never replace: named, it must be that very file. The `## Review` section
//! an older release's reviewer appended to the artifact, with no file. And
//! none at all: a request that was already retried and still has no review
//! records `NOT-READY` without one, and its row says no reviewer gave the
//! verdict (`Review Finished: no`).
//!
//! `--unit` is refused before this module is reached.
//!
//! # What upstream tests that changes no answer
//!
//! Upstream refuses a pending request "with no valid request binding". The
//! count of an attempt skips a request row whose binding does not read
//! (`reviewAttemptAccounting`), so no pending request is without one.
//!
//! In a swarm the verdict also removes the dispatch record of the main
//! workspace (`removeParentReviewerDispatch`). That is asked of a unit's
//! worktree, and `--unit` is refused before this module is reached.
//!
//! Five more, among the three reviews, each one said again at its line.
//! A section that is the review is not also "no review" (`embeddedLegacy`
//! tests `!incompleteFallback`, which already needs no section). Such a
//! section is never checked against a challenge: a request gives one only
//! for a section it saw, and a section it saw was not appended after it.
//! The table of a verdict with no review is not read: its body is empty,
//! and an empty body has no table. Headings are made level 3 only in a
//! file: a section starts at its `## Review` and has no `#` or `##` after
//! it. And the draft is removed only when one was read: there is no other.

use std::path::Path;
use std::time::SystemTime;

use focus_evidence::review::{self as review_paths};
use focus_evidence::review_findings::{self, Finding};
use focus_evidence::review_record::{self, StoredFinding, Written};
use focus_evidence::review_shown::{self, Context as Shown};
use focus_evidence::review_text::{self, Expected, Validity};
use focus_evidence::source::snapshot::UNBINDABLE;
use focus_evidence::source::state::same_workspace_source;
use focus_js::{EngineError, Result, num, re};
use focus_lifecycle::change_control::{self, Accepted};
use focus_lifecycle::review_artifacts::{self, Options as ArtifactOptions};
use focus_lifecycle::review_derived::{self, Pending};
use focus_lifecycle::review_pending::{self, Asked as PendingAsked};
use focus_lifecycle::review_receipts::REVIEW_FINISHED_FIELD;
use focus_lifecycle::review_request::request_command;
use focus_lifecycle::summary_evidence::{self, Asked as SummaryAsked, Evidence};
use focus_lifecycle::workflow::Workflow;
use focus_lifecycle::{gates, stage};
use focus_workspace::{files, links, observer};

use crate::review::{Asked, Context, Step, Stop, load_context, platform_shell, refuse, set};

/// What a verdict that was recorded says back.
pub(crate) struct Recorded {
    /// The record's path, from the intent's record.
    pub(crate) review_record: String,
    /// The readable copy's, when one was written.
    pub(crate) review_markdown: Option<String>,
}

/// A path under the record as it is said from the project, with `/`.
fn from_project(asked: &Asked<'_>, record: &Path, relative: &str) -> String {
    focus_js::path::relative(
        focus_js::path::Flavour::host(),
        &asked.project_dir,
        &asked.project_dir,
        &record.join(relative).to_string_lossy(),
    )
    .replace('\\', "/")
}

/// `realpathSync`, spelled as a person reads it: Windows reports a
/// verbatim path, Node's `realpath` does not.
fn real_path(path: &Path) -> Result<String> {
    let real = std::fs::canonicalize(path)
        .map_err(|err| EngineError::new(format!("{}: {err}", path.display())))?;
    let text = real.to_string_lossy();
    Ok(match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned(),
    })
}

/// What reading the review file gave.
enum Draft {
    /// Its bytes; `None` when there is no file.
    Read(Option<Vec<u8>>),
    /// `--review-file` names something that is not the request's file.
    Another,
}

/// The reviewer's file, read the way the record will be read back: through
/// no link, one name, within the size a record may have.
///
/// A file `--review-file` names lives inside the intent's record, where the
/// reviewer's slot is, reached through no link, and is the request's own
/// file: never another request's review or a copy of one.
fn read_draft(asked: &Asked<'_>, record: &Path, draft: &str) -> Result<Draft> {
    let root = real_path(record)?;
    let anchor = Path::new(&root);
    links::refuse_link_in_chain(anchor, Path::new(draft))?;
    let target = anchor.join(draft);
    // (Asked of the name itself and not of what a link leads to; the chain
    // above has refused a link by now, so the two questions have one answer
    // here and this is the one that is meant.)
    let present = std::fs::symlink_metadata(&target).is_ok();
    if let Some(named) = asked.review_file {
        let host = focus_js::path::Flavour::host();
        let resolved = focus_js::path::resolve(host, &asked.project_dir, &[named]);
        let relative =
            focus_js::path::relative(host, &asked.project_dir, &root, &resolved).replace('\\', "/");
        if relative.is_empty()
            || relative == ".."
            || relative.starts_with("../")
            || focus_js::path::is_absolute(host, &relative)
        {
            return Err(EngineError::new(
                "the path is outside the active intent record",
            ));
        }
        links::refuse_link_in_chain(anchor, Path::new(&relative))?;
        if !present || !files::same_file(&anchor.join(&relative), &target)? {
            return Ok(Draft::Another);
        }
    }
    if !present {
        return Ok(Draft::Read(None));
    }
    files::read_regular_within(&target, "review file", Some(review_record::MAX_BYTES))
        .map(|bytes| Draft::Read(Some(bytes)))
}

/// `removeRecordFileNoFollow`: the draft, once the record holds it.
fn remove_draft(record: &Path, draft: &str) -> Result<()> {
    observer::refuse_write("removeRecordFileNoFollow")?;
    let anchor = std::fs::canonicalize(record)
        .map_err(|err| EngineError::new(format!("{}: {err}", record.display())))?;
    links::refuse_link_in_chain(&anchor, Path::new(draft))?;
    let path = anchor.join(draft);
    match std::fs::remove_file(&path) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
            Err(EngineError::new(format!("{}: {err}", path.display())))
        }
        _ => Ok(()),
    }
}

/// The readable copy's path: `<the artifact's folder>/reviews/review-NN.md`,
/// numbered after the copies already there.
fn copy_path(record: &Path, artifact_key: &str) -> String {
    let folder = match artifact_key.rsplit_once('/') {
        Some((folder, _)) => format!("{folder}/reviews"),
        None => "reviews".to_owned(),
    };
    let mut next: u64 = 1;
    if let Ok(entries) = std::fs::read_dir(record.join(&folder)) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(digits) = name
                .strip_prefix("review-")
                .and_then(|rest| rest.strip_suffix(".md"))
                .filter(|digits| re::test("^[0-9]+$", "", digits))
            else {
                continue;
            };
            // (`Number.isSafeInteger`: a number past it names no copy.)
            if let Ok(number) = digits.parse::<u64>()
                && number <= 9_007_199_254_740_991
                && number >= next
            {
                next = number + 1;
            }
        }
    }
    format!("{folder}/review-{next:02}.md")
}

/// The record's finding, in the words older readers read.
fn stored(finding: &Finding, derived: bool) -> StoredFinding {
    let status = if !derived {
        finding.status.clone()
    } else if finding.resolved_by_reviewer == Some(true) || finding.status == "Resolved" {
        "Resolved".to_owned()
    } else if finding.introduced_in_review == Some(true) {
        "New".to_owned()
    } else {
        "Unresolved".to_owned()
    };
    StoredFinding {
        id: finding.id.clone(),
        severity: finding.severity.clone(),
        location: finding.location.clone(),
        finding: finding.finding.clone(),
        required_action: finding.required_action.clone(),
        status,
    }
}

/// The verdict, under the audit lock. `notices` takes what the Guard Policy
/// let through, which is said whether or not the verdict is recorded.
#[allow(clippy::too_many_lines)]
pub(crate) fn verdict(
    wf: &Workflow,
    asked: &Asked<'_>,
    verdict: &str,
    mut fields: Vec<(&'static str, String)>,
    notices: &mut Vec<String>,
) -> Step<Recorded> {
    let held = wf.lock("log review")?;
    let Asked {
        stage: slug,
        reviewer,
        single,
        iteration,
        ..
    } = *asked;
    let Context {
        state,
        node,
        record,
        workflow,
        attempt,
        merged,
        require_required_artifacts,
        ..
    } = load_context(wf, slug, reviewer, single)?;
    let pass = num::to_string(iteration);
    #[allow(clippy::float_cmp)]
    let Some(pending) = attempt
        .pending_requests
        .iter()
        .find(|(known, _)| *known == iteration)
        .map(|(_, request)| request)
    else {
        return refuse(format!(
            "Cannot record a verdict for review iteration {pass} on \"{slug}\" because no pending request with that number exists. Start or retry that review first."
        ));
    };
    let binding = &pending.binding;

    // A review can outlive a confirmation or its retraction: the summary's
    // admission is checked again, as it was when the review was requested.
    let summary = summary_evidence::check(
        wf,
        node,
        SummaryAsked {
            workflow: workflow.as_deref(),
            state: Some(&state),
            scope: None,
            unit: None,
        },
    )?;
    let (Evidence::Ok {
        change_control_read,
        ..
    }
    | Evidence::Refused {
        change_control_read,
        ..
    }) = &summary;
    if *change_control_read {
        change_control::governed_guard_policy(wf, &held, &state)?;
        let accepted: Vec<Accepted<'_>> = match &summary {
            Evidence::Ok {
                accepted_changes, ..
            } => accepted_changes
                .iter()
                .map(|change| Accepted {
                    checkpoint: "summary-confirmation",
                    stage: &change.stage,
                    unit: change.unit.as_deref(),
                    changed: Some(&change.changed),
                    recorded: &change.recorded,
                    current: &change.current,
                    notice: &change.notice,
                })
                .collect(),
            Evidence::Refused { .. } => Vec::new(),
        };
        notices.extend(change_control::record_accepted_changes(
            wf, &held, &accepted,
        )?);
    }
    if let Evidence::Refused { message, .. } = &summary {
        return refuse(format!("Cannot record a review verdict: {message}"));
    }

    let artifact_options = ArtifactOptions {
        require_required_artifacts,
        merged_bolt_units: Some(&merged),
    };
    let Some(taken) = review_artifacts::snapshot(wf, node, None, artifact_options) else {
        return refuse(format!(
            "Cannot record review for \"{slug}\": the declared artifact set changed during the snapshot or is no longer a set of regular files."
        ));
    };
    let Some(record) = record.as_deref() else {
        return refuse("Cannot resolve the active intent record.".to_owned());
    };
    let attempt_id = review_paths::attempt_id(&attempt.floor);
    let record_relative = review_paths::record_relative_path(slug, None, &attempt_id, iteration)
        .map_err(Stop::Failed)?;

    // A request whose outputs or source changed is requested again at the
    // same pass; restoring the old bytes would undo the current work. The
    // restore remedy stays for a request that cannot be replaced.
    let changed_remedy = |restore: &str| -> Step<String> {
        let status = review_pending::pending_review_request_status(
            wf,
            node,
            None,
            &attempt,
            PendingAsked {
                require_required_artifacts,
                merged_bolt_units: Some(&merged),
                single,
                source_state: None,
            },
        )?;
        #[allow(clippy::float_cmp)]
        let replaceable =
            status.is_some_and(|status| status.iteration == iteration && status.replaceable);
        Ok(if replaceable {
            format!(
                "Request it again so the reviewer reviews what is there now: `{}`.",
                request_command(wf, &asked.command(iteration), platform_shell())
            )
        } else {
            restore.to_owned()
        })
    };

    // The reviewer writes a review, never the artifact: the bytes it was
    // dispatched on must be the bytes on disk now. Under a Guard Policy
    // that is not strict a change made meanwhile does not discard the
    // review: the verdict covers what the reviewer saw.
    let appended = review_artifacts::appended_after_request(binding, &taken);
    let changes_accepted = gates::guard_policy_accepts_changes_of(wf, None);
    let artifacts_moved =
        !review_artifacts::request_artifacts_current(binding, &taken) && !appended;
    if artifacts_moved && !changes_accepted {
        return refuse(format!(
            "Cannot record the verdict for \"{slug}\" because its output documents changed after review iteration {pass} started. {}",
            changed_remedy(
                "Restore the bytes the reviewer was dispatched on and re-run that exact iteration; --retry-pending cannot rebaseline changed content."
            )?
        ));
    }

    // Each request is reviewed in one file: the one its row names, or for a
    // request from before per-request files, the pass's shared file.
    let draft = review_paths::draft_relative_path(
        slug,
        None,
        &attempt_id,
        iteration,
        if pending.own_review_file {
            binding.request_id.as_deref()
        } else {
            None
        },
    )
    .map_err(Stop::Failed)?;
    let draft_shown = from_project(asked, record, &draft);
    // (A file the command named is said as the command named it.)
    let said_file = asked.review_file.unwrap_or(&draft_shown).to_owned();
    let body = match read_draft(asked, record, &draft) {
        Ok(Draft::Read(body)) => body,
        Ok(Draft::Another) => {
            return refuse(format!(
                "Cannot record review for \"{slug}\": {said_file} is not the review file for iteration {pass}. Have the reviewer write its review to {draft_shown}, then record the verdict again."
            ));
        }
        Err(error) => {
            return refuse(format!(
                "Cannot record review for \"{slug}\": the review file {said_file} is not a plain readable file ({error})."
            ));
        }
    };
    if body.is_some() && !taken.appendix.is_empty() && appended {
        return refuse(format!(
            "Cannot record the verdict for \"{slug}\": a `## Review` section was appended to the reviewed artifact after review iteration {pass} started and a review file was also written. The review file is the review; remove the appended section so the artifact carries the bytes the reviewer was dispatched on."
        ));
    }
    // With no review file: a request already retried that still has no
    // review records NOT-READY without one; and a section appended to the
    // artifact since the request is the review an older release's reviewer
    // wrote. (The two exclude each other: only one has a section.)
    let incomplete = body.is_none() && !appended && pending.retried && verdict == "NOT-READY";
    let embedded = body.is_none() && appended;
    if body.is_none() && !incomplete && !embedded {
        // After a retry the slot was reopened, so a review written before
        // the retry is not there any more.
        return refuse(if pending.retried {
            format!(
                "Cannot record review for \"{slug}\": no review was written for iteration {pass} since the request was retried. The retry reopened the review slot, so dispatch the reviewer to write {draft_shown} again, then record the verdict; a retried attempt that ends with no review records --verdict NOT-READY without one."
            )
        } else {
            format!(
                "Cannot record review for \"{slug}\": no review was written for iteration {pass}. The reviewer writes its review to {draft_shown}; a retried incomplete attempt records --verdict NOT-READY without a review."
            )
        });
    }
    let standalone = body.is_some();
    let mut review_bytes = body.unwrap_or_else(|| taken.appendix.clone());

    // The review says what the command says. A review whose reviewer wrote
    // `## What I verified` is not a reason to run the review again: a
    // review file's `#` and `##` lines are recorded as `###`, and the same
    // check runs on those bytes. A section in the artifact is checked as it
    // is, and has to open with its heading; and where no reviewer wrote
    // anything there is nothing to check.
    if !incomplete {
        let expected = Expected {
            verdict,
            reviewer,
            iteration,
            // (Upstream passes the request's challenge when the review is
            // a section. No such request has one: a challenge is given
            // for a section that was already there, and a request that saw
            // a section has none appended after it. See `request_binding`
            // and `appended_after_request`.)
            review_challenge: None,
            standalone,
        };
        let mut validity = review_text::validate_review_appendix(&review_bytes, &expected);
        // (Only a file: a section's bytes start at a `## Review` with no
        // `#` or `##` heading after it, so there would be none to change.)
        let demoted = if standalone {
            review_text::demote_review_headings(&review_bytes)
        } else {
            None
        };
        if let Some(demoted) = demoted {
            let again = review_text::validate_review_appendix(&demoted.bytes, &expected);
            match (&validity, &again) {
                // (Whatever the first reading said: upstream's check and
                // its demotion read headings with two readers, which
                // disagree on a heading right after a table, so there the
                // first reading can already be valid. Here one reader does
                // both, and a review it demotes is one whose first reading
                // named a heading.)
                (_, Validity::Valid) => {
                    review_bytes = demoted.bytes;
                    validity = again;
                    set(
                        &mut fields,
                        "Review Headings Made Level 3",
                        demoted.changed.join("; "),
                    );
                }
                (
                    Validity::Invalid { heading: true, .. },
                    Validity::Invalid { heading: false, .. },
                ) => {
                    validity = again;
                }
                _ => {}
            }
        }
        if let Validity::Invalid { reason, .. } = validity {
            return refuse(format!(
                "Refusing REVIEW_COMPLETED for \"{slug}\": {reason}."
            ));
        }
    }

    // No reviewer wrote this verdict: the review did not finish, and every
    // reader of the row says so.
    if incomplete {
        set(&mut fields, REVIEW_FINISHED_FIELD, "no".to_owned());
    }
    set(
        &mut fields,
        "Request Fingerprint",
        binding.artifact_fingerprint.clone(),
    );
    let artifact_fingerprint = if artifacts_moved {
        binding.artifact_fingerprint.clone()
    } else {
        taken.fingerprint.clone()
    };
    set(
        &mut fields,
        "Artifact Fingerprint",
        artifact_fingerprint.clone(),
    );
    if let Some(id) = &binding.request_id {
        set(&mut fields, "Request Id", id.clone());
    }
    if let Some(legacy) = &binding.legacy_appendix {
        for (name, value) in review_paths::legacy_appendix_echo_fields(legacy) {
            set(&mut fields, name, value);
        }
    }
    // The receipt is bound to the source the reviewer inspected. Only a
    // stage that writes the workspace carries this binding.
    let mut source_fingerprint: Option<String> = None;
    if stage::workspace_requires(node) {
        let current = review_pending::current_source(wf)?
            .map_or_else(|| UNBINDABLE.to_owned(), |source| source.fingerprint);
        let Some(requested) = &binding.source_fingerprint else {
            return refuse(format!(
                "Refusing REVIEW_COMPLETED for \"{slug}\": the matching REVIEW_REQUESTED iteration {pass} has no source fingerprint. Modernize that exact request with --retry-pending before recording the verdict."
            ));
        };
        if !same_workspace_source(Some(requested), Some(&current)) && !changes_accepted {
            return refuse(format!(
                "Refusing REVIEW_COMPLETED for \"{slug}\": workspace source changed after REVIEW_REQUESTED iteration {pass}. {}",
                changed_remedy("Restore the requested source state and re-dispatch the reviewer.")?
            ));
        }
        // The same source; a request recorded before a file was excluded
        // by name keeps its own value, which the completion pairs with.
        set(&mut fields, "Request Source Fingerprint", requested.clone());
        set(&mut fields, "Source Fingerprint", requested.clone());
        source_fingerprint = Some(requested.clone());
    }

    // The findings: the review's table, joined to the stage's list. A stage
    // run by itself reviews for its own gate and keeps them as written.
    let artifact_key = taken.review_artifact.as_str();
    let finding_artifact = from_project(asked, record, artifact_key);
    // (The verdict with no review keeps an empty body, and an empty body
    // has no table: upstream skips the reading, which gives the same.)
    let record_body = if incomplete {
        String::new()
    } else {
        String::from_utf8_lossy(&review_bytes).into_owned()
    };
    let table =
        review_findings::read_findings_table(&record_body, &finding_artifact, Some(verdict), None);
    let derived = if workflow.is_none() {
        let derived = review_derived::derive(
            wf,
            node,
            &finding_artifact,
            None,
            Some(&Pending {
                artifact: &finding_artifact,
                body: &record_body,
                verdict,
                unreadable_reason: table.unreadable.as_deref(),
                allow_malformed: pending.retried,
                // (The section being recorded is not also the list's start.)
                seed_legacy: !embedded,
            }),
        )?;
        if let Some(report) = &derived.malformed_report {
            return refuse(format!(
                "Refusing REVIEW_COMPLETED for \"{slug}\": {report}. Rerun this review request with --retry-pending and dispatch the reviewer once more."
            ));
        }
        Some(derived)
    } else {
        None
    };
    let findings: Vec<Finding> = match (&derived, &table.unreadable) {
        (Some(derived), _) => derived.findings.clone(),
        (None, Some(reason)) => {
            if !pending.retried {
                return refuse(format!(
                    "Refusing REVIEW_COMPLETED for \"{slug}\": {reason}."
                ));
            }
            vec![review_findings::unreadable_findings_table_finding(
                &finding_artifact,
                reason,
                None,
            )]
        }
        (None, None) => table.findings,
    };
    let written = Written {
        stage: slug.to_owned(),
        unit: None,
        workflow: workflow.clone(),
        attempt: attempt_id,
        iteration,
        reviewer: reviewer.to_owned(),
        verdict: verdict.to_owned(),
        request_id: binding.request_id.clone(),
        request_challenge: binding
            .legacy_appendix
            .as_ref()
            .and_then(|legacy| legacy.challenge.clone()),
        artifact_fingerprint,
        source_fingerprint,
        unit_source_fingerprint: None,
        findings: findings
            .iter()
            .map(|finding| stored(finding, derived.is_some()))
            .collect(),
        derived_findings: derived.as_ref().map(|derived| {
            derived
                .findings
                .iter()
                .map(review_record::derived_record_finding)
                .collect()
        }),
        body: record_body,
        recorded_at: focus_audit::time::iso_timestamp(SystemTime::now()),
    };
    let serialized = review_record::serialize(&written);
    // Readers refuse a record over the cap, so one is never written.
    if serialized.len() as u64 > review_record::MAX_BYTES {
        return refuse(format!(
            "Cannot record the verdict for \"{slug}\": the review record would be {} bytes, over the {}-byte limit readers accept. Shorten the review file {said_file} and record the verdict again.",
            serialized.len(),
            review_record::MAX_BYTES
        ));
    }
    if let Err(error) = focus_hooks::engine::write_record_file_no_follow(
        record,
        &record_relative,
        serialized.as_bytes(),
    ) {
        return refuse(format!(
            "Cannot record the verdict for \"{slug}\": the review record {record_relative} cannot be written ({error})."
        ));
    }
    set(&mut fields, "Review Record", record_relative.clone());
    set(
        &mut fields,
        "Review Record Digest",
        review_record::digest(serialized.as_bytes()),
    );
    wf.emit_plain(&held, "REVIEW_COMPLETED", &fields)?;
    // The draft was the reviewer's input; the record now holds it. A file
    // the command named stays where it is, as upstream leaves it. (With no
    // file read there is none to remove, and removing one that is not
    // there is not an error: `standalone` only spares the attempt.)
    if standalone && asked.review_file.is_none() {
        remove_draft(record, &draft)?;
    }

    // A copy for people, beside the artifact the review is about. The
    // record stays the engine's source of truth and nothing reads the copy
    // back, so failing to write it never withholds the verdict.
    // (A verdict recorded with no review has nothing to copy.)
    let mut review_markdown = None;
    if !written.body.is_empty() {
        let copy_relative = copy_path(record, artifact_key);
        let copy = match &derived {
            None => written.body.clone(),
            Some(derived) => review_shown::readable_review_copy(
                &written.body,
                &Shown {
                    artifact: &derived.artifact,
                    findings: &derived.findings,
                    findings_text: derived.findings_text.as_deref(),
                },
            ),
        };
        match focus_hooks::engine::write_record_file_no_follow(
            record,
            &copy_relative,
            copy.as_bytes(),
        ) {
            Ok(_) => review_markdown = Some(copy_relative),
            Err(error) => {
                eprintln!("warning: the readable review copy was not written: {error}");
            }
        }
    }
    Ok(Recorded {
        review_record: record_relative,
        review_markdown,
    })
}

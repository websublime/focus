//! The list of findings the engine keeps for a review's scope, one review
//! after another.
//!
//! A reviewer does not own the list. Each review says what became of the
//! findings it was shown and what it found; the engine applies that to the
//! list it had ([`apply_review_body`], `applyReviewBody`,
//! `aidlc-review-brief.ts` L454), and a person's decision at a gate is
//! applied the same way ([`apply_disposition`], L706). A finding keeps its
//! id for good, a new one takes the next number nobody used, and what one
//! review said about a finding (its note, whether it was re-checked,
//! whether it was raised or fixed there) is that review's only. A record an
//! older release wrote is the whole list as its reviewer wrote it
//! ([`legacy_record_list`], L659).
//!
//! These are the functions over the list itself. Reading the ledger for the
//! reviews and the decisions to apply is `deriveReviewFindingsList`'s.

use focus_js::{re, text as js_text};

use crate::review_findings::{
    Finding, Now, PriorReport, is_unreadable_findings_table_finding, parse_findings_report,
    parse_review_section,
};

/// The digits of an `R-<n>` id, without the zeros that lead them. Ids from
/// older reviewers carry any number of digits, so the number is never
/// narrowed: it stays its digits.
fn id_number(id: &str) -> &str {
    let digits = re::find("^R-([0-9]+)$", "", id)
        .and_then(|found| re::group(id, &found, 1))
        .unwrap_or("");
    digits.trim_start_matches('0')
}

/// Which of two numbers written in digits is the greater.
fn greater<'d>(left: &'d str, right: &'d str) -> &'d str {
    if (right.len(), right) > (left.len(), left) {
        right
    } else {
        left
    }
}

/// `nextFindingId`: the number after the highest in the list, so it never
/// lands on an id already there.
#[must_use]
pub fn next_finding_id(findings: &[Finding]) -> String {
    let highest = findings
        .iter()
        .map(|finding| id_number(&finding.id))
        .fold("", greater);
    let mut digits: Vec<u8> = highest.bytes().rev().collect();
    let mut carried = true;
    for digit in &mut digits {
        if *digit == b'9' {
            *digit = b'0';
        } else {
            *digit += 1;
            carried = false;
            break;
        }
    }
    if carried {
        digits.push(b'1');
    }
    let next: String = digits
        .iter()
        .rev()
        .map(|digit| char::from(*digit))
        .collect();
    format!("R-{next:0>2}")
}

const SEVERITIES: [&str; 3] = ["Minor", "Major", "Critical"];

/// `canonicalSeverity`: a severity word in its canonical spelling,
/// whatever its case; any other word as written.
#[must_use]
pub fn canonical_severity(severity: &str) -> String {
    let trimmed = js_text::trim(severity);
    let lowered = js_text::to_lower_case(trimmed);
    SEVERITIES
        .into_iter()
        .find(|known| js_text::to_lower_case(known) == lowered)
        .unwrap_or(trimmed)
        .to_owned()
}

/// `findingSeverityRank`.
#[must_use]
pub fn severity_rank(severity: &str) -> Option<usize> {
    let canonical = canonical_severity(severity);
    SEVERITIES.into_iter().position(|known| known == canonical)
}

/// `reviewerNote`: the parts that say something, as one note.
#[must_use]
pub fn reviewer_note(parts: &[Option<&str>]) -> Option<String> {
    let said: Vec<&str> = parts
        .iter()
        .map(|part| part.map_or("", js_text::trim))
        .filter(|part| !part.is_empty())
        .collect();
    (!said.is_empty()).then(|| said.join("; "))
}

/// A text that says something, or nothing (`value || undefined`).
fn said(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// What a new finding says.
struct Raised<'r> {
    severity: &'r str,
    location: &'r str,
    finding: &'r str,
    required_action: &'r str,
}

/// `reportNewFinding`: a finding raised in this review, under the next id.
fn report_new_finding(
    findings: &[Finding],
    artifact: &str,
    unit: Option<&str>,
    row: &Raised<'_>,
    note: Option<&str>,
    related: Option<&str>,
) -> Finding {
    Finding {
        artifact: artifact.to_owned(),
        unit: unit.filter(|unit| !unit.is_empty()).map(str::to_owned),
        id: next_finding_id(findings),
        severity: canonical_severity(row.severity),
        location: row.location.to_owned(),
        finding: row.finding.to_owned(),
        required_action: row.required_action.to_owned(),
        status: "New".to_owned(),
        introduced_in_review: Some(true),
        reviewer_note: note.and_then(said),
        related_finding_id: related.and_then(said),
        ..Finding::default()
    }
    .with_fingerprint()
}

/// `applyPriorReport`: what a review says became of the findings it was
/// shown, applied to the list. The second answer is whether the report was
/// malformed: a row repeated, or (when `malformed_unknown_ids`) an id the
/// list does not have.
#[must_use]
pub fn apply_prior_report(
    findings: &[Finding],
    artifact: &str,
    unit: Option<&str>,
    rows: &[PriorReport],
    malformed_unknown_ids: bool,
) -> (Vec<Finding>, bool) {
    let mut next: Vec<Finding> = findings
        .iter()
        .map(|finding| Finding {
            introduced_in_review: Some(false),
            reviewer_note: None,
            not_rechecked: None,
            resolved_in_review: Some(false),
            ..finding.clone()
        })
        .collect();
    let mut mentioned: Vec<&str> = Vec::new();
    // The new finding each unknown prior id became, so a repeat is only a
    // note.
    let mut unknown: Vec<(&str, String)> = Vec::new();
    let mut malformed = false;
    for row in rows {
        let again = |finding: &Finding| {
            let now_or_note = if row.note.is_empty() {
                row.now.name()
            } else {
                &row.note
            };
            reviewer_note(&[
                finding.reviewer_note.as_deref(),
                Some(&format!("Additional report for {}: {now_or_note}", row.id)),
            ])
        };
        if let Some((_, created)) = unknown.iter().find(|(id, _)| *id == row.id) {
            malformed = true;
            if let Some(target) = next.iter_mut().find(|finding| finding.id == *created) {
                target.reviewer_note = again(target);
            }
            continue;
        }
        let Some(index) = next.iter().position(|finding| finding.id == row.id) else {
            if malformed_unknown_ids {
                malformed = true;
            }
            let reported = format!(
                "Reviewer reported {} as {}.",
                row.id,
                match row.now {
                    Now::Fixed => "fixed",
                    Now::StillApplies => "still applying",
                }
            );
            let created = report_new_finding(
                &next,
                artifact,
                unit,
                &Raised {
                    severity: if row.severity.is_empty() {
                        "Major"
                    } else {
                        &row.severity
                    },
                    location: &format!("{artifact} > findings report"),
                    finding: if row.note.is_empty() {
                        &reported
                    } else {
                        &row.note
                    },
                    required_action: if row.note.is_empty() {
                        "Review this concern at the gate."
                    } else {
                        &row.note
                    },
                },
                Some(&format!("Reviewer supplied prior ID {}", row.id)),
                None,
            );
            unknown.push((&row.id, created.id.clone()));
            next.push(created);
            continue;
        };
        if mentioned.contains(&row.id.as_str()) {
            malformed = true;
            next[index].reviewer_note = again(&next[index]);
            continue;
        }
        mentioned.push(&row.id);
        // A fixed finding the reviewer says applies again is back: a
        // decision made before it was fixed stands, otherwise it is open
        // again. The same holds for a decided finding the person reopened
        // as not fixed.
        let back = &mut next[index];
        if row.now == Now::StillApplies
            && (back.status == "Resolved" || (back.is_open() && back.earlier_decision.is_some()))
        {
            if back.earlier_decision.is_some() {
                back.reopened_reason = None;
            }
            back.status = back
                .earlier_decision
                .take()
                .unwrap_or_else(|| "Unresolved".to_owned());
            back.resolved_by_reviewer = None;
        }
        let current = next[index].clone();
        if current.is_decided() {
            if row.now == Now::Fixed {
                next[index] = Finding {
                    status: "Resolved".to_owned(),
                    resolved_by_reviewer: Some(true),
                    resolved_in_review: Some(true),
                    earlier_decision: Some(current.status.clone()),
                    reviewer_note: said(&row.note),
                    not_rechecked: None,
                    ..current
                };
                continue;
            }
            let decided_at = current
                .decided_at_severity
                .as_deref()
                .unwrap_or(&current.severity);
            let worse = match (severity_rank(decided_at), severity_rank(&row.severity)) {
                (Some(decided), Some(reported)) if reported > decided => Some(reported),
                _ => None,
            };
            // An earlier finding that points at this one, at this severity
            // or above, still open or already decided, carries the same
            // information: the person is not asked again, and the comment
            // is only a note.
            let escalated = |reported: usize| {
                next.iter().any(|finding| {
                    finding.related_finding_id.as_deref() == Some(current.id.as_str())
                        && finding.status != "Resolved"
                        && severity_rank(
                            finding
                                .decided_at_severity
                                .as_deref()
                                .unwrap_or(&finding.severity),
                        )
                        .is_some_and(|rank| rank >= reported)
                })
            };
            match worse {
                Some(reported) if !escalated(reported) => {
                    let raised = report_new_finding(
                        &next,
                        artifact,
                        unit,
                        &Raised {
                            severity: &row.severity,
                            location: &current.location,
                            finding: if row.note.is_empty() {
                                &current.finding
                            } else {
                                &row.note
                            },
                            required_action: &current.required_action,
                        },
                        Some(&format!(
                            "Severity increased from {decided_at}; worse than {}",
                            current.id
                        )),
                        Some(&current.id),
                    );
                    next.push(raised);
                }
                _ => next[index].reviewer_note = said(&row.note),
            }
            continue;
        }
        if current.is_open() {
            next[index] = match row.now {
                Now::Fixed => Finding {
                    status: "Resolved".to_owned(),
                    resolved_by_reviewer: Some(true),
                    resolved_in_review: Some(true),
                    reviewer_note: said(&row.note),
                    reopened_reason: None,
                    ..current
                },
                Now::StillApplies => {
                    let severity = canonical_severity(&row.severity);
                    Finding {
                        status: "Unresolved".to_owned(),
                        severity: if severity.is_empty() {
                            current.severity.clone()
                        } else {
                            severity
                        },
                        reviewer_note: said(&row.note),
                        not_rechecked: None,
                        reopened_reason: None,
                        ..current
                    }
                    .with_fingerprint()
                }
            };
        }
    }
    for finding in &mut next {
        if !mentioned.contains(&finding.id.as_str())
            && finding.is_open()
            && finding.introduced_in_review != Some(true)
        {
            finding.not_rechecked = Some(true);
        }
    }
    (next, malformed)
}

/// What [`apply_review_body`] gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub findings: Vec<Finding>,
    /// The review's findings could be read and still broke a rule of the
    /// report: a row repeated, an id the list does not have.
    pub malformed: bool,
    /// The report left out its Prior findings table.
    pub prior_missing: bool,
}

/// `applyReviewBody`: one review applied to the list it was shown.
///
/// # Errors
///
/// The review's findings cannot be read: the reader's sentence.
pub fn apply_review_body(
    findings: &[Finding],
    artifact: &str,
    unit: Option<&str>,
    body: &str,
) -> Result<Applied, String> {
    if let Some(report) = parse_findings_report(body)? {
        let mut seen: Vec<&str> = Vec::new();
        let mut repeated = false;
        for row in &report.prior {
            repeated |= seen.contains(&row.id.as_str());
            seen.push(&row.id);
        }
        let (mut next, malformed) =
            apply_prior_report(findings, artifact, unit, &report.prior, true);
        let mut malformed = malformed || repeated;
        for row in &report.new_findings {
            // An id the reviewer gave a new finding is ignored: the engine
            // assigns the next unused number.
            if row
                .supplied_id
                .as_deref()
                .is_some_and(|id| seen.contains(&id))
            {
                malformed = true;
            }
            let raised = report_new_finding(
                &next,
                artifact,
                unit,
                &Raised {
                    severity: &row.severity,
                    location: &row.location,
                    finding: &row.finding,
                    required_action: &row.required_action,
                },
                None,
                None,
            );
            next.push(raised);
        }
        return Ok(Applied {
            findings: next,
            malformed,
            prior_missing: report.prior_missing,
        });
    }

    // The six-column table, read for the transition: a row with a known id
    // is an assessment of that finding, and its status is never read as a
    // decision.
    let parsed = parse_review_section(body, artifact, unit)?;
    // (An id the list has twice is its last finding here, as upstream's map
    // keeps it.)
    let known = |id: &str| findings.iter().rev().find(|finding| finding.id == id);
    let mut prior_rows: Vec<PriorReport> = Vec::new();
    let mut new_rows: Vec<Finding> = Vec::new();
    // The row each unknown id became, so a repeat is a note on the first.
    let mut unknown: Vec<(String, usize)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let mut malformed = false;
    for row in parsed.findings {
        if let Some(current) = known(&row.id) {
            // A row carried forward repeats the finding; only changed
            // wording is something the reviewer said this round.
            let action = format!("Required action: {}", row.required_action);
            prior_rows.push(PriorReport {
                id: row.id.clone(),
                now: if row.status == "Resolved" {
                    Now::Fixed
                } else {
                    Now::StillApplies
                },
                severity: row.severity.clone(),
                note: reviewer_note(&[
                    (row.finding != current.finding).then_some(row.finding.as_str()),
                    (!row.required_action.is_empty()
                        && row.required_action != current.required_action)
                        .then_some(action.as_str()),
                ])
                .unwrap_or_default(),
            });
        } else {
            let fixed = row.status == "Resolved";
            if is_unreadable_findings_table_finding(&row.id, &row.location) {
                new_rows.push(Finding {
                    status: if fixed { "Resolved" } else { "Unresolved" }.to_owned(),
                    resolved_by_reviewer: Some(fixed),
                    resolved_in_review: Some(fixed),
                    ..row
                });
                continue;
            }
            // An unknown id is a new finding under the engine's next id. A
            // row that claimed to be a prior finding keeps the id it gave
            // as a note, and a repeat of that id is a note on the first row.
            if let Some((_, first)) = unknown.iter().find(|(id, _)| *id == row.id) {
                let first = &mut new_rows[*first];
                first.reviewer_note = reviewer_note(&[
                    first.reviewer_note.as_deref(),
                    Some(&format!(
                        "Additional report for {}: {}",
                        row.id, row.finding
                    )),
                ]);
                malformed = true;
                continue;
            }
            unknown.push((row.id.clone(), new_rows.len()));
            seen.push(row.id.clone());
            new_rows.push(Finding {
                id: String::new(),
                reviewer_note: (row.status != "New")
                    .then(|| format!("Reviewer supplied prior ID {}", row.id)),
                status: "New".to_owned(),
                ..row
            });
            continue;
        }
        let id = &prior_rows[prior_rows.len() - 1].id;
        if seen.contains(id) {
            malformed = true;
        }
        seen.push(id.clone());
    }
    let (mut next, prior_malformed) =
        apply_prior_report(findings, artifact, unit, &prior_rows, false);
    for row in new_rows {
        if is_unreadable_findings_table_finding(&row.id, &row.location) {
            next.push(row);
            continue;
        }
        let raised = report_new_finding(
            &next,
            artifact,
            unit,
            &Raised {
                severity: &row.severity,
                location: &row.location,
                finding: &row.finding,
                required_action: &row.required_action,
            },
            row.reviewer_note.as_deref(),
            None,
        );
        next.push(raised);
    }
    Ok(Applied {
        findings: next,
        malformed: malformed || prior_malformed,
        prior_missing: false,
    })
}

/// `withoutRoundMarks`: what one review said about a finding (its note,
/// whether it was re-checked, whether it was raised or fixed there) belongs
/// to that review only.
#[must_use]
pub fn without_round_marks(finding: &Finding) -> Finding {
    Finding {
        reviewer_note: None,
        not_rechecked: None,
        resolved_in_review: Some(false),
        introduced_in_review: Some(false),
        ..finding.clone()
    }
}

/// `legacyRecordList`: a record written before the engine kept the list is
/// the whole list as its reviewer wrote it. Its ids are kept, so earlier
/// decisions carry by id. A decided finding stays as decided unless the
/// record marks it fixed, a decision the record dropped is kept, and a
/// status the reviewer wrote is never read as a person's decision.
#[must_use]
pub fn legacy_record_list(current: &[Finding], recorded: &[Finding]) -> Vec<Finding> {
    // (An id the list has twice is its last finding, as upstream's map
    // keeps it.)
    let earlier = |id: &str| current.iter().rev().find(|finding| finding.id == id);
    let mut next: Vec<Finding> = recorded
        .iter()
        .map(|row| {
            let prior = earlier(&row.id);
            let fixed = row.status == "Resolved";
            if let Some(prior) = prior.filter(|prior| prior.is_decided()) {
                let kept = without_round_marks(prior);
                return if fixed {
                    Finding {
                        status: "Resolved".to_owned(),
                        resolved_by_reviewer: Some(true),
                        resolved_in_review: Some(true),
                        earlier_decision: Some(prior.status.clone()),
                        ..kept
                    }
                } else {
                    kept
                };
            }
            let mut listed = Finding {
                status: if fixed {
                    "Resolved"
                } else if prior.is_none() {
                    "New"
                } else {
                    "Unresolved"
                }
                .to_owned(),
                introduced_in_review: Some(prior.is_none()),
                ..row.clone()
            };
            if fixed {
                listed.resolved_by_reviewer = Some(true);
                listed.resolved_in_review = Some(prior.is_some_and(Finding::is_open));
                if let Some(decision) = prior.and_then(|prior| prior.earlier_decision.clone()) {
                    listed.earlier_decision = Some(decision);
                }
            } else if let Some(reason) = prior.and_then(|prior| prior.reopened_reason.clone()) {
                listed.reopened_reason = Some(reason);
            }
            listed
        })
        .collect();
    for prior in current {
        if prior.is_decided() && !recorded.iter().any(|row| row.id == prior.id) {
            next.push(without_round_marks(prior));
        }
    }
    next
}

/// `ReviewFindingDisposition`: what a person decided about a finding at a
/// gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disposition {
    pub artifact: String,
    pub id: String,
    pub fingerprint: String,
    /// `Accepted risk`, `Rejected: <reason>` or `Reopened: <reason>`.
    pub status: String,
    pub decided_at_severity: Option<String>,
    /// The record of the review the decision was made on (its path and its
    /// digest), when the decision names one.
    pub reviewed_record: Option<(String, String)>,
}

const REOPENED_PREFIX: &str = "Reopened: ";

/// `applyDisposition`: a person's decision, applied to the finding it was
/// made on. A decision that names its review applies only to the finding as
/// it then read; one that names none (an older release, or a list seeded
/// from a legacy section) carries by id.
pub fn apply_disposition(
    findings: &mut [Finding],
    disposition: &Disposition,
    legacy_severity: Option<&str>,
) {
    let Some(finding) = findings
        .iter_mut()
        .find(|finding| finding.artifact == disposition.artifact && finding.id == disposition.id)
    else {
        return;
    };
    if disposition.fingerprint != finding.fingerprint && disposition.reviewed_record.is_some() {
        return;
    }
    if let Some(reason) = disposition.status.strip_prefix(REOPENED_PREFIX) {
        if finding.resolved_by_reviewer != Some(true) {
            return;
        }
        finding.status = "Unresolved".to_owned();
        finding.reopened_reason = Some(reason.to_owned());
        finding.resolved_by_reviewer = Some(false);
        finding.resolved_in_review = Some(false);
        finding.not_rechecked = None;
        return;
    }
    if !finding.is_open() {
        return;
    }
    if disposition.status != "Accepted risk" && !disposition.status.starts_with("Rejected: ") {
        return;
    }
    finding.status.clone_from(&disposition.status);
    finding.decided_at_severity = Some(
        disposition
            .decided_at_severity
            .as_deref()
            .or(legacy_severity)
            .unwrap_or(&finding.severity)
            .to_owned(),
    );
    finding.reviewer_note = None;
    finding.not_rechecked = None;
}

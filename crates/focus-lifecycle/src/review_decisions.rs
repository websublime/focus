//! What a gate records of the person's decision about a review's findings
//! (`acceptedRiskDispositionField` and `rejectedFindingDispositionField`,
//! `aidlc-review-brief.ts` L1 293 and L1 346).
//!
//! An approval accepts the risk of every finding still open. A request for
//! changes may say of a finding that it is not one (`--reject-finding`), or
//! that one the reviewer found fixed is not (`--reopen-finding`). Either
//! way the decisions ride on the gate's row in one field, a small JSON
//! document, which [`crate::review_derived`] reads back when it replays the
//! list.
//!
//! The findings are the ones of [`crate::review_derived::contexts`]: the
//! list each scope of the stage stands at now. A gate of several stages (a
//! team's Unit gate) arrives with the units (slice 5); here a gate is one
//! stage's.

use focus_evidence::review_dispositions as decisions;
use focus_evidence::review_findings::Finding;
use focus_graph::runtime::Stage;
use focus_js::order::{cmp_utf16, locale_compare};
use focus_js::{EngineError, Result, json, re, text as js_text};
use serde_json::{Value, json as value};

use crate::review_derived::{self, Derived};
use crate::stage;
use crate::workflow::Workflow;

/// A decision about one finding, as the gate's row keeps it.
struct Decision {
    artifact: String,
    id: String,
    fingerprint: String,
    status: String,
    decided_at_severity: String,
    reviewed_record: Option<(String, String)>,
}

impl Decision {
    fn of(finding: &Finding, context: &Derived, status: String) -> Self {
        Self {
            artifact: finding.artifact.clone(),
            id: finding.id.clone(),
            fingerprint: finding.fingerprint.clone(),
            status,
            decided_at_severity: finding.severity.clone(),
            reviewed_record: context.review_record.clone(),
        }
    }

    fn entry(&self) -> Value {
        let mut entry = value!({
            "artifact": self.artifact,
            "id": self.id,
            "fingerprint": self.fingerprint,
            "status": self.status,
            "decided_at_severity": self.decided_at_severity,
        });
        if let Some((path, digest)) = &self.reviewed_record {
            entry["reviewed_record"] = value!({ "path": path, "digest": digest });
        }
        entry
    }
}

/// `serializeReviewFindingDispositions`: the field's value, the decisions
/// in the order of the findings they are about. Nothing for none.
fn serialize(mut decisions: Vec<Decision>) -> Option<String> {
    if decisions.is_empty() {
        return None;
    }
    decisions.sort_by(|a, b| {
        locale_compare(
            &decisions::key(&a.artifact, &a.id),
            &decisions::key(&b.artifact, &b.id),
        )
    });
    let entries: Vec<Value> = decisions.iter().map(Decision::entry).collect();
    Some(json::stringify(
        &value!({ "version": 1, "dispositions": entries }),
    ))
}

/// `acceptedRiskDispositionField`: what an approval decides. Every finding
/// of the stage still open (new, or unresolved) is accepted as a risk, at
/// the severity it has now.
///
/// # Errors
///
/// What [`review_derived::contexts`] does not read.
pub fn accepted_risk_field(wf: &Workflow, stage: &Stage) -> Result<Option<String>> {
    if stage::reviewer(stage).is_none() {
        return Ok(None);
    }
    let mut accepted = Vec::new();
    for context in review_derived::contexts(wf, stage, None)? {
        for finding in context.findings.iter().filter(|finding| finding.is_open()) {
            accepted.push(Decision::of(finding, &context, "Accepted risk".to_owned()));
        }
    }
    Ok(serialize(accepted))
}

/// Which of the two flags a decision came by.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Reject,
    Reopen,
}

impl Kind {
    const fn flag(self) -> &'static str {
        match self {
            Self::Reject => "--reject-finding",
            Self::Reopen => "--reopen-finding",
        }
    }

    const fn verb(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Reopen => "reopen",
        }
    }
}

/// What a flag's value names: a finding, and why.
struct Spec {
    artifact: String,
    id: String,
    reason: String,
}

/// `toPosix`: separators turned forward on Windows; untouched elsewhere.
fn to_posix(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    }
}

/// `parseFindingDispositionSpec`: `<review-artifact>#R-NN=<human reason>`.
fn parse_spec(raw: &str, kind: Kind) -> Result<Spec> {
    let trimmed = js_text::trim(raw);
    let Some(found) = re::find(r"^(?:(.*)#)?(R-[0-9]+)=(\S[\s\S]*)$", "", trimmed) else {
        return Err(EngineError::new(format!(
            "Invalid {} {}. Expected <review-artifact>#R-NN=<human reason>; the artifact may be its file name, or left out when one current finding carries the id.",
            kind.flag(),
            json::stringify(&value!(raw))
        )));
    };
    let group = |index: usize| re::group(trimmed, &found, index).unwrap_or("");
    Ok(Spec {
        artifact: to_posix(js_text::trim(group(1))),
        id: group(2).to_owned(),
        reason: js_text::trim(group(3)).to_owned(),
    })
}

/// `selectorNamesArtifact`: a selector names the finding the way it was
/// said: the full path, a path tail, the file name with or without `.md`,
/// or no artifact at all.
fn selector_names_artifact(selector: &str, artifact: &str) -> bool {
    if selector.is_empty() || selector == artifact {
        return true;
    }
    let file = if selector.ends_with(".md") {
        selector.to_owned()
    } else {
        format!("{selector}.md")
    };
    artifact == file || artifact.ends_with(&format!("/{file}"))
}

fn named(finding: &Finding) -> String {
    format!("{}#{}", finding.artifact, finding.id)
}

/// The names of `findings`, in the order upstream's `sort()` gives.
fn names<'f>(findings: impl Iterator<Item = &'f Finding>) -> String {
    let mut names: Vec<String> = findings.map(named).collect();
    names.sort_by(|a, b| cmp_utf16(a, b));
    names.join(", ")
}

/// `rejectedFindingDispositionField`: what a request for changes decides
/// about the findings it names. A selector is resolved to its finding
/// before anything is compared or recorded, so the file name and the full
/// path name the same finding once; one current finding matching is the
/// one meant, two are named back.
///
/// # Errors
///
/// A stage nobody reviews; a value that is not of the form; a selector
/// that names no current finding, or more than one; a finding named
/// twice; a finding the decision does not fit (rejecting one the reviewer
/// found fixed or one already decided, reopening one that was not found
/// fixed); and what [`review_derived::contexts`] does not read.
pub fn rejected_finding_field(
    wf: &Workflow,
    stage: &Stage,
    rejects: &[String],
    reopens: &[String],
) -> Result<Option<String>> {
    if rejects.is_empty() && reopens.is_empty() {
        return Ok(None);
    }
    if stage::reviewer(stage).is_none() {
        return Err(EngineError::new(format!(
            "Cannot reject review findings for stage \"{}\": the stage has no reviewer.",
            stage.slug
        )));
    }
    let contexts = review_derived::contexts(wf, stage, None)?;
    let findings: Vec<(&Derived, &Finding)> = contexts
        .iter()
        .flat_map(|context| {
            context
                .findings
                .iter()
                .map(move |finding| (context, finding))
        })
        .collect();
    let select = |raw: &String, kind: Kind| -> Result<(Spec, &Derived, &Finding)> {
        let spec = parse_spec(raw, kind)?;
        let asked = if spec.artifact.is_empty() {
            spec.id.clone()
        } else {
            format!("{}#{}", spec.artifact, spec.id)
        };
        let exact = findings
            .iter()
            .find(|(_, finding)| finding.artifact == spec.artifact && finding.id == spec.id);
        let matches: Vec<&(&Derived, &Finding)> = match exact {
            Some(exact) => vec![exact],
            None => findings
                .iter()
                .filter(|(_, finding)| {
                    finding.id == spec.id
                        && selector_names_artifact(&spec.artifact, &finding.artifact)
                })
                .collect(),
        };
        if matches.len() > 1 {
            return Err(EngineError::new(format!(
                "Cannot {} {asked}: it names {} current review findings at this gate: {}. Pass the one that is meant.",
                kind.verb(),
                matches.len(),
                names(matches.iter().map(|(_, finding)| *finding))
            )));
        }
        let Some((context, finding)) = matches.first() else {
            // The accepted selectors are named: a stem that does not match
            // a full path is otherwise invisible.
            let available = names(
                findings
                    .iter()
                    .filter(|(_, finding)| match kind {
                        Kind::Reject => finding.is_open(),
                        Kind::Reopen => finding.resolved_by_reviewer == Some(true),
                    })
                    .map(|(_, finding)| *finding),
            );
            let rest = if !available.is_empty() {
                format!(
                    "Current {} findings: {available}.",
                    match kind {
                        Kind::Reject => "rejectable",
                        Kind::Reopen => "reopenable",
                    }
                )
            } else if findings.is_empty() {
                "This gate has no current review findings.".to_owned()
            } else {
                match kind {
                    Kind::Reject => "This gate has no New or Unresolved review findings to reject.",
                    Kind::Reopen => "This gate has no Resolved (reviewer) findings to reopen.",
                }
                .to_owned()
            };
            return Err(EngineError::new(format!(
                "Cannot {} {asked}: it is not a current review finding for this gate. {rest}",
                kind.verb()
            )));
        };
        Ok((spec, *context, *finding))
    };
    let mut selected = Vec::new();
    for raw in rejects {
        selected.push((select(raw, Kind::Reject)?, Kind::Reject));
    }
    for raw in reopens {
        selected.push((select(raw, Kind::Reopen)?, Kind::Reopen));
    }
    let twice = |finding: &Finding| {
        EngineError::new(format!(
            "Finding {} cannot appear more than once across --reject-finding and --reopen-finding. Keep only the intended decision.",
            named(finding)
        ))
    };
    let key = |finding: &Finding| decisions::key(&finding.artifact, &finding.id);
    // A finding both rejected and reopened is refused before either is
    // looked at.
    for ((_, _, finding), kind) in &selected {
        if *kind == Kind::Reopen
            && selected
                .iter()
                .any(|((_, _, other), kind)| *kind == Kind::Reject && key(other) == key(finding))
        {
            return Err(twice(finding));
        }
    }
    let mut recorded: Vec<Decision> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for ((spec, context, finding), kind) in &selected {
        if seen.contains(&key(finding)) {
            return Err(twice(finding));
        }
        seen.push(key(finding));
        let name = named(finding);
        let fixed = finding.resolved_by_reviewer == Some(true);
        match kind {
            Kind::Reject if fixed => {
                return Err(EngineError::new(format!(
                    "Cannot reject {name}: the reviewer marked it fixed. If it is not fixed, pass --reopen-finding \"{name}=<reason>\" instead."
                )));
            }
            Kind::Reject if !finding.is_open() => {
                return Err(EngineError::new(format!(
                    "Cannot reject {name}: current status is {}.",
                    finding.status
                )));
            }
            Kind::Reopen if !fixed => {
                return Err(EngineError::new(format!(
                    "Cannot reopen {name}: only a Resolved (reviewer) finding can be reopened. Choose a resolved reviewer finding or leave ordinary revision feedback."
                )));
            }
            _ => {}
        }
        recorded.push(Decision::of(
            finding,
            context,
            match kind {
                Kind::Reject => format!("Rejected: {}", spec.reason),
                Kind::Reopen => format!("Reopened: {}", spec.reason),
            },
        ));
    }
    Ok(serialize(recorded))
}

//! Whether a stage's outputs descend from a summary confirmation a person
//! gave and that is still the current one (`checkSummaryConfirmationEvidence`,
//! `aidlc-lib.ts` L13 445).
//!
//! A stage that asks questions ends them with a summary, and the person's
//! "Looks correct" is recorded as a receipt row that binds the questions
//! file by a hash of what was confirmed. A gate then asks three things of
//! each questions file: that the file still says "Looks correct"; that the
//! newest receipt of the current attempt is that choice, over the answers
//! as they are now; and that each output was saved under it. A receipt
//! that minted an authorization id is held to by descent (the output's
//! newest write carries the id); one from before the ids, by order (a
//! write after the receipt).
//!
//! The answer is one of three: nothing is owed; everything is in order,
//! with the changes the Guard Policy let through; or a refusal, with its
//! code, its sentence and whether the confirmation is missing or stale.
//!
//! # The Guard Policy is read only when something changed
//!
//! Under `relaxed` or `off`, answers edited after the confirmation and an
//! output saved before it are recorded and said once instead of refused.
//! The policy is read the first time one of the two is met, and the answer
//! says whether it was. A policy that cannot be read is `strict`. This
//! check writes nothing: the accepted changes go back to the caller.
//!
//! # What is not here yet
//!
//! - A stage that runs for each unit: which units owe a summary is read
//!   off their lifecycle rows (`unitSkippedUnits`,
//!   `unitFinishedBeforeSummaryOn`), which arrive with units (slice 5).
//!   Such a stage is refused by name when it owes a confirmation.
//! - A row that names a unit while units are owned by a team: whether it
//!   belongs to the current claim (`eventMatchesClaimAttempt`) is deferred
//!   with team ownership, and refused by name.
//! - The recovery question a refusal carries (`evaluateGuardRefusal`) is
//!   slice 4's: until then a refusal is its code, sentence and coverage.
//!
//! # The questions file is not opened by name
//!
//! As where the receipt is recorded (`focus-log`): a questions file is read
//! as every file of the record that decides something, a regular file with
//! one name reached through no link. One that is not reads as a file with
//! no answer.
//!
//! # Tests that change no answer
//!
//! Kept where upstream has them:
//!
//! - A row is after another in its shard when its place is greater; "or
//!   the same" would say nothing else, since a boundary is never a receipt
//!   and a receipt is never the write it is compared with.
//! - The unit asked about, where a plain confirmation is looked for: a
//!   stage that keeps its files at the stage has no questions for a unit,
//!   and is answered before that.
//! - A write that names its file from `<project-dir>`: the name also ends
//!   with the file's path from the project, which is the second way a write
//!   is recognised.
//! - An earlier confirmation of the same answers must be before the
//!   receipt: every other receipt of the attempt is, or the two are at one
//!   instant in two shards and the order is refused first.
//!
//! # How a command is spelled
//!
//! Upstream names its log and orchestrator tools by their script
//! (`aidlc-log.ts decision …`); Focus says the command a person runs
//! (`focus engine log decision …`).

use std::path::{Path, PathBuf};

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::authorization::{self, attempt_floors, frontier};
use focus_evidence::summary::{self as confirmed, CHECKPOINT, HASH_SCOPE};
use focus_graph::runtime::Stage;
use focus_graph::vocabulary::artifact_filename;
use focus_guards::guard_operation::{
    EngineInvocation, RenderMode, RenderShell, render_engine_invocation,
};
use focus_js::order::cmp_utf16;
use focus_js::path::{self as jspath, Flavour};
use focus_js::text::{to_lower_case, trim};
use focus_js::{EngineError, Result, bail, re};
use focus_workspace::{files, links};
use sha2::{Digest, Sha256};

use crate::admission::summary_confirmation_owed_of;
use crate::gates::guard_policy_accepts_changes_of;
use crate::review_attempt::is_team_unit_ownership;
use crate::workflow::Workflow;
use crate::{reply, stage};

/// `LEGACY_SUMMARY_CONFIRMATION_HASH_SCOPES`: scopes a receipt may name
/// that are checked as the current one is.
const LEGACY_HASH_SCOPES: [&str; 1] = ["confirmed-content-v1"];

/// `SUMMARY_EVIDENCE_EVENTS`: the rows the evidence reads.
const EVIDENCE_EVENTS: [&str; 7] = [
    "WORKFLOW_STARTED",
    "STAGE_STARTED",
    "STAGE_JUMPED",
    "STAGE_COMPLETED",
    "SUMMARY_CONFIRMATION_RECORDED",
    "ARTIFACT_CREATED",
    "ARTIFACT_UPDATED",
];

/// The two choices a summary offers.
pub const CHOICES: [&str; 2] = ["Looks correct", "Request changes"];

/// `isSummaryConfirmationChoice`: "Looks correct" or "Request changes",
/// however it was capitalised or decorated.
#[must_use]
pub fn is_choice(text: &str) -> bool {
    let stripped = reply::strip_recommended_decorator(text);
    let normalized = to_lower_case(&re::replace("[.!]+$", "", trim(&stripped), ""));
    normalized == "looks correct" || normalized == "request changes"
}

/// `isSummaryConfirmationOptions`: a question whose offered options are the
/// summary's two choices.
#[must_use]
pub fn is_options(options: Option<&str>) -> bool {
    let Some(options) = options else {
        return false;
    };
    let offered: Vec<&str> = options
        .split(',')
        .map(trim)
        .filter(|option| !option.is_empty())
        .collect();
    offered.len() == 2 && offered.iter().all(|option| is_choice(option))
}

/// The two commands that record a summary confirmation.
pub struct Commands {
    pub decision: String,
    pub answer: String,
}

/// `summaryConfirmationCommands`: the two commands, flags filled in, every
/// argument quoted for this platform's shell.
///
/// # Errors
///
/// An argument cannot be written for the shell.
pub fn commands(
    wf: &Workflow,
    stage: &str,
    unit: Option<&str>,
    questions_file: Option<&str>,
    single: bool,
    details: &str,
) -> Result<Commands> {
    let shell = if cfg!(windows) {
        RenderShell::PowerShell
    } else {
        RenderShell::Posix
    };
    commands_for(wf, stage, unit, questions_file, single, details, shell)
}

/// [`commands`], quoted for the shell named.
///
/// # Errors
///
/// An argument cannot be written for the shell.
pub fn commands_for(
    wf: &Workflow,
    stage: &str,
    unit: Option<&str>,
    questions_file: Option<&str>,
    single: bool,
    details: &str,
    shell: RenderShell,
) -> Result<Commands> {
    let mut head: Vec<String> = ["--checkpoint", "summary-confirmation", "--stage", stage]
        .map(str::to_owned)
        .to_vec();
    if let Some(unit) = unit.filter(|unit| !unit.is_empty()) {
        head.extend(["--unit".to_owned(), unit.to_owned()]);
    }
    if single {
        head.push("--single".to_owned());
    }
    head.push("--questions-file".to_owned());
    head.push(
        questions_file.map_or_else(|| format!("<path to {stage}-questions.md>"), str::to_owned),
    );
    let render = |verb: &str, tail: &[&str]| -> Result<String> {
        let mut args = vec![verb.to_owned()];
        args.extend(head.iter().cloned());
        args.extend(tail.iter().map(|arg| (*arg).to_owned()));
        let invocation = EngineInvocation {
            route: "log".to_owned(),
            args,
        };
        render_engine_invocation(
            &invocation,
            RenderMode::Native,
            wf.layout.harness.dir(),
            shell,
        )
        .map_err(EngineError::new)
    };
    Ok(Commands {
        decision: render(
            "decision",
            &[
                "--decision",
                "Does this all look correct?",
                "--options",
                "Looks correct,Request changes",
            ],
        )?,
        answer: render("answer", &["--details", details])?,
    })
}

/// `AcceptedChange`: a change the Guard Policy let through, for the caller
/// to record and say once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedChange {
    pub stage: String,
    pub unit: Option<String>,
    /// The paths that changed, from the project.
    pub changed: Vec<String>,
    /// What the confirmation recorded, and what is there now.
    pub recorded: String,
    pub current: String,
    /// The one line the person hears.
    pub notice: String,
}

/// Whether a confirmation is not there or no longer the current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    Missing,
    Stale,
}

impl Coverage {
    /// As upstream writes it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Stale => "stale",
        }
    }
}

/// `SummaryConfirmationEvidence`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Evidence {
    /// Nothing stands in the way. `required` is `false` when the stage
    /// owes no confirmation.
    Ok {
        required: bool,
        accepted_changes: Vec<AcceptedChange>,
        /// The Guard Policy was read: something had changed.
        change_control_read: bool,
    },
    Refused {
        code: &'static str,
        message: String,
        coverage: Coverage,
        change_control_read: bool,
    },
}

/// What the evidence is asked of.
#[derive(Debug, Clone, Copy, Default)]
pub struct Asked<'a> {
    /// `single-stage:<slug>` for a stage run by itself.
    pub workflow: Option<&'a str>,
    /// The state the ceremony and the policy are read from.
    pub state: Option<&'a str>,
    /// The scope, when the caller names it itself (a stage run by itself
    /// reads the scope its own attempt recorded).
    pub scope: Option<&'a str>,
    pub unit: Option<&'a str>,
}

/// `SummaryQuestionFile`, of a stage that keeps its files at the stage.
struct Question {
    path: PathBuf,
    dir: PathBuf,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn shown(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `toPosix(relative(projectDir, path))`.
fn from_project(wf: &Workflow, path: &str) -> String {
    let project = shown(&wf.layout.project);
    jspath::relative(Flavour::host(), &project, &project, path).replace('\\', "/")
}

/// The file a row names, as a path from the project with forward slashes
/// (`toPosix(relative(projectDir, resolveAuditProjectPath(projectDir, file)))`).
pub(crate) fn row_path_from_project(wf: &Workflow, recorded: &str) -> String {
    from_project(wf, &row_path(&shown(&wf.layout.project), recorded))
}

/// `questionFilesInDir`: the `-questions.md` files of a folder, by name.
fn question_files_in(dir: &Path) -> Vec<Question> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with("-questions.md"))
        .collect();
    names.sort_by(|a, b| cmp_utf16(a, b));
    names
        .into_iter()
        .map(|name| Question {
            path: dir.join(name),
            dir: dir.to_path_buf(),
        })
        .collect()
}

/// A questions file's bytes, read as a file of the record that decides
/// something; `None` when it is not such a file.
fn read_question(wf: &Workflow, path: &Path) -> Option<Vec<u8>> {
    let record = wf.record_dir()?;
    let anchor = std::fs::canonicalize(&record).ok()?;
    let rel = path.strip_prefix(&record).ok()?;
    links::refuse_link_in_chain(&anchor, rel).ok()?;
    files::read_regular(path, "summary questions file").ok()
}

/// `eventMatchesClaimAttempt`, for a row that names a unit: every row
/// matches while units are not owned by a team.
///
/// # Errors
///
/// Units are owned by a team, which is not read yet.
fn refuse_team_claims(wf: &Workflow, row: &ShardEvent) -> Result<()> {
    if block_field(&row.block, "Unit").is_none_or(str::is_empty) {
        return Ok(());
    }
    match wf.read_state() {
        Ok(state) if is_team_unit_ownership(&state) => bail!(
            "A summary confirmation row names a unit while units are owned by a team, and Focus does not read team claims yet: whether the row belongs to the current claim cannot be told."
        ),
        _ => Ok(()),
    }
}

/// Whether a row's `Workflow` is the one asked about: the stage run by
/// itself, or the main workflow.
fn of_workflow(row: &ShardEvent, workflow: Option<&str>) -> bool {
    let named = block_field(&row.block, "Workflow");
    match workflow {
        Some(workflow) => named == Some(workflow),
        None => !named.is_some_and(|named| named.starts_with("single-stage:")),
    }
}

/// `summaryFlowStartedInAttempt`: a summary was asked or confirmed for the
/// stage in its current attempt.
fn flow_started(
    wf: &Workflow,
    stage: &Stage,
    asked: Asked<'_>,
    rows: &[ShardEvent],
) -> Result<bool> {
    let floors = attempt_floors(rows, &stage.slug, asked.workflow, &wf.install.graph);
    for row in rows {
        if row.event != "DECISION_RECORDED" && row.event != "SUMMARY_CONFIRMATION_RECORDED" {
            continue;
        }
        if block_field(&row.block, "Stage") != Some(stage.slug.as_str())
            || block_field(&row.block, "Checkpoint") != Some(CHECKPOINT)
            || !of_workflow(row, asked.workflow)
        {
            continue;
        }
        if asked.unit.is_some() && block_field(&row.block, "Unit") != asked.unit {
            continue;
        }
        refuse_team_claims(wf, row)?;
        // A tie across shards cannot prove the row is from before the
        // boundary.
        if floors.iter().all(|floor| {
            if row.shard == floor.shard {
                row.pos > floor.pos
            } else {
                cmp_utf16(&row.timestamp, &floor.timestamp).is_ge()
            }
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `plainSummaryHint`: what to say when the stage's summary was recorded
/// without the checkpoint's flags, which a gate never counts.
fn plain_hint(rows: &[ShardEvent], stage: &str, unit: Option<&str>) -> &'static str {
    let recorded = rows.iter().any(|row| {
        if row.event != "DECISION_RECORDED" && row.event != "QUESTION_ANSWERED" {
            return false;
        }
        if block_field(&row.block, "Stage") != Some(stage)
            || block_field(&row.block, "Checkpoint").is_some()
        {
            return false;
        }
        if unit.is_some() && block_field(&row.block, "Unit") != unit {
            return false;
        }
        if row.event == "DECISION_RECORDED" {
            is_options(block_field(&row.block, "Options"))
        } else {
            block_field(&row.block, "Details").is_some_and(is_choice)
        }
    });
    if recorded {
        " A confirmation was recorded for this stage without `--checkpoint summary-confirmation --questions-file <path>`; that plain form is an ordinary question and never counts. Record it again with the flags."
    } else {
        ""
    }
}

/// `resolveAuditProjectPath`: the file a row names, as a path from the
/// root.
fn row_path(project: &str, recorded: &str) -> String {
    let host = Flavour::host();
    const TOKEN: &str = "<project-dir>";
    if recorded == TOKEN {
        return jspath::resolve(host, project, &[project]);
    }
    if let Some(rest) = recorded
        .strip_prefix(TOKEN)
        .and_then(|rest| rest.strip_prefix(['/', '\\']))
    {
        return jspath::resolve(host, project, &[project, rest]);
    }
    if jspath::is_absolute(host, recorded) {
        recorded.to_owned()
    } else {
        jspath::resolve(host, project, &[project, recorded])
    }
}

/// Whether `later` is provably after `earlier`: by place within a shard, by
/// instant across shards, and not at all at one instant.
fn strictly_after(later: &ShardEvent, earlier: &ShardEvent) -> bool {
    if later.shard == earlier.shard {
        later.pos > earlier.pos
    } else {
        cmp_utf16(&later.timestamp, &earlier.timestamp).is_gt()
    }
}

/// `checkSummaryConfirmationEvidence`.
///
/// # Errors
///
/// The stage runs for each unit, or a row names a unit while units are
/// owned by a team: neither is read yet (see the module docs). Or one of
/// the two commands a refusal names cannot be written for the shell.
#[allow(clippy::too_many_lines)]
pub fn check(wf: &Workflow, stage: &Stage, asked: Asked<'_>) -> Result<Evidence> {
    let slug = stage.slug.as_str();
    let not_owed = Evidence::Ok {
        required: false,
        accepted_changes: Vec::new(),
        change_control_read: false,
    };
    if !summary_confirmation_owed_of(wf, stage, asked.state, asked.scope) {
        return Ok(not_owed);
    }
    if stage::is_per_unit(stage) {
        bail!(
            "\"{slug}\" runs for each unit and owes a summary confirmation: which units owe one is read off their rows, and Focus does not read those yet (units arrive with a later slice)."
        );
    }
    // The policy is read once, and only when something changed.
    let mut policy: Option<bool> = None;
    let accepts_changes = |policy: &mut Option<bool>| -> bool {
        *policy.get_or_insert_with(|| guard_policy_accepts_changes_of(wf, asked.state))
    };
    let mut accepted_changes: Vec<AcceptedChange> = Vec::new();
    let refused = |code: &'static str, message: String, coverage: Coverage, read: bool| {
        Ok(Evidence::Refused {
            code,
            message,
            coverage,
            change_control_read: read,
        })
    };

    let mut questions = match wf.record_dir() {
        Some(record) => question_files_in(&record.join(&stage.phase).join(slug)),
        None => Vec::new(),
    };
    // A file at the stage is no unit's.
    if asked.unit.is_some() {
        questions.clear();
    }
    let declared = stage::summary_confirmation(stage) == Some("required");
    let rows = wf.events();
    if questions.is_empty() {
        if !declared && !flow_started(wf, stage, asked, &rows)? {
            return Ok(not_owed);
        }
        let unit_text = asked
            .unit
            .filter(|unit| !unit.is_empty())
            .map(|unit| format!(" for unit \"{unit}\""))
            .unwrap_or_default();
        return refused(
            "SUMMARY_QUESTIONS_MISSING",
            format!(
                "Refusing to complete \"{slug}\"{unit_text}: its question flow has no {slug}-questions.md file. Create and answer the stage questions, then record the consolidated summary checkpoint before generating artifacts."
            ),
            Coverage::Missing,
            false,
        );
    }

    let events: Vec<ShardEvent> = rows
        .iter()
        .filter(|row| EVIDENCE_EVENTS.contains(&row.event.as_str()))
        .cloned()
        .collect();
    if events.is_empty() {
        return refused(
            "SUMMARY_RECEIPT_MISSING",
            format!(
                "Refusing to complete \"{slug}\": no human-backed consolidated summary confirmation receipt is recorded.{}",
                plain_hint(&rows, slug, asked.unit)
            ),
            Coverage::Missing,
            false,
        );
    }
    let ordering_failure = |evidence: &str, timestamp: &str, reconfirm: bool, read: bool| {
        let remedy = if reconfirm {
            format!(
                "Repeat the {} confirmation after that second, then regenerate or re-save each artifact so the audit records a strictly later write.",
                if asked.workflow.is_none() {
                    "summary"
                } else {
                    "isolated summary"
                }
            )
        } else {
            "Regenerate or re-save the artifact after that second so the audit records a strictly later write.".to_owned()
        };
        refused(
            "SUMMARY_ORDER_AMBIGUOUS",
            format!(
                "Refusing to complete \"{slug}\": {evidence} share audit Timestamp \"{timestamp}\" across different shards, so their causal order cannot be proven. {remedy}"
            ),
            Coverage::Stale,
            read,
        )
    };
    let floors = attempt_floors(&events, slug, asked.workflow, &wf.install.graph);
    // After every boundary, before one of them, or not provably either.
    let after_floor = |row: &ShardEvent| -> Option<bool> {
        let mut unordered = false;
        for floor in &floors {
            let after = if row.shard == floor.shard {
                row.pos > floor.pos
            } else if row.timestamp == floor.timestamp {
                unordered = true;
                continue;
            } else {
                cmp_utf16(&row.timestamp, &floor.timestamp).is_gt()
            };
            if !after {
                return Some(false);
            }
        }
        if unordered { None } else { Some(true) }
    };
    let project = shown(&wf.layout.project);

    for question in &questions {
        let path = shown(&question.path);
        let bytes = read_question(wf, &question.path);
        let text = bytes
            .as_deref()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
        let file_answer = text
            .as_deref()
            .and_then(|text| confirmed::answer(text).ok().flatten());
        let (Some(bytes), Some(text), Some("Looks correct")) =
            (bytes.as_deref(), text.as_deref(), file_answer.as_deref())
        else {
            return refused(
                "SUMMARY_ANSWER_INVALID",
                format!(
                    "Refusing to complete \"{slug}\": {path} must contain exactly one `[Answer]: Looks correct` in its Consolidated Summary Confirmation section."
                ),
                Coverage::Stale,
                policy.is_some(),
            );
        };

        let question_relative = from_project(wf, &path);
        let mut candidates: Vec<&ShardEvent> = Vec::new();
        for row in &events {
            if row.event != "SUMMARY_CONFIRMATION_RECORDED"
                || block_field(&row.block, "Stage") != Some(slug)
                || block_field(&row.block, "Checkpoint") != Some(CHECKPOINT)
                || !of_workflow(row, asked.workflow)
            {
                continue;
            }
            // A file at the stage is confirmed by a row that names no unit.
            if block_field(&row.block, "Unit").is_some() {
                continue;
            }
            if block_field(&row.block, "Questions File") == Some(question_relative.as_str()) {
                candidates.push(row);
            }
        }
        let ordered: Vec<&ShardEvent> = candidates
            .iter()
            .copied()
            .filter(|row| after_floor(row) == Some(true))
            .collect();
        let newest = frontier(&ordered);
        if newest.len() > 1 {
            return ordering_failure(
                "matching summary receipts",
                &newest[0].timestamp,
                true,
                policy.is_some(),
            );
        }
        let receipt = newest.first().copied();
        let unordered: Vec<&ShardEvent> = candidates
            .iter()
            .copied()
            .filter(|row| after_floor(row).is_none())
            .collect();
        if unordered.iter().any(|row| {
            receipt.is_none_or(|receipt| cmp_utf16(&row.timestamp, &receipt.timestamp).is_ge())
        }) {
            let timestamp = floors
                .first()
                .map_or(unordered[0].timestamp.as_str(), |floor| {
                    floor.timestamp.as_str()
                });
            return ordering_failure(
                "the current-attempt boundary and matching summary receipt",
                timestamp,
                true,
                policy.is_some(),
            );
        }
        let Some(receipt) =
            receipt.filter(|row| block_field(&row.block, "Details") == Some("Looks correct"))
        else {
            let named = commands(
                wf,
                slug,
                None,
                Some(&question_relative),
                asked.workflow.is_some(),
                "Looks correct",
            )?;
            return refused(
                "SUMMARY_RECEIPT_MISSING",
                format!(
                    "Refusing to complete \"{slug}\": no fresh human-backed consolidated summary confirmation is recorded.{} Present the summary and record it with `{}`, end the turn, then after the human responds run `{}`.",
                    plain_hint(&rows, slug, None),
                    named.decision,
                    named.answer
                ),
                Coverage::Missing,
                policy.is_some(),
            );
        };

        let hash_scope = block_field(&receipt.block, "Hash Scope");
        let recovery = if asked.workflow.is_none() {
            format!(
                "First repair the questions file: reset the existing consolidated-summary `[Answer]:` tag to blank and remove or repair every invalid or duplicate post-summary section named by the validation error. Only then re-present the consolidated summary and record a fresh confirmation with `focus engine log decision --checkpoint summary-confirmation --stage \"{slug}\" --questions-file \"<path>\" --decision \"Does this all look correct?\"`; end the turn, wait for the human's response, update the recorded answer, then run `focus engine log answer --checkpoint summary-confirmation --stage \"{slug}\" --questions-file \"<path>\" --details \"Looks correct\"`. Re-save each generated artifact, rerun the section-12a reviewer when this stage declares one, then retry the stage completion command. If a completion gate is already open or a terminal section-12a receipt freezes artifact writes, instead present Request Changes and end the turn. After a fresh human turn choosing it, run `focus engine orchestrate report --stage \"{slug}\" --result rejected --user-input \"Request Changes\" --reason '<requested changes>'`; then revise and re-confirm the summary, re-save the artifacts, rerun the reviewer, and report `--result revised`."
            )
        } else {
            format!(
                "First repair the questions file: reset the existing consolidated-summary `[Answer]:` tag to blank and remove or repair every invalid or duplicate post-summary section named by the validation error. Only then re-present the consolidated summary in the isolated run and record a fresh confirmation with `focus engine log decision --checkpoint summary-confirmation --stage \"{slug}\" --questions-file \"<path>\" --single --decision \"Does this all look correct?\"`; end the turn, wait for the human's response, update the recorded answer, then run `focus engine log answer --checkpoint summary-confirmation --stage \"{slug}\" --questions-file \"<path>\" --single --details \"Looks correct\"`. Regenerate or re-save each generated artifact, rerun the section-12a reviewer when this stage declares one, then run `focus engine orchestrate report --single --stage \"{slug}\" --result completed`."
            )
        };
        let legacy_scope = hash_scope.is_some_and(|scope| LEGACY_HASH_SCOPES.contains(&scope));
        if let Some(scope) = hash_scope
            && scope != HASH_SCOPE
            && !legacy_scope
        {
            return refused(
                "SUMMARY_HASH_SCOPE_INVALID",
                format!(
                    "Refusing to complete \"{slug}\": unsupported summary-confirmation Hash Scope \"{scope}\". Supported: \"{HASH_SCOPE}\"; legacy scopes checked under current semantics: {}. {recovery}",
                    LEGACY_HASH_SCOPES
                        .iter()
                        .map(|scope| format!("\"{scope}\""))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Coverage::Stale,
                policy.is_some(),
            );
        }
        let recovery_message = if hash_scope.is_none() {
            format!(
                "This is a legacy unscoped receipt, so it still verifies the whole questions file; an allowed post-confirmation append therefore requires reconfirmation to create a new scoped receipt. {recovery}"
            )
        } else {
            recovery
        };
        // A receipt verifies the questions file under the scope it
        // recorded: the whole file for one with no scope, the confirmed
        // content for a scoped one.
        let hash_for = |scope: Option<&str>| -> std::result::Result<String, String> {
            match scope {
                None => Ok(sha256_hex(bytes)),
                Some(_) => confirmed::content_hash(text),
            }
        };
        let current_hash = match hash_for(hash_scope) {
            Ok(hash) => hash,
            Err(error) => {
                return refused(
                    "SUMMARY_FILE_UNREADABLE",
                    format!(
                        "Refusing to complete \"{slug}\": {path} cannot be validated against its summary confirmation: {error}. {recovery_message}"
                    ),
                    Coverage::Stale,
                    policy.is_some(),
                );
            }
        };
        let confirmed_hash = block_field(&receipt.block, "Questions SHA-256");
        if confirmed_hash != Some(current_hash.as_str()) {
            if accepts_changes(&mut policy) {
                // An answer fixed or a question added after "Looks
                // correct" keeps the confirmation: recorded and said once.
                accepted_changes.push(AcceptedChange {
                    stage: slug.to_owned(),
                    unit: asked.unit.map(str::to_owned),
                    changed: vec![question_relative.clone()],
                    recorded: confirmed_hash.unwrap_or("(not recorded)").to_owned(),
                    current: current_hash.clone(),
                    notice: format!(
                        "{question_relative} changed after you confirmed its summary; carrying on with it as it is now."
                    ),
                });
            } else if legacy_scope {
                return refused(
                    "SUMMARY_CONTENT_SEMANTICS_CHANGED",
                    format!(
                        "Refusing to complete \"{slug}\": the summary-confirmation receipt for {path} predates the Markdown-parser upgrade. Either the confirmed content changed after confirmation or raw HTML content that v1 treated as confirmed text is no longer part of it. Raw HTML headings and control tags are now excluded from Markdown recognition. {recovery_message}"
                    ),
                    Coverage::Stale,
                    true,
                );
            } else {
                return refused(
                    "SUMMARY_CONTENT_STALE",
                    format!(
                        "Refusing to complete \"{slug}\": {path} changed after the human confirmed its summary. {recovery_message}"
                    ),
                    Coverage::Stale,
                    true,
                );
            }
        }

        // `summaryArtifactPaths`: the stage's outputs beside the questions.
        let artifacts: Vec<String> = stage
            .produces
            .iter()
            .chain(&stage.optional_produces)
            .filter(|name| !name.ends_with("-questions"))
            .map(|name| question.dir.join(artifact_filename(name)))
            .filter(|path| path.exists())
            .map(|path| shown(&path))
            .collect();
        for artifact in &artifacts {
            let artifact_relative = from_project(wf, artifact);
            let suffix = format!("/{artifact_relative}");
            let writes: Vec<&ShardEvent> = events
                .iter()
                .filter(|row| {
                    if row.event != "ARTIFACT_CREATED" && row.event != "ARTIFACT_UPDATED" {
                        return false;
                    }
                    let Some(file) = block_field(&row.block, "File") else {
                        return false;
                    };
                    // (A write is kept across a move of the workspace: the
                    // leading slash and the whole tail from the space down
                    // keep a partial name from matching.)
                    row_path(&project, file) == *artifact
                        || file.replace('\\', "/").ends_with(&suffix)
                })
                .collect();
            // A receipt that carries an authorization id decides by
            // descent, not by order: the output's newest write must carry
            // the id this receipt minted.
            let minted = block_field(&receipt.block, authorization::FIELD)
                .filter(|id| re::test("^[0-9a-f]{64}$", "", id));
            if let Some(minted) = minted {
                let newest_writes = frontier(&writes);
                let unauthorized: Vec<&ShardEvent> = newest_writes
                    .iter()
                    .copied()
                    .filter(|row| block_field(&row.block, authorization::FIELD) != Some(minted))
                    .collect();
                if newest_writes.is_empty() || !unauthorized.is_empty() {
                    // An output with no recorded write is missing
                    // evidence, not a changed input: it is refused under
                    // every policy.
                    if !unauthorized.is_empty() && accepts_changes(&mut policy) {
                        let mut stamps: Vec<&str> = unauthorized
                            .iter()
                            .map(|row| {
                                block_field(&row.block, authorization::FIELD).unwrap_or("unstamped")
                            })
                            .collect();
                        stamps.sort_by(|a, b| cmp_utf16(a, b));
                        stamps.dedup();
                        accepted_changes.push(AcceptedChange {
                            stage: slug.to_owned(),
                            unit: asked.unit.map(str::to_owned),
                            changed: vec![artifact_relative.clone()],
                            recorded: minted.to_owned(),
                            current: stamps.join(", "),
                            notice: format!(
                                "{artifact_relative} was saved before you confirmed the current summary; carrying on."
                            ),
                        });
                        continue;
                    }
                    let saved = if newest_writes.is_empty() {
                        "has no recorded write"
                    } else if unauthorized
                        .iter()
                        .any(|row| block_field(&row.block, authorization::FIELD).is_some())
                    {
                        "was last saved under a different summary confirmation"
                    } else {
                        "was last saved before the confirmed answers"
                    };
                    return refused(
                        "SUMMARY_ARTIFACT_UNAUTHORIZED",
                        format!(
                            "Refusing to continue \"{slug}\": this stage's output document {artifact} {saved}. Save the document again, so its write descends from the current confirmation, then continue."
                        ),
                        Coverage::Stale,
                        policy.is_some(),
                    );
                }
                continue;
            }
            let write_after_receipt = writes.iter().any(|row| strictly_after(row, receipt));
            // Confirming the same answers again does not take back what an
            // earlier, identical confirmation already allowed: one of this
            // attempt, before the receipt and before a write, with the
            // positive choice, over the answers as they are now under its
            // own scope.
            let earlier_identical = !write_after_receipt
                && ordered.iter().any(|candidate| {
                    if std::ptr::eq(*candidate, receipt) || !strictly_after(receipt, candidate) {
                        return false;
                    }
                    if block_field(&candidate.block, "Details") != Some("Looks correct") {
                        return false;
                    }
                    let scope = block_field(&candidate.block, "Hash Scope");
                    let known = scope.is_none_or(|scope| {
                        scope == HASH_SCOPE || LEGACY_HASH_SCOPES.contains(&scope)
                    });
                    let Some(hash) = known.then(|| hash_for(scope).ok()).flatten() else {
                        return false;
                    };
                    block_field(&candidate.block, "Questions SHA-256") == Some(hash.as_str())
                        && writes.iter().any(|row| strictly_after(row, candidate))
                });
            if !write_after_receipt && !earlier_identical {
                if writes
                    .iter()
                    .any(|row| row.timestamp == receipt.timestamp && row.shard != receipt.shard)
                {
                    return ordering_failure(
                        "the summary receipt and artifact write",
                        &receipt.timestamp,
                        false,
                        policy.is_some(),
                    );
                }
                return refused(
                    "SUMMARY_ARTIFACT_ORDER_INVALID",
                    format!(
                        "Refusing to continue \"{slug}\": this stage's output document {artifact} was not saved after the confirmed answers. Save the document after confirmation, then continue."
                    ),
                    Coverage::Stale,
                    policy.is_some(),
                );
            }
        }
    }
    Ok(Evidence::Ok {
        required: true,
        accepted_changes,
        change_control_read: policy.is_some(),
    })
}

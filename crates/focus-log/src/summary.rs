//! The summary confirmation as the log tool records it.
//!
//! A stage that asks questions ends them with a summary the person
//! confirms. The confirmation is a checkpoint with a record of its own: the
//! question is recorded with `--checkpoint summary-confirmation
//! --questions-file <path>`, and so is the choice. A question or an answer
//! recorded without them is an ordinary one, which a gate never counts, so
//! the stage would refuse for good; both are refused here with the two
//! commands that do count (`refusePlainSummaryConfirmation`,
//! `summaryConfirmationCommands`, `summaryQuestionEvidence` in
//! `aidlc-log.ts` and `aidlc-lib.ts`).
//!
//! # The questions file is not opened by name
//!
//! Upstream reads the file the flag names with `readFileSync`, so a link
//! there, or a directory above it that is one, brings in a file from
//! outside the record, and a file with a second name can be changed from
//! elsewhere on the disk. What is read here is hashed into the receipt a
//! gate weighs, so it is read as every other file of the record that
//! decides something: a regular file with one name, reached through no
//! link. That is a deliberate deviation.
//!
//! The folder of a stage is only listed to name its questions file in a
//! refusal: nothing is decided by that, and it is listed by name as
//! upstream lists it.
//!
//! # A test this filesystem cannot make
//!
//! A unit's questions file is looked for under the folder of that very
//! name, read off the record's listing. Where names differ by case and the
//! filesystem tells them apart, looking the folder up directly finds the
//! same; where it does not, it would find another unit's.

use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::authorization;
use focus_evidence::summary as confirmed;
use focus_guards::guard_operation::{
    EngineInvocation, RenderMode, RenderShell, render_engine_invocation,
};
use focus_js::path::{self as jspath, Flavour};
use focus_js::text::{to_lower_case, trim};
use focus_js::{EngineError, Result, bail, re};
use focus_lifecycle::artifacts::uses_stage_level_per_unit_artifacts;
use focus_lifecycle::workflow::Workflow;
use focus_lifecycle::{admission, presence, reply};
use focus_state::fields::get_field;
use focus_workspace::{files, links};

use crate::answer::offered_choice_label;

/// `SUMMARY_CONFIRMATION_CHOICES`.
pub(crate) const CHOICES: [&str; 2] = ["Looks correct", "Request changes"];
/// `SUMMARY_CONFIRMATION_CHECKPOINT`, as a row names it.
pub(crate) const CHECKPOINT: &str = confirmed::CHECKPOINT;

/// `isSummaryConfirmationChoice`: "Looks correct" or "Request changes",
/// however it was capitalised or decorated.
fn is_choice(text: &str) -> bool {
    let stripped = reply::strip_recommended_decorator(text);
    let normalized = to_lower_case(&re::replace("[.!]+$", "", trim(&stripped), ""));
    normalized == "looks correct" || normalized == "request changes"
}

/// `isSummaryConfirmationOptions`: a question whose offered options are the
/// summary's two choices.
pub(crate) fn is_options(options: Option<&str>) -> bool {
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

/// How the summary was asked, when the stage's latest question is it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Asked {
    /// Recorded with its checkpoint.
    Checkpoint,
    /// Recorded as a plain question that offers the summary's two choices.
    Plain,
}

/// `answersSummaryQuestion`: whether the stage's latest recorded question
/// is the summary's, and in which form. (Upstream narrows the questions to
/// one unit of work when the answer names one; an answer about a unit is
/// refused by name until slice 5.)
pub(crate) fn answers_summary_question(
    rows: &[ShardEvent],
    stage: &str,
    single: bool,
) -> Option<Asked> {
    let workflow = single.then(|| format!("single-stage:{stage}"));
    let decisions: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| {
            row.event == "DECISION_RECORDED"
                && block_field(&row.block, "Stage") == Some(stage)
                && block_field(&row.block, "Workflow") == workflow.as_deref()
        })
        .collect();
    let latest: Vec<Option<Asked>> = crate::answer::maximal(&decisions)
        .iter()
        .map(|row| match block_field(&row.block, "Checkpoint") {
            None => is_options(block_field(&row.block, "Options")).then_some(Asked::Plain),
            Some(checkpoint) => (checkpoint == CHECKPOINT).then_some(Asked::Checkpoint),
        })
        .collect();
    if latest.contains(&Some(Asked::Checkpoint)) {
        Some(Asked::Checkpoint)
    } else if latest.contains(&Some(Asked::Plain)) {
        Some(Asked::Plain)
    } else {
        None
    }
}

/// `questionFilesInDir`: the `-questions.md` files of a folder, by name.
fn question_files_in(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.ends_with("-questions.md"))
        .collect();
    names.sort_by(|a, b| focus_js::order::cmp_utf16(a, b));
    names.into_iter().map(|name| dir.join(name)).collect()
}

/// `summaryQuestionFileRelative`: the stage's questions file, for one unit
/// on a stage that runs per unit, named from the project; `None` when there
/// is none to name.
fn question_file_relative(
    wf: &Workflow,
    stage: &focus_graph::runtime::Stage,
    state: &str,
    unit: Option<&str>,
) -> Option<String> {
    let record = wf.record_dir()?;
    let stage_level = !focus_lifecycle::stage::is_per_unit(stage)
        || uses_stage_level_per_unit_artifacts(wf, get_field(state, "Scope").unwrap_or(""), state);
    let found = if stage_level {
        // A file at the stage is no unit's.
        unit.is_none()
            .then(|| question_files_in(&record.join(&stage.phase).join(&stage.slug)))
            .and_then(|files| files.into_iter().next())
    } else {
        // Under the unit asked for, when the record has a folder of that
        // very name (a filesystem that ignores case would find another).
        let unit = unit?;
        let construction = record.join("construction");
        let listed = std::fs::read_dir(&construction)
            .ok()?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .any(|name| name == unit);
        listed
            .then(|| question_files_in(&construction.join(unit).join(&stage.slug)))
            .and_then(|files| files.into_iter().next())
    }?;
    let host = Flavour::host();
    let project = wf.layout.project.to_string_lossy().into_owned();
    Some(jspath::relative(host, &project, &project, &found.to_string_lossy()).replace('\\', "/"))
}

/// The two commands that record a summary confirmation.
pub(crate) struct Commands {
    pub decision: String,
    pub answer: String,
}

/// `summaryConfirmationCommands`: the two commands, flags filled in, every
/// argument quoted for this platform's shell.
fn commands(
    wf: &Workflow,
    stage: &str,
    unit: Option<&str>,
    questions_file: Option<&str>,
    single: bool,
    details: &str,
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
    let shell = if cfg!(windows) {
        RenderShell::PowerShell
    } else {
        RenderShell::Posix
    };
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

/// Which verb a summary is being recorded with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verb {
    Decision,
    Answer,
}

/// What a plain call carries that says it is the summary.
pub(crate) struct Plain<'a> {
    pub stage: &'a str,
    pub unit: Option<&'a str>,
    pub single: bool,
    /// A question's `--options`.
    pub options: Option<&'a str>,
    /// An answer's `--details`.
    pub details: Option<&'a str>,
}

/// `refusePlainSummaryConfirmation`: a call with no `--checkpoint` that is
/// plainly the summary (a question offering its two choices; an answer to
/// the summary question open for the stage), on a stage that owes one, is
/// refused with the commands that count.
///
/// # Errors
///
/// The refusal; or the state file cannot be read.
pub(crate) fn refuse_plain(
    wf: &Workflow,
    rows: &[ShardEvent],
    verb: Verb,
    call: &Plain,
) -> Result<()> {
    let Some(stage) = wf.install.stage(call.stage) else {
        return Ok(());
    };
    // An answer names a summary choice by its label, with what to change
    // after it.
    let read = match verb {
        Verb::Answer => offered_choice_label(call.details.unwrap_or(""), &CHOICES),
        Verb::Decision => None,
    };
    // An answer is to the summary question when that is the question open
    // for this stage, whatever its words; a question is the summary by its
    // options. (Upstream also takes an answer that names one of the two
    // choices for the summary, and then lets it through when no summary
    // question is open: only the open question decides.)
    let asked = match verb {
        Verb::Answer => answers_summary_question(rows, &stage.slug, call.single),
        Verb::Decision => None,
    };
    let looks_like_summary = match verb {
        Verb::Decision => is_options(call.options),
        Verb::Answer => asked.is_some(),
    };
    if !looks_like_summary {
        return Ok(());
    }
    let state = wf.read_state()?;
    if !admission::summary_confirmation_owed(wf, stage, &state) {
        return Ok(());
    }
    // A change request that says what to change keeps what they asked for,
    // so nobody asks "What should change?" again. A summary asked in the
    // plain form is recorded with its checkpoint, and the person's reply to
    // it as first asked answers it: the agent passes the choice it read.
    let details = match (&read, asked) {
        (_, Some(Asked::Plain)) | (None, _) if verb == Verb::Answer => "<their choice>".to_owned(),
        (Some((choice, rest)), _) if *choice == CHOICES[1] => {
            if rest.is_empty() {
                CHOICES[1].to_owned()
            } else {
                format!("{}: {}", CHOICES[1], re::replace(r"\s+", "g", rest, " "))
            }
        }
        _ => CHOICES[0].to_owned(),
    };
    let unit = call
        .unit
        .or_else(|| focus_lifecycle::stage::is_per_unit(stage).then_some("<unit>"));
    let file = question_file_relative(wf, stage, &state, call.unit);
    let commands = commands(
        wf,
        &stage.slug,
        unit,
        file.as_deref(),
        call.single,
        &details,
    )?;
    let (noun, what) = match verb {
        Verb::Decision => ("decision", "a decision"),
        Verb::Answer => ("answer", "an answer"),
    };
    let why = format!(
        "\"{}\" owes a consolidated summary confirmation, and {what} without `--checkpoint summary-confirmation --questions-file <path>` is an ordinary question that never counts toward it.",
        stage.slug
    );
    match (verb, asked) {
        (Verb::Decision, _) => bail!(
            "Refusing to record this {noun}: {why} Run `{}` instead (the summary section needs exactly one blank `[Answer]:` line), end the turn, and after the human replies run `{}`.",
            commands.decision,
            commands.answer
        ),
        // The person already answered the recorded summary question: it is
        // recorded with the flags, without asking again.
        (Verb::Answer, Some(Asked::Checkpoint)) => bail!(
            "Refusing to record this {noun}: {why} The summary question is already recorded and answered; write the choice they made in its `[Answer]:` line and run `{}`{}",
            commands.answer,
            if details == "<their choice>" {
                " with \"Looks correct\" or 'Request changes: <what they asked to change>' (single-quoted) in place of <their choice>."
            } else {
                "."
            }
        ),
        // The summary was asked in the plain form and the person answered
        // it: it is recorded with the flags, and their reply answers it.
        (Verb::Answer, _) => bail!(
            "Refusing to record this {noun}: {why} Record the summary with `{}` (exactly one blank `[Answer]:` line in the summary section), write the choice they made in that line, and run `{}` with it in place of <their choice>, without asking them again: their reply to the summary as first asked answers it.",
            commands.decision,
            commands.answer
        ),
    }
}

/// `summaryQuestionEvidence`: the questions file the flag names, which has
/// to be a stage's `-questions.md` inside the record, with `expected` on
/// the one `[Answer]:` line of its summary section (nothing, for the
/// question; the choice, for the answer). Answers with what a summary's
/// row binds to: the file, named from the project, and the hash of what
/// was confirmed in it.
///
/// # Errors
///
/// The flag is missing; the file is outside the record, is not a questions
/// file, does not exist, is a link or behind one or not a plain file with
/// one name; its summary section does not hold `expected`; or it cannot be
/// hashed.
pub(crate) fn question_evidence(
    wf: &Workflow,
    supplied: Option<&str>,
    expected: &str,
) -> Result<(String, String)> {
    let Some(supplied) = supplied.filter(|supplied| !supplied.is_empty()) else {
        bail!(
            "Summary confirmation requires --questions-file <path> so the receipt can bind to the reviewed answers."
        );
    };
    let host = Flavour::host();
    let shown = |path: &Path| path.to_string_lossy().into_owned();
    let project = shown(&wf.layout.project);
    let absolute = jspath::resolve(host, &project, &[&project, supplied]);
    let root = wf.record_dir().map(|root| shown(&root));
    let inside = root.as_ref().is_some_and(|root| {
        absolute == *root || absolute.starts_with(&format!("{root}{}", host.separator()))
    });
    let Some(root) = root.filter(|_| inside) else {
        bail!(
            "Summary confirmation questions file must be inside the active intent record: {supplied}"
        );
    };
    if !absolute.ends_with("-questions.md") {
        bail!(
            "Summary confirmation questions file must be the stage's <slug>-questions.md file: {supplied}"
        );
    }
    // Not opened by name (see the module docs): no link on the way to it,
    // and a regular file with one name.
    let from_record = jspath::relative(host, &project, &root, &absolute);
    if let Ok(anchor) = std::fs::canonicalize(&root) {
        links::refuse_link_in_chain(&anchor, Path::new(&from_record))?;
    }
    if !Path::new(&absolute).exists() {
        bail!("Summary confirmation questions file does not exist: {supplied}");
    }
    let bytes = files::read_regular(Path::new(&absolute), "summary questions file")?;
    let body = String::from_utf8_lossy(&bytes);
    if confirmed::answer(&body)?.as_deref() != Some(expected) {
        let rendered = if expected.is_empty() {
            "a blank value"
        } else {
            expected
        };
        bail!(
            "Summary confirmation section in {supplied} must contain exactly one `[Answer]:` line with {rendered} before this command runs."
        );
    }
    let sha256 = confirmed::content_hash(&body).map_err(|why| {
        EngineError::new(format!(
            "Summary confirmation questions file {supplied} is invalid: {why}."
        ))
    })?;
    Ok((
        jspath::relative(host, &project, &project, &absolute).replace('\\', "/"),
        sha256,
    ))
}

/// What `pendingSummaryDecision` finds for a stage's summary.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    /// No unanswered summary question matches.
    Nothing,
    /// Rows that matter share an instant across shards, so that a reply
    /// after the question cannot be proven; the instant.
    Ambiguous(String),
    /// The summary's question is recorded and unanswered. `question` is its
    /// text when no reply of the person is recorded after it: the reply
    /// came before the question was recorded.
    Open { question: Option<String> },
}

/// Whether `candidate` is after `boundary`: by position in one shard, by
/// instant across shards, and not at all for two shards at one instant.
/// (No row is asked about itself.)
fn follows(candidate: &ShardEvent, boundary: &ShardEvent) -> Option<bool> {
    if candidate.shard == boundary.shard {
        return Some(candidate.pos > boundary.pos);
    }
    match focus_js::order::cmp_utf16(&candidate.timestamp, &boundary.timestamp) {
        std::cmp::Ordering::Equal => None,
        order => Some(order == std::cmp::Ordering::Greater),
    }
}

/// `pendingSummaryDecision`: whether the summary's question for this stage
/// and questions file is recorded and still unanswered, in the stage's
/// current run when it is run by itself.
pub(crate) fn pending_decision(
    rows: &[ShardEvent],
    stage: &str,
    workflow: Option<&str>,
    questions_file: &str,
) -> Pending {
    let field = |row: &ShardEvent, name: &str| block_field(&row.block, name).map(str::to_owned);
    let of_this_run = |row: &ShardEvent| {
        field(row, "Stage").as_deref() == Some(stage)
            && field(row, "Workflow").as_deref() == workflow
    };
    let is_action = |row: &ShardEvent| {
        matches!(
            row.event.as_str(),
            "DECISION_RECORDED" | "SUMMARY_CONFIRMATION_RECORDED"
        )
    };
    let entries: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| match row.event.as_str() {
            // A turn that was only a command to the engine is no reply.
            "HUMAN_TURN" => presence::is_reply_turn(&row.event, &row.block),
            "STAGE_COMPLETED" => of_this_run(row),
            _ if is_action(row) => {
                of_this_run(row)
                    && field(row, "Checkpoint").as_deref() == Some(CHECKPOINT)
                    && field(row, "Unit").is_none()
                    && field(row, "Questions File").as_deref() == Some(questions_file)
            }
            _ => false,
        })
        .collect();
    let completions: Vec<&ShardEvent> = entries
        .iter()
        .filter(|row| row.event == "STAGE_COMPLETED")
        .copied()
        .collect();
    let floors = authorization::frontier(&completions);
    // After every floor, before one of them, or neither can be said.
    let after_floor = |row: &ShardEvent| -> Option<bool> {
        let relations: Vec<Option<bool>> = floors.iter().map(|floor| follows(row, floor)).collect();
        if relations.iter().all(|relation| *relation == Some(true)) {
            Some(true)
        } else if relations.contains(&Some(false)) {
            Some(false)
        } else {
            None
        }
    };
    let actions: Vec<&ShardEvent> = entries
        .iter()
        .filter(|row| is_action(row))
        .copied()
        .collect();
    let ordered: Vec<&ShardEvent> = actions
        .iter()
        .filter(|row| after_floor(row) == Some(true))
        .copied()
        .collect();
    let latest = authorization::frontier(&ordered);
    let latest_instant = latest.first().map(|row| row.timestamp.clone());
    let unordered: Vec<&ShardEvent> = actions
        .iter()
        .filter(|row| after_floor(row).is_none())
        .copied()
        .collect();
    let not_before_latest = |row: &&ShardEvent| {
        latest_instant.as_ref().is_none_or(|instant| {
            focus_js::order::cmp_utf16(&row.timestamp, instant) != std::cmp::Ordering::Less
        })
    };
    if unordered.iter().any(not_before_latest) {
        let instant = floors
            .first()
            .map_or(&unordered[0].timestamp, |floor| &floor.timestamp);
        return Pending::Ambiguous(instant.clone());
    }
    let Some(latest_instant) = latest_instant else {
        return Pending::Nothing;
    };
    let questions = latest
        .iter()
        .filter(|row| row.event == "DECISION_RECORDED")
        .count();
    if questions != 0 && questions != latest.len() {
        return Pending::Ambiguous(latest_instant);
    }
    if questions == 0 {
        return Pending::Nothing;
    }
    // The person's reply since their last answer is theirs to give whenever
    // the question was recorded; this says only whether it came after the
    // question's record. A turn in another shard at the record's own
    // instant proves neither.
    let relations: Vec<Vec<Option<bool>>> = entries
        .iter()
        .filter(|row| row.event == "HUMAN_TURN")
        .map(|human| {
            latest
                .iter()
                .map(|question| follows(human, question))
                .collect()
        })
        .collect();
    if relations
        .iter()
        .any(|of| of.iter().all(|relation| *relation == Some(true)))
    {
        return Pending::Open { question: None };
    }
    if relations
        .iter()
        .any(|of| !of.contains(&Some(false)) && of.contains(&None))
    {
        return Pending::Ambiguous(latest_instant);
    }
    Pending::Open {
        question: Some(field(latest[0], "Decision").unwrap_or_default()),
    }
}

/// What the answer to a summary carries into its receipt.
pub(crate) struct Choice<'a> {
    pub stage: &'a str,
    pub single: bool,
    /// `Looks correct` or `Request changes`.
    pub choice: &'a str,
    /// What the person asked to change, after the choice.
    pub feedback: Option<&'a str>,
    /// The questions file, named from the project, and its hash.
    pub questions_file: &'a str,
    pub questions_sha256: &'a str,
}

/// The summary branch of `handleAnswer`, under the lock: the question has
/// to be recorded and unanswered and the person has to have replied since
/// their last answer; then the stage's authorization is written (or
/// withdrawn, for a change request) and the receipt after it. A receipt
/// that cannot be written leaves the authorization as it was.
///
/// # Errors
///
/// What upstream refuses, in its words.
pub(crate) fn record_choice(
    wf: &Workflow,
    held: &focus_workspace::lock::WorkspaceLock,
    fields: &mut Vec<(&str, String)>,
    answer: &Choice,
) -> Result<serde_json::Value> {
    let workflow = answer
        .single
        .then(|| format!("single-stage:{}", answer.stage));
    let rows = wf.events();
    let question = match pending_decision(
        &rows,
        answer.stage,
        workflow.as_deref(),
        answer.questions_file,
    ) {
        Pending::Ambiguous(instant) => bail!(
            "Refusing to record summary confirmation: matching prompt, response, or run-boundary events share audit Timestamp \"{instant}\" across different shards, so a human response after this prompt cannot be proven. Present a fresh summary prompt after that second, end the turn, and record the human's new response."
        ),
        Pending::Nothing => bail!(
            "Cannot record the summary choice because no matching unanswered summary question exists for this stage and work item. Record the question before presenting it, then wait for the human's choice."
        ),
        Pending::Open { question } => question,
    };
    if !presence::human_presence_guard_disabled(wf) && !presence::human_replied_since_gate(wf) {
        bail!(
            "Cannot record the summary choice because no human reply has arrived since their last answer. End the turn, wait for the human's choice, then try again.{}",
            presence::unattended_human_presence_hint_for_a_reply_not_given(wf)
        );
    }
    // Their words go on the receipt. A reply that came before the question
    // was recorded is the agent's reading, so the engine says it back.
    let words = presence::latest_person_turn_words(wf)
        .filter(|words| !words.is_empty() && !reply::is_non_answer(Some(words)));
    if let Some(words) = &words {
        fields.push(("Person Reply", words.clone()));
    }
    let say = question.map(|question| {
        let kept = words.as_ref().map_or_else(
            || "reply".to_owned(),
            |words| format!("\"{}\"", trim(&re::replace(r"\s+", "g", words, " "))),
        );
        format!("Recorded your {kept} for \"{question}\".")
    });
    // The confirmation authorizes the outputs generated from it: the id is
    // minted from the attempt, the scope and what was confirmed.
    let floors =
        authorization::attempt_floors(&rows, answer.stage, workflow.as_deref(), &wf.install.graph);
    let attempt = authorization::attempt_identity(&floors);
    let id = authorization::id(
        &attempt,
        answer.stage,
        workflow.as_deref(),
        answer.questions_file,
        answer.questions_sha256,
        answer.choice,
    );
    let positive = answer.choice == CHOICES[0];
    if positive {
        fields.push((authorization::FIELD, id.clone()));
    }
    // The registry first, the receipt second, both under the lock. A
    // registry that cannot be written refuses the answer before any
    // receipt exists, so the question and the person's turn are still
    // there for another try.
    let record = wf.record_dir();
    let saved = (|| -> Result<Option<Vec<u8>>> {
        let Some(record) = &record else {
            bail!("Cannot resolve the active intent record.");
        };
        let target = authorization::target(record, answer.stage)?;
        let before = if target.symlink_metadata().is_ok() {
            Some(files::read_regular_within(
                &target,
                "summary authorization",
                Some(64 * 1024),
            )?)
        } else {
            None
        };
        if positive {
            authorization::write(
                record,
                &authorization::Authorization {
                    id: id.clone(),
                    stage: answer.stage.to_owned(),
                    workflow: workflow.clone(),
                    attempt,
                    questions_file: answer.questions_file.to_owned(),
                    questions_sha256: answer.questions_sha256.to_owned(),
                    choice: answer.choice.to_owned(),
                    recorded_at: focus_audit::time::iso_timestamp(std::time::SystemTime::now()),
                },
            )?;
        } else {
            authorization::clear(record, answer.stage)?;
        }
        Ok(before)
    })();
    let before = match saved {
        Ok(before) => before,
        Err(why) => bail!(
            "Cannot record the summary choice: its authorization record could not be saved ({why}). Nothing was recorded; the pending question is still answerable. Repair the record directory and run this command again."
        ),
    };
    if let Err(why) = wf.emit(held, "SUMMARY_CONFIRMATION_RECORDED", fields) {
        // The registry goes back to what it held, so no authorization
        // stands without the receipt that minted it.
        let rollback = match record
            .as_ref()
            .map(|record| authorization::restore(record, answer.stage, before.as_deref()))
        {
            Some(Err(restore)) => format!(
                " The authorization record could not be restored either ({restore}); remove {} under the intent record before retrying.",
                authorization::relative_path(answer.stage).unwrap_or_default()
            ),
            _ => String::new(),
        };
        bail!("{why}.{rollback}");
    }
    let mut out = serde_json::Map::new();
    out.insert("emitted".to_owned(), "SUMMARY_CONFIRMATION_RECORDED".into());
    out.insert("checkpoint".to_owned(), "summary-confirmation".into());
    out.insert("stage".to_owned(), answer.stage.into());
    out.insert("choice".to_owned(), answer.choice.into());
    if positive {
        out.insert("summary_authorization_id".to_owned(), id.into());
    }
    if let Some(feedback) = answer.feedback {
        out.insert("feedback".to_owned(), feedback.into());
    }
    if let Some(say) = say {
        out.insert("say".to_owned(), say.into());
    }
    Ok(serde_json::Value::Object(out))
}

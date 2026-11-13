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

use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::summary as confirmed;
use focus_guards::guard_operation::{
    EngineInvocation, RenderMode, RenderShell, render_engine_invocation,
};
use focus_js::path::{self as jspath, Flavour};
use focus_js::text::{to_lower_case, trim};
use focus_js::{EngineError, Result, bail, re};
use focus_lifecycle::artifacts::uses_stage_level_per_unit_artifacts;
use focus_lifecycle::workflow::Workflow;
use focus_lifecycle::{admission, reply};
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
/// is the summary's, and in which form.
pub(crate) fn answers_summary_question(
    rows: &[ShardEvent],
    stage: &str,
    unit: Option<&str>,
    single: bool,
) -> Option<Asked> {
    let workflow = single.then(|| format!("single-stage:{stage}"));
    let decisions: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| {
            row.event == "DECISION_RECORDED"
                && block_field(&row.block, "Stage") == Some(stage)
                && block_field(&row.block, "Workflow") == workflow.as_deref()
                && unit.is_none_or(|unit| block_field(&row.block, "Unit") == Some(unit))
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
    let stage_level = !focus_graph::query::is_per_unit(stage)
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
    // for this stage and work item, whatever its words; a question, by its
    // options.
    let asked = match verb {
        Verb::Answer => answers_summary_question(rows, &stage.slug, call.unit, call.single),
        Verb::Decision => None,
    };
    let looks_like_summary = match verb {
        Verb::Decision => is_options(call.options),
        Verb::Answer => asked.is_some() || call.details.is_some_and(is_choice) || read.is_some(),
    };
    if !looks_like_summary {
        return Ok(());
    }
    let state = wf.read_state()?;
    if !admission::summary_confirmation_owed(wf, stage, &state) {
        return Ok(());
    }
    if verb == Verb::Answer && asked.is_none() {
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
        .or_else(|| focus_graph::query::is_per_unit(stage).then_some("<unit>"));
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

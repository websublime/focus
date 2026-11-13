//! `log answer`: the person's reply to a question the agent asked, written
//! to the ledger as a `QUESTION_ANSWERED` row (`handleAnswer` in
//! `aidlc-log.ts`). The agent passes the choice it read in the reply; the
//! engine records it beside the person's own words and reads no meaning
//! into them. What it does check is that there is a reply: a dismissed
//! question is no answer, a choice the text credits to the assistant is not
//! the person's, an approval is a gate's and not a question's, and an
//! answer needs a message of the person's that has not answered anything
//! yet, or the message named as the one that did.
//!
//! An answer the person left to the agent (`--on-instruction`, with their
//! own words) is the one answer that needs no reply: it needs the person
//! to have said something in this piece of work, and is recorded as the
//! agent's choice at their word. An approval is never one of those.
//!
//! What is ported is the answer to a stage's question. Refused by name
//! until the slice that brings them: the engine's own questions, each with
//! a record of its own (the recovery question, slice 4; a summary
//! confirmation, 2c; Construction's checkpoints and a question about one
//! unit of work, 5; Plan Approval, 11).

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_js::text::{slice_utf16, to_lower_case, trim};
use focus_js::{Result, bail, re};
use focus_lifecycle::presence::{self, TurnState};
use focus_lifecycle::reply::{self, DecisionKind};
use focus_lifecycle::workflow::Workflow;
use focus_lifecycle::{admission, events};
use focus_state::progress::{CheckboxState, parse_checkboxes};
use serde_json::{Value, json};

use crate::decision::unknown_checkpoint;
use crate::message::{self, What};
use crate::summary::{self, Plain, Verb};
use crate::{answer_text, flags};

/// `ANSWER_MODE_LABELS`.
const ANSWER_MODE_LABELS: [(&str, &str); 3] = [
    ("guide", "Guide me"),
    ("file", "I'll edit the file"),
    ("chat", "Chat"),
];

/// `offeredChoiceLabel`: the choice a reply names, when it starts with one
/// of `choices` (after an option's letter or number, and quotes), with
/// whatever follows the label.
#[must_use]
pub fn offered_choice_label<'a>(details: &str, choices: &[&'a str]) -> Option<(&'a str, String)> {
    let text = reply::strip_recommended_decorator(trim(details));
    let text = re::replace(r"^(?:(?:[A-Za-z]|\d+)[.)])\s*", "", &text, "");
    let text = re::replace(r#"^["'`]+|["'`]+$"#, "g", &text, "");
    let text = trim(&text);
    for choice in choices {
        let units = focus_js::text::len_utf16(choice);
        let head = slice_utf16(text, 0, units);
        if to_lower_case(&head) != to_lower_case(choice) {
            continue;
        }
        let tail = slice_utf16(text, units, usize::MAX);
        if tail.is_empty() || re::test(r"^[\s:;,.!-]", "", &tail) {
            let rest = re::replace(r"^[\s:;,.!-]+", "", &tail, "");
            return Some((choice, trim(&rest).to_owned()));
        }
    }
    None
}

/// `attemptEventDefinitelyBefore`.
fn definitely_before(earlier: &ShardEvent, later: &ShardEvent) -> bool {
    if earlier.shard == later.shard {
        return earlier.pos < later.pos;
    }
    focus_js::order::cmp_utf16(&earlier.timestamp, &later.timestamp) == std::cmp::Ordering::Less
}

/// `maximalAttemptEvents`: the rows nothing is definitely after. Position
/// within a shard and instant between shards are two orders, so three rows
/// can each have another after them; then the rows of the latest instant
/// stand for all.
pub(crate) fn maximal<'a>(rows: &[&'a ShardEvent]) -> Vec<&'a ShardEvent> {
    let kept: Vec<&ShardEvent> = rows
        .iter()
        .enumerate()
        .filter(|(at, candidate)| {
            !rows
                .iter()
                .enumerate()
                .any(|(other_at, other)| other_at != *at && definitely_before(candidate, other))
        })
        .map(|(_, row)| *row)
        .collect();
    if !kept.is_empty() || rows.is_empty() {
        return kept;
    }
    let latest = rows
        .iter()
        .map(|row| row.timestamp.as_str())
        .max_by(|a, b| focus_js::order::cmp_utf16(a, b))
        .unwrap_or_default();
    rows.iter()
        .filter(|row| row.timestamp == latest)
        .copied()
        .collect()
}

/// `handleAnswer`. `open` selects the piece of work the answer belongs to.
///
/// # Errors
///
/// What upstream refuses, in its words; what Focus refuses by name.
pub fn answer(open: &dyn Fn() -> Result<Workflow>, args: &[String]) -> Result<Value> {
    let (_, flags) = flags::parse(args)?;
    let Some(stage) = flags.given("stage") else {
        bail!("Missing --stage <slug>");
    };
    // Text that holds shell characters arrives in a file the agent wrote,
    // so no shell reads it on the way.
    let mut texts = [
        (
            "details",
            "details-file",
            flags.get("details").map(str::to_owned),
        ),
        (
            "on-instruction",
            "on-instruction-file",
            flags.get("on-instruction").map(str::to_owned),
        ),
    ];
    for (text, file, value) in &mut texts {
        let Some(name) = flags.get(file) else {
            continue;
        };
        if value.is_some() {
            bail!("Pass either --{text} or --{file}, not both.");
        }
        *value = Some(answer_text::read(&open()?, name)?);
    }
    let [(_, _, details), (_, _, instruction)] = texts;
    let Some(said) = details.as_deref().filter(|details| !details.is_empty()) else {
        bail!("Missing --details <text> (or --details-file <file>)");
    };
    // A stage question the person left to the agent. Checkpoints and
    // approvals stay the person's own each time, and an unattended run has
    // nobody to hand anything over.
    let instruction =
        instruction.map(|words| trim(&re::replace(r"\s+", "g", &words, " ")).to_owned());
    if let Some(instruction) = &instruction {
        if instruction.is_empty() {
            bail!("--on-instruction needs the person's own words that left the choice to you.");
        }
        if let Some(checkpoint) = flags.get("checkpoint") {
            bail!(
                "--on-instruction is for a stage question the person left to you; --checkpoint {checkpoint} is theirs to answer. Ask them and record their reply."
            );
        }
        if !presence::attended() {
            bail!(
                "--on-instruction needs a person in the session; {}=1 is set.",
                focus_workspace::env::full_name("UNATTENDED")
            );
        }
    }
    match flags.get("checkpoint") {
        None => {}
        Some("guard-recovery") => bail!(
            "--checkpoint guard-recovery records the remedy the person picked after a refusal, and Focus does not ask the recovery question yet."
        ),
        Some("summary-confirmation") => {}
        Some(checkpoint @ ("verification-command" | "construction-policy")) => bail!(
            "--checkpoint {checkpoint} records an answer at one of Construction's checkpoints, and Focus does not run Construction yet."
        ),
        Some("plan-approval") => bail!(
            "--checkpoint plan-approval records the Plan Approval choice, and Focus does not run Plan Approval yet."
        ),
        Some(other) => bail!("{}", unknown_checkpoint(other)),
    }
    for (flag, what) in [
        ("override", "a break-glass reason for Plan Approval"),
        ("override-file", "a break-glass reason for Plan Approval"),
        (
            "batch-file",
            "the Plan Approval choices of several units of work at once",
        ),
    ] {
        if flags.get(flag).is_some() {
            bail!("--{flag} records {what}, and Focus does not run Plan Approval yet.");
        }
    }
    if flags.given("unit").is_some() {
        bail!("--unit records an answer about one unit of work, and Focus does not run units yet.");
    }
    let wf = open()?;
    let rows = wf.events();
    let stage_node = wf.install.stage(stage);
    // An answer to the stage's summary question that does not say so never
    // counts: refused with the commands that do.
    if flags.get("checkpoint").is_none() {
        summary::refuse_plain(
            &wf,
            &rows,
            Verb::Answer,
            &Plain {
                stage,
                unit: None,
                single: flags.get("single").is_some(),
                options: None,
                details: Some(said),
            },
        )?;
    }
    // The mode question: the agent reads the way the person chose from
    // their reply and records its option, which later stages reuse.
    let mut details = said.to_owned();
    if !reply::is_non_answer(Some(said))
        && let Some(asked) = events::open_decision_block(&rows, stage)
        && events::is_answer_mode_decision(&asked)
    {
        let labels: Vec<&str> = ANSWER_MODE_LABELS.iter().map(|(_, label)| *label).collect();
        let mode = events::answer_mode_from_reply(Some(said)).or_else(|| {
            offered_choice_label(said, &labels)
                .and_then(|(choice, _)| events::answer_mode_from_reply(Some(choice)))
        });
        let Some(mode) = mode else {
            bail!(
                "--details {} does not name a way to answer. Pass the way the person chose, \"{}\", \"{}\" or \"{}\", as you read it from their reply; their own words are kept beside it.",
                reply::format_received_reply(Some(said)),
                labels[0],
                labels[1],
                labels[2]
            );
        };
        details = ANSWER_MODE_LABELS
            .iter()
            .find(|(name, _)| *name == mode)
            .map_or(details, |(_, label)| (*label).to_owned());
    }
    // The summary's choice: the agent names the one the person made; what
    // they asked to change may follow the label.
    let summary_checkpoint = flags.get("checkpoint") == Some("summary-confirmation");
    if summary_checkpoint && stage_node.is_some_and(focus_lifecycle::stage::is_per_unit) {
        bail!(
            "\"{stage}\" runs per unit of work, and a unit's summary confirmation binds to the unit's own attempt: Focus does not run units yet."
        );
    }
    let mut feedback: Option<String> = None;
    if summary_checkpoint && !reply::is_non_answer(Some(said)) {
        let Some((choice, rest)) = offered_choice_label(said, &summary::CHOICES) else {
            bail!(
                "Cannot record the summary choice because --details {} does not name a choice. Pass the choice the person made, \"{}\" or \"{}\", as you read it from their reply, with what they asked to change after it.",
                reply::format_received_reply(Some(said)),
                summary::CHOICES[0],
                summary::CHOICES[1]
            );
        };
        details = choice.to_owned();
        feedback = (choice == summary::CHOICES[1] && !rest.is_empty()).then_some(rest);
    }
    // A cancelled, dismissed or timed-out question box is not an answer.
    if reply::is_non_answer(Some(&details)) {
        bail!(
            "Cannot record reply {} because it represents a dismissed question, not a human answer. Re-present the question and wait for a real response before trying again.",
            reply::format_received_reply(Some(&details))
        );
    }
    let summary_evidence = if summary_checkpoint {
        Some(summary::question_evidence(
            &wf,
            flags.get("questions-file"),
            &details,
        )?)
    } else {
        None
    };
    let mut fields: Vec<(&str, String)> =
        vec![("Stage", stage.to_owned()), ("Details", details.clone())];
    if let Some((questions_file, sha256)) = &summary_evidence {
        fields.push(("Checkpoint", summary::CHECKPOINT.to_owned()));
        fields.push(("Questions File", questions_file.clone()));
        fields.push(("Questions SHA-256", sha256.clone()));
        fields.push(("Hash Scope", focus_evidence::summary::HASH_SCOPE.to_owned()));
        // A change request's own words ride on the receipt as its feedback.
        if let Some(feedback) = &feedback {
            fields.push(("Feedback", re::replace(r"\s+", "g", feedback, " ")));
        }
    }
    if flags.get("single") == Some("true") {
        fields.push(("Workflow", format!("single-stage:{stage}")));
    }

    // Classification and emission run under one lock: a gate cannot open
    // between the checkbox read below and the row.
    let held = wf.lock("log answer")?;
    let rows = wf.events();
    let state = wf.read_state()?;
    let autonomous = stage_node.is_some_and(|node| node.phase == "construction")
        && admission::is_autonomous_mode(&state);
    let unguarded = presence::human_presence_guard_disabled(&wf);
    // The authorship floor: the same answer the agent wrote for itself. A
    // person is in the session, just not at this question.
    if !autonomous
        && !unguarded
        && let Some(marker) =
            reply::self_attributed_decision_marker(Some(said), DecisionKind::Answer)
    {
        bail!(
            "Cannot record this answer for \"{stage}\" because --details says it was chosen by the assistant ({}: \"{}\"). This question must be answered by the human. Re-present it and wait for their reply.",
            marker.category,
            marker.phrase
        );
    }
    // The summary is a checkpoint of the person's: a fresh turn of theirs is
    // needed whatever Construction's autonomy.
    if let Some((questions_file, sha256)) = &summary_evidence {
        let out = summary::record_choice(
            &wf,
            &held,
            &mut fields,
            &summary::Choice {
                stage,
                single: flags.get("single") == Some("true"),
                choice: &details,
                feedback: feedback.as_deref(),
                questions_file,
                questions_sha256: sha256,
            },
        )?;
        drop(held);
        return Ok(out);
    }
    // An approval is a gate's, not a question's: an answer at an open gate
    // with no question asked since it opened is acknowledged and not
    // written, so it does not spend the reply the approval needs.
    let at_gate = parse_checkboxes(&state).iter().any(|checkbox| {
        checkbox.slug == stage && checkbox.state == CheckboxState::AwaitingApproval
    });
    if at_gate && !events::has_pending_decision_at_gate(&rows, stage) {
        if instruction.is_some() {
            bail!(
                "An approval is the person's own each time, so --on-instruction cannot record it. Ask them."
            );
        }
        if !autonomous && !unguarded && !presence::human_replied_since_gate(&wf) {
            bail!(
                "Cannot record this approval choice because no new human reply has arrived. After the human types their choice, use `focus engine orchestrate report` --result approved or rejected; do not use `focus engine log` answer for an approval.{}",
                presence::unattended_human_presence_hint(&wf)
            );
        }
        return Ok(json!({
            "skipped": "QUESTION_ANSWERED",
            "stage": stage,
            "reason": "approval-gate-report-owned",
        }));
    }
    // The message that answered: named by the agent, or inherited from the
    // question it was logged with. A named one that fails its proof
    // refuses; an inherited one falls back to the presence test below.
    let mut named = None;
    if instruction.is_none() && !autonomous {
        if let Some(id) = flags.get("message") {
            match message::prove(&wf, id, What::Answer) {
                Ok(proved) => named = Some(proved),
                Err(refusal) => bail!("{refusal}"),
            }
        } else if let Some(inherited) = message::latest_decision_message_id(&wf, stage, None) {
            named = message::prove(&wf, &inherited, What::Answer).ok();
        }
    }
    // The person's own words go on the record beside the choice the agent
    // read in them.
    let own_words =
        |words: Option<String>| words.filter(|words| !reply::is_non_answer(Some(words)));
    if let Some(named) = &named {
        fields.push(("Message Id", named.id.clone()));
        if let Some(words) = own_words(named.words.clone().filter(|words| !words.is_empty())) {
            fields.push(("Person Reply", words));
        }
    } else if instruction.is_none()
        && !autonomous
        && let Some(words) =
            own_words(presence::latest_person_turn_words(&wf).filter(|words| !words.is_empty()))
    {
        fields.push(("Person Reply", words));
    }
    // One reply answers every question that was open when it arrived, each
    // as its own answer. A question asked after it waits for the next
    // reply, unless the message that answered it arrived before it was
    // logged: then the agent names that message instead of asking again.
    if let Some(instruction) = &instruction {
        // `personSpokeInThisWork`: a turn that can carry a request, or the
        // request that started the work. Their handing a choice over can
        // come before the question, so the words are what the record keeps.
        // (A field is read trimmed: a request of white space is none.)
        let spoke = rows.iter().any(|row| {
            presence::is_request_turn(&row.event, &row.block)
                || (row.event == "WORKFLOW_STARTED"
                    && !block_field(&row.block, "Request").unwrap_or("").is_empty())
        });
        if !spoke {
            // Upstream asks for this hint without naming the project, so it
            // is the hint of the project its own script is installed in.
            // Focus has one project in an invocation, the one the command
            // runs for, and the two are the same wherever the engine runs
            // for the project that holds it.
            bail!(
                "Nothing the person said in this piece of work is on record, so no choice was left to you. Ask them.{}",
                presence::unattended_human_presence_hint(&wf)
            );
        }
        fields.push((
            "Answer Source",
            "chosen by the agent as the person asked".to_owned(),
        ));
        fields.push(("Instruction", instruction.clone()));
    } else if !autonomous
        && !unguarded
        && named.is_none()
        && !presence::human_replied_since_gate(&wf)
        && !(presence::attended()
            && presence::human_turn_state(&wf, true, false) == TurnState::Answered)
    {
        let late = message::messages_before_question(&wf, stage, None);
        let why = if late.is_empty() {
            format!(
                "Cannot record this answer because no new human reply has arrived for the question. Wait for the human to type an answer, then try again.{}",
                presence::command_turn_hint(&wf)
            )
        } else {
            format!(
                "Cannot record this answer: the question was logged after the person's reply. {}",
                message::late_question_hint(&late)
            )
        };
        bail!("{why}{}", presence::unattended_human_presence_hint(&wf));
    }
    // Where the person replied in a question box, an answer none of their
    // picks carried is still recorded, with a note saying what they picked.
    if instruction.is_none()
        && !autonomous
        && let Some(note) = presence::picker_answer_note(&wf, &details)
    {
        fields.push(("Picker Note", note));
    }
    wf.emit(&held, "QUESTION_ANSWERED", &fields)?;
    drop(held);
    Ok(json!({"emitted": "QUESTION_ANSWERED", "stage": stage}))
}

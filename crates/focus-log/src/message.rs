//! The proof of a message (`proveMessage` in `aidlc-log.ts`). A question
//! logged after the person already replied names the message that answered
//! it. The agent maps, the engine proves: that the record exists, carries
//! words of the person's own, was not spent by a later decision, and came
//! through the chat this command serves. The engine reads no words.
//!
//! # Tests that another test covers
//!
//! Three things change no answer when taken out, and are kept as upstream
//! has them. The last refusal (the message came through another chat than
//! the one that has spoken since the last decision) is never reached: the
//! message named is itself one of the messages counted, unless it is older
//! than the last decision, and then it was refused as spent. Of two
//! spending rows with the same instant either is the latest. And the id
//! written on the row is the proved record's, which is the id it was read
//! by.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_hooks::messages::{self, Stored};
use focus_js::order::cmp_utf16;
use focus_js::re;
use focus_js::text::{len_utf16, slice_utf16, trim};
use focus_lifecycle::workflow::Workflow;
use focus_workspace::records;

/// `MESSAGE_SPENDING_EVENTS`: a decision of another kind than an answer.
/// One message backs at most one approval plus any answers, so an answer
/// never spends it.
const SPENDING_EVENTS: [&str; 8] = [
    "GATE_APPROVED",
    "GATE_REJECTED",
    "PLAN_APPROVAL_RECORDED",
    "SUMMARY_CONFIRMATION_RECORDED",
    "VERIFICATION_COMMAND_RECORDED",
    "CONSTRUCTION_POLICY_RECORDED",
    "QUESTION_UNANSWERED",
    // A routing answer sends the words before it to the work they named.
    "REQUEST_ROUTED",
];

/// What a message is named to back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum What {
    Answer,
    Decision,
}

impl What {
    fn word(self) -> &'static str {
        match self {
            Self::Answer => "answer",
            Self::Decision => "decision",
        }
    }
}

/// Two instants as the ledger and the store write them, compared as text
/// in UTF-16 units, as upstream compares them.
fn before(a: &str, b: &str) -> bool {
    cmp_utf16(a, b) == Ordering::Less
}

/// `latestSpendingTimestamp`.
fn latest_spending(rows: &[ShardEvent]) -> Option<&str> {
    rows.iter()
        .filter(|row| SPENDING_EVENTS.contains(&row.event.as_str()))
        .map(|row| row.timestamp.as_str())
        .fold(None, |latest: Option<&str>, at| match latest {
            Some(latest) if !before(latest, at) => Some(latest),
            _ => Some(at),
        })
}

/// `messageIsReply`: the message carries the person's words, and its turn
/// was not only a command to the engine or a question about a switch (the
/// marks the presence check reads on the `HUMAN_TURN` row).
fn is_reply(rows: &[ShardEvent], message: &Stored) -> bool {
    if message.words.is_none() {
        return false;
    }
    !rows.iter().any(|row| {
        row.event == "HUMAN_TURN"
            && block_field(&row.block, "Message Id") == Some(message.id.as_str())
            && matches!(
                block_field(&row.block, "Reply"),
                Some("command" | "question")
            )
    })
}

fn chat_of(message: &Stored) -> &str {
    message.session.as_deref().unwrap_or("terminal")
}

/// The chat this command serves, when the engine can tell.
fn invoking_chat(wf: &Workflow) -> Option<String> {
    records::invoking_session_id(&wf.layout, &wf.invoker)
        .ok()
        .flatten()
}

/// The chats that have sent messages since the last spending row.
fn chats_since<'a>(stored: &'a [Stored], spent_at: Option<&str>) -> BTreeSet<&'a str> {
    stored
        .iter()
        .filter(|sent| spent_at.is_none_or(|spent_at| !before(&sent.at, spent_at)))
        .map(chat_of)
        .collect()
}

/// The questions of a stage, and of one unit of work when one is named.
fn decision_rows<'a>(
    rows: &'a [ShardEvent],
    stage: &str,
    unit: Option<&str>,
) -> Vec<&'a ShardEvent> {
    rows.iter()
        .filter(|row| {
            row.event == "DECISION_RECORDED"
                && block_field(&row.block, "Stage") == Some(stage)
                && unit.is_none_or(|unit| block_field(&row.block, "Unit") == Some(unit))
        })
        .collect()
}

/// `latestDecisionMessageId`: the message the latest question of this scope
/// was logged with, for the answers that follow it.
#[must_use]
pub fn latest_decision_message_id(
    wf: &Workflow,
    stage: &str,
    unit: Option<&str>,
) -> Option<String> {
    let rows = wf.events();
    let question = *decision_rows(&rows, stage, unit).last()?;
    block_field(&question.block, "Message Id").map(str::to_owned)
}

/// `messagesBeforeQuestion`: the person's messages that arrived before the
/// latest question of this scope was logged. The turn row that names a
/// message orders it against the question's row; a message with no turn
/// row yet (the first message of new work) is ordered by its instant.
#[must_use]
pub fn messages_before_question(wf: &Workflow, stage: &str, unit: Option<&str>) -> Vec<Stored> {
    let rows = wf.events();
    let Some(question) = decision_rows(&rows, stage, unit).last().copied() else {
        return Vec::new();
    };
    let spent_at = latest_spending(&rows);
    let chat = invoking_chat(wf);
    let stored = messages::list_messages(&wf.layout);
    // With no chat the engine can name, the hint follows the proof's
    // one-chat rule.
    if chat.is_none() && chats_since(&stored, spent_at).len() > 1 {
        return Vec::new();
    }
    let turn_of = |id: &str| {
        rows.iter().rfind(|row| {
            row.event == "HUMAN_TURN" && block_field(&row.block, "Message Id") == Some(id)
        })
    };
    stored
        .iter()
        .filter(|message| {
            if chat
                .as_deref()
                .is_some_and(|chat| message.session.as_deref() != Some(chat))
            {
                return false;
            }
            if spent_at.is_some_and(|spent_at| before(&message.at, spent_at)) {
                return false;
            }
            if !is_reply(&rows, message) {
                return false;
            }
            match turn_of(&message.id) {
                None => before(&message.at, &question.timestamp),
                Some(turn) if turn.shard_index == question.shard_index => turn.pos < question.pos,
                Some(turn) => before(&turn.timestamp, &question.timestamp),
            }
        })
        .cloned()
        .collect()
}

/// `lateQuestionHint`: what a refused answer says when the person's reply
/// arrived before its question was logged.
#[must_use]
pub fn late_question_hint(candidates: &[Stored]) -> String {
    let shown: Vec<String> = candidates
        .iter()
        .map(|message| {
            let words = message.words.as_deref().unwrap_or(&message.text);
            let words = trim(&re::replace(r"\s+", "g", words, " ")).to_owned();
            let cut = if len_utf16(&words) > 40 {
                format!("{}...", slice_utf16(&words, 0, 40))
            } else {
                words
            };
            format!("{} \"{cut}\"", message.id)
        })
        .collect();
    if let [only] = shown.as_slice() {
        return format!(
            "The person's message arrived before this question was logged: {only}. If it answers the question, record the answer with --message {}; do not ask them again.",
            candidates[0].id
        );
    }
    format!(
        "The person's messages arrived before this question was logged: {}. If one answers the question, record the answer with --message <id>, the id of the message it answers; do not ask them again.",
        shown.join(", ")
    )
}

/// `proveMessage`: the message behind `id`, or the sentence that refuses
/// it.
///
/// # Errors
///
/// No record has that id; the message carried no words of the person's;
/// it arrived before the last approval or decision; it came through
/// another chat, or the chat cannot be told and more than one has spoken.
pub fn prove(wf: &Workflow, id: &str, what: What) -> Result<Stored, String> {
    let what = what.word();
    let Some(message) = messages::read_message(&wf.layout, id) else {
        return Err(format!(
            "Cannot record this {what}: no message record {id} exists in this project. Name the id of the person's message that answered (a refused answer names it), or wait for their reply."
        ));
    };
    let rows = wf.events();
    if !is_reply(&rows, &message) {
        return Err(format!(
            "Cannot record this {what}: message {id} carried no words of the person's own (a command to Focus, or a message with no words), so it answers nothing. Name a message that carries their reply."
        ));
    }
    let spent_at = latest_spending(&rows);
    if let Some(spent_at) = spent_at
        && before(&message.at, spent_at)
    {
        return Err(format!(
            "Cannot record this {what}: message {id} arrived before the last approval or decision ({spent_at}), so it is spent. Only a message the person sent after it can answer this question."
        ));
    }
    if let Some(chat) = invoking_chat(wf) {
        if message.session.as_deref() != Some(chat.as_str()) {
            return Err(format!(
                "Cannot record this {what}: message {id} came through another chat, not this one. Name a message the person sent in this chat."
            ));
        }
        return Ok(message);
    }
    let stored = messages::list_messages(&wf.layout);
    let chats = chats_since(&stored, spent_at);
    if chats.len() > 1 {
        return Err(format!(
            "Cannot record this {what}: the engine cannot tell which chat this command serves, and more than one chat has sent messages since the last decision. Run it from the chat that asked, or wait for the person's reply there."
        ));
    }
    if chats.len() == 1 && !chats.contains(chat_of(&message)) {
        return Err(format!(
            "Cannot record this {what}: message {id} came through another chat, not the one that has spoken since the last decision."
        ));
    }
    Ok(message)
}

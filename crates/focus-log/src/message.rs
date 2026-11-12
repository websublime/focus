//! The proof of a message (`proveMessage` in `aidlc-log.ts`). A question
//! logged after the person already replied names the message that answered
//! it. The agent maps, the engine proves: that the record exists, carries
//! words of the person's own, was not spent by a later decision, and came
//! through the chat this command serves. The engine reads no words.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_hooks::messages::{self, Stored};
use focus_js::order::cmp_utf16;
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
    // The chat this command serves, when the engine can tell.
    let chat = records::invoking_session_id(&wf.layout, &wf.invoker)
        .ok()
        .flatten();
    if let Some(chat) = chat {
        if message.session.as_deref() != Some(chat.as_str()) {
            return Err(format!(
                "Cannot record this {what}: message {id} came through another chat, not this one. Name a message the person sent in this chat."
            ));
        }
        return Ok(message);
    }
    // The chats that have sent messages since the last spending row.
    let stored = messages::list_messages(&wf.layout);
    let chats: BTreeSet<&str> = stored
        .iter()
        .filter(|sent| spent_at.is_none_or(|spent_at| !before(&sent.at, spent_at)))
        .map(chat_of)
        .collect();
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

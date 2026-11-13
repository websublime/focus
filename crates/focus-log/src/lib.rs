//! `focus engine log`: the interactions of a piece of work, written to its
//! ledger as they happen. A question the agent asks is a `DECISION_RECORDED`
//! row and the person's reply a `QUESTION_ANSWERED` one; a pipeline's
//! handoffs and a stage's reviews have rows of their own. What a transition
//! later weighs (a summary confirmed, a review that is still fresh) is read
//! off these rows.
//!
//! Mirrors `aidlc-log.ts`. The verbs are ported one at a time; one that is
//! not there yet is refused by name.

pub mod answer;
pub mod answer_text;
pub mod answers;
pub mod decision;
pub mod flags;
pub mod message;
mod summary;

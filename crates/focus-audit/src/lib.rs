//! The audit ledger.
//!
//! Every decision a workflow takes is recorded as an entry in an append-only
//! Markdown file, a shard, under the intent's record. The ledger is the
//! engine's memory: guards read it as evidence that a person approved a gate,
//! the runtime graph is derived from it, and a workflow resumes from it. So
//! the format matters twice over: an entry must be impossible to forge from a
//! field value, and a reader must find it again exactly.
//!
//! This crate owns the format and nothing else yet:
//!
//! - [`events`]: the vocabulary, the 117 event types an entry may name;
//! - [`entry`]: what an entry is, what the ledger refuses, and the bytes a
//!   block is written as;
//! - [`read`]: how blocks and fields are found again;
//! - [`time`]: the timestamp an entry carries.
//!
//! Which shard an entry goes to, the lock a write takes and the `audit`
//! command come with the record resolution of the workspace. Ported from
//! `aidlc-audit.ts` and the readers it shares with `aidlc-lib.ts`, and held to
//! their answers by the oracle corpus under `fixtures/`.

pub mod entry;
pub mod events;
pub mod oracle;
pub mod read;
pub mod time;

pub use entry::{Entry, Fields, SHARD_TITLE, render_block, validate};
pub use focus_js::{EngineError, Result};

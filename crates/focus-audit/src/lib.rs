//! The audit ledger.
//!
//! Every decision a workflow takes is recorded as an entry in an append-only
//! Markdown file, a shard, under the intent's record. The ledger is the
//! engine's memory: guards read it as evidence that a person approved a gate,
//! the runtime graph is derived from it, and a workflow resumes from it. So
//! the format matters twice over: an entry must be impossible to forge from a
//! field value, and a reader must find it again exactly.
//!
//! This crate owns the format and where a record's ledger is on disk:
//!
//! - [`events`]: the vocabulary, the 117 event types an entry may name;
//! - [`entry`]: what an entry is, what the ledger refuses, and the bytes a
//!   block is written as;
//! - [`read`]: how blocks and fields are found again;
//! - [`time`]: the timestamp an entry carries;
//! - [`shards`]: several shards read as one ledger, with each event's place
//!   kept and a copied shard's rows read once;
//! - [`ledger`]: this clone's identity, the shard it writes, and every shard
//!   of a record read from disk;
//! - [`append`]: writing entries and notes to a shard, under the workspace
//!   lock and without trusting the path;
//! - [`history`]: a record's events and notes in one order, as a person or
//!   an agent reads them.
//!
//! Ported from `aidlc-audit.ts` and the readers it shares with
//! `aidlc-lib.ts`, and held to their answers by the oracle corpus and the
//! recorded differential under `fixtures/`.

pub mod append;
pub mod entry;
pub mod events;
pub mod history;
pub mod ledger;
pub mod memory;
pub mod oracle;
pub mod read;
pub mod runtime_graph;
pub mod shards;
pub mod summary;
pub mod time;
pub mod units;

pub use append::Note;
pub use entry::{Entry, Fields, SHARD_TITLE, render_block, validate};
pub use focus_js::{EngineError, Result};
pub use ledger::{CloneIdentity, Ledger, Machine};
pub use shards::{ShardEvent, ShardNote, ShardText};

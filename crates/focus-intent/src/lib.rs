//! Creating a piece of work.
//!
//! `intent-create` is where a workflow begins: a record directory is minted
//! under the space (`<YYMMDD>-<label>`), its ledger gets the rows of the
//! initialization stages, its state file is built from the scope's plan
//! (or the plan composed for it) and the workspace scan, and the record is
//! listed in the space's registry and selected. Everything is refused
//! before anything is written: a refused creation creates nothing.
//!
//! Mirrors `handleIntentCreate` and `handleIntentCreateStateBuild` in
//! `aidlc-utility.ts`, with `mintIntentRecord` and `registerIntentRecord`
//! in `aidlc-lib.ts`.

pub mod create;
pub mod names;
pub mod plan;
pub mod record;
pub mod template;

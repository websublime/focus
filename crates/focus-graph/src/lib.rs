//! The stage graph.
//!
//! The AI-DLC method describes a delivery lifecycle as stages (stations), grouped into
//! phases and selected by scopes (production lines). People author those as
//! Markdown files with frontmatter; the engine compiles them into two JSON
//! files and reads only those at runtime:
//!
//! - `stage-graph.json`: every stage, its dependency edges, the artifacts it
//!   produces and consumes, and the rules and sensors that apply to it;
//! - `scope-grid.json`: for each scope, which stages execute.
//!
//! This crate owns that compile ([`compile::compile`]), the schemas behind it,
//! and read-only queries over a compiled graph ([`query`]). It is a port of
//! upstream's `aidlc-graph.ts`, `aidlc-stage-schema.ts`, `aidlc-sensor-schema.ts`,
//! `aidlc-rule-schema.ts` and the loaders they use from `aidlc-lib.ts`, held to
//! byte parity with them by the oracle corpus under `fixtures/`.

pub mod agents;
pub mod ars;
pub mod compile;
pub mod composed;
pub mod export;
mod fsx;
pub mod guard_memory;
pub mod oracle;
pub mod proposal;
pub mod query;
pub mod rules;
pub mod runtime;
pub mod scopes;
pub mod sensors;
pub mod stage_schema;
pub mod vocabulary;
pub mod write;

pub use compile::{Compiled, GraphStage, Sources, compile};
pub use focus_js::{EngineError, Result};

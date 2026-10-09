//! Where things live in a project that runs Focus, and how they get written.
//!
//! A project has two trees the engine cares about:
//!
//! - the **harness tree** (`.claude`, `.cursor` or `.focus`): the engine's own
//!   files for one coding assistant, including the compiled data it reads;
//! - the **workspace** (`focus/`): what the team owns and commits, shared by
//!   every harness installed in the project.
//!
//! This crate answers "which project, which harness, which path"
//! ([`project`], [`Layout`]), reads the harness manifest ([`manifest`]), and
//! owns the two write disciplines everything else builds on: a write is
//! atomic ([`atomic`]), and a read-modify-write happens under the workspace
//! lock ([`lock`]).
//!
//! Upstream resolves all of this inside `aidlc-lib.ts` and
//! `aidlc-runtime-paths.ts`, mostly from the location of the running script.
//! Focus is one binary installed outside the project, so nothing here derives
//! from where the executable sits.

pub mod atomic;
pub mod env;
pub mod links;
pub mod lock;
pub mod manifest;
pub mod names;
pub mod project;

pub use lock::WorkspaceLock;
pub use project::Layout;

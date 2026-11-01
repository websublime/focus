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
//! ([`project`], [`Layout`]) and "which record" ([`records`]), reads the
//! harness manifest ([`manifest`]) and the settings files ([`settings`]), and
//! owns the disciplines everything else builds on: a write is atomic
//! ([`atomic`]), a read-modify-write happens under the workspace lock
//! ([`lock`]), and a file whose path is not trusted is opened without
//! following a link and checked to be what the path names ([`files`],
//! [`links`]).
//!
//! Upstream resolves all of this inside `aidlc-lib.ts` and
//! `aidlc-runtime-paths.ts`, mostly from the location of the running script.
//! Focus is one binary installed outside the project, so nothing here derives
//! from where the executable sits.

pub mod active_memory;
pub mod ancestry;
pub mod atomic;
pub mod env;
pub mod files;
pub mod links;
pub mod lock;
pub mod manifest;
pub mod names;
pub mod observer;
pub mod oracle;
pub mod project;
pub mod records;
pub mod scan;
pub mod settings;
pub mod terminal;

pub use lock::WorkspaceLock;
pub use project::Layout;

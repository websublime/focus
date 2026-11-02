//! The guards: Guard Policy, fences and the authority that covers an action.
//!
//! A guard is a check the engine runs before an agent's action goes through:
//! that code is not written before its plan was approved, that a reviewed
//! document is not edited after its review, that a stage is moved on only by
//! the engine's own steps. Each check is a *fence*, and whether a fence
//! holds is decided from three things read here:
//!
//! - the **Guard Policy** of the workflow ([`policy`]): `strict`, `relaxed`
//!   or `off`, from the intent's own state line, else the scope's default,
//!   with a team's memory files able to pin it;
//! - the **fences** themselves ([`fences`]): which ones the policy lowers,
//!   which a person switched off or on for this piece of work, and which an
//!   environment kill switch holds down;
//! - the **authority** behind the action ([`authority`]): a person's recent
//!   message, an instruction the engine has in force, or neither. It decides
//!   nothing about a fence, and is recorded with every decision.
//!
//! [`workflow`] reads the three for a project and answers one fence at a
//! time. The guards that act on the decision (the hooks of M4, the state
//! transitions of M3) are elsewhere; this crate only decides.
//!
//! Ported from `aidlc-lib.ts` and `aidlc-guard-fences.ts`, and held to
//! their answers by the oracle corpus.

pub mod authority;
pub mod engine_command;
pub mod fences;
pub mod integrity;
pub mod oracle;
pub mod policy;
pub mod reviewer_scope;
pub mod shell;
pub mod state_transition;
pub mod workflow;

pub use authority::{Authority, Decision, FenceDecision};
pub use fences::{Fence, FenceResolution, Setting};
pub use policy::{GuardPolicy, Resolution};

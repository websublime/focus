//! What a decision is weighed against.
//!
//! A gate, a review and a plan approval are each about the project's source
//! as it stood at one moment. This crate holds how that moment is written
//! down and read back: [`source`] is the identity of the source, as a
//! listing of every path with what it held.
//!
//! The crate sits below the lifecycle, the hooks and the orchestrator, which
//! all read the same evidence.

pub mod oracle;
pub mod source;

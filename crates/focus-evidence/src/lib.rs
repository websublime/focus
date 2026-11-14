//! What a decision is weighed against.
//!
//! A gate, a review and a plan approval are each about the project's source
//! as it stood at one moment. This crate holds how that moment is written
//! down and read back: [`source`] is the identity of the source, as a
//! listing of every path with what it held. A summary confirmation is about
//! what a person read in a questions file: [`summary`] reads their choice
//! off the file and takes the hash of what they confirmed. A review is about
//! the artifacts and the source a reviewer was sent to: [`review`] reads what
//! a request's row binds its review to.
//!
//! The crate sits below the lifecycle, the hooks and the orchestrator, which
//! all read the same evidence.

pub mod authorization;
pub mod oracle;
pub mod pipeline;
pub mod review;
pub mod review_record;
pub mod source;
pub mod summary;

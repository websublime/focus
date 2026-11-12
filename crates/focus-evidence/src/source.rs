//! The identity of the project's source.
//!
//! [`listing`] is the per-path record of a source tree and the text it is
//! kept as; [`walk`] reads one from the files, with the fingerprint of the
//! whole; [`raw`] remembers the hashes taken over raw line endings. The
//! snapshots a stage keeps of a listing are a later slice of the same
//! milestone.

pub mod listing;
pub mod raw;
pub mod walk;

//! The identity of the project's source.
//!
//! [`listing`] is the per-path record of a source tree and the text it is
//! kept as; [`walk`] reads one from the files, with the fingerprint of the
//! whole; [`git`] reads the commit an embedded repository is on; [`raw`]
//! remembers the hashes taken over raw line endings; [`snapshot`] keeps
//! the listing a stage starts from beside the record.

pub mod git;
pub mod listing;
pub mod raw;
pub mod snapshot;
pub mod walk;

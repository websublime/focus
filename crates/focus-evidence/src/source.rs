//! The identity of the project's source.
//!
//! [`listing`] is the per-path record of a source tree and the text it is
//! kept as. The walk that produces a listing from the files, and the
//! snapshots a stage keeps of one, are later slices of the same milestone.

pub mod listing;

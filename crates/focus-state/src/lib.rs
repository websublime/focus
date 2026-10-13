//! The state file of a workflow.
//!
//! Each intent record holds a `focus-state.md`: the workflow's scope and
//! status as bulleted fields, and under Stage Progress one checkbox line per
//! stage, with the action the approved plan gave it. The ledger says what
//! happened; the state file says where the workflow stands.
//!
//! Only the readers something already needs are here: the checkbox lines
//! ([`progress`]), which the runtime summary lays over the runtime graph once
//! a workflow is complete. Writing the file, the fields, and the transitions
//! between stages come with the workflow machines (M3).
//!
//! The crate sits above `focus-audit`: a workflow's moves are written to the
//! ledger, and the bulleted field reader the two share lives there.
//!
//! Ported from `aidlc-lib.ts`, and held to its answers by the oracle corpus.

pub mod oracle;
pub mod progress;

pub use progress::{
    Checkbox, CheckboxState, completed_overlay, parse_checkboxes, parse_stage_suffixes,
};

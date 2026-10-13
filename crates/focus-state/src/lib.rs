//! The state file of a workflow.
//!
//! Each intent record holds a `focus-state.md`: the workflow's scope and
//! status as bulleted fields, and under Stage Progress one checkbox line per
//! stage, with the action the approved plan gave it. The ledger says what
//! happened; the state file says where the workflow stands.
//!
//! What is here is the file as data: its bulleted fields ([`fields`]) and
//! its checkbox lines ([`progress`]), read and written as text, so that the
//! file stays byte for byte what upstream would have written. Reading and
//! writing the file on disk, and the transitions between stages, come with
//! the workflow machines (M3).
//!
//! The crate sits above `focus-audit`: a workflow's moves are written to the
//! ledger, and the bulleted field reader the two share lives there.
//!
//! Ported from `aidlc-lib.ts`, and held to its answers by the oracle corpus.

pub mod fields;
pub mod file;
pub mod oracle;
pub mod progress;

pub use fields::{get_field, set_field};
pub use file::{read_state_file, write_state_file};
pub use progress::{
    Checkbox, CheckboxState, completed_overlay, count_checkboxes, parse_checkboxes,
    parse_stage_suffixes, set_checkbox, set_stage_suffix,
};

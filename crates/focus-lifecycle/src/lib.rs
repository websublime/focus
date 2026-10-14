//! Moving a piece of work through its stages.
//!
//! The state command's transitions are the orchestrator's bookkeeping once a
//! decision is made: `advance` closes a stage and opens the next in-scope
//! one, `finalize` closes one and moves the cursor without opening the next,
//! `complete-workflow` closes the last, `skip` passes a stage (and, with
//! `--route`, moves on from it), and `resume` reads where the work stands.
//! Each one runs whole under the workspace lock, the ledger rows first and
//! the state file after them, so the two never disagree.
//!
//! Before a transition changes anything it passes the admission chain
//! ([`admission`]): the workflow is not archived, the stage's declared
//! artifacts exist, a code-producing stage has source work, and the
//! evidence the stage owes is there. The evidence that is an `aidlc-log`
//! record (a summary confirmation, a reviewer's receipt, a pipeline's
//! handoffs) is the attestation's (M5), so a transition that owes it is
//! refused by name until then; with the ceremony off, the review class
//! `none`, or the kill switches set, upstream owes none and the transition
//! runs whole.
//!
//! The gate verbs (`gate-start`, `approve`, `reject`, `revise`) are the next
//! slice: they read the person's reply and the sensors' verdicts.
//!
//! Mirrors `handleAdvance`, `handleFinalize`, `handleCompleteWorkflow`,
//! `handleSkip`, `handleResume` and `admitStageAction` in `aidlc-state.ts`.

pub mod admission;
pub mod artifacts;
pub mod events;
pub mod registry;
pub mod stage;
pub mod transitions;
pub mod validity;
pub mod workflow;

//! What a review request says back: the two commands of a review, the note
//! for the conductor, and the sentences more than one tool ends a refusal
//! with.
//!
//! `renderReviewRequestCommand` and `renderReviewVerdictCommand`
//! (`aidlc-lib.ts` L30 058): the request and its verdict, rendered once,
//! with the same scope and the project the request belongs to.
//! `reviewRequestNote` (`aidlc-directive.ts` L1 108),
//! `reviewRecoverySpentMessage` (`aidlc-log.ts` L2 779) and
//! `unitsBlockRepair` (`aidlc-lib.ts` L41 831).
//!
//! # What is not here yet
//!
//! The sentence of a spent recovery for an autonomous bolt, which names the
//! bolt to abort: the swarm gives that input, and Focus has none.

use focus_guards::guard_operation::{
    EngineInvocation, RenderMode, RenderShell, render_engine_invocation,
};
use focus_js::num;

use crate::workflow::Workflow;

/// What a review command names.
#[derive(Debug, Clone, Copy)]
pub struct ReviewCommand<'a> {
    pub project_dir: &'a str,
    pub stage: &'a str,
    pub reviewer: &'a str,
    pub unit: Option<&'a str>,
    pub single: bool,
    pub iteration: f64,
    /// Repeat the same pass's request (the request only).
    pub retry_pending: bool,
}

/// `renderReviewCommand`.
fn render(wf: &Workflow, input: &ReviewCommand<'_>, verdict: bool, shell: RenderShell) -> String {
    let mut args: Vec<String> = [
        "review",
        "--stage",
        input.stage,
        "--reviewer",
        input.reviewer,
    ]
    .map(str::to_owned)
    .to_vec();
    if let Some(unit) = input.unit.filter(|unit| !unit.is_empty()) {
        args.extend(["--unit".to_owned(), unit.to_owned()]);
    }
    if input.single {
        args.push("--single".to_owned());
    }
    args.extend(["--iteration".to_owned(), num::to_string(input.iteration)]);
    if input.retry_pending && !verdict {
        args.push("--retry-pending".to_owned());
    }
    if verdict {
        args.extend(["--verdict".to_owned(), "<READY|NOT-READY>".to_owned()]);
    }
    args.extend(["--project-dir".to_owned(), input.project_dir.to_owned()]);
    // (The harness directory is one of the install's own: the renderer has
    // nothing to refuse.)
    render_engine_invocation(
        &EngineInvocation {
            route: "log".to_owned(),
            args,
        },
        RenderMode::Native,
        wf.layout.harness.dir(),
        shell,
    )
    .unwrap_or_default()
}

/// `renderReviewRequestCommand`: the command that asks for a review pass.
#[must_use]
pub fn request_command(wf: &Workflow, input: &ReviewCommand<'_>, shell: RenderShell) -> String {
    render(wf, input, false, shell)
}

/// `renderReviewVerdictCommand`: the command that records the pass's
/// verdict, the verdict left for the conductor to fill in.
#[must_use]
pub fn verdict_command(wf: &Workflow, input: &ReviewCommand<'_>, shell: RenderShell) -> String {
    render(wf, input, true, shell)
}

/// `reviewRequestNote`: what the conductor does with a request it made.
#[must_use]
pub fn request_note(reviewer: &str, review_file: &str, record_verdict: &str) -> String {
    format!(
        "Dispatch {reviewer} as a subagent and have it write {review_file}: that file is the reviewer's, so never write it yourself and never stand in for it. When its verdict is back, record it with `{record_verdict}`."
    )
}

/// `reviewRecoverySpentMessage`: the one recovery review was used and the
/// output changed again.
#[must_use]
pub fn recovery_spent_message(
    stage: &str,
    guidance: Option<&str>,
    request_changes_reset_valid: bool,
) -> String {
    format!(
        "Cannot start another review for \"{stage}\": the one recovery review was already used, and this stage's output document changed again afterward. {}{}",
        guidance.unwrap_or_default(),
        if request_changes_reset_valid {
            " Only a human Request Changes decision resets the review attempt; do not record that rejection on the human's behalf."
        } else {
            ""
        }
    )
}

/// `unitsBlockRepair`: the units block cannot be read, and how it is
/// written again.
#[must_use]
pub fn units_block_repair(reason: &str, detail: &str) -> String {
    format!(
        "The units block in inception/units-generation/unit-of-work-dependency.md cannot be read ({reason}: {detail}), and Construction walks its Units from it. Write it from the Units in unit-of-work.md and the dependencies that file describes: one fenced yaml block that starts with `units:` and has, for each Unit, `- name: <unit>` and `depends_on: [<the Units it depends on>]` (`[]` for none), plus `kind: service|spec|ui|packaging|library` when its kind is known."
    )
}

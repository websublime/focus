//! The Plan Approval guard's pure decision (`aidlc-plan-approval-guard.ts`,
//! "The pure decision"): how a developer handoff is judged against the
//! approval evidence gathered for each Code Generation target, and the
//! refusals the guard prints. Everything here is side-effect free; the hook
//! around it (M4, slice 4e-ii) gathers the evidence from disk and wires the
//! harness payload, the state file and the exit code.
//!
//! The stage prose says generation never begins before the person answers
//! "Approve Plan". A handoff to the developer must name exactly one target
//! (`FOCUS-UNIT: <unit>` or `FOCUS-STAGE: code-generation`), that target
//! must have current approval evidence, and the handoff's one
//! `FOCUS-TESTING-CONTRACT` line must be the approved plan's. Missing,
//! conflicting, unknown and stale evidence block instead of guessing.
//!
//! Held to upstream's answers by the oracle corpus (`plan_dispatch`,
//! `plan_reason`).

use focus_js::re;
use focus_js::text::trim;

use crate::guard_operation::{RenderShell, quote_command_argument};
use crate::posture::prompt_testing_contract_markers;

/// The one stage this guard holds.
pub const GUARDED_STAGE: &str = "code-generation";
/// The one dispatch target it inspects.
pub const GUARDED_AGENT: &str = "focus-developer-agent";
/// The Unit field of a block row for the zero-Unit stage-level work.
pub const STAGE_TARGET: &str = "stage-level";
/// The subagent-dispatch tool names across harness payload shapes.
pub const DISPATCH_TOOLS: [&str; 2] = ["Task", "Agent"];
/// The engine's Plan Approval question is the active directive.
pub const PLAN_APPROVAL_ASK_OPEN: &str = "the engine is asking the person to approve the plan";
pub const NO_CURRENT_DIRECTIVE: &str =
    "the current state has no matching v2 code-generation active directive";
/// The engine asked the person how to recover and waits for the answer.
pub const ENGINE_QUESTION_OPEN: &str =
    "the engine is waiting for the person's answer to its recovery question";
/// `AS_ITS_OWN_COMMAND` (`aidlc-testing-posture.ts`).
pub const AS_ITS_OWN_COMMAND: &str = "exactly as written, as a command of its own (no `cd` before it, no pipe or second command after it)";

/// Per-target evidence the hook gathers from disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitEvidence {
    /// The Unit's name, or `None` for the zero-Unit stage-level work.
    pub unit: Option<String>,
    /// `code-generation-plan.md` exists and is non-empty.
    pub plan_exists: bool,
    /// `unit-test-instructions.md` exists and is non-empty.
    pub instructions_exist: bool,
    /// The Plan Approval question records an explicit "Approve Plan".
    pub approved: bool,
    /// The plan's Testing Contract matches the current posture.
    pub contract_valid: bool,
    /// The recorded fingerprint matches the plan, instructions and contract.
    pub fingerprint_valid: bool,
    pub receipt_valid: bool,
    /// The approved Testing Contract hash, binding the worker brief.
    pub contract_hash: Option<String>,
    /// The evaluator's own sentence when the receipt is not valid.
    pub reason: Option<String>,
    /// The plan's terminal `## Review` appendix, which the approval excludes.
    pub review_appendix: Option<String>,
}

/// What is wrong with a developer handoff itself: it names no target or
/// several, one this workflow does not build, or the wrong contract line
/// for an approved plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffDefect {
    Targets,
    UnknownTarget,
    Contract,
}

impl HandoffDefect {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Targets => "targets",
            Self::UnknownTarget => "unknown-target",
            Self::Contract => "contract",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "targets" => Some(Self::Targets),
            "unknown-target" => Some(Self::UnknownTarget),
            "contract" => Some(Self::Contract),
            _ => None,
        }
    }
}

/// The decision's verdict; `mentioned` carries the explicit marker values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanApprovalVerdict {
    pub block: bool,
    pub mentioned: Vec<String>,
    /// The handoff carried the plan's review appendix, bytes the approval
    /// excludes.
    pub appendix_in_brief: bool,
    pub handoff: Option<HandoffDefect>,
}

/// `approvalEvidenceIsCurrent`.
#[must_use]
pub fn approval_evidence_is_current(evidence: Option<&UnitEvidence>) -> bool {
    evidence.is_some_and(|evidence| {
        evidence.plan_exists
            && evidence.instructions_exist
            && evidence.approved
            && evidence.contract_valid
            && evidence.fingerprint_valid
            && evidence.receipt_valid
            && evidence.contract_hash.is_some()
    })
}

/// `normalizeStageName`: a state-file stage value for comparison, so a
/// display-cased `Code Generation` compares equal to its slug.
#[must_use]
pub fn normalize_stage_name(value: &str) -> String {
    re::replace(r"\s+", "g", &trim(value).to_lowercase(), "-")
}

fn markers(text: &str, pattern: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(marker) = re::find(pattern, "", line) {
            let value = trim(re::group(line, &marker, 1).unwrap_or("")).to_owned();
            if !value.is_empty() && !found.contains(&value) {
                found.push(value);
            }
        }
    }
    found
}

/// `promptUnitMarkers`: the distinct, non-empty `FOCUS-UNIT` values in
/// encounter order.
#[must_use]
pub fn prompt_unit_markers(text: &str) -> Vec<String> {
    markers(text, r"^[ \t]*FOCUS-UNIT[ \t]*:[ \t]*(.*?)[ \t]*$")
}

/// `promptStageMarkers`: the distinct, non-empty `FOCUS-STAGE` values,
/// normalised, in encounter order.
#[must_use]
pub fn prompt_stage_markers(text: &str) -> Vec<String> {
    let mut stages: Vec<String> = Vec::new();
    for stage in markers(text, r"^[ \t]*FOCUS-STAGE[ \t]*:[ \t]*(.*?)[ \t]*$") {
        let normalized = normalize_stage_name(&stage);
        if !stages.contains(&normalized) {
            stages.push(normalized);
        }
    }
    stages
}

/// `promptCarriesReviewAppendix`: whitespace-insensitive containment of a
/// non-trivial appendix in the prompt. The heading alone is not evidence;
/// the appendix's content lines are.
fn prompt_carries_review_appendix(prompt_text: &str, appendix: Option<&str>) -> bool {
    let Some(appendix) = appendix.filter(|text| !text.is_empty()) else {
        return false;
    };
    let fold = |text: &str| trim(&re::replace(r"\s+", "g", text, " ")).to_owned();
    let content = fold(&re::replace(
        r"^\s*##[ \t]*Review\b[^\n]*",
        "i",
        appendix,
        "",
    ));
    if content.is_empty() {
        return false;
    }
    fold(prompt_text).contains(&content)
}

/// `evaluatePlanApprovalDispatch`: blocks a developer dispatch for Code
/// Generation unless the prompt names exactly one known target with current
/// approval evidence and that target's contract line.
#[must_use]
pub fn evaluate_plan_approval_dispatch(
    tool_name: &str,
    subagent_type: &str,
    prompt_text: &str,
    current_stage: &str,
    units: &[UnitEvidence],
) -> PlanApprovalVerdict {
    if !DISPATCH_TOOLS.contains(&tool_name)
        || subagent_type != GUARDED_AGENT
        || normalize_stage_name(current_stage) != GUARDED_STAGE
    {
        return PlanApprovalVerdict::default();
    }
    let marked_units = prompt_unit_markers(prompt_text);
    let marked_stages = prompt_stage_markers(prompt_text);
    let mut mentioned = marked_units.clone();
    mentioned.extend(marked_stages.iter().map(|stage| format!("stage:{stage}")));
    if marked_units.len() + marked_stages.len() != 1 {
        return PlanApprovalVerdict {
            block: true,
            mentioned,
            appendix_in_brief: false,
            handoff: Some(HandoffDefect::Targets),
        };
    }
    let target = if marked_units.len() == 1 {
        units
            .iter()
            .find(|unit| unit.unit.as_deref() == Some(marked_units[0].as_str()))
    } else if marked_stages[0] == GUARDED_STAGE {
        units.iter().find(|unit| unit.unit.is_none())
    } else {
        None
    };
    let contract_markers = prompt_testing_contract_markers(prompt_text);
    let appendix_in_brief = target.is_some_and(|target| {
        prompt_carries_review_appendix(prompt_text, target.review_appendix.as_deref())
    });
    let approved = approval_evidence_is_current(target);
    let contract_matches = contract_markers.len() == 1
        && target.is_some_and(|target| {
            target.contract_hash.as_deref() == Some(contract_markers[0].as_str())
        });
    let handoff = if target.is_none() {
        Some(HandoffDefect::UnknownTarget)
    } else if approved && !contract_matches {
        Some(HandoffDefect::Contract)
    } else {
        None
    };
    PlanApprovalVerdict {
        block: target.is_none() || !approved || !contract_matches || appendix_in_brief,
        mentioned,
        appendix_in_brief,
        handoff,
    }
}

/// `nextOnItsOwn`: the `next` every refusal names, with how to run it.
#[must_use]
pub fn next_on_its_own() -> String {
    format!("`focus engine orchestrate next` {AS_ITS_OWN_COMMAND}")
}

/// The Code Generation targets the current step builds (`None`: the
/// zero-Unit stage-level work).
pub type BriefTargets = [Option<String>];

fn stage_marker() -> String {
    format!("stage:{GUARDED_STAGE}")
}

/// `briefCommand`: the `brief` that hands the developer one target and its
/// contract, for the targets the current step builds, else for the one
/// target the handoff names. Never a placeholder.
fn brief_command(mentioned: &[String], targets: Option<&BriefTargets>) -> String {
    let tool = "focus engine testing-posture";
    let named: Option<Vec<Option<String>>> = match targets {
        Some(targets) if !targets.is_empty() => Some(targets.to_vec()),
        _ if mentioned.len() == 1 => Some(vec![
            (mentioned[0] != stage_marker()).then(|| mentioned[0].clone()),
        ]),
        _ => None,
    };
    let Some(named) = named else {
        return format!(
            "`{tool} brief` for the target the current step names (`--unit` and its Unit, or `--stage-level` for zero-Unit work)"
        );
    };
    named
        .iter()
        .map(|unit| {
            format!(
                "`{tool} brief {}`",
                unit.as_ref().map_or_else(
                    || "--stage-level".to_owned(),
                    |unit| format!(
                        "--unit {}",
                        quote_command_argument(unit, RenderShell::Posix)
                    )
                )
            )
        })
        .collect::<Vec<_>>()
        .join(" or ")
}

/// The scope a refusal names for a mentioned target. Upstream reads the
/// first mentioned value whether or not there is one, so none reads as
/// JavaScript's `undefined`; the hook never asks for that.
fn scope_of(mentioned: Option<&str>) -> String {
    match mentioned {
        Some(mentioned) if mentioned == stage_marker() => {
            "the zero-Unit stage-level implementation".to_owned()
        }
        Some(mentioned) => format!("unit {mentioned}"),
        None => "unit undefined".to_owned(),
    }
}

/// `appendixBlockReason`.
#[must_use]
pub fn appendix_block_reason(mentioned: &[String]) -> String {
    let scope = scope_of(mentioned.first().map(String::as_str));
    format!(
        "Code generation cannot start for {scope} because the developer handoff carries the plan's terminal `## Review` appendix. That appendix is excluded from the approval fingerprint, so nobody approved it as work. Hand the developer the plan BODY and the unit-test instructions only: run {} and pass its output verbatim, then retry the handoff.",
        brief_command(mentioned, None)
    )
}

/// ` Reason: <detail>.` for a detail that says something; an empty one is
/// no detail.
fn reason_clause(detail: Option<&str>) -> String {
    detail
        .filter(|detail| !detail.is_empty())
        .map_or_else(String::new, |detail| {
            format!(" Reason: {}.", re::replace(r"\.+$", "", detail, ""))
        })
}

/// `blockReason`: the refusal handed back through the harness's error
/// channel, naming the missing evidence and the steps that produce it.
#[must_use]
pub fn block_reason(
    mentioned: &[String],
    detail: Option<&str>,
    targets: Option<&BriefTargets>,
) -> String {
    let scope = match mentioned.len() {
        1 => scope_of(Some(&mentioned[0])),
        0 => "one target, but the brief does not name it".to_owned(),
        _ => format!(
            "one target, but the brief names several ({})",
            mentioned.join(", ")
        ),
    };
    format!(
        "Code generation cannot start for {scope} because its plan and test instructions are not approved yet.{} Finish code-generation-plan.md and unit-test-instructions.md, then run {}: the engine asks the person to approve the plan, and the `next` after their answer hands over the build. Then hand the developer the output of {} first, as printed: it names the one target and its Testing Contract.",
        reason_clause(detail),
        next_on_its_own(),
        brief_command(mentioned, targets)
    )
}

/// `handoffBlockReason`: a handoff refused for what is wrong with it, so it
/// never says the plan is unapproved when it is approved.
#[must_use]
pub fn handoff_block_reason(
    mentioned: &[String],
    cause: HandoffDefect,
    targets: Option<&BriefTargets>,
) -> String {
    let what = match cause {
        HandoffDefect::Targets if mentioned.len() > 1 => {
            format!("names several targets ({})", mentioned.join(", "))
        }
        HandoffDefect::Targets => "names no target".to_owned(),
        HandoffDefect::UnknownTarget => format!(
            "names {}, which is not a Code Generation target of this workflow",
            mentioned.first().map_or("undefined", String::as_str)
        ),
        HandoffDefect::Contract => {
            "has an FOCUS-TESTING-CONTRACT line that is missing, repeated, or not the approved plan's"
                .to_owned()
        }
    };
    format!(
        "Code generation cannot start: the developer handoff {what}. Hand the developer the output of {} first, exactly as printed: it names the one target and its Testing Contract. Do not write FOCUS-UNIT, FOCUS-STAGE, or FOCUS-TESTING-CONTRACT lines yourself.",
        brief_command(mentioned, targets)
    )
}

/// `receiptDetail`: the evaluator's reason for the first mentioned target
/// whose receipt is not valid.
#[must_use]
pub fn receipt_detail(evidence: &[UnitEvidence], mentioned: &[String]) -> Option<String> {
    for name in mentioned {
        let unit = (*name != stage_marker()).then_some(name.as_str());
        if let Some(found) = evidence.iter().find(|entry| entry.unit.as_deref() == unit)
            && !found.receipt_valid
            && let Some(reason) = found.reason.as_deref().filter(|reason| !reason.is_empty())
        {
            return Some(reason.to_owned());
        }
    }
    None
}

/// `mutationBlockReason`: the refused path or command stays out of it.
#[must_use]
pub fn mutation_block_reason(
    unit: Option<&str>,
    opaque_shell: bool,
    detail: Option<&str>,
) -> String {
    let scope = unit.map_or_else(
        || "the zero-Unit stage-level implementation".to_owned(),
        |unit| format!("unit {unit}"),
    );
    let action = if opaque_shell {
        "run mutation-capable shell commands"
    } else {
        "modify workspace paths"
    };
    format!(
        "Code generation cannot {action} for {scope} because the plan, unit-test instructions, and current Testing Contract do not have a current matching approval.{} Writes inside the selected code-generation record directory remain available for planning. When the plan is ready, run {}: the engine asks the person to approve it before any code is written.",
        reason_clause(detail),
        next_on_its_own()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evidence(unit: Option<&str>, hash: Option<&str>) -> UnitEvidence {
        UnitEvidence {
            unit: unit.map(str::to_owned),
            plan_exists: true,
            instructions_exist: true,
            approved: true,
            contract_valid: true,
            fingerprint_valid: true,
            receipt_valid: true,
            contract_hash: hash.map(str::to_owned),
            reason: None,
            review_appendix: None,
        }
    }

    fn dispatch(prompt: &str, stage: &str, units: &[UnitEvidence]) -> PlanApprovalVerdict {
        evaluate_plan_approval_dispatch("Task", GUARDED_AGENT, prompt, stage, units)
    }

    #[test]
    fn markers_are_distinct_and_normalised() {
        assert_eq!(
            prompt_unit_markers("FOCUS-UNIT: a\n  FOCUS-UNIT : b \nFOCUS-UNIT: a\nFOCUS-UNIT:\n"),
            ["a", "b"]
        );
        assert_eq!(
            prompt_stage_markers("FOCUS-STAGE: Code  Generation\r\nFOCUS-STAGE: code-generation\n"),
            ["code-generation"]
        );
        assert_eq!(
            normalize_stage_name("  Code Generation "),
            "code-generation"
        );
    }

    #[test]
    fn a_handoff_names_one_approved_target_and_its_contract() {
        let hash = format!("sha256:{}", "a".repeat(64));
        let units = [
            evidence(None, Some(&hash)),
            evidence(Some("u"), Some(&hash)),
        ];
        let good = format!("FOCUS-UNIT: u\nFOCUS-TESTING-CONTRACT: {hash}\n");
        let verdict = dispatch(&good, "Code Generation", &units);
        assert!(!verdict.block);
        assert_eq!(verdict.mentioned, ["u"]);
        assert!(
            !evaluate_plan_approval_dispatch("Task", "other", &good, GUARDED_STAGE, &units).block
        );
        assert!(
            !evaluate_plan_approval_dispatch("Read", GUARDED_AGENT, &good, GUARDED_STAGE, &units)
                .block
        );
        assert!(!dispatch(&good, "build", &units).block);
        let none = dispatch("hi", GUARDED_STAGE, &units);
        assert!(none.block && none.mentioned.is_empty());
        assert_eq!(none.handoff, Some(HandoffDefect::Targets));
        let two = evaluate_plan_approval_dispatch(
            "Agent",
            GUARDED_AGENT,
            "FOCUS-UNIT: u\nFOCUS-STAGE: code-generation\n",
            GUARDED_STAGE,
            &units,
        );
        assert_eq!(two.mentioned, ["u", "stage:code-generation"]);
        assert_eq!(two.handoff, Some(HandoffDefect::Targets));
        assert_eq!(
            dispatch("FOCUS-UNIT: v\n", GUARDED_STAGE, &units).handoff,
            Some(HandoffDefect::UnknownTarget)
        );
        assert_eq!(
            dispatch("FOCUS-STAGE: build\n", GUARDED_STAGE, &units).handoff,
            Some(HandoffDefect::UnknownTarget)
        );
        assert_eq!(
            dispatch("FOCUS-UNIT: u\n", GUARDED_STAGE, &units).handoff,
            Some(HandoffDefect::Contract)
        );
        let mut unapproved = units.clone();
        unapproved[1].approved = false;
        let verdict = dispatch(&good, GUARDED_STAGE, &unapproved);
        assert!(verdict.block && verdict.handoff.is_none());
        let mut appended = units.clone();
        appended[1].review_appendix = Some("## Review\n\nLooks  fine\n".into());
        let quoting = format!("{good}The review said: Looks fine.\n");
        let verdict = dispatch(&quoting, GUARDED_STAGE, &appended);
        assert!(verdict.block && verdict.appendix_in_brief);
        appended[1].review_appendix = Some("## Review\n".into());
        assert!(!dispatch(&quoting, GUARDED_STAGE, &appended).block);
    }

    #[test]
    fn the_refusals_name_the_brief_for_the_target() {
        let stage = vec!["stage:code-generation".to_owned()];
        assert!(
            block_reason(&stage, None, None)
                .contains("`focus engine testing-posture brief --stage-level`")
        );
        assert!(block_reason(&[], None, None).contains("the brief does not name it"));
        assert!(
            block_reason(&["a".into(), "b".into()], Some("stale."), None)
                .contains(" Reason: stale. Finish")
        );
        assert!(!block_reason(&stage, Some(""), None).contains("Reason"));
        assert!(
            block_reason(&["a".into()], None, Some(&[Some("x y".into()), None])).contains(
                "`focus engine testing-posture brief --unit 'x y'` or `focus engine testing-posture brief --stage-level`"
            )
        );
        assert!(
            handoff_block_reason(&[], HandoffDefect::Targets, None).contains("names no target")
        );
        assert!(
            handoff_block_reason(&["v".into()], HandoffDefect::UnknownTarget, None)
                .contains("names v, which is not")
        );
        assert!(
            appendix_block_reason(&["u".into()])
                .starts_with("Code generation cannot start for unit u because")
        );
        assert!(
            mutation_block_reason(None, true, None)
                .contains("cannot run mutation-capable shell commands for the zero-Unit")
        );
        let mut stale = evidence(Some("u"), None);
        stale.receipt_valid = false;
        stale.reason = Some("moved".into());
        assert_eq!(
            receipt_detail(std::slice::from_ref(&stale), &["u".into()]).as_deref(),
            Some("moved")
        );
        assert!(receipt_detail(&[stale], &stage).is_none());
    }
}

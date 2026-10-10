//! The event vocabulary.
//!
//! Every row of the ledger names one of these event types. The list and its
//! order are upstream's (`VALID_EVENT_TYPES` in `aidlc-audit.ts`): the order
//! is part of the refusal a caller gets for an unknown type, which lists them
//! all. Each type has the heading its block is written under
//! (`EVENT_HEADINGS`).

/// Every event type with the heading of its block, in upstream's order.
pub const EVENTS: [(&str, &str); 117] = [
    ("STAGE_STARTED", "Stage Start"),
    ("STAGE_AWAITING_APPROVAL", "Stage Awaiting Approval"),
    ("STAGE_REVISING", "Stage Revising"),
    ("STAGE_COMPLETED", "Stage Completion"),
    ("STAGE_JUMPED", "Stage Jump"),
    ("STAGE_SKIPPED", "Stage Skip"),
    ("PHASE_STARTED", "Phase Start"),
    ("PHASE_COMPLETED", "Phase Completion"),
    ("PHASE_VERIFIED", "Phase Verification"),
    ("PHASE_SKIPPED", "Phase Skip"),
    ("WORKFLOW_STARTED", "Workflow Start"),
    ("WORKFLOW_COMPLETED", "Workflow Completion"),
    ("WORKFLOW_PARKED", "Workflow Parked"),
    ("WORKFLOW_UNPARKED", "Workflow Unparked"),
    ("WORKFLOW_ARCHIVED", "Workflow Archived"),
    ("WORKFLOW_UNARCHIVED", "Workflow Unarchived"),
    ("INTENT_REPOS_CHANGED", "Intent Repos Changed"),
    ("SESSION_STARTED", "Session Start"),
    ("SESSION_RESUMED", "Session Resume"),
    ("SESSION_COMPACTED", "Session Compacted"),
    ("SESSION_ENDED", "Session End"),
    ("HUMAN_TURN", "Human Turn"),
    ("HOST_TURN", "Host Turn"),
    ("WORKSPACE_SCAFFOLDED", "Workspace Scaffolded"),
    ("WORKSPACE_SCANNED", "Workspace Scanned"),
    ("WORKSPACE_INITIALISED", "Workspace Initialised"),
    ("DECISION_RECORDED", "Decision Recorded"),
    ("GATE_APPROVED", "Gate Approved"),
    ("GATE_REJECTED", "Gate Rejected"),
    ("QUESTION_ANSWERED", "Question Answered"),
    ("QUESTION_UNANSWERED", "Question Unanswered"),
    ("QUESTION_REPLIED", "Question Replied"),
    ("REQUEST_ROUTED", "Request Routed"),
    (
        "SUMMARY_CONFIRMATION_RECORDED",
        "Summary Confirmation Recorded",
    ),
    (
        "VERIFICATION_COMMAND_RECORDED",
        "Verification Command Recorded",
    ),
    (
        "CONSTRUCTION_POLICY_RECORDED",
        "Construction Policy Recorded",
    ),
    (
        "CHECKPOINT_VERIFICATION_RECORDED",
        "Checkpoint Verification Recorded",
    ),
    ("PLAN_APPROVAL_RECORDED", "Plan Approval Recorded"),
    ("PLAN_APPROVAL_OVERRIDDEN", "Plan Approval Overridden"),
    ("PLAN_APPROVAL_SKIPPED", "Plan Approval Skipped"),
    ("REVIEW_REQUESTED", "Review Requested"),
    ("REVIEW_COMPLETED", "Review Completed"),
    ("PIPELINE_LINK_COMPLETED", "Pipeline Link Completed"),
    ("UNIT_STARTED", "Unit Started"),
    ("UNIT_PAUSED", "Unit Paused"),
    ("UNIT_RESUMED", "Unit Resumed"),
    ("UNIT_COMPLETED", "Unit Completed"),
    ("UNIT_SKIPPED", "Unit Skipped"),
    ("ARTIFACT_CREATED", "Artifact Created"),
    ("ARTIFACT_UPDATED", "Artifact Updated"),
    ("ARTIFACT_REUSED", "Artifact Reused"),
    ("SUBAGENT_COMPLETED", "Subagent Completed"),
    ("SUBAGENT_PROMPT_UNMATCHED", "Subagent Prompt Unmatched"),
    ("REVIEWER_SCOPE_BLOCKED", "Reviewer Scope Blocked"),
    ("REVIEW_FREEZE_BLOCKED", "Review Freeze Blocked"),
    ("PLAN_APPROVAL_BLOCKED", "Plan Approval Blocked"),
    ("GUARD_DISABLED", "Guard Disabled"),
    ("DOCUMENT_INDEXED", "Document Indexed"),
    ("DOCUMENT_UPDATED", "Document Updated"),
    ("DOCUMENT_REMOVED", "Document Removed"),
    ("HEALTH_CHECKED", "Health Check"),
    ("SCOPE_DETECTED", "Scope Detection"),
    ("SCOPE_CHANGED", "Scope Change"),
    ("PLUGIN_SELECTION_CHANGED", "Plugin Selection Change"),
    ("DEPTH_CHANGED", "Depth Change"),
    ("TEST_STRATEGY_CHANGED", "Test Strategy Change"),
    ("REVIEW_CLASS_CHANGED", "Review Class Change"),
    ("GUARD_POLICY_SET", "Guard Policy Set"),
    ("CHANGE_CONTROL_SET", "Change Control Set"),
    ("CHANGE_ACCEPTED", "Change Accepted"),
    ("GUARD_RESTORED", "Guard Restored"),
    ("GUARD_STOOD_ASIDE", "Guard Stood Aside"),
    ("CEREMONY_SET", "Ceremony Set"),
    ("RECOMPOSED", "Plan Recomposed"),
    ("WORKSPACE_RECLASSIFIED", "Workspace Reclassified"),
    ("SCOPE_SAVED", "Scope Saved"),
    ("ERROR_LOGGED", "Error Logged"),
    ("RECOVERY_COMPLETED", "Recovery Completed"),
    ("COORDINATION_STOOD_ASIDE", "Coordination Stood Aside"),
    ("BOLT_STARTED", "Bolt Started"),
    ("BOLT_COMPLETED", "Bolt Completed"),
    ("BOLT_FAILED", "Bolt Failed"),
    ("AUTONOMY_MODE_SET", "Autonomy Mode Set"),
    ("UNIT_OWNERSHIP_SET", "Unit Ownership Set"),
    ("UNIT_GATE_RHYTHM_SET", "Unit Gate Rhythm Set"),
    ("CONSTRUCTION_POLICY_SET", "Construction Policy Set"),
    ("UNIT_MERGED", "Unit Merged"),
    ("WORKTREE_CREATED", "Worktree Created"),
    ("WORKTREE_MERGED", "Worktree Merged"),
    ("WORKTREE_DISCARDED", "Worktree Discarded"),
    ("STATE_FORKED", "State Forked"),
    ("STATE_MERGED", "State Merged"),
    ("AUDIT_FORKED", "Audit Forked"),
    ("AUDIT_MERGED", "Audit Merged"),
    ("PRACTICES_DISCOVERED", "Practices Discovered"),
    ("PRACTICES_AFFIRMED", "Practices Affirmed"),
    ("PRACTICES_OVERRIDE", "Practices Override"),
    ("PRACTICES_SECTION_EMPTY", "Practices Section Empty"),
    ("MERGE_DISPATCH_INVOKED", "Merge Dispatch Invoked"),
    ("MERGE_DISPATCH_RETURNED", "Merge Dispatch Returned"),
    ("MERGE_DISPATCH_FALLBACK", "Merge Dispatch Fallback"),
    ("SENSOR_FIRED", "Sensor Fired"),
    ("SENSOR_PASSED", "Sensor Passed"),
    ("SENSOR_FAILED", "Sensor Failed"),
    ("SENSOR_BUDGET_OVERRIDE", "Sensor Budget Override"),
    ("GUARDRAIL_LOADED", "Guardrail Loaded"),
    ("MEMORY_EMPTY", "Memory Empty"),
    ("RULE_LEARNED", "Rule Learned"),
    ("SENSOR_PROPOSED", "Sensor Proposed"),
    ("SWARM_STARTED", "Swarm Started"),
    ("SWARM_UNIT_CONVERGED", "Swarm Unit Converged"),
    ("SWARM_SOURCE_MERGED", "Swarm Source Merged"),
    ("SWARM_UNIT_FAILED", "Swarm Unit Failed"),
    ("SWARM_BATON_RETURNED", "Swarm Baton Returned"),
    ("SWARM_COMPLETED", "Swarm Completed"),
    ("SWARM_DEGRADED", "Swarm Degraded"),
    ("SOURCE_COMMITTED", "Source Committed"),
];

/// Event types only their owning hook or tool may write. The `audit` command
/// refuses them outright, whatever the environment says: each one is either a
/// person's own act (a turn, a reply) or a receipt a tool derives from work it
/// verified.
///
/// Upstream's `CLI_RESERVED_EVENT_TYPES`.
pub const RESERVED_FOR_OWNERS: [&str; 17] = [
    "HUMAN_TURN",
    "HOST_TURN",
    "QUESTION_UNANSWERED",
    "QUESTION_REPLIED",
    "SUMMARY_CONFIRMATION_RECORDED",
    "VERIFICATION_COMMAND_RECORDED",
    "CONSTRUCTION_POLICY_RECORDED",
    "CHECKPOINT_VERIFICATION_RECORDED",
    "PLAN_APPROVAL_RECORDED",
    "PLAN_APPROVAL_OVERRIDDEN",
    "PLAN_APPROVAL_SKIPPED",
    "ARTIFACT_CREATED",
    "ARTIFACT_UPDATED",
    "ARTIFACT_REUSED",
    "REVIEW_REQUESTED",
    "REVIEW_COMPLETED",
    "PIPELINE_LINK_COMPLETED",
];

/// Event types that carry authority: gate resolutions, approvals, presence,
/// unit and bolt lifecycle, provenance. Each has one owning emitter inside
/// the engine, so the `audit` command refuses them and a conductor cannot
/// mint authority it does not have; everything else stays appendable as the
/// diagnostic escape hatch. `FOCUS_ALLOW_DIRECT_AUDIT_EVENTS=1` lifts the
/// refusal, for fixtures that stand in for the owning emitters.
///
/// Upstream's `CLI_PROTECTED_EVENT_TYPES`.
pub const PROTECTED_FROM_THE_COMMAND: [&str; 49] = [
    "STAGE_COMPLETED",
    "HUMAN_TURN",
    "HOST_TURN",
    "QUESTION_UNANSWERED",
    "QUESTION_REPLIED",
    "REQUEST_ROUTED",
    "GATE_APPROVED",
    "GATE_REJECTED",
    "QUESTION_ANSWERED",
    "PLAN_APPROVAL_RECORDED",
    "PLAN_APPROVAL_OVERRIDDEN",
    "PLAN_APPROVAL_SKIPPED",
    "REVIEW_REQUESTED",
    "REVIEW_COMPLETED",
    "PIPELINE_LINK_COMPLETED",
    "ARTIFACT_REUSED",
    "SWARM_STARTED",
    "SWARM_UNIT_CONVERGED",
    "SWARM_SOURCE_MERGED",
    "AUTONOMY_MODE_SET",
    "UNIT_OWNERSHIP_SET",
    "UNIT_GATE_RHYTHM_SET",
    "CONSTRUCTION_POLICY_SET",
    "UNIT_STARTED",
    "UNIT_PAUSED",
    "UNIT_RESUMED",
    "UNIT_COMPLETED",
    "UNIT_SKIPPED",
    "UNIT_MERGED",
    "DOCUMENT_INDEXED",
    "DOCUMENT_UPDATED",
    "DOCUMENT_REMOVED",
    "BOLT_STARTED",
    "BOLT_COMPLETED",
    "BOLT_FAILED",
    "AUDIT_FORKED",
    "AUDIT_MERGED",
    "STATE_FORKED",
    "STATE_MERGED",
    "WORKTREE_CREATED",
    "WORKTREE_MERGED",
    "WORKTREE_DISCARDED",
    "SOURCE_COMMITTED",
    "GUARD_POLICY_SET",
    "CHANGE_CONTROL_SET",
    "CHANGE_ACCEPTED",
    "GUARD_RESTORED",
    "GUARD_STOOD_ASIDE",
    "CEREMONY_SET",
];

/// The heading a block of this event type is written under; `None` when the
/// type is not one the ledger accepts.
#[must_use]
pub fn heading(event_type: &str) -> Option<&'static str> {
    EVENTS
        .iter()
        .find(|(name, _)| *name == event_type)
        .map(|(_, heading)| *heading)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_has_no_repeats_and_every_type_has_a_heading() {
        for (index, (name, heading)) in EVENTS.iter().enumerate() {
            assert!(!heading.is_empty(), "{name}");
            assert!(
                EVENTS[..index].iter().all(|(earlier, _)| earlier != name),
                "{name} is listed twice"
            );
        }
        assert_eq!(heading("STAGE_STARTED"), Some("Stage Start"));
        assert_eq!(heading("stage_started"), None);
    }
}

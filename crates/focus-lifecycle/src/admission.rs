//! The guard chain a lifecycle action passes before it changes anything.
//!
//! Listed once, so the enforcing transition and the router that previews
//! it (M5) see the same refusal: the workflow is not archived; the stage's
//! declared artifacts exist, and a code-producing stage left source work;
//! the stage's summary confirmation, its reviewer's receipt and its
//! pipeline's handoffs are recorded when owed; and its Construction
//! checkpoints are approved.
//!
//! The evidence checks read `aidlc-log` records and compare the source
//! the review saw with the source there now (the attestation, M5). Focus
//! does not weigh that evidence yet: a transition that owes a confirmation,
//! a receipt or a handoff is refused by name, and one that owes none (the
//! ceremony is off, the review class resolves to `none`, the stage has no
//! reviewer or is not a pipeline, or the kill switches are set) passes as
//! upstream passes it. A Construction stage is refused by name as well: its
//! checkpoints, units and swarms are M4.
//!
//! A guard refusal names its code and the step that clears it. Upstream
//! appends the guard-recovery ask (the remedies the conductor may offer)
//! as a JSON line after the sentence and keeps a refusal record under the
//! engine directory; both are the recovery machinery of M5, so Focus prints
//! the sentence alone.
//!
//! Mirrors `admitStageAction`, `verifyGateOpeningGuards`,
//! `verifyStageArtifacts`, `verifySummaryConfirmationPrecondition`,
//! `verifyReviewerPrecondition`, `verifyPipelineLinkPrecondition`,
//! `verifyConstructionCheckpointPrecondition`, `artifactGuardDisabled`
//! and `refuseStateGuard` in `aidlc-state.ts`, and `summaryConfirmationOwed`,
//! `resolveReviewClass`, `resolveCeremony` and `reviewerGateGuardDisabled`
//! in `aidlc-lib.ts`.

use focus_graph::runtime::Stage;
use focus_js::re;
use focus_js::text::trim;
use focus_js::{EngineError, Result, bail};
use focus_state::fields::get_field;
use focus_state::progress::{CheckboxState, parse_checkboxes};
use focus_workspace::settings::Places;
use focus_workspace::{Layout, env};

use crate::artifacts::{produces_artifacts_exist, workspace_has_work};
use crate::stage;
use crate::workflow::Workflow;

/// What the action is: closing the stage, or opening its approval gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Complete,
    PresentApprovalGate,
}

impl Action {
    /// `reviewerPreconditionPrefix`.
    fn prefix(self, slug: &str) -> String {
        match self {
            Self::Complete => format!("Cannot complete \"{slug}\""),
            Self::PresentApprovalGate => format!("Cannot present \"{slug}\" for approval"),
        }
    }
}

/// `resolveProjectFlag`: the environment, else the settings files.
fn project_flag(layout: &Layout, name: &str) -> Option<String> {
    Places::of(&layout.project)
        .flag(&env::full_name(name), &|variable| {
            std::env::var(variable).ok()
        })
        .ok()
        .flatten()
}

/// `artifactGuardDisabled`.
#[must_use]
pub fn artifact_guard_disabled(layout: &Layout) -> bool {
    project_flag(layout, "SKIP_ARTIFACT_GUARD").as_deref() == Some("1")
}

/// `reviewerGateGuardDisabled`: the environment alone.
#[must_use]
pub fn reviewer_gate_guard_disabled() -> bool {
    env::var("SKIP_REVIEWER_GATE_GUARD").as_deref() == Some("1")
}

/// `assertWorkflowNotArchived`.
///
/// # Errors
///
/// The state's `Status` is `Archived`.
pub fn assert_not_archived(state: &str, operation: &str) -> Result<()> {
    if get_field(state, "Status") == Some("Archived") {
        bail!(
            "Workflow is Archived, so {operation} is refused. Bring it back first with `/focus intent unarchive <name>`."
        );
    }
    Ok(())
}

/// A guard refusal: the sentence upstream prints first. Its reason code
/// (`REQUIRED_ARTIFACTS_MISSING`, …) goes on the ask upstream prints after
/// it, which is M5's.
fn guard_refusal(message: String) -> EngineError {
    EngineError::new(message)
}

/// `verifyStageArtifacts`: the declared artifacts exist, and a
/// code-producing stage has source work.
///
/// # Errors
///
/// `REQUIRED_ARTIFACTS_MISSING` or `REQUIRED_SOURCE_WORK_MISSING`.
pub fn verify_stage_artifacts(wf: &Workflow, stage: &Stage, action: Action) -> Result<()> {
    if artifact_guard_disabled(&wf.layout) {
        return Ok(());
    }
    if !produces_artifacts_exist(wf, stage)? {
        // REQUIRED_ARTIFACTS_MISSING
        return Err(guard_refusal(format!(
            "{}: none of its declared artifacts exist under the intent's record directory. The stage protocol requires {} to produce output before the gate. Produce the artifacts before completing. (declared: {})",
            action.prefix(&stage.slug),
            stage::name(stage),
            if stage.produces.is_empty() {
                "none".to_owned()
            } else {
                stage.produces.join(", ")
            }
        )));
    }
    if stage::workspace_requires(stage) && !workspace_has_work(wf) {
        // REQUIRED_SOURCE_WORK_MISSING
        return Err(guard_refusal(format!(
            "{}: it is a code-producing stage (workspace_requires) but no source work is evident outside the focus/ workspace tree. In a git workspace this means no uncommitted change and no code in the last commit; otherwise no source file exists. Planning docs alone do not satisfy {} - write the code to the workspace (a source change not committed yet counts, as does code in the last commit), or the person can choose Request Changes and say what is missing.",
            action.prefix(&stage.slug),
            stage::name(stage)
        )));
    }
    Ok(())
}

/// `parseCeremonyStateLine`: `on` or `off`, read off a state line.
fn ceremony_state_value(raw: Option<&str>) -> Option<&'static str> {
    let raw = trim(raw?);
    let found = re::find(r"^(on|off)\b(?:\s*\((.*)\))?\s*$", "i", raw)?;
    match re::group(raw, &found, 1)?.to_lowercase().as_str() {
        "on" => Some("on"),
        _ => Some("off"),
    }
}

/// `resolveCeremony(...).value` for `summary_confirmation`: the kill
/// switch, else the state's line, else the scope's setting, else on.
#[must_use]
pub fn summary_confirmation_setting(wf: &Workflow, state: &str) -> &'static str {
    if project_flag(&wf.layout, "DISABLE_SUMMARY_CONFIRMATION").as_deref() == Some("1") {
        return "off";
    }
    if let Some(value) = ceremony_state_value(get_field(state, "Summary Confirmation")) {
        return value;
    }
    let scope = get_field(state, "Scope")
        .map(|scope| trim(scope).to_lowercase())
        .unwrap_or_default();
    match wf.install.definition(&scope) {
        Some(definition) if definition.meta.ceremony_setting("summary_confirmation") == "off" => {
            "off"
        }
        _ => "on",
    }
}

/// `summaryConfirmationOwed`: whether the stage owes a summary confirmation.
#[must_use]
pub fn summary_confirmation_owed(wf: &Workflow, stage: &Stage, state: &str) -> bool {
    if project_flag(&wf.layout, "SKIP_SUMMARY_CONFIRMATION_GUARD").as_deref() == Some("1") {
        return false;
    }
    if summary_confirmation_setting(wf, state) == "off" {
        return false;
    }
    if stage.phase == "initialization" {
        return false;
    }
    if stage.phase == "construction" && is_autonomous_mode(state) {
        return false;
    }
    stage::summary_confirmation(stage).is_some()
}

/// `isAutonomousMode`.
fn is_autonomous_mode(state: &str) -> bool {
    get_field(state, "Construction Autonomy Mode").map(trim) == Some("autonomous")
}

/// `verifySummaryConfirmationPrecondition`: refused by name when owed.
///
/// # Errors
///
/// The stage owes a summary confirmation, which Focus cannot read yet.
pub fn verify_summary_confirmation(wf: &Workflow, stage: &Stage, state: &str) -> Result<()> {
    if !summary_confirmation_owed(wf, stage, state) {
        return Ok(());
    }
    bail!(
        "\"{}\" owes a summary confirmation (the Summary Confirmation ceremony is on for this piece of work), which is recorded with `focus engine log` and compared with the stage's outputs: Focus does not read that record yet. Turn the ceremony off for this piece of work (`--summary-confirmation off` at creation, or {}=1) to run the transition whole.",
        stage.slug,
        env::full_name("SKIP_SUMMARY_CONFIRMATION_GUARD")
    )
}

/// `resolveReviewClass`: the stage's class, capped by the state's
/// `Review Override` or the scope's `review_cap`.
#[must_use]
pub fn resolve_review_class(wf: &Workflow, stage: &Stage, state: &str) -> &'static str {
    let as_class = |value: Option<&str>| -> Option<&'static str> {
        match value? {
            "none" => Some("none"),
            "advisory" => Some("advisory"),
            "adversarial" => Some("adversarial"),
            _ => None,
        }
    };
    let rank = |class: &str| match class {
        "none" => 0,
        "advisory" => 1,
        _ => 2,
    };
    let Some(declared) = as_class(Some(stage::review_class(stage).unwrap_or("adversarial"))) else {
        return "none";
    };
    let scope = get_field(state, "Scope").unwrap_or("");
    let cap = as_class(get_field(state, "Review Override")).or_else(|| {
        as_class(
            wf.install
                .definition(scope)
                .and_then(|definition| definition.meta.review_cap.as_deref()),
        )
    });
    match cap {
        Some(cap) if rank(cap) < rank(declared) => cap,
        _ => declared,
    }
}

/// Whether the ledger holds a review row of the current workflow, which
/// Focus cannot weigh.
fn has_review_rows(wf: &Workflow) -> bool {
    wf.events()
        .iter()
        .any(|row| row.event.starts_with("REVIEW_"))
}

/// `verifyReviewerPrecondition`: nothing to enforce without a reviewer or
/// with the class `none`; otherwise refused by name when a receipt is
/// owed, or when the ledger holds review rows an already-closed stage's
/// recovery would weigh.
///
/// # Errors
///
/// The stage owes a review receipt, or has review rows to weigh.
pub fn verify_reviewer(
    wf: &Workflow,
    stage: &Stage,
    state: &str,
    require_receipt: bool,
) -> Result<()> {
    let Some(reviewer) = stage::reviewer(stage) else {
        return Ok(());
    };
    if resolve_review_class(wf, stage, state) == "none" {
        return Ok(());
    }
    if has_review_rows(wf) {
        bail!(
            "\"{}\" has review rows in its ledger, which name what {reviewer} saw and when: Focus does not weigh review receipts yet.",
            stage.slug
        );
    }
    if require_receipt {
        bail!(
            "\"{}\" owes a review receipt from {reviewer} (its review class resolves to {}), which is recorded with `focus engine log review` and compared with the stage's outputs: Focus does not read that record yet. Create the piece of work with `--review none` to run the transition whole.",
            stage.slug,
            resolve_review_class(wf, stage, state)
        );
    }
    Ok(())
}

/// `verifyPipelineLinkPrecondition`: a pipeline stage owes its handoffs.
///
/// # Errors
///
/// The stage is a pipeline and the evidence is not disabled.
pub fn verify_pipeline_link(wf: &Workflow, stage: &Stage) -> Result<()> {
    if stage::mode(stage) != Some("pipeline")
        || project_flag(&wf.layout, "DISABLE_ENSEMBLE_EVIDENCE").as_deref() == Some("1")
    {
        return Ok(());
    }
    bail!(
        "\"{}\" is a pipeline stage and owes the handoffs recorded with `focus engine log link` for the current run: Focus does not read those records yet. Set {}=1 only to recover a legitimately-run in-flight pipeline.",
        stage.slug,
        env::full_name("DISABLE_ENSEMBLE_EVIDENCE")
    )
}

/// What stands in for Construction's checkpoints, units and swarms (M4).
///
/// # Errors
///
/// The stage is a Construction stage.
pub fn refuse_construction(stage: &Stage) -> Result<()> {
    if stage.phase == "construction" {
        bail!(
            "\"{}\" is a Construction stage, which runs per unit of work with checkpoints, unit gates and swarms: Focus does not run Construction yet. Skip the stage, or wait for the units milestone.",
            stage.slug
        );
    }
    Ok(())
}

/// `verifyGateOpeningGuards`: what opening a gate checks.
///
/// # Errors
///
/// Any guard above refuses.
pub fn verify_gate_opening_guards(wf: &Workflow, stage: &Stage, state: &str) -> Result<()> {
    verify_stage_artifacts(wf, stage, Action::PresentApprovalGate)?;
    verify_summary_confirmation(wf, stage, state)?;
    verify_pipeline_link(wf, stage)?;
    if !reviewer_gate_guard_disabled() {
        verify_reviewer(wf, stage, state, true)?;
    }
    Ok(())
}

/// `admitStageAction`: the chain, for a serial stage. `operation` is how
/// the archived refusal names the command.
///
/// # Errors
///
/// Any guard refuses; the stage is a Construction stage.
pub fn admit(
    wf: &Workflow,
    state: &str,
    stage: &Stage,
    action: Action,
    operation: &str,
) -> Result<()> {
    assert_not_archived(state, operation)?;
    refuse_construction(stage)?;
    if action != Action::Complete {
        verify_gate_opening_guards(wf, stage, state)?;
        return Ok(());
    }
    let already_completed = parse_checkboxes(state)
        .iter()
        .any(|line| line.slug == stage.slug && line.state == CheckboxState::Completed);
    verify_reviewer(wf, stage, state, !already_completed)?;
    if !already_completed {
        verify_stage_artifacts(wf, stage, Action::Complete)?;
        verify_summary_confirmation(wf, stage, state)?;
        verify_pipeline_link(wf, stage)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use focus_harness::Harness;
    use focus_workspace::records::{Invoker, Selection};
    use serde_json::json;

    use super::*;

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// An installed project with one record, opened as a workflow.
    fn workflow(name: &str, state: &str) -> (Workflow, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("focus-admission-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream/installed/claude");
        copy_dir(&fixtures, &root);
        let record = root.join("focus/spaces/default/intents/260105-add-login");
        std::fs::create_dir_all(&record).unwrap();
        std::fs::write(record.join("focus-state.md"), state).unwrap();
        let layout = Layout::new(root.clone(), Harness::Claude);
        let selection = Selection {
            space: "default".to_owned(),
            intent: Some("260105-add-login".to_owned()),
            session_id: None,
            binding: None,
        };
        (
            Workflow::open(layout, selection, Invoker::default()).unwrap(),
            root,
        )
    }

    fn stage_of(node: serde_json::Value) -> Stage {
        focus_graph::runtime::load_from_value(&json!([node])).remove(0)
    }

    const STATE: &str = "- **Scope**: feature\n- **Project Type**: Greenfield\n- **Review Override**: \n- **Summary Confirmation**: on (from scope feature)\n";

    #[test]
    fn a_code_producing_stage_needs_source_work() {
        let (wf, root) = workflow("source", STATE);
        // No artifacts declared: the stage passes on them; the source work is what is missing.
        let stage = stage_of(
            json!({"slug": "builder", "phase": "ideation", "name": "Builder", "workspace_requires": true, "produces": []}),
        );
        let refused = verify_stage_artifacts(&wf, &stage, Action::Complete).unwrap_err();
        assert!(refused.message().starts_with("Cannot complete \"builder\": it is a code-producing stage (workspace_requires) but no source work is evident outside the focus/ workspace tree."), "{refused}");
        let gate = verify_stage_artifacts(&wf, &stage, Action::PresentApprovalGate).unwrap_err();
        assert!(
            gate.message().starts_with(
                "Cannot present \"builder\" for approval: it is a code-producing stage"
            ),
            "{gate}"
        );
        // A planning doc under the workspace is not source work; a file under src/ is.
        std::fs::write(root.join("focus/notes.md"), "x").unwrap();
        assert!(verify_stage_artifacts(&wf, &stage, Action::Complete).is_err());
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        assert!(verify_stage_artifacts(&wf, &stage, Action::Complete).is_ok());
        // A stage that does not produce code is not asked.
        let plain = stage_of(
            json!({"slug": "writer", "phase": "ideation", "name": "Writer", "produces": []}),
        );
        std::fs::remove_dir_all(root.join("src")).unwrap();
        assert!(verify_stage_artifacts(&wf, &plain, Action::Complete).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_summary_is_owed_by_the_phases_that_ask_one() {
        let (wf, root) = workflow("summary", STATE);
        let at = |phase: &str, declared: bool| {
            let mut node = json!({"slug": "s", "phase": phase, "name": "S"});
            if declared {
                node["summary_confirmation"] = json!("required");
            }
            stage_of(node)
        };
        assert!(!summary_confirmation_owed(
            &wf,
            &at("initialization", true),
            STATE
        ));
        assert!(summary_confirmation_owed(&wf, &at("ideation", true), STATE));
        assert!(summary_confirmation_owed(
            &wf,
            &at("operation", true),
            STATE
        ));
        assert!(!summary_confirmation_owed(
            &wf,
            &at("operation", false),
            STATE
        ));
        assert!(summary_confirmation_owed(
            &wf,
            &at("construction", true),
            STATE
        ));
        let autonomous = format!("{STATE}- **Construction Autonomy Mode**: autonomous\n");
        assert!(!summary_confirmation_owed(
            &wf,
            &at("construction", true),
            &autonomous
        ));
        assert!(summary_confirmation_owed(
            &wf,
            &at("operation", true),
            &autonomous
        ));
        let off = STATE.replace("on (from scope feature)", "off (set by you)");
        assert!(!summary_confirmation_owed(&wf, &at("ideation", true), &off));
        // With no line, the scope decides; an unknown scope reads as on.
        let no_line = STATE.replace("- **Summary Confirmation**: on (from scope feature)\n", "");
        assert_eq!(summary_confirmation_setting(&wf, &no_line), "on");
        assert_eq!(
            summary_confirmation_setting(&wf, &no_line.replace("feature", "nope")),
            "on"
        );
        assert!(
            verify_summary_confirmation(&wf, &at("ideation", true), STATE)
                .unwrap_err()
                .message()
                .contains("owes a summary confirmation")
        );
        assert!(verify_summary_confirmation(&wf, &at("ideation", true), &off).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_pipeline_owes_its_handoffs_and_a_reviewed_stage_its_receipt() {
        let (wf, root) = workflow("pipeline", STATE);
        let pipeline =
            stage_of(json!({"slug": "p", "phase": "inception", "name": "P", "mode": "pipeline"}));
        assert!(
            verify_pipeline_link(&wf, &pipeline)
                .unwrap_err()
                .message()
                .contains("is a pipeline stage and owes the handoffs")
        );
        for mode in ["inline", "subagent", "mob"] {
            let other =
                stage_of(json!({"slug": "o", "phase": "inception", "name": "O", "mode": mode}));
            assert!(verify_pipeline_link(&wf, &other).is_ok(), "{mode}");
        }
        let reviewed = stage_of(
            json!({"slug": "r", "phase": "inception", "name": "R", "reviewer": "focus-product-lead-agent", "review_class": "advisory"}),
        );
        assert_eq!(resolve_review_class(&wf, &reviewed, STATE), "advisory");
        assert_eq!(
            resolve_review_class(
                &wf,
                &reviewed,
                &STATE.replace("- **Review Override**: \n", "- **Review Override**: none\n")
            ),
            "none"
        );
        assert_eq!(
            resolve_review_class(
                &wf,
                &reviewed,
                &STATE.replace(
                    "- **Review Override**: \n",
                    "- **Review Override**: adversarial\n"
                )
            ),
            "advisory"
        );
        assert!(verify_reviewer(&wf, &reviewed, STATE, true).unwrap_err().message().contains("owes a review receipt from focus-product-lead-agent (its review class resolves to advisory)"));
        assert!(verify_reviewer(&wf, &reviewed, STATE, false).is_ok());
        let unreviewed = stage_of(json!({"slug": "u", "phase": "inception", "name": "U"}));
        assert!(verify_reviewer(&wf, &unreviewed, STATE, true).is_ok());
        let construction = stage_of(json!({"slug": "c", "phase": "construction", "name": "C"}));
        assert!(
            refuse_construction(&construction)
                .unwrap_err()
                .message()
                .contains("is a Construction stage")
        );
        assert!(refuse_construction(&unreviewed).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_ceremony_line_reads_on_or_off() {
        assert_eq!(ceremony_state_value(Some("off (set by you)")), Some("off"));
        assert_eq!(ceremony_state_value(Some("ON")), Some("on"));
        assert_eq!(
            ceremony_state_value(Some("  on (from scope feature)  ")),
            Some("on")
        );
        assert_eq!(ceremony_state_value(Some("offline")), None);
        assert_eq!(ceremony_state_value(Some("")), None);
        assert_eq!(ceremony_state_value(None), None);
    }

    #[test]
    fn an_archived_workflow_is_refused_by_the_operation() {
        assert!(assert_not_archived("- **Status**: Running\n", "advance").is_ok());
        assert_eq!(
            assert_not_archived("- **Status**: Archived\n", "focus engine state advance")
                .unwrap_err()
                .message(),
            "Workflow is Archived, so focus engine state advance is refused. Bring it back first with `/focus intent unarchive <name>`."
        );
    }
}

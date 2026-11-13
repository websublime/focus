//! `log decision`: a question the agent asks the person, written to the
//! ledger as a `DECISION_RECORDED` row before the turn ends, so the answer
//! that follows has something to answer (`handleDecision` in
//! `aidlc-log.ts`).
//!
//! What is ported is the question of a stage, with the message that
//! already answered it when it names one. The questions the engine
//! pairs with a record of its own are refused by name until the slice that
//! brings the record: a stage's summary confirmation (slice 2c), the
//! verification command and the policy of Construction, and a question
//! about one unit of work (slice 5), and Plan Approval (slice 11).

use focus_js::text::{to_lower_case, trim};
use focus_js::{Result, bail, re};
use focus_lifecycle::workflow::Workflow;
use focus_lifecycle::{admission, gate, presence, reply};
use serde_json::{Value, json};

use crate::flags::{self, Flags};
use crate::message::{self, What};

/// `unknownCheckpointMessage`.
pub(crate) fn unknown_checkpoint(checkpoint: &str) -> String {
    if checkpoint == "learnings" {
        return "The learnings question takes no --checkpoint: run the same command without it."
            .to_owned();
    }
    format!(
        "Unknown --checkpoint \"{checkpoint}\". Accepted: summary-confirmation, plan-approval, verification-command, construction-policy, guard-recovery"
    )
}

/// `isSummaryConfirmationChoice`: "Looks correct" or "Request changes",
/// however it was capitalised or decorated.
fn is_summary_choice(text: &str) -> bool {
    let stripped = reply::strip_recommended_decorator(text);
    let normalized = to_lower_case(&re::replace("[.!]+$", "", trim(&stripped), ""));
    normalized == "looks correct" || normalized == "request changes"
}

/// `isSummaryConfirmationOptions`: a question whose offered options are the
/// summary's two choices.
pub(crate) fn is_summary_options(options: Option<&str>) -> bool {
    let Some(options) = options else {
        return false;
    };
    let offered: Vec<&str> = options
        .split(',')
        .map(trim)
        .filter(|option| !option.is_empty())
        .collect();
    offered.len() == 2 && offered.iter().all(|option| is_summary_choice(option))
}

/// `refusePlainSummaryConfirmation`, for a question: one that offers the
/// summary's two choices, for a stage that owes a summary confirmation, is
/// not an ordinary question. Upstream refuses it with the two commands that
/// record a summary; those are slice 2c, and until then Focus refuses it by
/// name.
fn refuse_plain_summary(wf: &Workflow, flags: &Flags, stage: &str) -> Result<()> {
    let Some(stage) = wf.install.stage(stage) else {
        return Ok(());
    };
    if !is_summary_options(flags.get("options")) {
        return Ok(());
    }
    let state = wf.read_state()?;
    if admission::summary_confirmation_owed(wf, stage, &state) {
        bail!(
            "\"{}\" owes a consolidated summary confirmation, and this question offers its two choices: it is recorded with `--checkpoint summary-confirmation --questions-file <path>`, and Focus does not record summary confirmations yet.",
            stage.slug
        );
    }
    Ok(())
}

/// `handleDecision`. `open` selects the piece of work the question belongs
/// to; it runs after the arguments are read, as upstream reads them before
/// it looks for a workflow.
///
/// # Errors
///
/// What upstream refuses, in its words; what Focus refuses by name.
pub fn decision(open: &dyn Fn() -> Result<Workflow>, args: &[String]) -> Result<Value> {
    let (_, flags) = flags::parse(args)?;
    let Some(stage) = flags.given("stage") else {
        bail!("Missing --stage <slug>");
    };
    let Some(question) = flags.given("decision") else {
        bail!("Missing --decision <text>");
    };
    match flags.get("checkpoint") {
        None => {}
        Some("summary-confirmation") => bail!(
            "--checkpoint summary-confirmation records the question of a stage's summary confirmation, and Focus does not record summary confirmations yet."
        ),
        Some(checkpoint @ ("verification-command" | "construction-policy")) => bail!(
            "--checkpoint {checkpoint} records a question of Construction's checkpoints, and Focus does not run Construction yet."
        ),
        Some("plan-approval") => bail!(
            "--checkpoint plan-approval records the Plan Approval question, and Focus does not run Plan Approval yet."
        ),
        Some(other) => bail!("{}", unknown_checkpoint(other)),
    }
    let wf = open()?;
    refuse_plain_summary(&wf, &flags, stage)?;
    // A question that says the person already confirmed records a choice
    // they never made: the engine's carrying-on line is not their answer.
    if !presence::human_presence_guard_disabled(&wf)
        && let Some(implied) = reply::implied_confirmation_marker(Some(question))
    {
        bail!(
            "Cannot record this question for \"{stage}\" because --decision says the person confirmed it ({}: \"{}\"). Focus's carrying-on line is not their answer. Ask them and record what they say, or carry on with no record.",
            implied.category,
            implied.phrase
        );
    }
    if flags.get("batch-file").is_some() {
        bail!(
            "--batch-file records the Plan Approval questions of several units of work at once, and Focus does not run Plan Approval yet."
        );
    }
    if flags.given("unit").is_some() {
        bail!(
            "--unit records a question about one unit of work, and Focus does not run units yet."
        );
    }
    let mut fields: Vec<(&str, String)> = vec![
        ("Stage", stage.to_owned()),
        ("Decision", question.to_owned()),
    ];
    if let Some(options) = flags.given("options") {
        fields.push(("Options", options.to_owned()));
    }
    if let Some(rationale) = flags.given("rationale") {
        fields.push(("Rationale", rationale.to_owned()));
    }
    if flags.get("single") == Some("true") {
        fields.push(("Workflow", format!("single-stage:{stage}")));
    }
    // A question logged after the person already replied names the message
    // that answered it; its answers inherit the id.
    if let Some(id) = flags.get("message") {
        match message::prove(&wf, id, What::Decision) {
            Ok(proved) => fields.push(("Message Id", proved.id)),
            Err(refusal) => bail!("{refusal}"),
        }
    }
    // A new question retires the consent kept for the chat that asks it.
    let session = flags
        .get("session")
        .map(trim)
        .filter(|session| !session.is_empty())
        .map(str::to_owned)
        .or_else(|| focus_workspace::ancestry::resolve(&wf.layout))
        .unwrap_or_else(|| "*".to_owned());
    let held = wf.lock("log decision")?;
    gate::withdraw_protected_questions_of(&wf, &held, &session)
        .map_err(|err| err.prefixed("Audit emission failed"))?;
    wf.emit(&held, "DECISION_RECORDED", &fields)?;
    drop(held);
    Ok(json!({"emitted": "DECISION_RECORDED", "stage": stage}))
}

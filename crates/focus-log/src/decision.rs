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

use focus_js::text::trim;
use focus_js::{Result, bail};
use focus_lifecycle::workflow::Workflow;
use focus_lifecycle::{gate, presence, reply};
use serde_json::{Value, json};

use crate::flags;
use crate::message::{self, What};
use crate::summary::{self, Plain, Verb};

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
        Some("summary-confirmation") => {}
        Some(checkpoint @ ("verification-command" | "construction-policy")) => bail!(
            "--checkpoint {checkpoint} records a question of Construction's checkpoints, and Focus does not run Construction yet."
        ),
        Some("plan-approval") => bail!(
            "--checkpoint plan-approval records the Plan Approval question, and Focus does not run Plan Approval yet."
        ),
        Some(other) => bail!("{}", unknown_checkpoint(other)),
    }
    let wf = open()?;
    if flags.get("checkpoint").is_none() {
        summary::refuse_plain(
            &wf,
            &[],
            Verb::Decision,
            &Plain {
                stage,
                unit: flags.get("unit"),
                single: flags.get("single").is_some(),
                options: flags.get("options"),
                details: None,
            },
        )?;
    }
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
    // The summary's question binds to the questions file the person reads:
    // its summary section still has its one `[Answer]:` line blank.
    let summary_evidence = match flags.get("checkpoint") {
        Some("summary-confirmation") => Some(summary::question_evidence(
            &wf,
            flags.get("questions-file"),
            "",
        )?),
        _ => None,
    };
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
    // The hash is the answer's to record: here it says the file can be
    // one.
    if let Some((questions_file, _)) = summary_evidence {
        fields.push(("Checkpoint", summary::CHECKPOINT.to_owned()));
        fields.push(("Questions File", questions_file));
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

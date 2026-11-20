//! The series of a guard's refusals, and the question that renders one.
//!
//! `recordGuardRefusal` and what is around it (`aidlc-lib.ts` L30 985 to
//! L31 370). A guard that refuses does not only say why: it ends with a
//! question, the recovery ask, built from the ways on the evaluator found
//! ([`crate::guard_refusal`]). What the question is depends on how many
//! times the same situation has refused, so each refusal is counted in a
//! record kept for its stage and unit under the engine folder
//! (`guard-refusals/<key>.json`).
//!
//! - The first time, the conductor is handed the ways on that are its own
//!   work, and takes them without asking (`agent_work`).
//! - When there are none, or the same refusal comes back, the person is
//!   asked, with the ways on that need them; a repeat says "still".
//! - With no way on that can be done now, the question is the terminal
//!   one: where the work stopped and that it needs the person. Past the
//!   cap it carries the series' signature, to report.
//!
//! "The same situation" is the signature of the guard's state: the stage,
//! the unit, the stage's state, the attempt as the guard saw it
//! ([`crate::guard_attempt`]), the resources whose change is progress, and
//! the latest boundary after which a repeat is a new situation (a session
//! starting or resuming, the work starting, a jump, a rejection at this
//! stage's gate, the unit's bolt starting again).
//!
//! The record carries no authority: a write that fails only under-counts,
//! and never relaxes a guard. It is read by its name, as upstream reads
//! it, and not as a trusted file of the workspace: nothing is decided by
//! it but which of the questions above is asked, and a record that does
//! not read as one is a series that starts again.

use std::time::SystemTime;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_js::json::{stringify, stringify_pretty};
use focus_js::text::trim;
use focus_workspace::files::write_engine_file_no_follow;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::gate::sort_attempt_events;
use crate::guard_attempt::attempt_value;
use crate::guard_refusal::{Attempt, Interaction, Refusal, Remedy, refusal_value, remedy_value};
use crate::stage;
use crate::workflow::Workflow;

/// `GUARD_REFUSAL_STREAK_CAP`: past this many refusals of one situation
/// with nothing to do, the question names the situation for a report.
pub const STREAK_CAP: f64 = 2.0;
/// `GUARD_RECOVERY_ASK_TYPE`.
pub const RECOVERY_ASK_TYPE: &str = "guard-recovery";
/// The folder of the series, under the engine's.
pub const SERIES_DIR: &str = "guard-refusals";

/// `GuardRecoveryAskData`: the question a refusal ends with.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    pub question: String,
    pub stage: String,
    pub unit: Option<String>,
    pub reason_codes: Vec<String>,
    /// The ways on the conductor may offer now.
    pub remedies: Vec<Remedy>,
    /// The terminal question only: the series' signature.
    pub state_signature: Option<String>,
    /// The ways on are the conductor's own work: it carries out the first
    /// that applies without asking.
    pub agent_work: bool,
    /// The terminal question only: the refusal as the tool told it.
    pub detail: Option<String>,
}

impl Ask {
    /// The ask as upstream writes it, in the order of its keys.
    #[must_use]
    pub fn to_value(&self) -> Value {
        let mut out = Map::new();
        out.insert("kind".to_owned(), json!("ask"));
        out.insert("ask_type".to_owned(), json!(RECOVERY_ASK_TYPE));
        out.insert("response_route".to_owned(), json!("execute-remedy"));
        out.insert("question".to_owned(), json!(self.question));
        out.insert("stage".to_owned(), json!(self.stage));
        if let Some(unit) = &self.unit {
            out.insert("unit".to_owned(), json!(unit));
        }
        out.insert("reason_codes".to_owned(), json!(self.reason_codes));
        out.insert(
            "remedies".to_owned(),
            Value::Array(self.remedies.iter().map(remedy_value).collect()),
        );
        if let Some(signature) = &self.state_signature {
            out.insert("state_signature".to_owned(), json!(signature));
        }
        if self.agent_work {
            out.insert("agent_work".to_owned(), json!(true));
        }
        if let Some(detail) = &self.detail {
            out.insert("detail".to_owned(), json!(detail));
        }
        Value::Object(out)
    }
}

/// `GuardRefusalStreak`: how many times the situation has refused, its
/// signature, and the question that renders it.
#[derive(Debug, Clone, PartialEq)]
pub struct Streak {
    pub count: f64,
    pub signature: String,
    pub ask: Ask,
}

/// What the record of a series keeps of the last refusal.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub state_signature: String,
    pub signature: String,
    pub count: f64,
    pub codes: Vec<String>,
    pub reset_token: String,
    pub refusal: Value,
    pub updated_at: String,
}

impl Record {
    /// The record as upstream writes it, with what a hook's refusal leaves
    /// for the next step to ask.
    #[must_use]
    pub fn to_value(&self, left: Option<(&Ask, &str)>) -> Value {
        let mut out = Map::new();
        out.insert("version".to_owned(), json!(1));
        out.insert("stateSignature".to_owned(), json!(self.state_signature));
        out.insert("signature".to_owned(), json!(self.signature));
        out.insert("count".to_owned(), json!(self.count));
        out.insert("codes".to_owned(), json!(self.codes));
        out.insert("resetToken".to_owned(), json!(self.reset_token));
        out.insert("refusal".to_owned(), self.refusal.clone());
        out.insert("updatedAt".to_owned(), json!(self.updated_at));
        if let Some((ask, approval)) = left {
            out.insert("pendingAsk".to_owned(), ask.to_value());
            out.insert("pendingApproval".to_owned(), json!(approval));
        }
        Value::Object(out)
    }
}

fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `guardRefusalPath`: the name of the series of a stage and a unit, in
/// the engine folder.
#[must_use]
pub fn series_name(stage: &str, unit: Option<&str>) -> String {
    format!(
        "{SERIES_DIR}/{}.json",
        sha256_hex(&format!("{stage}\0{}", unit.unwrap_or_default()))
    )
}

/// The names a row lists in `field`, as written between the commas.
fn listed<'a>(block: &'a str, field: &str) -> Vec<&'a str> {
    block_field(block, field)
        .unwrap_or_default()
        .split(',')
        .map(trim)
        .collect()
}

/// `gateEventCovers`: a gate decision for this stage, and for this unit or
/// every unit.
fn gate_event_covers(event: &ShardEvent, stage: &str, unit: Option<&str>) -> bool {
    let stages = block_field(&event.block, "Gate Stages")
        .or_else(|| block_field(&event.block, "Stage"))
        .unwrap_or_default();
    if !stages.split(',').map(trim).any(|named| named == stage) {
        return false;
    }
    block_field(&event.block, "Unit").is_none_or(|named| Some(named) == unit)
}

/// `guardRefusalResetToken`: the latest boundary after which a repeat is a
/// new situation; empty when the ledger has none.
#[must_use]
pub fn reset_token(events: &[ShardEvent], stage: &str, unit: Option<&str>) -> String {
    let boundaries: Vec<ShardEvent> = events
        .iter()
        .filter(|event| match event.event.as_str() {
            "SESSION_STARTED" | "SESSION_RESUMED" | "WORKFLOW_STARTED" | "STAGE_JUMPED" => true,
            "GATE_REJECTED" => gate_event_covers(event, stage, unit),
            "BOLT_STARTED" => {
                unit.is_some_and(|unit| listed(&event.block, "Bolt names").contains(&unit))
            }
            _ => false,
        })
        .cloned()
        .collect();
    sort_attempt_events(&boundaries)
        .last()
        .map_or_else(String::new, |latest| {
            format!(
                "{}\0{}\0{}\0{}",
                latest.event, latest.timestamp, latest.shard, latest.pos
            )
        })
}

/// `guardRefusalApprovalToken`: the latest approval of this stage (or of
/// this unit's step); empty when there is none.
#[must_use]
pub fn approval_token(events: &[ShardEvent], stage: &str, unit: Option<&str>) -> String {
    let approvals: Vec<ShardEvent> = events
        .iter()
        .filter(|event| event.event == "GATE_APPROVED" && gate_event_covers(event, stage, unit))
        .cloned()
        .collect();
    sort_attempt_events(&approvals)
        .last()
        .map_or_else(String::new, |latest| {
            format!("{}\0{}\0{}", latest.timestamp, latest.shard, latest.pos)
        })
}

/// What a record that reads as one says of the series so far.
struct Prior {
    state_signature: String,
    signature: String,
    count: f64,
    codes: Vec<String>,
    reset_token: String,
}

/// `readGuardRefusalRecord`: the record at `path`, when it is one: version
/// 1 with every field of the kind it is written as.
fn read_record(path: &std::path::Path) -> Option<(Prior, Value)> {
    let text = std::fs::read(path).ok()?;
    let value = focus_js::json::parse(&String::from_utf8_lossy(&text)).ok()?;
    let record = value.as_object()?;
    if record.get("version").and_then(Value::as_f64) != Some(1.0) {
        return None;
    }
    let text_of = |key: &str| record.get(key).and_then(Value::as_str);
    let count = record
        .get("count")
        .and_then(Value::as_f64)
        .filter(|count| count.fract() == 0.0 && *count >= 1.0 && count.is_finite())?;
    let codes: Vec<String> = record
        .get("codes")?
        .as_array()?
        .iter()
        .map(|code| code.as_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    text_of("updatedAt")?;
    record.get("refusal")?.as_object()?;
    if record.get("pendingAsk").is_some_and(|ask| !ask.is_object())
        || record
            .get("pendingApproval")
            .is_some_and(|approval| !approval.is_string())
    {
        return None;
    }
    let prior = Prior {
        state_signature: text_of("stateSignature")?.to_owned(),
        signature: text_of("signature")?.to_owned(),
        count,
        codes,
        reset_token: text_of("resetToken")?.to_owned(),
    };
    Some((prior, value))
}

/// The stage, and its unit when there is one, as the person knows them.
fn target(wf: &Workflow, refusal: &Refusal) -> String {
    let name = wf
        .install
        .stage(&refusal.stage)
        .map_or(refusal.stage.as_str(), stage::name);
    match refusal.unit.as_deref().filter(|unit| !unit.is_empty()) {
        Some(unit) => format!("{name} for {unit}"),
        None => name.to_owned(),
    }
}

/// `guardRecoveryQuestion`: the one line the person reads above the ways
/// on. A repeat says so.
fn question(wf: &Workflow, refusal: &Refusal, again: bool) -> String {
    format!(
        "{}{} can't go ahead as things stand: which way would you like to go on?",
        target(wf, refusal),
        if again { " still" } else { "" }
    )
}

/// `guardRecoveryAskForRefusal`: the ask for the ways on of `offered` that
/// can be done now; `None` when there is none.
fn ask_for(wf: &Workflow, refusal: &Refusal, offered: &[&Remedy]) -> Option<Ask> {
    let remedies: Vec<Remedy> = offered
        .iter()
        .filter(|remedy| remedy.executable_now)
        .map(|remedy| (*remedy).clone())
        .collect();
    if remedies.is_empty() {
        return None;
    }
    Some(Ask {
        question: question(wf, refusal, false),
        stage: refusal.stage.clone(),
        unit: refusal.unit.clone().filter(|unit| !unit.is_empty()),
        reason_codes: vec![refusal.code.clone()],
        remedies,
        state_signature: None,
        agent_work: false,
        detail: None,
    })
}

/// `guardTerminalAskForRefusal`: the question of a refusal with nothing
/// that can be done now. It carries no remedy, so nothing can be carried
/// out from it; the signature is what marks it terminal.
fn terminal_ask(
    wf: &Workflow,
    refusal: &Refusal,
    count: f64,
    codes: &[String],
    signature: &str,
) -> Ask {
    let situation = format!(
        "I stopped at {}: this step cannot go ahead, and there is nothing I can safely do about it on my own.",
        target(wf, refusal)
    );
    let why = trim(&refusal.user_message);
    Ask {
        question: if count > STREAK_CAP {
            format!(
                "{situation} It has stopped here {} times now. Tell me how you want to proceed. (To report this, include {signature}.)",
                focus_js::num::to_string(count)
            )
        } else {
            format!("{situation} Tell me how you want to proceed.")
        },
        stage: refusal.stage.clone(),
        unit: refusal.unit.clone().filter(|unit| !unit.is_empty()),
        reason_codes: codes.to_vec(),
        remedies: Vec::new(),
        state_signature: Some(signature.to_owned()),
        agent_work: false,
        detail: Some(format!(
            "{} on {} ({}).{}",
            refusal.code,
            refusal.blocked_action,
            refusal.state,
            if why.is_empty() {
                String::new()
            } else {
                format!(" {why}")
            }
        )),
    }
}

/// The signature of the guard's state: what makes two refusals the same
/// situation.
#[must_use]
pub fn state_signature(
    refusal: &Refusal,
    attempt: &Attempt,
    reset_token: &str,
    resources: &[String],
) -> String {
    let said = attempt_value(attempt);
    let or_null = |key: &str| said.get(key).cloned().unwrap_or(Value::Null);
    let mut sorted = resources.to_vec();
    sorted.sort_by(|a, b| focus_js::order::cmp_utf16(a, b));
    let mut state = Map::new();
    state.insert("stage".to_owned(), json!(refusal.stage));
    state.insert("unit".to_owned(), json!(refusal.unit));
    state.insert("state".to_owned(), json!(refusal.state));
    for key in [
        "floor",
        "reviewBudget",
        "recovery",
        "pendingReview",
        "repairReview",
        "nextReview",
        "summaryCoverage",
        "reviewCoverage",
        "sourceCoverage",
    ] {
        state.insert(key.to_owned(), or_null(key));
    }
    state.insert("resetToken".to_owned(), json!(reset_token));
    state.insert("resources".to_owned(), json!(sorted));
    sha256_hex(&stringify(&Value::Object(state)))
}

/// `guardRefusalStreakView`: what the series would say if this refusal
/// were recorded now. It reads the record so far and writes nothing. With
/// `as_recorded` an unchanged refusal reads as the series stands, not as
/// the repeat a recording would make.
#[must_use]
pub fn streak_view(
    wf: &Workflow,
    refusal: &Refusal,
    attempt: &Attempt,
    resources: &[String],
    as_recorded: bool,
) -> (Streak, Record) {
    let unit = refusal.unit.as_deref();
    let path = wf
        .selection
        .engine_dir(&wf.layout)
        .join(series_name(&refusal.stage, unit));
    let prior = read_record(&path).map(|(prior, _)| prior);
    let reset_token = reset_token(&wf.events(), &refusal.stage, unit);
    let state_signature = state_signature(refusal, attempt, &reset_token, resources);
    let codes = match &prior {
        Some(prior)
            if prior.state_signature == state_signature && prior.reset_token == reset_token =>
        {
            let mut codes = prior.codes.clone();
            if !codes.contains(&refusal.code) {
                codes.push(refusal.code.clone());
            }
            // (A set keeps the first of each code; the order is then the
            // sort's.)
            let mut seen: Vec<String> = Vec::new();
            for code in codes {
                if !seen.contains(&code) {
                    seen.push(code);
                }
            }
            seen.sort_by(|a, b| focus_js::order::cmp_utf16(a, b));
            seen
        }
        _ => vec![refusal.code.clone()],
    };
    let signature = sha256_hex(&stringify(
        &json!({ "stateSignature": state_signature, "codes": codes }),
    ));
    let count = match &prior {
        Some(prior) if prior.signature == signature => {
            if as_recorded {
                prior.count
            } else {
                prior.count + 1.0
            }
        }
        _ => 1.0,
    };
    let record = Record {
        state_signature,
        signature: signature.clone(),
        count,
        codes: codes.clone(),
        reset_token,
        refusal: refusal_value(refusal),
        updated_at: focus_audit::time::iso_timestamp(SystemTime::now()),
    };
    // The conductor takes the ways on it can do itself, the first time.
    // Only when the same refusal comes back, or there are none, is the
    // person asked, and then only with the ways on that need them.
    let own = |remedy: &&Remedy| remedy.interaction == Interaction::ExternalWork;
    let streak = |ask: Ask| Streak {
        count,
        signature: signature.clone(),
        ask,
    };
    #[allow(clippy::float_cmp)]
    let first = count == 1.0;
    if first
        && let Some(own_work) = ask_for(
            wf,
            refusal,
            &refusal.remedies.iter().filter(own).collect::<Vec<_>>(),
        )
    {
        return (
            streak(Ask {
                agent_work: true,
                ..own_work
            }),
            record,
        );
    }
    let theirs: Vec<&Remedy> = refusal
        .remedies
        .iter()
        .filter(|remedy| !own(remedy))
        .collect();
    let ask = match ask_for(wf, refusal, &theirs) {
        Some(ask) if first => ask,
        Some(ask) => Ask {
            reason_codes: codes,
            question: question(wf, refusal, true),
            ..ask
        },
        None => terminal_ask(wf, refusal, count, &codes, &signature),
    };
    (streak(ask), record)
}

/// `recordGuardRefusal`: one refusal recorded against the series of its
/// stage and unit, and the question that renders it. The record is the
/// only write. `leave_ask` keeps the question in the record for the next
/// step to put to the person (a hook's refusal).
#[must_use]
pub fn record_refusal(
    wf: &Workflow,
    refusal: &Refusal,
    attempt: &Attempt,
    resources: &[String],
    leave_ask: bool,
) -> Streak {
    let (streak, record) = streak_view(wf, refusal, attempt, resources, false);
    let unit = refusal.unit.as_deref();
    let approval = leave_ask.then(|| approval_token(&wf.events(), &refusal.stage, unit));
    let left = approval.as_deref().map(|approval| (&streak.ask, approval));
    // (A write that fails under-counts the repeats; it never relaxes a
    // guard. A link planted in the engine folder is one such failure.)
    let _ = write_engine_file_no_follow(
        &wf.layout,
        &wf.selection,
        &series_name(&refusal.stage, unit),
        &format!("{}\n", stringify_pretty(&record.to_value(left))),
    );
    streak
}

/// `guardRefusalOutput`: the refusal as a tool that enforces prints it:
/// the sentence, then the ask as the last line, so the router can read it
/// back into the directive it would have given itself. The refusal is
/// recorded.
#[must_use]
pub fn refusal_output(
    wf: &Workflow,
    refusal: &Refusal,
    attempt: &Attempt,
    resources: &[String],
) -> String {
    let streak = record_refusal(wf, refusal, attempt, resources, false);
    format!(
        "{}\n{}",
        refusal.user_message,
        stringify(&streak.ask.to_value())
    )
}

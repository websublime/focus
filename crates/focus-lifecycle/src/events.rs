//! What a transition reads back from the ledger: whether a stage's row was
//! already written, what a routed skip already recorded, and the answer mode
//! the person chose earlier.
//!
//! Shards are concatenated in file-name order, which is not time order, so
//! every reader here orders rows by timestamp first (as JavaScript compares
//! the strings) and by position in the text second.
//!
//! Mirrors `hasStageAuditEvent`, `orderedMainWorkflowAudit`,
//! `currentRoutedSkipAuditTail` and `auditTailHasFields` in
//! `aidlc-state.ts`, and `nextOpenDecision`, `isAnswerModeDecision`,
//! `answerModeFromReply`, `latestRecordedAnswerMode` and
//! `answerModeStageStartedFields` in `aidlc-lib.ts`.

use focus_audit::read::{block_field, find_all_events};
use focus_audit::shards::ShardEvent;
use focus_js::order::cmp_utf16;
use focus_js::re;
use focus_js::text::trim;

/// `auditField`: the value of a block's field, as the state tool reads it
/// (the first line starting with the bold label).
fn audit_field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let label = format!("**{name}**:");
    block
        .split('\n')
        .find_map(|line| line.strip_prefix(label.as_str()))
        .map(trim)
}

/// A row of a `--single` stage run, which belongs to no main workflow.
fn is_single_stage_row(block: &str) -> bool {
    audit_field(block, "Workflow").is_some_and(|workflow| workflow.starts_with("single-stage:"))
}

/// `hasStageAuditEvent`: whether the current workflow (since its last
/// `WORKFLOW_STARTED`) recorded `event` for `slug`.
#[must_use]
pub fn has_stage_audit_event(audit: &str, event: &str, slug: &str) -> bool {
    if audit.is_empty() {
        return false;
    }
    let since = find_all_events(audit, "WORKFLOW_STARTED", None)
        .last()
        .map(|start| start.timestamp.clone())
        .unwrap_or_default();
    find_all_events(audit, event, None).iter().any(|found| {
        if !since.is_empty() && cmp_utf16(&found.timestamp, &since).is_lt() {
            return false;
        }
        if is_single_stage_row(&found.block) {
            return false;
        }
        audit_field(&found.block, "Stage") == Some(slug)
    })
}

/// One event of the main workflow, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ordered {
    pub event: String,
    pub block: String,
    pub timestamp: String,
    pub position: usize,
}

/// `orderedMainWorkflowAudit`: the rows of the current workflow, from its
/// last `WORKFLOW_STARTED`, ordered by timestamp then position; single-stage
/// rows and blocks with no event left out.
#[must_use]
pub fn ordered_main_workflow_audit(audit: &str) -> Vec<Ordered> {
    if audit.is_empty() {
        return Vec::new();
    }
    let mut events: Vec<Ordered> = audit
        .replace("\r\n", "\n")
        .split("\n---\n")
        .enumerate()
        .filter_map(|(position, block)| {
            let event = audit_field(block, "Event")?;
            if is_single_stage_row(block) {
                return None;
            }
            Some(Ordered {
                event: event.to_owned(),
                block: block.to_owned(),
                timestamp: audit_field(block, "Timestamp").unwrap_or("").to_owned(),
                position,
            })
        })
        .collect();
    events.sort_by(|a, b| cmp_utf16(&a.timestamp, &b.timestamp).then(a.position.cmp(&b.position)));
    match events
        .iter()
        .rposition(|event| event.event == "WORKFLOW_STARTED")
    {
        Some(start) => events.split_off(start),
        None => events,
    }
}

/// `currentRoutedSkipAuditTail`: the rows after this stage's latest skip,
/// when its latest boundary row is a `STAGE_SKIPPED` (a later
/// `STAGE_STARTED` starts a fresh attempt). `None` otherwise.
#[must_use]
pub fn current_routed_skip_audit_tail(audit: &str, slug: &str) -> Option<Vec<Ordered>> {
    let events = ordered_main_workflow_audit(audit);
    let mut latest: Option<(usize, bool)> = None;
    for (index, event) in events.iter().enumerate() {
        if audit_field(&event.block, "Stage") != Some(slug) {
            continue;
        }
        match event.event.as_str() {
            "STAGE_STARTED" => latest = Some((index, false)),
            "STAGE_SKIPPED" => latest = Some((index, true)),
            _ => {}
        }
    }
    match latest {
        Some((index, true)) => Some(events[index + 1..].to_vec()),
        _ => None,
    }
}

/// `auditTailHasFields`: whether `events` holds an `event` carrying every
/// one of `fields`.
#[must_use]
pub fn audit_tail_has_fields(events: &[Ordered], event: &str, fields: &[(&str, &str)]) -> bool {
    events.iter().any(|row| {
        row.event == event
            && fields
                .iter()
                .all(|(field, value)| audit_field(&row.block, field) == Some(*value))
    })
}

// --- Decision pairing -------------------------------------------------------

const DECISION_CLOSING_EVENTS: [&str; 5] = [
    "QUESTION_ANSWERED",
    "SUMMARY_CONFIRMATION_RECORDED",
    "VERIFICATION_COMMAND_RECORDED",
    "CONSTRUCTION_POLICY_RECORDED",
    "PLAN_APPROVAL_RECORDED",
];
const DECISION_GATE_ANSWER_EVENTS: [&str; 2] = ["GATE_APPROVED", "GATE_REJECTED"];

/// `DECISION_PAIRING_EVENTS`: every event a decision reader looks at.
fn is_pairing_event(event: &str) -> bool {
    event == "DECISION_RECORDED"
        || DECISION_CLOSING_EVENTS.contains(&event)
        || DECISION_GATE_ANSWER_EVENTS.contains(&event)
}

/// `decisionAnsweredBy`: whether `event` closes the open decision.
fn decision_answered_by(open: Option<&str>, event: &str, block: &str) -> bool {
    if DECISION_CLOSING_EVENTS.contains(&event) {
        return true;
    }
    let Some(open) = open else {
        return false;
    };
    if !DECISION_GATE_ANSWER_EVENTS.contains(&event) {
        return false;
    }
    let checkpoint = block_field(open, "Checkpoint").unwrap_or("");
    let (gate_checkpoints, key): (&[&str], &str) = match checkpoint {
        "Swarm Batch Approval" => (&["swarm-batch"], "Batch number"),
        "Construction Unit Approval" => (&["construction-unit", "walking-skeleton"], "Unit"),
        _ => return false,
    };
    let Some(gate_checkpoint) = block_field(block, "Checkpoint") else {
        return false;
    };
    if !gate_checkpoints.contains(&gate_checkpoint) {
        return false;
    }
    let Some(want) = block_field(open, key) else {
        return false;
    };
    if block_field(block, key) != Some(want) {
        return false;
    }
    if checkpoint == "Construction Unit Approval"
        && let Some(kind) = block_field(open, "Kind")
    {
        let expected = match kind {
            "unit" => Some("construction-unit"),
            "skeleton" => Some("walking-skeleton"),
            _ => None,
        };
        if expected != Some(gate_checkpoint) {
            return false;
        }
    }
    match block_field(open, "Fingerprint") {
        None => true,
        Some(fingerprint) => block_field(block, "Fingerprint") == Some(fingerprint),
    }
}

/// `nextOpenDecision`: the open `DECISION_RECORDED` block after `event`.
#[must_use]
pub fn next_open_decision(open: Option<&str>, event: &str, block: &str) -> Option<String> {
    if event == "DECISION_RECORDED" {
        return Some(block.to_owned());
    }
    if decision_answered_by(open, event, block) {
        None
    } else {
        open.map(str::to_owned)
    }
}

/// The rows a decision reader walks, in the ledger's order: by instant,
/// then as the shards are read one after the other.
fn pairing_rows<'a>(rows: &'a [ShardEvent], also: Option<&str>) -> Vec<&'a ShardEvent> {
    let mut events: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| is_pairing_event(&row.event) || Some(row.event.as_str()) == also)
        .collect();
    events.sort_by(|a, b| {
        cmp_utf16(&a.timestamp, &b.timestamp)
            .then(a.shard_index.cmp(&b.shard_index))
            .then(a.pos.cmp(&b.pos))
    });
    events
}

/// `openDecisionBlock(pd, stage)`: the stage's open `DECISION_RECORDED`
/// block, or `None` when every question it was asked is answered.
#[must_use]
pub fn open_decision_block(rows: &[ShardEvent], stage: &str) -> Option<String> {
    let mut open: Option<String> = None;
    for row in pairing_rows(rows, None) {
        if block_field(&row.block, "Stage") == Some(stage) {
            open = next_open_decision(open.as_deref(), &row.event, &row.block);
        }
    }
    open
}

/// `hasPendingDecisionAtGate`: a question of the stage asked after its
/// gate last opened, and not answered since. An answer at an open gate
/// belongs to such a question; with none, it is the gate's own.
#[must_use]
pub fn has_pending_decision_at_gate(rows: &[ShardEvent], stage: &str) -> bool {
    let events = pairing_rows(rows, Some("STAGE_AWAITING_APPROVAL"));
    let Some(gate_open) = events.iter().rposition(|row| {
        row.event == "STAGE_AWAITING_APPROVAL" && block_field(&row.block, "Stage") == Some(stage)
    }) else {
        return false;
    };
    let mut open: Option<String> = None;
    for row in &events[gate_open + 1..] {
        if block_field(&row.block, "Stage") == Some(stage) {
            open = next_open_decision(open.as_deref(), &row.event, &row.block);
        }
    }
    open.is_some()
}

// --- Answer mode ------------------------------------------------------------

/// The `STAGE_STARTED` field recording the mode a stage starts with.
pub const ANSWER_MODE_FIELD: &str = "Answer Mode";
const ANSWER_MODE_QUESTION_PREFIX: &str = "How would you like to answer";
const ANSWER_MODES: [(&str, &str); 3] = [
    ("guide", "Guide me"),
    ("file", "I'll edit the file"),
    ("chat", "Chat"),
];

fn plain(text: &str) -> String {
    trim(text).replace('\u{2019}', "'").to_lowercase()
}

/// `isAnswerModeDecision`: a recorded decision that is the mode question.
#[must_use]
pub fn is_answer_mode_decision(block: &str) -> bool {
    if block_field(block, "Checkpoint").is_some() {
        return false;
    }
    if trim(block_field(block, "Decision").unwrap_or("")).starts_with(ANSWER_MODE_QUESTION_PREFIX) {
        return true;
    }
    let offered: Vec<String> = block_field(block, "Options")
        .unwrap_or("")
        .split(',')
        .map(plain)
        .filter(|option| !option.is_empty())
        .collect();
    offered.len() == ANSWER_MODES.len()
        && ANSWER_MODES
            .iter()
            .all(|(_, label)| offered.contains(&plain(label)))
}

/// `answerModeFromReply`: the mode a recorded answer names, by its label
/// or its number; `None` for anything else.
#[must_use]
pub fn answer_mode_from_reply(details: Option<&str>) -> Option<&'static str> {
    let details = details.filter(|details| !details.is_empty())?;
    let text = re::replace(r#"^["'`]+|["'`]+$"#, "g", trim(details), "");
    let text = plain(&text);
    let numbered = re::find(r"^([123])(?:\s*[.):]\s*(.*))?$", "", &text);
    let label = match &numbered {
        Some(found) => trim(re::group(&text, found, 2).unwrap_or("")).to_owned(),
        None => text.clone(),
    };
    let by_label = ANSWER_MODES
        .iter()
        .find(|(_, name)| name.to_lowercase() == label)
        .map(|(mode, _)| *mode);
    match numbered {
        Some(found) => {
            let number: usize = re::group(&text, &found, 1)?.parse().ok()?;
            let by_number = ANSWER_MODES[number - 1].0;
            if label.is_empty() || by_label == Some(by_number) {
                Some(by_number)
            } else {
                None
            }
        }
        None => by_label,
    }
}

/// `latestRecordedAnswerMode`: the person's latest answer to the mode
/// question, as `(mode, stage)`.
#[must_use]
pub fn latest_recorded_answer_mode(rows: &[ShardEvent]) -> Option<(&'static str, String)> {
    let mut events: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| is_pairing_event(&row.event))
        .filter(|row| !is_single_stage_row(&row.block))
        .collect();
    events.sort_by(|a, b| {
        cmp_utf16(&a.timestamp, &b.timestamp)
            .then(a.shard_index.cmp(&b.shard_index))
            .then(a.pos.cmp(&b.pos))
    });
    let mut open: Vec<(String, Option<String>)> = Vec::new();
    let mut latest = None;
    for row in events {
        let Some(stage) = block_field(&row.block, "Stage") else {
            continue;
        };
        let key = format!("{stage}\0{}", block_field(&row.block, "Unit").unwrap_or(""));
        let before = open
            .iter()
            .find(|(known, _)| *known == key)
            .and_then(|(_, open)| open.clone());
        let after = next_open_decision(before.as_deref(), &row.event, &row.block);
        if let Some(before) = &before
            && after.is_none()
            && row.event == "QUESTION_ANSWERED"
            && is_answer_mode_decision(before)
            && let Some(mode) = answer_mode_from_reply(block_field(&row.block, "Details"))
        {
            latest = Some((mode, stage.to_owned()));
        }
        match open.iter_mut().find(|(known, _)| *known == key) {
            Some(entry) => entry.1 = after,
            None => open.push((key, after)),
        }
    }
    latest
}

/// `answerModeStageStartedFields`: the `Answer Mode` field of a stage that
/// reuses the person's earlier choice; nothing when none is recorded.
#[must_use]
pub fn answer_mode_fields(rows: &[ShardEvent]) -> Vec<(&'static str, String)> {
    match latest_recorded_answer_mode(rows) {
        Some((mode, stage)) => vec![(ANSWER_MODE_FIELD, format!("{mode} (reused from {stage})"))],
        None => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(event: &str, at: &str, fields: &[(&str, &str)]) -> String {
        let mut text = format!("\n## {event}\n**Timestamp**: {at}\n**Event**: {event}\n");
        for (name, value) in fields {
            text.push_str(&format!("**{name}**: {value}\n"));
        }
        text.push_str("\n---\n");
        text
    }

    fn row(event: &str, at: &str, pos: usize, fields: &[(&str, &str)]) -> ShardEvent {
        ShardEvent {
            block: block(event, at, fields),
            event: event.to_owned(),
            pos,
            shard: "s.md".to_owned(),
            shard_index: 0,
            timestamp: at.to_owned(),
        }
    }

    #[test]
    fn a_stage_row_counts_only_in_the_current_workflow() {
        let audit = format!(
            "# Log\n{}{}{}{}",
            block("STAGE_COMPLETED", "2026-01-01T00:00:00Z", &[("Stage", "a")]),
            block(
                "WORKFLOW_STARTED",
                "2026-01-02T00:00:00Z",
                &[("Scope", "feature")]
            ),
            block("STAGE_COMPLETED", "2026-01-03T00:00:00Z", &[("Stage", "b")]),
            block(
                "STAGE_COMPLETED",
                "2026-01-04T00:00:00Z",
                &[("Stage", "c"), ("Workflow", "single-stage:c")]
            ),
        );
        assert!(!has_stage_audit_event(&audit, "STAGE_COMPLETED", "a"));
        assert!(has_stage_audit_event(&audit, "STAGE_COMPLETED", "b"));
        assert!(!has_stage_audit_event(&audit, "STAGE_COMPLETED", "c"));
        assert!(!has_stage_audit_event("", "STAGE_COMPLETED", "b"));
    }

    #[test]
    fn a_routed_skip_leaves_a_tail_until_the_stage_starts_again() {
        let audit = format!(
            "# Log\n{}{}{}{}",
            block("WORKFLOW_STARTED", "2026-01-01T00:00:00Z", &[]),
            block("STAGE_SKIPPED", "2026-01-02T00:00:00Z", &[("Stage", "a")]),
            block(
                "PHASE_COMPLETED",
                "2026-01-02T00:00:00Z",
                &[("From phase", "ideation"), ("To phase", "inception")]
            ),
            block("STAGE_STARTED", "2026-01-02T00:00:00Z", &[("Stage", "b")]),
        );
        let tail = current_routed_skip_audit_tail(&audit, "a").unwrap();
        assert_eq!(tail.len(), 2);
        assert!(audit_tail_has_fields(
            &tail,
            "PHASE_COMPLETED",
            &[("From phase", "ideation"), ("To phase", "inception")]
        ));
        assert!(!audit_tail_has_fields(
            &tail,
            "PHASE_COMPLETED",
            &[("To phase", "(end)")]
        ));
        assert!(audit_tail_has_fields(
            &tail,
            "STAGE_STARTED",
            &[("Stage", "b")]
        ));
        assert!(current_routed_skip_audit_tail(&audit, "b").is_none());
        let restarted = format!(
            "{audit}{}",
            block("STAGE_STARTED", "2026-01-03T00:00:00Z", &[("Stage", "a")])
        );
        assert!(current_routed_skip_audit_tail(&restarted, "a").is_none());
    }

    #[test]
    fn the_answer_mode_is_read_off_the_closed_mode_question() {
        assert_eq!(answer_mode_from_reply(Some("Guide me")), Some("guide"));
        assert_eq!(
            answer_mode_from_reply(Some("\"I\u{2019}ll edit the file\"")),
            Some("file")
        );
        assert_eq!(answer_mode_from_reply(Some("3")), Some("chat"));
        assert_eq!(
            answer_mode_from_reply(Some("2. I'll edit the file")),
            Some("file")
        );
        assert_eq!(answer_mode_from_reply(Some("2) Chat")), None);
        assert_eq!(answer_mode_from_reply(Some("whatever")), None);
        assert_eq!(answer_mode_from_reply(None), None);
        let question = row(
            "DECISION_RECORDED",
            "2026-01-01T00:00:00Z",
            1,
            &[
                ("Stage", "intent-capture"),
                (
                    "Decision",
                    "I made questions. How would you like to answer them?",
                ),
                ("Options", "Guide me, I'll edit the file, Chat"),
            ],
        );
        assert!(is_answer_mode_decision(&question.block));
        let answer = row(
            "QUESTION_ANSWERED",
            "2026-01-01T00:00:01Z",
            2,
            &[("Stage", "intent-capture"), ("Details", "Chat")],
        );
        let rows = vec![answer.clone(), question.clone()];
        assert_eq!(
            latest_recorded_answer_mode(&rows),
            Some(("chat", "intent-capture".to_owned()))
        );
        assert_eq!(
            answer_mode_fields(&rows),
            [(
                "Answer Mode",
                "chat (reused from intent-capture)".to_owned()
            )]
        );
        // An answer with no open question, or to another question, names nothing.
        assert_eq!(latest_recorded_answer_mode(&[answer]), None);
        let other = row(
            "DECISION_RECORDED",
            "2026-01-01T00:00:00Z",
            1,
            &[
                ("Stage", "intent-capture"),
                ("Decision", "Which colour?"),
                ("Options", "red, blue"),
            ],
        );
        let rows = vec![
            other,
            row(
                "QUESTION_ANSWERED",
                "2026-01-01T00:00:01Z",
                2,
                &[("Stage", "intent-capture"), ("Details", "Chat")],
            ),
        ];
        assert_eq!(latest_recorded_answer_mode(&rows), None);
        // A checkpoint decision is not the mode question, even with the options.
        let checkpoint = row(
            "DECISION_RECORDED",
            "2026-01-01T00:00:00Z",
            1,
            &[
                ("Stage", "x"),
                ("Checkpoint", "Plan Approval"),
                ("Options", "Guide me, I'll edit the file, Chat"),
            ],
        );
        assert!(!is_answer_mode_decision(&checkpoint.block));
    }

    #[test]
    fn a_gate_row_closes_only_its_own_checkpoint() {
        let open = "**Checkpoint**: Construction Unit Approval\n**Unit**: api\n**Kind**: unit\n**Fingerprint**: f1\n";
        let approved = "**Checkpoint**: construction-unit\n**Unit**: api\n**Fingerprint**: f1\n";
        assert_eq!(
            next_open_decision(Some(open), "GATE_APPROVED", approved),
            None
        );
        let other_unit = "**Checkpoint**: construction-unit\n**Unit**: web\n**Fingerprint**: f1\n";
        assert_eq!(
            next_open_decision(Some(open), "GATE_APPROVED", other_unit).as_deref(),
            Some(open)
        );
        let skeleton = "**Checkpoint**: walking-skeleton\n**Unit**: api\n**Fingerprint**: f1\n";
        assert_eq!(
            next_open_decision(Some(open), "GATE_REJECTED", skeleton).as_deref(),
            Some(open)
        );
        let stale = "**Checkpoint**: construction-unit\n**Unit**: api\n**Fingerprint**: f0\n";
        assert_eq!(
            next_open_decision(Some(open), "GATE_APPROVED", stale).as_deref(),
            Some(open)
        );
        assert_eq!(next_open_decision(None, "GATE_APPROVED", approved), None);
        assert_eq!(
            next_open_decision(Some(open), "STAGE_STARTED", "").as_deref(),
            Some(open)
        );
        assert_eq!(
            next_open_decision(Some(open), "DECISION_RECORDED", "**Decision**: x\n").as_deref(),
            Some("**Decision**: x\n")
        );
    }
}

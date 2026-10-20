//! Whether a person has acted: the human turns on the ledger against the
//! decisions that consume them.
//!
//! The human-turn hook writes a `HUMAN_TURN` row for every message the
//! person sends; a gate decision, an answer, a recorded confirmation
//! consumes the turn before it. A decision at a gate needs a turn after
//! the last such resolution, and, once the gate was put to the person,
//! after its presentation on this clone's shard. Shards are read one by
//! one: position orders rows within a shard; across shards a same-second
//! tie is unordered and fails closed.
//!
//! What a refusal for want of a reply adds: whether the person's message
//! since was a command, and whether the hooks that would have recorded a
//! reply are running here at all (the heartbeats under the engine
//! directory, and the harness's own advice when they are not).
//!
//! Mirrors `humanTurnState`, `humanActedSinceGate`, `humanRepliedSinceGate`,
//! `commandTurnHint`, `personRepliedSincePresentation`,
//! `gatePresentationStart`, `unattendedHumanPresenceHint`, `hookLiveness`,
//! `hooksNeverRanHere`, `promptHookRanRecently`, `hooksOffAgentStep`,
//! `hookActivation` and `inHostShell` in `aidlc-lib.ts`.

use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::{copied_blocks, shard_blocks};
use focus_js::date::parse_instant;
use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{json, re};
use focus_workspace::files::read_append_only;
use focus_workspace::names::{CLI, WORKSPACE_DIR};
use focus_workspace::records::valid_session_id;
use focus_workspace::{env, links, terminal};
use serde_json::Value;

use crate::workflow::Workflow;

/// The rows that resolve a gate or a question.
pub const GATE_RESOLUTION_EVENTS: [&str; 7] = [
    "GATE_APPROVED",
    "GATE_REJECTED",
    "QUESTION_ANSWERED",
    "SUMMARY_CONFIRMATION_RECORDED",
    "VERIFICATION_COMMAND_RECORDED",
    "CONSTRUCTION_POLICY_RECORDED",
    "PLAN_APPROVAL_RECORDED",
];
const DOCUMENT_AUDIT_EVENTS: [&str; 3] =
    ["DOCUMENT_INDEXED", "DOCUMENT_UPDATED", "DOCUMENT_REMOVED"];
/// The `Reply` marks of a turn that was only a command, or only a question
/// about a switch.
pub const COMMAND_TURN_REPLY: &str = "command";
pub const QUESTION_TURN_REPLY: &str = "question";

/// `isReplyTurn`: a human turn that replied.
#[must_use]
pub fn is_reply_turn(event: &str, block: &str) -> bool {
    if event != "HUMAN_TURN" {
        return false;
    }
    let mark = block_field(block, "Reply");
    mark != Some(COMMAND_TURN_REPLY) && mark != Some(QUESTION_TURN_REPLY)
}

fn is_request_turn(event: &str, block: &str) -> bool {
    event == "HUMAN_TURN" && block_field(block, "Reply") != Some(QUESTION_TURN_REPLY)
}

/// Where the latest human turn stands against the resolutions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Acted,
    Answered,
    Consumed,
    None,
}

#[derive(Debug, Clone)]
struct Mark {
    ts: String,
    shard: usize,
    pos: usize,
    human: bool,
    event: String,
}

fn before(a: &Mark, b: &Mark) -> bool {
    cmp_utf16(&a.ts, &b.ts).is_lt() || (a.ts == b.ts && a.shard == b.shard && a.pos < b.pos)
}

/// `usedOnlyByAnswers`.
fn used_only_by_answers(
    humans: &[Mark],
    resolutions: &[Mark],
    decisions: &[Mark],
    picks: &[(usize, usize)],
    latest: &str,
) -> bool {
    let latest: Vec<&Mark> = humans.iter().filter(|human| human.ts == latest).collect();
    if latest.len() != 1 {
        return false;
    }
    let turn = latest[0];
    if resolutions
        .iter()
        .any(|r| r.ts == turn.ts && r.shard != turn.shard)
    {
        return false;
    }
    let after: Vec<&Mark> = resolutions
        .iter()
        .filter(|r| {
            cmp_utf16(&r.ts, &turn.ts).is_gt()
                || (r.ts == turn.ts && r.shard == turn.shard && r.pos > turn.pos)
        })
        .collect();
    if after.is_empty() || !after.iter().all(|r| r.event == "QUESTION_ANSWERED") {
        return false;
    }
    if decisions.iter().all(|d| before(d, turn)) {
        return true;
    }
    after.len()
        < picks
            .iter()
            .filter(|(shard, at)| *shard == turn.shard && *at == turn.pos)
            .count()
}

/// `humanTurnState`: with `replies`, a turn that was only a command or a
/// question about a switch is no reply; with `requests`, such a question
/// ends the reach of the turns before it.
#[must_use]
pub fn human_turn_state(wf: &Workflow, replies: bool, requests: bool) -> TurnState {
    let ledger = wf.ledger();
    let mut unreadable = Vec::new();
    let Ok(shards) = ledger.shards(&wf.selector(), &wf.invoker, Some(&mut unreadable)) else {
        return TurnState::None;
    };
    let project =
        std::fs::canonicalize(&wf.layout.project).unwrap_or_else(|_| wf.layout.project.clone());
    let mut texts = Vec::new();
    for (index, shard) in shards.iter().enumerate() {
        match read_append_only(shard) {
            Some(bytes) => {
                let rel = shard.strip_prefix(&wf.layout.project).unwrap_or(shard);
                if links::refuse_link_in_chain(&project, rel).is_err() {
                    return TurnState::None;
                }
                texts.push((
                    index,
                    focus_audit::shards::ShardText {
                        shard: shard.to_string_lossy().into_owned(),
                        content: String::from_utf8_lossy(&bytes).into_owned(),
                    },
                ));
            }
            // Only a vanished shard may be skipped; anything else fails closed.
            None if !shard.exists() => {}
            None => return TurnState::None,
        }
    }
    if !unreadable.is_empty() {
        return TurnState::None;
    }
    let own = ledger.known_shard_name().ok().flatten();
    let only_texts: Vec<focus_audit::shards::ShardText> =
        texts.iter().map(|(_, t)| t.clone()).collect();
    let copied = copied_blocks(&only_texts, || own);
    let mut events: Vec<Mark> = Vec::new();
    let mut decisions: Vec<Mark> = Vec::new();
    let mut picks: Vec<(usize, usize)> = Vec::new();
    let mut saw_presence_tracking = false;
    for ((shard, text), skip) in texts.iter().zip(&copied) {
        let blocks = shard_blocks(&text.content);
        let mut last_turn: Option<usize> = None;
        for (pos, block) in blocks.iter().enumerate() {
            if skip.contains(&pos) {
                continue;
            }
            let Some(event) = block_field(block, "Event").filter(|event| !event.is_empty()) else {
                continue;
            };
            if !DOCUMENT_AUDIT_EVENTS.contains(&event) && event != "HOST_TURN" {
                saw_presence_tracking = true;
            }
            let ts = block_field(block, "Timestamp").unwrap_or("").to_owned();
            if event == "DECISION_RECORDED" {
                decisions.push(Mark {
                    ts: ts.clone(),
                    shard: *shard,
                    pos,
                    human: false,
                    event: event.to_owned(),
                });
            }
            if event == "HUMAN_TURN" {
                last_turn = Some(pos);
            }
            if event == "QUESTION_REPLIED"
                && let Some(turn) = last_turn
            {
                picks.push((*shard, turn));
            }
            let is_resolution = GATE_RESOLUTION_EVENTS.contains(&event)
                || event == "QUESTION_UNANSWERED"
                || (replies && event == "REQUEST_ROUTED")
                || (event == "AUTONOMY_MODE_SET"
                    && block_field(block, "Mode") == Some("autonomous"));
            if !is_resolution && event != "HUMAN_TURN" {
                continue;
            }
            if replies && event == "HUMAN_TURN" && !is_reply_turn(event, block) {
                continue;
            }
            let question_turn = requests && event == "HUMAN_TURN" && !is_request_turn(event, block);
            events.push(Mark {
                ts,
                shard: *shard,
                pos,
                human: event == "HUMAN_TURN" && !question_turn,
                event: if question_turn {
                    "QUESTION_TURN".to_owned()
                } else {
                    event.to_owned()
                },
            });
        }
    }
    if events.is_empty() {
        return if saw_presence_tracking {
            TurnState::None
        } else {
            TurnState::Acted
        };
    }
    let humans: Vec<Mark> = events.iter().filter(|e| e.human).cloned().collect();
    if humans.is_empty() {
        return TurnState::None;
    }
    let resolutions: Vec<Mark> = events.iter().filter(|e| !e.human).cloned().collect();
    if resolutions.is_empty() {
        return TurnState::Acted;
    }
    let latest_of = |marks: &[Mark]| -> String {
        marks.iter().fold(String::new(), |latest, mark| {
            if cmp_utf16(&mark.ts, &latest).is_gt() {
                mark.ts.clone()
            } else {
                latest
            }
        })
    };
    let latest_human = latest_of(&humans);
    let latest_resolution = latest_of(&resolutions);
    match cmp_utf16(&latest_human, &latest_resolution) {
        std::cmp::Ordering::Greater => TurnState::Acted,
        std::cmp::Ordering::Less => {
            if used_only_by_answers(&humans, &resolutions, &decisions, &picks, &latest_human) {
                TurnState::Answered
            } else {
                TurnState::Consumed
            }
        }
        std::cmp::Ordering::Equal => {
            let latest_humans: Vec<&Mark> =
                humans.iter().filter(|h| h.ts == latest_human).collect();
            let latest_resolutions: Vec<&Mark> = resolutions
                .iter()
                .filter(|r| r.ts == latest_resolution)
                .collect();
            let proven = latest_humans.iter().any(|human| {
                latest_resolutions
                    .iter()
                    .all(|r| r.shard == human.shard && r.pos < human.pos)
            });
            if proven {
                TurnState::Acted
            } else if used_only_by_answers(&humans, &resolutions, &decisions, &picks, &latest_human)
            {
                TurnState::Answered
            } else {
                TurnState::Consumed
            }
        }
    }
}

/// `humanActedSinceGate`.
#[must_use]
pub fn human_acted_since_gate(wf: &Workflow) -> bool {
    human_turn_state(wf, false, false) == TurnState::Acted
}

/// `humanRepliedSinceGate`: a turn that was only a command is no reply.
#[must_use]
pub fn human_replied_since_gate(wf: &Workflow) -> bool {
    human_turn_state(wf, true, false) == TurnState::Acted
}

/// `commandTurnHint`.
#[must_use]
pub fn command_turn_hint(wf: &Workflow) -> String {
    if human_acted_since_gate(wf) {
        " The person's message since then was a command to Focus, not a reply to this question: carry out the command and leave the question open for their reply.".to_owned()
    } else {
        String::new()
    }
}

/// `humanPresenceGuardDisabled`.
#[must_use]
pub fn human_presence_guard_disabled(wf: &Workflow) -> bool {
    focus_workspace::settings::Places::of(&wf.layout.project)
        .flag(&env::full_name("SKIP_HUMAN_PRESENCE_GUARD"), &|name| {
            std::env::var(name).ok()
        })
        .ok()
        .flatten()
        .as_deref()
        == Some("1")
}

const GATE_WORDS_LIFECYCLE_EVENTS: [&str; 7] = [
    "STAGE_STARTED",
    "STAGE_AWAITING_APPROVAL",
    "STAGE_REVISING",
    "STAGE_COMPLETED",
    "STAGE_SKIPPED",
    "GATE_APPROVED",
    "GATE_REJECTED",
];

fn answered_by(event: &str) -> bool {
    GATE_RESOLUTION_EVENTS.contains(&event)
        || event == "AUTONOMY_MODE_SET"
        || event == "REQUEST_ROUTED"
}

/// `gatePresentationStart`: the byte offset where the stage's gate words
/// may begin in this clone's shard, or `None` when its latest lifecycle
/// row is not a presentation.
#[must_use]
pub fn gate_presentation_start(content: &str, stage: &str, first_showing: bool) -> Option<usize> {
    let separator = re::compile(r"\r?\n---\r?\n", "g");
    let mut start = 0;
    let mut from: Option<usize> = None;
    let mut shown: Option<usize> = None;
    let mut revised = false;
    let mut matches = separator.find_iter(content).peekable();
    loop {
        let next = matches.next();
        let end = next
            .as_ref()
            .map_or(content.len(), |found| found.range().start);
        let block = content[start..end].replace("\r\n", "\n");
        let event = block_field(&block, "Event");
        if let Some(event) = event
            && GATE_WORDS_LIFECYCLE_EVENTS.contains(&event)
            && block_field(&block, "Stage") == Some(stage)
        {
            if event == "STAGE_AWAITING_APPROVAL"
                && block_field(&block, "Recovered") != Some("true")
            {
                from = Some(match shown {
                    Some(at) if first_showing && revised => at,
                    _ => start,
                });
                if !revised || shown.is_none() {
                    shown = Some(start);
                }
            } else {
                from = None;
                if event == "STAGE_REVISING" {
                    revised = true;
                } else if event != "GATE_REJECTED" {
                    shown = None;
                    revised = false;
                }
            }
        } else if let Some(event) = event
            && from.is_some()
            && answered_by(event)
        {
            from = Some(start);
        }
        match next {
            None => break,
            Some(found) => start = found.range().end,
        }
    }
    from
}

/// `personRepliedSincePresentation`: whether a reply turn is on this
/// clone's shard after the stage's latest presentation; `None` when the
/// stage is not waiting on that question, or the shard cannot be read.
#[must_use]
pub fn person_replied_since_presentation(
    wf: &Workflow,
    stage: &str,
    first_showing: bool,
) -> Option<bool> {
    let shard = wf.ledger().audit_file(&wf.selector(), &wf.invoker).ok()?;
    let content = String::from_utf8_lossy(&read_append_only(&shard)?).into_owned();
    let from = gate_presentation_start(&content, stage, first_showing)?;
    Some(
        shard_blocks(&content[from..])
            .iter()
            .any(|block| is_reply_turn(block_field(block, "Event").unwrap_or(""), block)),
    )
}

// --- Hooks ---------------------------------------------------------------------

/// Five minutes: heartbeats and rows are written in the same turn.
const HOOK_HEARTBEAT_STALE_SLACK_MS: i64 = 5 * 60 * 1000;
const HOOKS_OFF_AGENT_RULES: &str = "Do not run Focus's hook scripts yourself, do not offer to switch any Focus check off, and do not look for another cause. While you fix this, do not take on other doctor problems, and change nothing outside this project's folder.";
const NO_CHECK_OFF_OFFER: &str = "Never offer to turn a check off for them.";
const NOT_ANSWERED_YET_STEP: &str =
    "The person has not answered yet: end your turn; their next reply answers it.";

/// The harness's hook-activation advice, read off its manifest.
fn hook_activation(wf: &Workflow) -> Option<Value> {
    let bytes = std::fs::read(wf.layout.manifest()).ok()?;
    json::parse(&String::from_utf8_lossy(&bytes))
        .ok()?
        .get("hookActivation")
        .cloned()
        .filter(Value::is_object)
}

/// `inHostShell`: the environment matches the host's marks (`NAME=value`,
/// or `NAME` for any value).
fn in_host_shell(marks: Option<&Value>, lookup: &dyn Fn(&str) -> Option<String>) -> bool {
    let entries: Vec<(Option<String>, Option<String>)> = marks
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|entry| {
            let (name, value) = entry
                .split_once('=')
                .map_or((entry, None), |(n, v)| (n, Some(v)));
            (
                value.map(str::to_lowercase),
                lookup(name).map(|actual| trim(&actual).to_lowercase()),
            )
        })
        .collect();
    if entries.iter().any(|(value, actual)| {
        value.is_some()
            && actual
                .as_deref()
                .is_some_and(|actual| !actual.is_empty() && Some(actual) != value.as_deref())
    }) {
        return false;
    }
    entries.iter().any(|(value, actual)| match value {
        None => actual.as_deref().is_some_and(|actual| !actual.is_empty()),
        Some(value) => actual.as_deref() == Some(value),
    })
}

fn fill_activation_text(text: &str, project: &Path) -> String {
    text.replace("<entry>", "/focus")
        .replace("<next>", &format!("`{CLI} engine orchestrate next`"))
        .replace("<folder>", &project.display().to_string())
}

/// `plainProjectFile`: nothing, or a regular file with one name, behind
/// no link.
fn plain_project_file(project: &Path, rel: &str) -> bool {
    if links::refuse_link_in_chain(project, Path::new(rel)).is_err() {
        return false;
    }
    match std::fs::symlink_metadata(project.join(rel)) {
        Err(_) => true,
        Ok(meta) => meta.is_file() && hard_links(&meta) == 1,
    }
}

#[cfg(unix)]
fn hard_links(meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    meta.nlink()
}

#[cfg(not(unix))]
fn hard_links(_meta: &std::fs::Metadata) -> u64 {
    1
}

/// `hooksOffAgentStep`: what the agent does when this harness's hooks are
/// not running here; `None` for a harness that declares no step.
fn hooks_off_agent_step(wf: &Workflow) -> Option<String> {
    let activation = hook_activation(wf)?;
    let step = activation.get("agentStep").and_then(Value::as_str)?;
    let project = &wf.layout.project;
    if let Some(edits) = activation.get("agentStepEdits").and_then(Value::as_str)
        && !plain_project_file(project, edits)
    {
        return Some(format!(
            "{HOOKS_OFF_AGENT_RULES} `{edits}` in this project is a link, so do not change it. Show the person this line and end your turn: \"{edits} in this project is a link, so it was left as it is. Make it a plain file in this project, then send your next message.\""
        ));
    }
    let in_host = activation.get("agentStepInHost");
    let text = match in_host {
        Some(host) if in_host_shell(host.get("env"), &|name| std::env::var(name).ok()) => {
            host.get("text").and_then(Value::as_str).unwrap_or(step)
        }
        _ => step,
    };
    Some(format!(
        "{HOOKS_OFF_AGENT_RULES} {}",
        fill_activation_text(text, project)
    ))
}

/// The hooks-health directory of the record (`hooksHealthReadDir`).
fn hooks_health_dir(wf: &Workflow) -> std::path::PathBuf {
    wf.selection.engine_dir(&wf.layout).join("hooks-health")
}

struct Liveness {
    has_hook_fired_content: bool,
    newest_stage_or_gate_event_ms: Option<i64>,
}

/// `hookLiveness`, as far as a refusal reads it.
fn hook_liveness(wf: &Workflow) -> Liveness {
    let dir = hooks_health_dir(wf);
    let has_hook_fired_content = std::fs::read_dir(&dir).is_ok_and(|entries| {
        entries
            .filter_map(std::result::Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().ends_with(".last"))
    });
    let newest = wf
        .events()
        .iter()
        .filter(|event| event.event.starts_with("STAGE_") || event.event.starts_with("GATE_"))
        .filter_map(|event| parse_instant(&event.timestamp))
        .fold(None, |newest: Option<i64>, ms| {
            Some(newest.map_or(ms, |n| n.max(ms)))
        });
    Liveness {
        has_hook_fired_content,
        newest_stage_or_gate_event_ms: newest,
    }
}

/// `hooksNeverRanHere`: a stage or gate event and no heartbeat at all,
/// with no link on the way to the status files.
fn hooks_never_ran_here(wf: &Workflow) -> bool {
    let liveness = hook_liveness(wf);
    let never_fired =
        !liveness.has_hook_fired_content && liveness.newest_stage_or_gate_event_ms.is_some();
    never_fired && !hook_status_path_linked(wf)
}

fn hook_status_path_linked(wf: &Workflow) -> bool {
    let Ok(anchor) = std::fs::canonicalize(&wf.layout.project) else {
        return false;
    };
    let dir = hooks_health_dir(wf);
    let Ok(rel) = dir.strip_prefix(&wf.layout.project) else {
        return false;
    };
    let parts: Vec<String> = rel
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    (1..=parts.len()).any(|count| {
        links::refuse_link_in_chain(&anchor, Path::new(&parts[..count].join("/"))).is_err()
    })
}

/// `promptHookRanRecently`: the prompt hook's heartbeat is recent and the
/// work has not moved on since.
fn prompt_hook_ran_recently(wf: &Workflow) -> bool {
    let path = hooks_health_dir(wf).join("record-human-turn.last");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Some(beat) = parse_instant(trim(&raw)) else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
        });
    let age = now - beat;
    if !(0..=HOOK_HEARTBEAT_STALE_SLACK_MS).contains(&age) {
        return false;
    }
    match hook_liveness(wf).newest_stage_or_gate_event_ms {
        None => true,
        Some(newest) => newest - beat <= HOOK_HEARTBEAT_STALE_SLACK_MS,
    }
}

/// `unattendedHumanPresenceHint`: what a refusal for want of a reply adds.
/// (Upstream's `missedReply: false`, a refusal for a reply not given yet,
/// is the log tool's, M5.)
#[must_use]
pub fn unattended_human_presence_hint(wf: &Workflow) -> String {
    if env::var("UNATTENDED").as_deref() == Some("1") {
        return format!(
            " {}=1 is set, so automated prompt submissions cannot count as a human reply. Unset {} before returning to interactive mode, then submit a new human response.",
            env::full_name("UNATTENDED"),
            env::full_name("UNATTENDED")
        );
    }
    if terminal::person_at_own_terminal(&wf.layout) {
        return format!(" {}", terminal::OWN_TERMINAL_PRESENCE_STEP);
    }
    if hooks_never_ran_here(wf)
        && let Some(step) = hooks_off_agent_step(wf)
    {
        return format!(
            " If the person already replied, that reply was not recorded because Focus's hooks are not running here, so do not ask them to answer again; do this instead: {step} {NO_CHECK_OFF_OFFER}"
        );
    }
    if prompt_hook_ran_recently(wf) {
        return format!(" {NOT_ANSWERED_YET_STEP}");
    }
    let activation = hook_activation(wf);
    let host = activation
        .as_ref()
        .and_then(|activation| activation.get("missedReplyInHost"));
    let missed = match host {
        Some(host) if in_host_shell(host.get("env"), &|name| std::env::var(name).ok()) => {
            host.get("text").and_then(Value::as_str).map(str::to_owned)
        }
        _ => activation
            .as_ref()
            .and_then(|activation| activation.get("missedReply"))
            .and_then(Value::as_str)
            .map(str::to_owned),
    }
    .unwrap_or_else(|| {
        "If the person already replied, that reply was not recorded for this question. Tell them exactly this, with nothing about why: \"Your answer didn't reach Focus. Please give it once more. If it happens again, type /focus --doctor.\"".to_owned()
    });
    format!(" {missed} {NO_CHECK_OFF_OFFER}")
}

/// Where a gate-words record would be for the session, when one is there:
/// the human-turn hook's record of what the person typed (M4), which Focus
/// does not read.
#[must_use]
pub fn gate_words_record(wf: &Workflow) -> Option<std::path::PathBuf> {
    let session = wf
        .selection
        .session_id
        .as_deref()
        .or(wf.invoker.session_override.as_deref())?;
    valid_session_id(Some(session))?;
    let segment = focus_intent::create::runtime_session_segment(session);
    if segment.is_empty() {
        return None;
    }
    let path = wf
        .selection
        .engine_dir(&wf.layout)
        .join("gate-words")
        .join(format!("{segment}.json"));
    path.exists().then_some(path)
}

/// The workspace directory's name, for messages that name the tree.
#[must_use]
pub fn workspace_dir() -> &'static str {
    WORKSPACE_DIR
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

    #[test]
    fn a_presentation_starts_the_gate_words() {
        let shown = block(
            "STAGE_AWAITING_APPROVAL",
            "2026-01-01T00:00:01Z",
            &[("Stage", "a")],
        );
        let content = format!(
            "# Log\n{}{shown}",
            block("STAGE_STARTED", "2026-01-01T00:00:00Z", &[("Stage", "a")])
        );
        let from = gate_presentation_start(&content, "a", false).unwrap();
        assert!(
            content[from..].starts_with("\n## STAGE_AWAITING_APPROVAL")
                || content[from..].starts_with("## STAGE_AWAITING_APPROVAL"),
            "{}",
            &content[from..from + 40]
        );
        // Another stage's rows do not count; a decision closes it; a backfilled row is no showing.
        assert!(gate_presentation_start(&content, "b", false).is_none());
        let decided = format!(
            "{content}{}",
            block("GATE_APPROVED", "2026-01-01T00:00:02Z", &[("Stage", "a")])
        );
        assert!(gate_presentation_start(&decided, "a", false).is_none());
        let backfilled = format!(
            "# Log\n{}",
            block(
                "STAGE_AWAITING_APPROVAL",
                "2026-01-01T00:00:01Z",
                &[("Stage", "a"), ("Recovered", "true")]
            )
        );
        assert!(gate_presentation_start(&backfilled, "a", false).is_none());
        // An answer to another question after the showing moves the start past it.
        let answered = format!(
            "{content}{}",
            block(
                "QUESTION_ANSWERED",
                "2026-01-01T00:00:02Z",
                &[("Stage", "a")]
            )
        );
        let later = gate_presentation_start(&answered, "a", false).unwrap();
        assert!(later > from);
        // A re-entered gate counts from the first showing of the attempt with `first_showing`.
        let reentered = format!(
            "{content}{}{}{}",
            block("GATE_REJECTED", "2026-01-01T00:00:02Z", &[("Stage", "a")]),
            block("STAGE_REVISING", "2026-01-01T00:00:02Z", &[("Stage", "a")]),
            block(
                "STAGE_AWAITING_APPROVAL",
                "2026-01-01T00:00:03Z",
                &[("Stage", "a")]
            )
        );
        assert_eq!(gate_presentation_start(&reentered, "a", true), Some(from));
        assert!(gate_presentation_start(&reentered, "a", false).unwrap() > from);
    }

    #[test]
    fn a_turn_replies_unless_it_was_a_command() {
        assert!(is_reply_turn("HUMAN_TURN", "**Event**: HUMAN_TURN\n"));
        assert!(!is_reply_turn(
            "HUMAN_TURN",
            "**Event**: HUMAN_TURN\n**Reply**: command\n"
        ));
        assert!(!is_reply_turn(
            "HUMAN_TURN",
            "**Event**: HUMAN_TURN\n**Reply**: question\n"
        ));
        assert!(!is_reply_turn("GATE_APPROVED", ""));
    }

    #[test]
    fn host_marks_are_matched_against_the_environment() {
        let marks = serde_json::json!(["TERM_PROGRAM=cursor", "CURSOR_TRACE_ID"]);
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |name: &str| {
                vars.iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert!(!in_host_shell(Some(&marks), &env(&[])));
        assert!(in_host_shell(
            Some(&marks),
            &env(&[("CURSOR_TRACE_ID", "x")])
        ));
        assert!(!in_host_shell(
            Some(&marks),
            &env(&[("TERM_PROGRAM", "vscode")])
        ));
        assert!(!in_host_shell(
            Some(&marks),
            &env(&[("TERM_PROGRAM", "vscode"), ("CURSOR_TRACE_ID", "x")])
        ));
        assert!(in_host_shell(
            Some(&marks),
            &env(&[("TERM_PROGRAM", " Cursor ")])
        ));
        assert!(!in_host_shell(None, &env(&[("TERM_PROGRAM", "cursor")])));
    }

    /// An installed project with one record and two shards, opened as a workflow.
    fn workflow(name: &str, mine: &str, theirs: &str) -> (Workflow, std::path::PathBuf) {
        use focus_harness::Harness;
        use focus_workspace::Layout;
        use focus_workspace::records::{Invoker, Selection};
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
        let root =
            std::env::temp_dir().join(format!("focus-presence-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fixtures =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream/installed/claude");
        copy_dir(&fixtures, &root);
        let record = root.join("focus/spaces/default/intents/260105-add-login");
        std::fs::create_dir_all(record.join("audit")).unwrap();
        std::fs::write(record.join("focus-state.md"), "- **Scope**: feature\n").unwrap();
        std::fs::write(record.join("audit/laptop-abc123.md"), mine).unwrap();
        std::fs::write(record.join("audit/desktop-def456.md"), theirs).unwrap();
        std::fs::write(root.join("focus/.focus-clone-id"), "abc123\nlaptop\n").unwrap();
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

    /// The latest turn used only by answers is `Answered`; a decision recorded
    /// in the same second on another shard is not before it, and the turn is
    /// then `Consumed`. The states beyond `Acted` are the question flow's.
    #[test]
    fn a_turn_used_only_by_answers_is_told_from_a_consumed_one() {
        let shown = block(
            "STAGE_AWAITING_APPROVAL",
            "2026-01-01T00:00:01Z",
            &[("Stage", "a")],
        );
        let turn = block("HUMAN_TURN", "2026-01-01T00:00:02Z", &[("Session", "s1")]);
        let answer = block(
            "QUESTION_ANSWERED",
            "2026-01-01T00:00:03Z",
            &[("Stage", "a")],
        );
        let mine = format!("# Log\n{shown}{turn}{answer}");
        let decided_before = block(
            "DECISION_RECORDED",
            "2026-01-01T00:00:01Z",
            &[("Stage", "a")],
        );
        let (wf, root) = workflow("answered", &mine, &format!("# Log\n{decided_before}"));
        assert_eq!(human_turn_state(&wf, true, false), TurnState::Answered);
        let _ = std::fs::remove_dir_all(root);
        let decided_tied = block(
            "DECISION_RECORDED",
            "2026-01-01T00:00:02Z",
            &[("Stage", "a")],
        );
        let (wf, root) = workflow("consumed", &mine, &format!("# Log\n{decided_tied}"));
        assert_eq!(human_turn_state(&wf, true, false), TurnState::Consumed);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A shard directory behind a link is not read: the state is `None`. The
    /// hooks' health directory behind a link is not read either: the hooks
    /// are not said to have never run.
    #[cfg(unix)]
    #[test]
    fn a_directory_behind_a_link_fails_closed() {
        let turn = block("HUMAN_TURN", "2026-01-01T00:00:02Z", &[("Session", "s1")]);
        let started = block("STAGE_STARTED", "2026-01-01T00:00:01Z", &[("Stage", "a")]);
        let (wf, root) = workflow("linked", &format!("# Log\n{started}{turn}"), "# Log\n");
        assert_eq!(human_turn_state(&wf, true, false), TurnState::Acted);
        assert!(hooks_never_ran_here(&wf));
        let record = root.join("focus/spaces/default/intents/260105-add-login");
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), record.join(".focus-engine")).unwrap();
        assert!(!hooks_never_ran_here(&wf));
        std::fs::rename(record.join("audit"), root.join("audit-elsewhere")).unwrap();
        std::os::unix::fs::symlink(root.join("audit-elsewhere"), record.join("audit")).unwrap();
        assert_eq!(human_turn_state(&wf, true, false), TurnState::None);
        let _ = std::fs::remove_dir_all(root);
    }
}

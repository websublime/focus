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
use focus_audit::shards::{ShardEvent, copied_blocks, shard_blocks};
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

/// `isRequestTurn`: a human turn that can carry a request (anything but a
/// question about a switch).
#[must_use]
pub fn is_request_turn(event: &str, block: &str) -> bool {
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

/// `requestOutlivesItsApproval`: the person's latest turn asked for what
/// runs now, and the approvals given since are the one in that message and
/// the run's own, which do not use it up.
fn request_outlives_its_approval(wf: &Workflow, replies: bool) -> bool {
    let mut unreadable = Vec::new();
    let Ok(rows) = wf
        .ledger()
        .read_events(&wf.selector(), &wf.invoker, Some(&mut unreadable))
    else {
        return false;
    };
    if !unreadable.is_empty() {
        return false;
    }
    let turns: Vec<&ShardEvent> = rows
        .iter()
        .filter(|row| row.event == "HUMAN_TURN")
        .collect();
    let latest_ts = turns.iter().fold(String::new(), |latest, row| {
        if cmp_utf16(&row.timestamp, &latest).is_gt() {
            row.timestamp.clone()
        } else {
            latest
        }
    });
    let latest: Vec<&ShardEvent> = turns
        .iter()
        .copied()
        .filter(|row| row.timestamp == latest_ts)
        .collect();
    let Some(first) = latest.first() else {
        return false;
    };
    if latest
        .iter()
        .any(|row| row.shard_index != first.shard_index)
    {
        return false;
    }
    let turn = latest.iter().copied().fold(
        *first,
        |last, row| if row.pos > last.pos { row } else { last },
    );
    if !is_request_turn(&turn.event, &turn.block)
        || (replies && !is_reply_turn(&turn.event, &turn.block))
    {
        return false;
    }
    let mut theirs: Vec<String> = Vec::new();
    for row in &rows {
        let after = cmp_utf16(&row.timestamp, &turn.timestamp).is_gt()
            || (row.timestamp == turn.timestamp
                && (row.shard_index != turn.shard_index || row.pos > turn.pos));
        if !after {
            continue;
        }
        if row.event == "GATE_APPROVED" {
            if block_field(&row.block, "User Input").is_none()
                || block_field(&row.block, "Autonomous") == Some("true")
                || block_field(&row.block, crate::gate::APPROVED_TOGETHER_WITH_FIELD).is_some()
            {
                continue;
            }
            let key = format!("{}:{}", row.shard_index, row.pos);
            if !theirs.contains(&key) {
                theirs.push(key);
            }
        } else if row.event == "PLAN_APPROVAL_RECORDED" {
            if !theirs.iter().any(|key| key == "plan") {
                theirs.push("plan".to_owned());
            }
        } else if GATE_RESOLUTION_EVENTS.contains(&row.event.as_str())
            || row.event == "QUESTION_UNANSWERED"
            || (row.event == "AUTONOMY_MODE_SET"
                && block_field(&row.block, "Mode") == Some("autonomous"))
        {
            return false;
        }
    }
    theirs.is_empty() || (theirs.len() == 1 && is_reply_turn(&turn.event, &turn.block))
}

/// `personSpokeSinceGate`: a person has spoken since the last decision and
/// a human turn is on record. With `replies`, the turn must be a reply;
/// with `requests`, anything but a question about a switch; with
/// `outlives_approval`, the approval given in the message and the run's own
/// do not use it up.
#[must_use]
pub fn person_spoke_since_gate(
    wf: &Workflow,
    replies: bool,
    requests: bool,
    outlives_approval: bool,
) -> bool {
    if human_turn_state(wf, replies, requests) != TurnState::Acted
        && !(outlives_approval && request_outlives_its_approval(wf, replies))
    {
        return false;
    }
    let Ok(rows) = wf.ledger().read_events(&wf.selector(), &wf.invoker, None) else {
        return false;
    };
    rows.iter().any(|row| {
        if replies {
            is_reply_turn(&row.event, &row.block)
        } else if requests {
            is_request_turn(&row.event, &row.block)
        } else {
            row.event == "HUMAN_TURN"
        }
    })
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
pub(crate) fn hook_activation(wf: &Workflow) -> Option<Value> {
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
        Ok(meta) => meta.is_file() && focus_workspace::files::hard_links(&meta) == 1,
    }
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
#[must_use]
pub fn unattended_human_presence_hint(wf: &Workflow) -> String {
    presence_hint(wf, true)
}

/// `unattendedHumanPresenceHint` with `missedReply: false`: the caller
/// refuses for a reply not given yet, not for one the hooks missed, so the
/// lines about a reply that did not arrive are left out.
#[must_use]
pub fn unattended_human_presence_hint_for_a_reply_not_given(wf: &Workflow) -> String {
    presence_hint(wf, false)
}

fn presence_hint(wf: &Workflow, missed_reply: bool) -> String {
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
    if !missed_reply {
        return String::new();
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

/// `GATE_WORDS_MAX_FILE_BYTES`: a ceiling against a corrupt or foreign
/// record.
const GATE_WORDS_MAX_FILE_BYTES: u64 = 256 * 1024;

/// The gate-words record the human-turn hook keeps for a chat: what the
/// person typed there, indexed by the shard's size after each turn's row.
struct GateWordsRecord {
    /// The project-relative audit shard the offsets index.
    shard: String,
    /// The largest offset of a message that was not kept, or 0.
    dropped: u64,
    messages: Vec<(u64, String)>,
}

/// `resolveInvokingSessionId`: the chat this command runs for, as the
/// selection resolved it from the environment (no command names a session
/// at a gate).
fn invoking_session(wf: &Workflow) -> Option<&str> {
    valid_session_id(wf.selection.session_id.as_deref())
}

/// `readGateWords`: the record of the chat this command runs for, when it
/// is one of ours.
fn read_gate_words(wf: &Workflow) -> Option<GateWordsRecord> {
    read_gate_words_of(wf, invoking_session(wf)?)
}

/// `gateWordsPath`: where a chat's gate-words record is, whether or not
/// one is there; nowhere for a chat whose name leaves no file name.
fn gate_words_path(wf: &Workflow, session: &str) -> Option<std::path::PathBuf> {
    let segment = focus_intent::create::runtime_session_segment(session);
    if segment.is_empty() {
        return None;
    }
    Some(
        wf.selection
            .engine_dir(&wf.layout)
            .join("gate-words")
            .join(format!("{segment}.json")),
    )
}

/// `readGateWords(pd, session)`: the record a chat's turns are kept in.
fn read_gate_words_of(wf: &Workflow, session: &str) -> Option<GateWordsRecord> {
    let path = gate_words_path(wf, session)?;
    if !path.exists() {
        return None;
    }
    let bytes = focus_workspace::files::read_regular(&path, "gate words").ok()?;
    if bytes.len() as u64 > GATE_WORDS_MAX_FILE_BYTES {
        return None;
    }
    let value = json::parse(&String::from_utf8_lossy(&bytes)).ok()?;
    let record = value.as_object()?;
    if record.get("version") != Some(&Value::from(1))
        || record.get("session").and_then(Value::as_str) != Some(session)
    {
        return None;
    }
    let shard = record.get("shard")?.as_str()?.to_owned();
    let dropped = safe_integer(record.get("dropped")?)?;
    let messages = record
        .get("messages")?
        .as_array()?
        .iter()
        .map(|message| {
            let message = message.as_object()?;
            Some((
                safe_integer(message.get("offset")?)?,
                message.get("text")?.as_str()?.to_owned(),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(GateWordsRecord {
        shard,
        dropped,
        messages,
    })
}

/// This clone's shard of the record: its text, and its path in the project
/// as a gate-words record names it.
fn own_shard(wf: &Workflow) -> Option<(String, String)> {
    let shard = wf.ledger().audit_file(&wf.selector(), &wf.invoker).ok()?;
    let relative = shard
        .strip_prefix(&wf.layout.project)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    let content = String::from_utf8_lossy(&read_append_only(&shard)?).into_owned();
    Some((content, relative))
}

/// `latestPersonTurn(pd)?.words`: the words the person's latest chat turn
/// in this clone's shard was kept with, right after its row; `None` when
/// there is no turn, or its chat kept none (a slash command, a picked
/// option, a message too long to keep). It only words a record, so nothing
/// unreadable is an error.
#[must_use]
pub fn latest_person_turn_words(wf: &Workflow) -> Option<String> {
    let (content, relative) = own_shard(wf)?;
    // The last `HUMAN_TURN` block, with the bytes it and its separator take.
    let separator = re::compile(r"\r?\n---\r?\n", "g");
    let mut turn: Option<(Option<String>, usize, usize)> = None;
    let mut start = 0;
    let mut ends: Vec<(usize, usize)> = separator
        .find_iter(&content)
        .map(|found| (found.range().start, found.range().end))
        .collect();
    ends.push((content.len(), content.len()));
    for (end, next) in ends {
        let block = content[start..end].replace("\r\n", "\n");
        if block_field(&block, "Event") == Some("HUMAN_TURN") {
            turn = Some((
                block_field(&block, "Session").map(str::to_owned),
                start,
                next,
            ));
        }
        start = next;
    }
    let (session, from, to) = turn?;
    let record = read_gate_words_of(
        wf,
        session.as_deref().filter(|session| !session.is_empty())?,
    )?;
    if record.shard != relative {
        return None;
    }
    // The hook keeps a turn's words at the shard's size right after its row.
    record
        .messages
        .into_iter()
        .find(|(offset, _)| *offset > from as u64 && *offset <= to as u64)
        .map(|(_, text)| text)
}

/// `pickerAnswerNote`: where the person's latest turn was a reply in a
/// question box, the note an answer gets when none of their picks carried
/// it: the agent logged a choice they never saw, or one they did not pick.
/// `None` when the turn was typed, when a pick matches (with or without the
/// recommended decorator), or when the answer is only an option's letter
/// or number. A note on the record, never a refusal.
#[must_use]
pub fn picker_answer_note(wf: &Workflow, details: &str) -> Option<String> {
    let (content, _) = own_shard(wf)?;
    let content = content.replace("\r\n", "\n");
    let turn = content
        .split("\n---\n")
        .filter(|block| block_field(block, "Event") == Some("HUMAN_TURN"))
        .last()?;
    let raw = block_field(turn, "Picked").filter(|raw| !raw.is_empty())?;
    let picked = json::parse(raw).ok()?;
    let picked: Vec<&str> = picked
        .as_array()
        .filter(|picked| !picked.is_empty())?
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()?;
    let plain = |text: &str| {
        focus_js::text::to_lower_case(trim(&crate::reply::strip_recommended_decorator(text)))
    };
    let answer = plain(details);
    if answer.is_empty() || re::test(r"^(?:[a-z]|\d+)[.)]?$", "", &answer) {
        return None;
    }
    if picked.iter().any(|pick| {
        let pick = plain(pick);
        pick == answer || answer.contains(&pick) || pick.contains(&answer)
    }) {
        return None;
    }
    Some(format!(
        "Not what the person picked in the picker ({}).",
        picked
            .iter()
            .map(|pick| format!("\"{pick}\""))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// `humanTurnMintAllowed`: an unattended run has nobody to reply.
#[must_use]
pub fn attended() -> bool {
    env::var("UNATTENDED").as_deref() != Some("1")
}

/// `Number.isSafeInteger` for an offset: a whole number of bytes.
fn safe_integer(value: &Value) -> Option<u64> {
    let number = value.as_f64()?;
    (number.fract() == 0.0 && number.abs() <= 9_007_199_254_740_991.0 && number >= 0.0)
        .then_some(number as u64)
}

/// `gateWordsSincePresentation`: the messages this chat's person typed
/// since the stage's latest presentation, in order; `None` when there are
/// none, the record is another shard's, or a message typed since was not
/// kept (the words kept are not everything the person said).
#[must_use]
pub fn gate_words_since_presentation(wf: &Workflow, stage: &str) -> Option<Vec<String>> {
    let record = read_gate_words(wf)?;
    if record.messages.is_empty() {
        return None;
    }
    let shard = wf.ledger().audit_file(&wf.selector(), &wf.invoker).ok()?;
    let relative = shard
        .strip_prefix(&wf.layout.project)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    if relative != record.shard {
        return None;
    }
    let content = String::from_utf8_lossy(&read_append_only(&shard)?).into_owned();
    let from = gate_presentation_start(&content, stage, false)?;
    let floor = from as u64;
    if record.dropped > floor {
        return None;
    }
    let words: Vec<String> = record
        .messages
        .into_iter()
        .filter(|(offset, _)| *offset > floor)
        .map(|(_, text)| text)
        .collect();
    (!words.is_empty()).then_some(words)
}

/// `personsLatestGatePick`: the option the person's latest message at the
/// gate picked exactly ("2", "Request Changes"), or `None` for anything
/// else. Approve and Request Changes are always the gate's first two
/// options; Accept as-is is offered third once the revision cap is reached.
#[must_use]
pub fn persons_latest_gate_pick(
    wf: &Workflow,
    stage: &str,
    accept_as_is: bool,
) -> Option<&'static str> {
    let words: Vec<String> = gate_words_since_presentation(wf, stage)
        .unwrap_or_default()
        .into_iter()
        .filter(|text| !crate::reply::is_non_answer(Some(text)))
        .collect();
    let labels: &[&str] = if accept_as_is {
        &[
            "Approve",
            "Request Changes",
            crate::reply::ACCEPT_AS_IS_CHOICE,
        ]
    } else {
        &crate::reply::APPROVAL_GATE_LABELS
    };
    match crate::reply::exact_option_pick(words.last().map(String::as_str), labels)? {
        0 => Some("Approve"),
        1 => Some("Request Changes"),
        2 => Some(crate::reply::ACCEPT_AS_IS_CHOICE),
        _ => None,
    }
}

/// `personsGateWords`: every message this chat's person typed since the
/// gate was presented, verbatim, joined by line breaks, for the record.
/// Host-made cancellation text is left out; nothing else is judged.
#[must_use]
pub fn persons_gate_words(wf: &Workflow, stage: &str) -> Option<String> {
    let words: Vec<String> = gate_words_since_presentation(wf, stage)?
        .into_iter()
        .filter(|text| !crate::reply::is_non_answer(Some(text)))
        .collect();
    (!words.is_empty()).then(|| words.join("\n"))
}

/// `personsGateFeedback`: what the person asked to change, in their own
/// words: every message since the presentation, leaving out one that is
/// only an option pick, which says nothing about what to change.
#[must_use]
pub fn persons_gate_feedback(wf: &Workflow, stage: &str) -> Option<String> {
    let said: Vec<String> = gate_words_since_presentation(wf, stage)?
        .into_iter()
        .filter(|text| {
            !crate::reply::is_non_answer(Some(text))
                && crate::reply::exact_option_pick(Some(text), &crate::reply::APPROVAL_GATE_LABELS)
                    .is_none()
        })
        .collect();
    (!said.is_empty()).then(|| said.join("\n"))
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
        workflow_for(name, mine, theirs, None)
    }

    /// The same, opened for one chat.
    fn workflow_for(
        name: &str,
        mine: &str,
        theirs: &str,
        session: Option<&str>,
    ) -> (Workflow, std::path::PathBuf) {
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
            session_id: session.map(str::to_owned),
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

    const MY_SHARD: &str = "focus/spaces/default/intents/260105-add-login/audit/laptop-abc123.md";

    /// A gate shown on my shard, then the given messages typed there, one
    /// `HUMAN_TURN` row each: the shard's text and the offsets a record
    /// would index them by (the shard's size after each row).
    fn gate_shard(texts: &[&str]) -> (String, Vec<u64>) {
        let mut content = format!(
            "# Log\n{}{}",
            block("STAGE_STARTED", "2026-01-01T00:00:00Z", &[("Stage", "a")]),
            block(
                "STAGE_AWAITING_APPROVAL",
                "2026-01-01T00:00:01Z",
                &[("Stage", "a")]
            )
        );
        let mut offsets = Vec::new();
        for (n, _) in texts.iter().enumerate() {
            content.push_str(&block(
                "HUMAN_TURN",
                &format!("2026-01-01T00:00:0{}Z", n + 2),
                &[("Session", "s1"), ("Reply", "yes")],
            ));
            offsets.push(content.len() as u64);
        }
        (content, offsets)
    }

    fn write_record(root: &Path, segment: &str, record: &Value) {
        let dir =
            root.join("focus/spaces/default/intents/260105-add-login/.focus-engine/gate-words");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{segment}.json")),
            serde_json::to_string(record).unwrap(),
        )
        .unwrap();
    }

    fn record(shard: &str, dropped: u64, messages: &[(u64, &str)]) -> Value {
        serde_json::json!({
            "version": 1,
            "session": "s1",
            "shard": shard,
            "dropped": dropped,
            "messages": messages
                .iter()
                .map(|(offset, text)| serde_json::json!({"offset": offset, "text": text}))
                .collect::<Vec<_>>(),
        })
    }

    /// The person's words since the presentation are read from this chat's
    /// record, in order; a pick is told from feedback; a non-answer and an
    /// exact pick are left out of the feedback; words typed before the
    /// showing are not at the gate.
    #[test]
    fn the_persons_words_at_the_gate_are_read_from_the_chats_record() {
        let texts = ["Looks wrong", "Cancelled.", "Fix the title", "2"];
        let (mine, offsets) = gate_shard(&texts);
        let (wf, root) = workflow_for("gate-words", &mine, "# Log\n", Some("s1"));
        let messages: Vec<(u64, &str)> = offsets.iter().copied().zip(texts).collect();
        write_record(&root, "s1", &record(MY_SHARD, 0, &messages));
        assert_eq!(
            gate_words_since_presentation(&wf, "a").as_deref(),
            Some(&["Looks wrong", "Cancelled.", "Fix the title", "2"].map(str::to_owned)[..])
        );
        assert_eq!(
            persons_latest_gate_pick(&wf, "a", false),
            Some("Request Changes")
        );
        assert_eq!(
            persons_gate_words(&wf, "a").as_deref(),
            Some("Looks wrong\nFix the title\n2")
        );
        assert_eq!(
            persons_gate_feedback(&wf, "a").as_deref(),
            Some("Looks wrong\nFix the title")
        );
        // A gate shown again after a revision counts from its latest
        // showing: a message typed between the two showings is not at it.
        let reshown = format!(
            "{mine}{}{}{}",
            block("GATE_REJECTED", "2026-01-01T00:00:07Z", &[("Stage", "a")]),
            block("STAGE_REVISING", "2026-01-01T00:00:07Z", &[("Stage", "a")]),
            block(
                "STAGE_AWAITING_APPROVAL",
                "2026-01-01T00:00:08Z",
                &[("Stage", "a")]
            )
        );
        let (again, root_again) =
            workflow_for("gate-words-reshown", &reshown, "# Log\n", Some("s1"));
        write_record(
            &root_again,
            "s1",
            &record(MY_SHARD, 0, &[(offsets[3], "2")]),
        );
        assert!(gate_words_since_presentation(&again, "a").is_none());
        write_record(
            &root_again,
            "s1",
            &record(
                MY_SHARD,
                0,
                &[(offsets[3], "2"), (reshown.len() as u64, "1")],
            ),
        );
        assert_eq!(
            persons_latest_gate_pick(&again, "a", false),
            Some("Approve")
        );
        let _ = std::fs::remove_dir_all(root_again);
        // Another stage was not presented; a message at the showing's own
        // offset was typed before it.
        assert!(gate_words_since_presentation(&wf, "b").is_none());
        let floor = gate_presentation_start(&mine, "a", false).unwrap() as u64;
        write_record(
            &root,
            "s1",
            &record(MY_SHARD, 0, &[(floor, "before"), (offsets[0], "after")]),
        );
        assert_eq!(persons_gate_words(&wf, "a").as_deref(), Some("after"));
        // The last message decides the pick; a non-answer after it does not.
        write_record(
            &root,
            "s1",
            &record(MY_SHARD, 0, &[(offsets[0], "1"), (offsets[1], "timed out")]),
        );
        assert_eq!(persons_latest_gate_pick(&wf, "a", false), Some("Approve"));
        assert!(persons_gate_feedback(&wf, "a").is_none());
        // Accept as-is is an option only once offered.
        write_record(&root, "s1", &record(MY_SHARD, 0, &[(offsets[0], "3")]));
        assert_eq!(persons_latest_gate_pick(&wf, "a", false), None);
        assert_eq!(
            persons_latest_gate_pick(&wf, "a", true),
            Some(crate::reply::ACCEPT_AS_IS_CHOICE)
        );
        assert_eq!(persons_gate_feedback(&wf, "a").as_deref(), Some("3"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// A record that is not this chat's, this shard's, or whole is no
    /// record: words were said that were not kept, another shard's offsets
    /// index nothing here, and a malformed field fails closed.
    #[test]
    fn a_gate_words_record_that_cannot_be_trusted_is_not_read() {
        let (mine, offsets) = gate_shard(&["Fix it"]);
        let (wf, root) = workflow_for("gate-words-untrusted", &mine, "# Log\n", Some("s1"));
        let good = record(MY_SHARD, 0, &[(offsets[0], "Fix it")]);
        write_record(&root, "s1", &good);
        assert_eq!(persons_gate_words(&wf, "a").as_deref(), Some("Fix it"));
        let floor = gate_presentation_start(&mine, "a", false).unwrap() as u64;
        // A message since the showing was dropped; one before it is no loss.
        write_record(
            &root,
            "s1",
            &record(MY_SHARD, floor + 1, &[(offsets[0], "Fix it")]),
        );
        assert!(persons_gate_words(&wf, "a").is_none());
        write_record(
            &root,
            "s1",
            &record(MY_SHARD, floor, &[(offsets[0], "Fix it")]),
        );
        assert_eq!(persons_gate_words(&wf, "a").as_deref(), Some("Fix it"));
        // Another shard, another session, another version, no messages.
        let other_shard = MY_SHARD.replace("laptop-abc123", "desktop-def456");
        write_record(
            &root,
            "s1",
            &record(&other_shard, 0, &[(offsets[0], "Fix it")]),
        );
        assert!(persons_gate_words(&wf, "a").is_none());
        let mut other_session = good.clone();
        other_session["session"] = Value::from("s2");
        write_record(&root, "s1", &other_session);
        assert!(persons_gate_words(&wf, "a").is_none());
        let mut other_version = good.clone();
        other_version["version"] = Value::from(2);
        write_record(&root, "s1", &other_version);
        assert!(persons_gate_words(&wf, "a").is_none());
        write_record(&root, "s1", &record(MY_SHARD, 0, &[]));
        assert!(persons_gate_words(&wf, "a").is_none());
        // Offsets and `dropped` are whole non-negative safe integers.
        for bad in [
            Value::from(1.5),
            Value::from(-1),
            Value::from("12"),
            Value::from(9_007_199_254_740_992_u64),
        ] {
            let mut broken = good.clone();
            broken["messages"][0]["offset"] = bad.clone();
            write_record(&root, "s1", &broken);
            assert!(persons_gate_words(&wf, "a").is_none(), "offset {bad}");
            let mut broken = good.clone();
            broken["dropped"] = bad.clone();
            write_record(&root, "s1", &broken);
            assert!(persons_gate_words(&wf, "a").is_none(), "dropped {bad}");
        }
        let mut no_text = good.clone();
        no_text["messages"][0]["text"] = Value::from(7);
        write_record(&root, "s1", &no_text);
        assert!(persons_gate_words(&wf, "a").is_none());
        // Not JSON, not an object, over the ceiling.
        let path = gate_words_path(&wf, "s1").unwrap();
        std::fs::write(&path, "{").unwrap();
        assert!(persons_gate_words(&wf, "a").is_none());
        std::fs::write(&path, "[]").unwrap();
        assert!(persons_gate_words(&wf, "a").is_none());
        let mut huge = good.clone();
        huge["pad"] = Value::from("x".repeat(GATE_WORDS_MAX_FILE_BYTES as usize));
        write_record(&root, "s1", &huge);
        assert!(persons_gate_words(&wf, "a").is_none());
        // Without a chat there is no record to read, and the record's name
        // is the chat's segment.
        write_record(&root, "s1", &good);
        let (alone, _) = workflow_for("gate-words-alone", &mine, "# Log\n", None);
        assert!(invoking_session(&alone).is_none());
        assert!(persons_gate_words(&alone, "a").is_none());
        assert!(gate_words_path(&wf, "///").is_none());
        assert!(
            gate_words_path(&wf, invoking_session(&wf).unwrap())
                .unwrap()
                .ends_with(".focus-engine/gate-words/s1.json")
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

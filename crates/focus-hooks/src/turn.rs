//! What a prompt-submit payload says, read as the human-turn hook reads
//! it: the text of the person's reply (a typed prompt, or the envelope a
//! picker delivers its selection in), the question a picker asked, the
//! labels it offered and the picks it carried back, and who sent the turn
//! (an adapter's word, Claude Code's own transcript, or the host's fixed
//! sentences). Pure over the payload; the transcript reader opens the file
//! the host names, as upstream does, and only reports on it.
//!
//! Mirrors the private readers of `aidlc-record-human-turn.ts` and, from
//! `aidlc-lib.ts`, `hostEnvelopeTurnText`, `hostTemplateOrigin`,
//! `suppliedTurnOrigin`, `claudeTranscriptOrigin` and `emptyPickerResult`.

use std::io::{Read, Seek, SeekFrom};

use focus_js::re;
use focus_js::text::trim;
use serde_json::{Map, Value};

/// Who sent a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOrigin {
    /// A person's, by the host's record (`source: record`) or by default.
    Person { recorded: bool },
    /// The host's own prompt: its notice or brief.
    Host { reason: String, source: HostSource },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSource {
    Record,
    Template,
}

impl HostSource {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Template => "template",
        }
    }
}

/// `HOST_TURN_HEADER_RE`: Kiro Crew's header before the person's own turn.
const HOST_TURN_HEADER: &str = r"\[CURRENT USER REQUEST (?:--|—) respond to this\]\r?\n";

/// `hostEnvelopeTurnText`: the person's own turn out of a host envelope,
/// the text after the last header; a prompt without one, unchanged.
#[must_use]
pub fn host_envelope_turn_text(prompt: &str) -> &str {
    re::find_all(HOST_TURN_HEADER, "g", prompt)
        .last()
        .map_or(prompt, |found| &prompt[found.range.end..])
}

/// `CLAUDE_HOST_TEMPLATES`: Claude Code's turn when a background task
/// finishes, one element and nothing else.
const CLAUDE_HOST_TEMPLATES: [(&str, &str); 1] = [(
    "claude task notification",
    r"^<task-notification>\r?\n[\s\S]*\r?\n</task-notification>$",
)];

/// `hostTemplateOrigin`: the whole prompt is one of the host's fixed
/// sentences.
#[must_use]
pub fn host_template_origin(prompt: &str) -> Option<TurnOrigin> {
    let text = trim(prompt);
    if text.is_empty() {
        return None;
    }
    CLAUDE_HOST_TEMPLATES
        .iter()
        .find(|(_, pattern)| re::test(pattern, "", text))
        .map(|(name, _)| TurnOrigin::Host {
            reason: (*name).to_owned(),
            source: HostSource::Template,
        })
}

/// `suppliedTurnOrigin`: what an adapter decided about the turn, when its
/// payload says so. Any other shape decides nothing.
#[must_use]
pub fn supplied_turn_origin(value: Option<&Value>) -> Option<TurnOrigin> {
    let origin = value?.as_object()?;
    let field = |key: &str| origin.get(key).and_then(Value::as_str);
    if field("kind") == Some("person") {
        return Some(TurnOrigin::Person {
            recorded: field("source") == Some("record"),
        });
    }
    if field("kind") != Some("host") {
        return None;
    }
    let source = match field("source") {
        Some("record") => HostSource::Record,
        Some("template") => HostSource::Template,
        _ => return None,
    };
    let reason = field("reason")
        .map(trim)
        .filter(|reason| !reason.is_empty())?;
    Some(TurnOrigin::Host {
        reason: focus_js::text::slice_utf16(reason, 0, 200),
        source,
    })
}

/// `tailText`: the last `max_bytes` of a file as text, from its first whole
/// line.
fn tail_text(path: &str, max_bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let size = file.metadata().ok()?.len();
    let start = size.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    let text = String::from_utf8_lossy(&buffer).into_owned();
    if start == 0 {
        return Some(text);
    }
    Some(
        text.find('\n')
            .map_or_else(String::new, |newline| text[newline + 1..].to_owned()),
    )
}

/// `entryText`: a chat entry's text, as a string or the text parts of a
/// content list.
fn entry_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                let record = part.as_object()?;
                (record.get("type") == Some(&Value::String("text".into())))
                    .then(|| record.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect(),
        _ => String::new(),
    }
}

/// `newestUserEntry`: the newest `type: "user"` entry of a JSONL chat record
/// whose text is exactly this prompt and whose prompt id matches.
fn newest_user_entry(
    path: &str,
    prompt: &str,
    prompt_id: Option<&str>,
) -> Option<Map<String, Value>> {
    let text = tail_text(path, 1 << 20)?;
    let want = trim(prompt);
    for line in text.split('\n').rev() {
        let line = trim(line);
        if line.is_empty() {
            continue;
        }
        let Ok(Value::Object(record)) = focus_js::json::parse(line) else {
            continue;
        };
        if record.get("type") != Some(&Value::String("user".into())) {
            continue;
        }
        if let Some(id) = prompt_id
            && record.get("promptId") != Some(&Value::String(id.to_owned()))
        {
            continue;
        }
        let content = match record.get("message") {
            Some(Value::Object(message)) => message.get("content"),
            _ => record.get("content"),
        };
        if trim(&entry_text(content)) == want {
            return Some(record);
        }
    }
    None
}

/// `claudeTranscriptOrigin`: who sent a prompt in Claude Code, from its own
/// transcript: the newest user row for this prompt id with exactly this
/// text names the turn's origin. No row, nothing is known.
#[must_use]
pub fn claude_transcript_origin(
    transcript_path: Option<&Value>,
    prompt_id: Option<&Value>,
    prompt: &str,
) -> Option<TurnOrigin> {
    let path = transcript_path?.as_str()?;
    if !path.ends_with(".jsonl") || trim(prompt).is_empty() {
        return None;
    }
    let id = prompt_id
        .and_then(Value::as_str)
        .map(trim)
        .filter(|id| !id.is_empty());
    let entry = newest_user_entry(path, prompt, id)?;
    let origin_kind = entry
        .get("origin")
        .and_then(Value::as_object)
        .and_then(|origin| origin.get("kind"))
        .and_then(Value::as_str);
    let turn_origin = entry.get("turnOrigin").and_then(Value::as_str);
    if entry.get("promptSource") == Some(&Value::String("system".into()))
        || turn_origin == Some("task_notification")
        || origin_kind == Some("task-notification")
    {
        return Some(TurnOrigin::Host {
            reason: turn_origin.or(origin_kind).unwrap_or("system").to_owned(),
            source: HostSource::Record,
        });
    }
    Some(TurnOrigin::Person { recorded: true })
}

/// `extractResponseText`: the text a reply carries. A string is the text,
/// unless it is a JSON envelope (an object, an array, a quoted string),
/// which is unwrapped; a scalar that happens to parse ("1", "true") is the
/// text the person typed. An object yields its first answer-like field,
/// then any field with text.
#[must_use]
pub fn extract_response_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => {
            let trimmed = trim(text);
            if trimmed.is_empty() {
                return String::new();
            }
            if let Ok(parsed) = focus_js::json::parse(trimmed)
                && (parsed.is_string() || parsed.is_object() || parsed.is_array())
            {
                return extract_response_text(Some(&parsed));
            }
            trimmed.to_owned()
        }
        Some(Value::Array(entries)) => entries
            .iter()
            .map(|entry| extract_response_text(Some(entry)))
            .find(|text| !text.is_empty())
            .unwrap_or_default(),
        Some(Value::Object(record)) => {
            for key in [
                "answer",
                "answers",
                "selected",
                "selection",
                "value",
                "label",
                "text",
            ] {
                if let Some(field) = record.get(key) {
                    let text = extract_response_text(Some(field));
                    if !text.is_empty() {
                        return text;
                    }
                }
            }
            record
                .values()
                .map(|entry| extract_response_text(Some(entry)))
                .find(|text| !text.is_empty())
                .unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// `extractQuestionText`: the one question a picker asked. `None` when the
/// input is not an object or names no question; `Some("")` for a
/// multi-question payload, which is never paired with a reply.
#[must_use]
pub fn extract_question_text(value: Option<&Value>) -> Option<String> {
    let input = value?.as_object()?;
    if let Some(Value::Array(questions)) = input.get("questions") {
        if questions.len() != 1 {
            return Some(String::new());
        }
        return extract_question_text(questions.first());
    }
    input
        .get("question")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// `singlePickerQuestion`: the one question of a picker input.
fn single_picker_question(value: Option<&Value>) -> Option<&Map<String, Value>> {
    let input = value?.as_object()?;
    match input.get("questions") {
        Some(Value::Array(questions)) => {
            if questions.len() == 1 {
                questions[0].as_object()
            } else {
                None
            }
        }
        _ => Some(input),
    }
}

/// `extractOptionLabels`: the option labels of a single-question picker, as
/// strings or `{label}`; `None` when any option has no label.
#[must_use]
pub fn extract_option_labels(value: Option<&Value>) -> Option<Vec<String>> {
    let question = single_picker_question(value)?;
    let options = question.get("options")?.as_array()?;
    options
        .iter()
        .map(|option| match option {
            Value::String(label) => Some(label.clone()),
            Value::Object(record) => record
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned),
            _ => None,
        })
        .collect()
}

/// A tool response as the picker's object: a string is parsed first.
fn response_object(response: Option<&Value>) -> Option<Map<String, Value>> {
    match response? {
        Value::String(text) => match focus_js::json::parse(text) {
            Ok(Value::Object(map)) => Some(map),
            _ => None,
        },
        Value::Object(map) => Some(map.clone()),
        _ => None,
    }
}

/// `carriesSeveralPicks`: a multi-select picker, or a reply carrying more
/// than one pick, is not a single choice.
#[must_use]
pub fn carries_several_picks(tool_input: Option<&Value>, tool_response: Option<&Value>) -> bool {
    if single_picker_question(tool_input)
        .is_some_and(|question| question.get("multiSelect") == Some(&Value::Bool(true)))
    {
        return true;
    }
    let Some(response) = response_object(tool_response) else {
        return false;
    };
    let Some(Value::Object(answers)) = response.get("answers") else {
        return false;
    };
    answers.values().any(|answer| {
        let picks = match answer {
            Value::Object(record) => record.get("answers"),
            other => Some(other),
        };
        matches!(picks, Some(Value::Array(list)) if list.len() > 1)
    })
}

/// `emptyPickerResult`: a question box that came back with nothing picked,
/// exactly `{"answers":{}}`.
#[must_use]
pub fn empty_picker_result(tool_response: Option<&Value>) -> bool {
    let response = match tool_response {
        Some(Value::String(text)) => match focus_js::json::parse(text) {
            Ok(parsed) => parsed,
            Err(_) => return false,
        },
        Some(value) => value.clone(),
        None => return false,
    };
    let Value::Object(map) = response else {
        return false;
    };
    if map.len() != 1 {
        return false;
    }
    matches!(map.get("answers"), Some(Value::Object(answers)) if answers.is_empty())
}

/// One entry of what a question box carried back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerReply {
    pub question: String,
    pub reply: Option<String>,
}

/// `pickerReplies`: one entry per question the box asked, in the order
/// shown, `reply` empty where the person left a question blank; a pick for
/// a question the box did not list follows them. Strings are cut to
/// `limit` UTF-16 units, and `cut` says whether any was.
#[must_use]
pub fn picker_replies(
    payload: &Map<String, Value>,
    limit: Option<usize>,
) -> (Vec<PickerReply>, bool) {
    let mut cut = false;
    let mut kept = |value: &str| -> String {
        match limit {
            Some(limit) if value.encode_utf16().count() > limit => {
                cut = true;
                focus_js::text::slice_utf16(value, 0, limit)
            }
            _ => value.to_owned(),
        }
    };
    let response = payload
        .get("picker_reply")
        .or_else(|| payload.get("tool_response"))
        .or_else(|| payload.get("toolResponse"));
    let Some(response) = response else {
        return (Vec::new(), cut);
    };
    let response = match response {
        Value::String(text) => match focus_js::json::parse(text) {
            Ok(parsed) => parsed,
            Err(_) => return (Vec::new(), cut),
        },
        other => other.clone(),
    };
    let Some(Value::Object(picks)) = response.as_object().and_then(|map| map.get("answers")) else {
        return (Vec::new(), cut);
    };
    let picks = picks.clone();
    let reply_of = |value: &Value, kept: &mut dyn FnMut(&str) -> String| -> Option<String> {
        let chosen = match value {
            Value::Object(record) => record.get("answers"),
            other => Some(other),
        };
        let list: Vec<&Value> = match chosen {
            Some(Value::Array(list)) => list.iter().collect(),
            Some(other) => vec![other],
            None => Vec::new(),
        };
        let reply: Vec<&str> = list
            .into_iter()
            .filter_map(Value::as_str)
            .filter(|pick| !trim(pick).is_empty())
            .collect();
        if reply.is_empty() {
            None
        } else {
            Some(kept(&reply.join(", ")))
        }
    };
    let tool_input = payload
        .get("tool_input")
        .or_else(|| payload.get("toolInput"));
    let asked: Vec<&Value> = tool_input
        .and_then(Value::as_object)
        .and_then(|input| input.get("questions"))
        .and_then(Value::as_array)
        .map(|questions| questions.iter().collect())
        .unwrap_or_default();
    let mut entries = Vec::new();
    let mut used: Vec<String> = Vec::new();
    for entry in asked {
        let question = entry.as_object();
        let Some(text) = question
            .and_then(|q| q.get("question"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        // Claude Code keys answers by the question text, Codex by the id.
        let id = question.and_then(|q| q.get("id")).and_then(Value::as_str);
        let key = match id {
            Some(id) if picks.contains_key(id) => Some(id.to_owned()),
            _ if picks.contains_key(text) => Some(text.to_owned()),
            _ => None,
        };
        if let Some(key) = &key {
            used.push(key.clone());
        }
        let reply = key.and_then(|key| reply_of(&picks[&key], &mut kept));
        entries.push(PickerReply {
            question: kept(text),
            reply,
        });
    }
    for (key, value) in &picks {
        if used.contains(key) {
            continue;
        }
        if let Some(reply) = reply_of(value, &mut kept) {
            entries.push(PickerReply {
                question: kept(key),
                reply: Some(reply),
            });
        }
    }
    (entries, cut)
}

/// `pickedLabels`: every pick a picker reply reports, in its order: one
/// label per question, or several for a multi-select.
#[must_use]
pub fn picked_labels(tool_response: Option<&Value>) -> Vec<String> {
    let response = match tool_response {
        Some(Value::String(text)) => match focus_js::json::parse(text) {
            Ok(parsed) => parsed,
            Err(_) => return Vec::new(),
        },
        Some(value) => value.clone(),
        None => return Vec::new(),
    };
    let Some(answers) = response.as_object().and_then(|map| map.get("answers")) else {
        return Vec::new();
    };
    let mut labels = Vec::new();
    fn add(value: &Value, labels: &mut Vec<String>) {
        match value {
            Value::String(text) => {
                if !trim(text).is_empty() {
                    labels.push(trim(text).to_owned());
                }
            }
            Value::Array(entries) => {
                for entry in entries {
                    add(entry, labels);
                }
            }
            Value::Object(record) => {
                if let Some(nested) = record.get("answers") {
                    add(nested, labels);
                }
            }
            _ => {}
        }
    }
    match answers {
        Value::Array(list) => {
            for answer in list {
                add(answer, &mut labels);
            }
        }
        Value::Object(map) => {
            for answer in map.values() {
                add(answer, &mut labels);
            }
        }
        _ => {}
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn a_reply_is_its_text_or_the_text_inside_its_envelope() {
        let text = |value: Value| extract_response_text(Some(&value));
        assert_eq!(text(json!("  use postgres ")), "use postgres");
        assert_eq!(text(json!("")), "");
        assert_eq!(text(json!("   ")), "");
        // A JSON scalar is the text the person typed; an envelope is opened.
        assert_eq!(text(json!("1")), "1");
        assert_eq!(text(json!("true")), "true");
        assert_eq!(text(json!("null")), "null");
        assert_eq!(text(json!("\"quoted\"")), "quoted");
        assert_eq!(text(json!("{\"answer\": \"two\"}")), "two");
        assert_eq!(text(json!("[\"three\"]")), "three");
        assert_eq!(text(json!({"answers": {"Q": "a"}})), "a");
        assert_eq!(text(json!({"other": "x", "label": "picked"})), "picked");
        assert_eq!(text(json!({"other": "x"})), "x");
        assert_eq!(text(json!(["", " ", "last"])), "last");
        assert_eq!(text(json!(7)), "");
        assert_eq!(text(json!(null)), "");
        assert_eq!(extract_response_text(None), "");
    }

    #[test]
    fn a_picker_asks_one_question_and_offers_labels() {
        assert_eq!(
            extract_question_text(Some(&json!({"questions": [{"question": "Q"}]}))),
            Some("Q".to_owned())
        );
        assert_eq!(
            extract_question_text(Some(
                &json!({"questions": [{"question": "Q"}, {"question": "R"}]})
            )),
            Some(String::new())
        );
        assert_eq!(
            extract_question_text(Some(&json!({"question": "Q"}))),
            Some("Q".to_owned())
        );
        assert_eq!(extract_question_text(Some(&json!({"other": 1}))), None);
        assert_eq!(extract_question_text(Some(&json!("Q"))), None);
        assert_eq!(
            extract_option_labels(Some(
                &json!({"questions": [{"options": ["a", {"label": "b"}]}]})
            )),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        assert_eq!(
            extract_option_labels(Some(&json!({"options": ["a", {"nope": "b"}]}))),
            None
        );
        assert_eq!(
            extract_option_labels(Some(&json!({"questions": [{}, {}]}))),
            None
        );
        assert_eq!(extract_option_labels(Some(&json!({}))), None);
    }

    #[test]
    fn several_picks_are_a_multi_select_or_a_reply_with_more_than_one() {
        assert!(carries_several_picks(
            Some(&json!({"questions": [{"multiSelect": true}]})),
            Some(&json!({"answers": {"Q": "a"}}))
        ));
        assert!(carries_several_picks(
            None,
            Some(&json!({"answers": {"Q": ["a", "b"]}}))
        ));
        assert!(carries_several_picks(
            None,
            Some(&json!({"answers": {"Q": {"answers": ["a", "b"]}}}))
        ));
        assert!(carries_several_picks(
            None,
            Some(&json!("{\"answers\":{\"Q\":[\"a\",\"b\"]}}"))
        ));
        assert!(!carries_several_picks(
            None,
            Some(&json!({"answers": {"Q": ["a"]}}))
        ));
        assert!(!carries_several_picks(
            None,
            Some(&json!({"answers": ["a", "b"]}))
        ));
        assert!(!carries_several_picks(None, Some(&json!("{nope"))));
        assert!(!carries_several_picks(None, None));
    }

    #[test]
    fn the_box_carries_back_each_question_and_its_pick() {
        let payload = map(json!({
            "tool_input": {"questions": [
                {"question": "One", "options": ["a"]},
                {"question": "Two", "options": ["c"]},
                {"id": "q3", "question": "Three"},
                {"question": 4},
            ]},
            "tool_response": {"answers": {"One": "a", "Extra": {"answers": ["z", " ", "y"]}, "q3": ["", "t"], "Blank": ""}},
        }));
        let (entries, cut) = picker_replies(&payload, None);
        assert!(!cut);
        assert_eq!(
            entries,
            vec![
                PickerReply {
                    question: "One".into(),
                    reply: Some("a".into())
                },
                PickerReply {
                    question: "Two".into(),
                    reply: None
                },
                PickerReply {
                    question: "Three".into(),
                    reply: Some("t".into())
                },
                PickerReply {
                    question: "Extra".into(),
                    reply: Some("z, y".into())
                },
            ]
        );
        let (entries, cut) = picker_replies(&payload, Some(2));
        assert!(cut);
        assert_eq!(entries[0].question, "On");
        assert_eq!(entries[3].reply.as_deref(), Some("z,"));
        // A string response is parsed; a Codex picker_reply comes first.
        let payload = map(
            json!({"picker_reply": {"answers": {"Q": "p"}}, "tool_response": "{\"answers\":{\"Q\":\"t\"}}"}),
        );
        assert_eq!(
            picker_replies(&payload, None).0[0].reply.as_deref(),
            Some("p")
        );
        let payload = map(json!({"tool_response": "{nope"}));
        assert!(picker_replies(&payload, None).0.is_empty());
        let payload = map(json!({"tool_response": {"answers": ["a"]}}));
        assert!(picker_replies(&payload, None).0.is_empty());
        assert_eq!(
            picked_labels(Some(
                &json!({"answers": {"One": " a ", "Two": {"answers": ["b", 1, ""]}, "Three": ["c", ["d"]]}})
            )),
            ["a", "b", "c", "d"]
        );
        assert_eq!(
            picked_labels(Some(&json!({"answers": ["x", "y"]}))),
            ["x", "y"]
        );
        assert!(picked_labels(Some(&json!({"answers": "x"}))).is_empty());
        assert!(picked_labels(Some(&json!("{nope"))).is_empty());
        assert!(picked_labels(None).is_empty());
    }

    #[test]
    fn an_empty_box_is_exactly_an_empty_answers_object() {
        assert!(empty_picker_result(Some(&json!({"answers": {}}))));
        assert!(empty_picker_result(Some(&json!("{\"answers\":{}}"))));
        assert!(!empty_picker_result(Some(&json!({"answers": {}, "x": 1}))));
        assert!(!empty_picker_result(Some(&json!({"answers": []}))));
        assert!(!empty_picker_result(Some(&json!({"answers": {"a": 1}}))));
        assert!(!empty_picker_result(Some(&json!("{nope"))));
        assert!(!empty_picker_result(Some(&json!([]))));
        assert!(!empty_picker_result(None));
    }

    #[test]
    fn the_envelope_and_the_hosts_sentences_are_read() {
        assert_eq!(
            host_envelope_turn_text("a\n[CURRENT USER REQUEST -- respond to this]\nb"),
            "b"
        );
        assert_eq!(
            host_envelope_turn_text(
                "x\n[CURRENT USER REQUEST \u{2014} respond to this]\r\ny\n[CURRENT USER REQUEST -- respond to this]\nz"
            ),
            "z"
        );
        assert_eq!(host_envelope_turn_text("plain"), "plain");
        assert_eq!(
            host_template_origin("  <task-notification>\nx\n</task-notification>  "),
            Some(TurnOrigin::Host {
                reason: "claude task notification".into(),
                source: HostSource::Template
            })
        );
        assert_eq!(
            host_template_origin("<task-notification>\nx\n</task-notification> more"),
            None
        );
        assert_eq!(host_template_origin("   "), None);
        assert_eq!(
            supplied_turn_origin(Some(
                &json!({"kind": "host", "reason": " r ", "source": "record"})
            )),
            Some(TurnOrigin::Host {
                reason: "r".into(),
                source: HostSource::Record
            })
        );
        assert_eq!(
            supplied_turn_origin(Some(&json!({"kind": "person", "source": "record"}))),
            Some(TurnOrigin::Person { recorded: true })
        );
        assert_eq!(
            supplied_turn_origin(Some(&json!({"kind": "person", "source": "x"}))),
            Some(TurnOrigin::Person { recorded: false })
        );
        assert_eq!(
            supplied_turn_origin(Some(
                &json!({"kind": "host", "reason": " ", "source": "record"})
            )),
            None
        );
        assert_eq!(
            supplied_turn_origin(Some(
                &json!({"kind": "host", "reason": "r", "source": "other"})
            )),
            None
        );
        assert_eq!(supplied_turn_origin(Some(&json!("host"))), None);
        let long = "r".repeat(300);
        match supplied_turn_origin(Some(
            &json!({"kind": "host", "reason": long, "source": "template"}),
        )) {
            Some(TurnOrigin::Host { reason, .. }) => assert_eq!(reason.len(), 200),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_transcript_names_the_turns_origin() {
        let dir =
            std::env::temp_dir().join(format!("focus-turn-transcript-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("chat.jsonl");
        std::fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"promptId\":\"p1\",\"turnOrigin\":\"human\",\"message\":{\"content\":\"use postgres\"}}\n",
                "{nope\n",
                "{\"type\":\"user\",\"promptId\":\"p1\",\"promptSource\":\"system\",\"turnOrigin\":\"task_notification\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"use postgres\"}]}}\n",
                "{\"type\":\"assistant\",\"promptId\":\"p1\",\"promptSource\":\"system\",\"message\":{\"content\":\"use postgres\"}}\n",
            ),
        )
        .unwrap();
        let given = json!(path.to_str().unwrap());
        let host = TurnOrigin::Host {
            reason: "task_notification".into(),
            source: HostSource::Record,
        };
        // The newest user row for the prompt id with exactly this text.
        assert_eq!(
            claude_transcript_origin(Some(&given), Some(&json!("p1")), " use postgres "),
            Some(host.clone())
        );
        assert_eq!(
            claude_transcript_origin(Some(&given), None, "use postgres"),
            Some(host)
        );
        assert_eq!(
            claude_transcript_origin(Some(&given), Some(&json!("p2")), "use postgres"),
            None
        );
        assert_eq!(
            claude_transcript_origin(Some(&given), Some(&json!("p1")), "other"),
            None
        );
        assert_eq!(
            claude_transcript_origin(Some(&given), Some(&json!("p1")), "  "),
            None
        );
        assert_eq!(
            claude_transcript_origin(Some(&json!("chat.txt")), None, "use postgres"),
            None
        );
        assert_eq!(
            claude_transcript_origin(Some(&json!(1)), None, "use postgres"),
            None
        );
        assert_eq!(claude_transcript_origin(None, None, "use postgres"), None);
        std::fs::write(&path, "{\"type\":\"user\",\"promptId\":\"p1\",\"origin\":{\"kind\":\"task-notification\"},\"content\":\"x\"}\n").unwrap();
        assert_eq!(
            claude_transcript_origin(Some(&given), Some(&json!("p1")), "x"),
            Some(TurnOrigin::Host {
                reason: "task-notification".into(),
                source: HostSource::Record
            })
        );
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"promptId\":\"p1\",\"content\":\"x\"}\n",
        )
        .unwrap();
        assert_eq!(
            claude_transcript_origin(Some(&given), Some(&json!("p1")), "x"),
            Some(TurnOrigin::Person { recorded: true })
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

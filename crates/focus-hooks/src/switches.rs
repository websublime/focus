//! The one line the person hears while a recorded switch keeps one of their
//! checks off (`aidlc-recorded-switches.ts`, the reading side).
//!
//! A recorded switch counts the moment it is in a settings file, however it
//! got there. What the engine owes the person is to say, in plain words,
//! which check is off, since when, how it was set, and the way back. The
//! record of how each was set lives in the clone's runtime directory
//! (`.focus-sessions/recorded-switches.json`) and every read of it fails
//! open: it can only ever change a sentence. The side that writes the
//! record (`config flags`) is M8's.

use std::time::SystemTime;

use chrono::{DateTime, Datelike, Local, Timelike};
use focus_js::json;
use focus_workspace::Layout;
use focus_workspace::names::SESSIONS_DIR;
use focus_workspace::settings::{Layer, Places, RECORDABLE_BYPASSES};
use serde_json::Value;

const RECORD_FILE: &str = "recorded-switches.json";
const RECORD_MAX_BYTES: u64 = 64 * 1024;
const QUOTE_MAX_CHARS: usize = 200;

/// `PERSON_CHECK_SWITCH_LABELS`: the switches that take a check from the
/// person, and what each check is called.
const LABELS: [(&str, &str); 9] = [
    ("FOCUS_DISABLE_PLAN_APPROVAL_GUARD", "plan approval check"),
    ("FOCUS_DISABLE_REVIEW_FREEZE_HOOK", "review freeze check"),
    (
        "FOCUS_DISABLE_REVIEWER_SCOPE_HOOK",
        "reviewer read scope check",
    ),
    ("FOCUS_SKIP_HUMAN_PRESENCE_GUARD", "human presence check"),
    ("FOCUS_DISABLE_SUMMARY_CONFIRMATION", "summary confirmation"),
    (
        "FOCUS_SKIP_SUMMARY_CONFIRMATION_GUARD",
        "summary confirmation check",
    ),
    ("FOCUS_SKIP_ARTIFACT_GUARD", "stage output check"),
    ("FOCUS_SKIP_REVISION_BACKSTOP", "revision backstop"),
    ("FOCUS_DISABLE_ENSEMBLE_EVIDENCE", "pipeline handoff check"),
];

/// `PERSON_SETTING_PURPOSES`: what each setting does for the person.
const PURPOSES: [(&str, &str); 12] = [
    (
        "plan approval check",
        "it shows you the plan before any code is written",
    ),
    (
        "plan approval",
        "it shows you the plan before any code is written",
    ),
    (
        "review freeze check",
        "it stops edits to work you already approved",
    ),
    (
        "state transition check",
        "it keeps the engine, not an agent, moving the work along",
    ),
    (
        "reviewer read scope check",
        "it keeps a reviewer reading only the part it was asked about",
    ),
    (
        "human presence check",
        "it needs a real reply from you behind every approval",
    ),
    (
        "summary confirmation",
        "it reads your words back to you before the work goes on",
    ),
    (
        "summary confirmation check",
        "it reads your words back to you before the work goes on",
    ),
    (
        "stage output check",
        "a stage must have written its files before it finishes",
    ),
    (
        "revision backstop",
        "it records a change you made at a gate that nobody wrote down",
    ),
    (
        "pipeline handoff check",
        "each agent's handoff is recorded before a pipeline stage finishes",
    ),
    ("Guard Policy", "it sets how many checks run"),
];

/// `settingPurpose`: ` (what it does)` after a setting's name, or nothing.
#[must_use]
pub fn setting_purpose(label: &str) -> String {
    PURPOSES
        .iter()
        .find(|(name, _)| *name == label)
        .map_or_else(String::new, |(_, purpose)| format!(" ({purpose})"))
}

/// One entry of the record: how a switch was set.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Recorded {
    name: String,
    target: String,
    since: String,
    how: String,
    words: Option<String>,
}

/// `readRecord`: the record's valid entries; nothing when it is not there
/// or cannot be read.
fn read_record(layout: &Layout) -> Vec<Recorded> {
    let path = layout.workspace_root().join(SESSIONS_DIR).join(RECORD_FILE);
    if !path.exists() {
        return Vec::new();
    }
    let Ok(bytes) = focus_workspace::files::read_regular(&path, "recorded switches") else {
        return Vec::new();
    };
    if bytes.len() as u64 > RECORD_MAX_BYTES {
        return Vec::new();
    }
    let Ok(Value::Object(record)) = json::parse(&String::from_utf8_lossy(&bytes)) else {
        return Vec::new();
    };
    if record.get("version") != Some(&Value::from(1)) {
        return Vec::new();
    }
    let Some(Value::Array(switches)) = record.get("switches") else {
        return Vec::new();
    };
    switches
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
            let name = text("name")?;
            if !RECORDABLE_BYPASSES.contains(&name.as_str())
                || !LABELS.iter().any(|(switch, _)| *switch == name)
            {
                return None;
            }
            let target = text("target")?;
            if !["local", "project", "global"].contains(&target.as_str()) {
                return None;
            }
            let how = text("how")?;
            if how != "chat" && how != "other" {
                return None;
            }
            if entry.get("words").is_some_and(|words| !words.is_string())
                || entry.get("announced").is_some_and(|at| !at.is_string())
            {
                return None;
            }
            Some(Recorded {
                name,
                target,
                since: text("since")?,
                how,
                words: text("words"),
            })
        })
        .collect()
}

/// `switchesOff`: the switches that take a check from the person and are
/// off because a settings file says so; a variable of the same name in the
/// environment decides instead, as the flag resolution reads it.
fn switches_off(layout: &Layout) -> Vec<(String, Layer, Option<Recorded>, SystemTime)> {
    let places = Places::of(&layout.project);
    let Ok(resolved) = places.resolve() else {
        return Vec::new();
    };
    let bypasses: Vec<String> = resolved
        .bypasses()
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let record = read_record(layout);
    LABELS
        .iter()
        // `Object.hasOwn(env, name)`: the variable set at all, to anything.
        .filter(|(name, _)| {
            bypasses.iter().any(|set| set == name) && std::env::var_os(name).is_none()
        })
        .filter_map(|(name, _)| {
            let layer = places.bypass_recorded_in(name)?;
            let entry = record
                .iter()
                .find(|entry| entry.name == *name && entry.target == layer.target())
                .cloned();
            let modified = std::fs::metadata(places.path(layer))
                .and_then(|meta| meta.modified())
                .unwrap_or_else(|_| SystemTime::now());
            Some(((*name).to_owned(), layer, entry, modified))
        })
        .collect()
}

fn pad(value: u32) -> String {
    format!("{value:02}")
}

/// `clock`: a recorded instant as the person reads it, in local time: the
/// time of day today, the date and time otherwise.
fn clock(at: Option<DateTime<Local>>, now: DateTime<Local>) -> String {
    let Some(at) = at else {
        return "an earlier session".to_owned();
    };
    let time = format!("{}:{}", pad(at.hour()), pad(at.minute()));
    if at.date_naive() == now.date_naive() {
        time
    } else {
        format!("{}-{}-{} {time}", at.year(), pad(at.month()), pad(at.day()))
    }
}

/// `quoted`: the person's words as a switch line quotes them: one line,
/// capped, double quotes turned.
#[must_use]
pub fn quoted(words: &str) -> String {
    let flat = focus_js::re::replace(r"\s+", "g", words, " ");
    let flat = focus_js::text::trim(&flat).replace('"', "'");
    let units: Vec<u16> = flat.encode_utf16().collect();
    if units.len() > QUOTE_MAX_CHARS {
        let cut = String::from_utf16_lossy(&units[..QUOTE_MAX_CHARS]);
        format!("{}...", focus_js::text::trim_end(&cut))
    } else {
        flat
    }
}

fn where_(layer: Layer) -> &'static str {
    if layer == Layer::Machine {
        "on this machine"
    } else {
        "for this project"
    }
}

/// `switchesOffLines(projectDir)` for a session start: each switch still
/// off, worded as an offer the person answers in the chat.
#[must_use]
pub fn switches_off_lines(layout: &Layout) -> Vec<String> {
    let now = Local::now();
    switches_off(layout)
        .into_iter()
        .map(|(name, layer, entry, modified)| {
            let label = LABELS
                .iter()
                .find(|(switch, _)| *switch == name)
                .map_or(name.as_str(), |(_, label)| label);
            let since = match &entry {
                Some(entry) => clock(
                    focus_js::date::parse_instant(&entry.since).and_then(|millis| {
                        let millis = u64::try_from(millis).ok()?;
                        Some(DateTime::<Local>::from(
                            SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(millis),
                        ))
                    }),
                    now,
                ),
                None => clock(Some(DateTime::<Local>::from(modified)), now),
            };
            let how = match &entry {
                Some(entry)
                    if entry.how == "chat"
                        && entry.words.as_deref().is_some_and(|w| !w.is_empty()) =>
                {
                    format!(
                        ", because you said: \"{}\"",
                        quoted(entry.words.as_deref().unwrap_or(""))
                    )
                }
                _ => String::new(),
            };
            format!(
                "The {label}{} is off {} since {since}{how}. Do you want it back on?",
                setting_purpose(label),
                where_(layer)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "focus-hooks-switches-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("focus/.focus-sessions")).unwrap();
            Self(Layout::new(root, Harness::Claude))
        }

        fn write(&self, rel: &str, text: &str) {
            std::fs::write(self.0.project.join(rel), text).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    const LOCAL: &str = "focus.settings.local.json";
    const RECORD: &str = "focus/.focus-sessions/recorded-switches.json";
    const FREEZE: &str = r#"{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["FOCUS_DISABLE_REVIEW_FREEZE_HOOK"]}}"#;

    fn at(iso: &str) -> DateTime<Local> {
        DateTime::<Local>::from(
            SystemTime::UNIX_EPOCH
                + std::time::Duration::from_millis(
                    u64::try_from(focus_js::date::parse_instant(iso).unwrap()).unwrap(),
                ),
        )
    }

    #[test]
    fn a_setting_is_explained_in_a_few_words_or_not_at_all() {
        assert_eq!(
            setting_purpose("review freeze check"),
            " (it stops edits to work you already approved)"
        );
        assert_eq!(
            setting_purpose("Guard Policy"),
            " (it sets how many checks run)"
        );
        assert_eq!(setting_purpose("something else"), "");
    }

    #[test]
    fn the_persons_words_are_quoted_on_one_capped_line() {
        assert_eq!(
            quoted("  turn  the \"freeze\"\noff  "),
            "turn the 'freeze' off"
        );
        // 200 characters kept, the trailing space dropped before the mark.
        let shown = quoted(&"word ".repeat(60));
        assert_eq!(shown.encode_utf16().count(), 199 + 3);
        assert!(shown.ends_with("word..."));
        assert_eq!(quoted(&"é".repeat(250)), format!("{}...", "é".repeat(200)));
    }

    #[test]
    fn an_instant_reads_as_the_time_today_and_the_date_otherwise() {
        let now = at("2026-03-10T15:04:00Z");
        assert_eq!(clock(None, now), "an earlier session");
        let today = clock(Some(now), now);
        assert!(focus_js::re::test(r"^\d\d:\d\d$", "", &today), "{today}");
        let earlier = clock(Some(at("2026-01-05T10:00:00Z")), now);
        assert!(
            focus_js::re::test(r"^2026-01-0[56] \d\d:\d\d$", "", &earlier),
            "{earlier}"
        );
    }

    #[test]
    fn a_recorded_switch_is_worded_with_its_date_and_the_persons_words() {
        let tree = Tree::new("recorded");
        assert!(switches_off_lines(&tree.0).is_empty());
        tree.write(LOCAL, FREEZE);
        tree.write(
            RECORD,
            r#"{"version": 1, "switches": [{"name": "FOCUS_DISABLE_REVIEW_FREEZE_HOOK", "target": "local", "since": "2026-01-05T10:00:00Z", "how": "chat", "words": "freeze off"}]}"#,
        );
        let lines = switches_off_lines(&tree.0);
        assert_eq!(lines.len(), 1);
        assert!(
            focus_js::re::test(
                r#"^The review freeze check \(it stops edits to work you already approved\) is off for this project since 2026-01-0[56] \d\d:\d\d, because you said: "freeze off"\. Do you want it back on\?$"#,
                "",
                &lines[0]
            ),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn an_unrecorded_or_unreadable_record_dates_the_switch_by_its_file() {
        let tree = Tree::new("unrecorded");
        tree.write(LOCAL, FREEZE);
        let lines = switches_off_lines(&tree.0);
        assert_eq!(lines.len(), 1);
        assert!(
            focus_js::re::test(
                r"^The review freeze check \(it stops edits to work you already approved\) is off for this project since \d\d:\d\d\. Do you want it back on\?$",
                "",
                &lines[0]
            ),
            "{}",
            lines[0]
        );
        // A record of another version, an entry with a bad date, one set
        // some other way: the switch is still off, worded without the words.
        tree.write(RECORD, r#"{"version": 2}"#);
        assert_eq!(switches_off_lines(&tree.0).len(), 1);
        tree.write(
            RECORD,
            r#"{"version": 1, "switches": [{"name": "FOCUS_DISABLE_REVIEW_FREEZE_HOOK", "target": "local", "since": "nope", "how": "other", "words": "ignored"}]}"#,
        );
        assert_eq!(
            switches_off_lines(&tree.0)[0],
            "The review freeze check (it stops edits to work you already approved) is off for this project since an earlier session. Do you want it back on?"
        );
    }

    #[test]
    fn a_refused_settings_file_or_a_switch_the_environment_sets_names_nothing() {
        let tree = Tree::new("refused");
        tree.write(
            LOCAL,
            r#"{"flags": {"bypasses": ["FOCUS_DISABLE_REVIEW_FREEZE_HOOK"]}}"#,
        );
        assert!(switches_off_lines(&tree.0).is_empty());
        // A switch that is not one of the person's checks is not reported.
        tree.write(
            LOCAL,
            r#"{"schemaVersion": 1, "flags": {"schemaVersion": 1, "bypasses": ["FOCUS_DISABLE_SENSORS"]}}"#,
        );
        assert!(switches_off_lines(&tree.0).is_empty());
    }
}

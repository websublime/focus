//! The switch a person types in chat, read as upstream reads it
//! (`parseTypedGuardSwitchRequest` in `aidlc-lib.ts`): the plain words
//! for "no plan stops on this piece of work" or a Guard Policy word, or a
//! `/focus` line whose flags carry settings. Pure: what the switch does
//! once read (the setter) is the config command's, not this module's.
//!
//! `split_kiro_command_args` is the word splitter Kiro's command line and
//! the typed line share (`splitKiroCommandArgs`), kept here with its one
//! reader in Focus so far. Both index the text by UTF-16 code unit, as
//! upstream does.

use focus_js::re;

use crate::fences::Fence;

/// `TYPED_INTENT_SETTING_KEYS`: the settings a typed line may carry.
const TYPED_INTENT_SETTING_KEYS: [&str; 12] = [
    "depth",
    "test-strategy",
    "review",
    "guard-policy",
    "sensors",
    "learnings",
    "summary-confirmation",
    "plan-approval",
    "guard.plan-approval",
    "guard.review-freeze",
    "guard.state-transition",
    "guard.reviewer-scope",
];

/// `TYPED_REQUEST_FLAGS`: the request's own flags, which carry no switch.
const TYPED_REQUEST_FLAGS: [&str; 4] = ["skip", "add", "project-type", "collaborators"];

/// "skip plan approval", "turn off plan approval for this work", "no more
/// plan approvals", "plan approval off": an instruction, never a question.
const PLAN_APPROVAL_OFF_WORDS: &str = r"^(?:please\s+)?(?:(?:skip|stop|drop|disable|turn off|switch off|no more|no)\s+(?:the\s+)?plan[- ]approvals?|(?:turn|switch)\s+(?:the\s+)?plan[- ]approvals?\s+off|plan[- ]approvals?\s+off|(?:don'?t|do not|stop)\s+ask(?:ing)?\s+(?:me\s+)?to\s+approve\s+(?:the\s+|each\s+|every\s+)?(?:code\s+)?plans?)(?:\s+(?:for|on)\s+(?:this|the rest of this)(?:\s+piece of)?\s+(?:work|intent|feature|project|task))?(?:,?\s+please)?$";

const GUARD_POLICY_WORDS: &str = r"^(?:guard[- ]policy|change[- ]control)\s+(relaxed|off)$";

/// The entry of a typed command: `/focus`, `$focus` or the bare word.
const COMMAND_ENTRY: &str = r"^(?:/focus|\$focus|focus)(?:\s+|$)";

/// One switch the person typed: a setting key and `relaxed` or `off`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardSwitch {
    /// `guard-policy`, `guard.<fence>`, `summary-confirmation` or
    /// `plan-approval`.
    pub key: String,
    pub value: String,
}

/// A setting typed as a flag, as the config setter names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedSetting {
    pub key: String,
    pub value: String,
}

/// What a typed line asks for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TypedSwitchRequest {
    pub switches: Vec<GuardSwitch>,
    pub settings: Vec<TypedSetting>,
    pub space: Option<String>,
    pub intent: Option<String>,
    pub scope: Option<String>,
    /// Read as a switch, but not readable as a whole: what the person hears.
    pub error: Option<String>,
    /// `--plan-approval off` typed as a flag of the new work the message
    /// describes.
    pub new_work_plan_approval_off: bool,
    /// `--guard-policy relaxed|off` typed as a flag of the new work.
    pub new_work_guard_policy: Option<String>,
    /// Sensors, learnings or summary confirmation typed as flags of the new
    /// work, in the order upstream lists them.
    pub new_work_ceremonies: Vec<(String, String)>,
    /// `--guard.<fence> off` typed as flags of the new work.
    pub new_work_fences_off: Vec<Fence>,
    /// The plain-words switch asked as a question ("skip plan approval?").
    pub asked: bool,
    /// The words typed after the flags, when there are any.
    pub words: Option<String>,
    /// A flag-shaped token before the person's words that this parser cannot
    /// read.
    pub unread: Option<String>,
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn from_units(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn is_ws(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(focus_js::text::is_js_whitespace)
}

/// `/[\p{L}\p{N}]/u.test(ch)` on one code unit: a lone surrogate is neither.
fn is_letter(unit: Option<&u16>) -> bool {
    unit.and_then(|&unit| char::from_u32(u32::from(unit)))
        .is_some_and(|ch| ch.is_alphabetic() || ch.is_numeric())
}

/// `apostropheIsLetter`: an apostrophe between letters is a letter, and so
/// is one that ends a word when nothing later closes it ("users' files").
fn apostrophe_is_letter(raw: &[u16], i: usize) -> bool {
    if i > 0 && is_letter(raw.get(i - 1)) && is_letter(raw.get(i + 1)) {
        return true;
    }
    for j in i + 1..raw.len() {
        if raw[j] == u16::from(b'\'') && !(is_letter(raw.get(j - 1)) && is_letter(raw.get(j + 1))) {
            return false;
        }
    }
    true
}

/// `splitKiroCommandArgs`: the words of a typed line, quotes and the
/// escapes Kiro passes through consumed.
#[must_use]
pub fn split_kiro_command_args(raw: &str) -> Vec<String> {
    let raw = units(raw);
    let mut tokens: Vec<String> = Vec::new();
    let mut current: Vec<u16> = Vec::new();
    let mut quote: Option<u16> = None;
    let mut started = false;
    let backslash = u16::from(b'\\');
    let single = u16::from(b'\'');
    let double = u16::from(b'"');
    let mut i = 0;
    while i < raw.len() {
        let ch = raw[i];
        if ch == backslash {
            let next = raw.get(i + 1).copied();
            let output_value_token = tokens.last().is_some_and(|token| token == "--output");
            let current_text = from_units(&current);
            let windows_path_token =
                re::test(r"^[A-Za-z]:$", "", &current_text) || current.contains(&backslash);
            let mut following_token: Vec<u16> = Vec::new();
            if next.is_some_and(is_ws) {
                let mut start = i + 1;
                while start < raw.len() && is_ws(raw[start]) {
                    start += 1;
                }
                let mut end = start;
                while end < raw.len() && !is_ws(raw[end]) {
                    end += 1;
                }
                following_token = raw[start..end].to_vec();
            }
            let following = from_units(&following_token);
            let ends_output_path_before_option = output_value_token
                && quote.is_none()
                && (following == "--export" || following == "--output");
            let closes_quoted_output_path = output_value_token
                && quote.is_some()
                && next == quote
                && raw.get(i + 2).is_none_or(|&after| is_ws(after));
            let escapes_whitespace = quote.is_none()
                && !windows_path_token
                && !ends_output_path_before_option
                && next.is_some_and(is_ws);
            let escapes_shell_separator =
                quote.is_none() && !windows_path_token && next == Some(u16::from(b';'));
            let escapes_quote = next.is_some()
                && !windows_path_token
                && !closes_quoted_output_path
                && ((quote.is_none() && (next == Some(single) || next == Some(double)))
                    || (quote.is_some() && next == quote));
            if escapes_whitespace || escapes_shell_separator || escapes_quote {
                current.push(next.unwrap_or_default());
                i += 1;
            } else {
                current.push(backslash);
            }
            started = true;
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            started = true;
            i += 1;
            continue;
        }
        if ch == single && started && apostrophe_is_letter(&raw, i) {
            current.push(ch);
            i += 1;
            continue;
        }
        if ch == single || ch == double {
            quote = Some(ch);
            started = true;
            i += 1;
            continue;
        }
        if is_ws(ch) {
            if started {
                tokens.push(from_units(&current));
                current.clear();
                started = false;
            }
            i += 1;
            continue;
        }
        current.push(ch);
        started = true;
        i += 1;
    }
    if started {
        tokens.push(from_units(&current));
    }
    tokens
}

/// `parseWorkspaceCommand(tokens).kind !== "not-workspace"`: the line is a
/// workspace command, which owns the whole invocation.
fn is_workspace_command(tokens: &[String]) -> bool {
    matches!(
        tokens.first().map(String::as_str),
        Some("space-create" | "intent" | "space")
    )
}

/// `guard.plan-approval` is another spelling of `plan-approval`, and
/// `change-control` of `guard-policy`.
fn setting_name(key: &str) -> &str {
    match key {
        "change-control" => "guard-policy",
        "guard.plan-approval" => "plan-approval",
        other => other,
    }
}

fn known_flag(key: &str) -> bool {
    key == "space"
        || key == "intent"
        || key == "scope"
        || TYPED_REQUEST_FLAGS.contains(&key)
        || TYPED_INTENT_SETTING_KEYS.contains(&setting_name(key))
}

/// Insertion-ordered map: upstream's `Map` keeps the order keys were first
/// set, and a value set again keeps its place.
fn set_ordered(map: &mut Vec<(String, String)>, key: &str, value: &str) {
    match map.iter_mut().find(|(known, _)| known == key) {
        Some(entry) => entry.1 = value.to_owned(),
        None => map.push((key.to_owned(), value.to_owned())),
    }
}

fn get_ordered<'a>(map: &'a [(String, String)], key: &str) -> Option<&'a str> {
    map.iter()
        .find(|(known, _)| known == key)
        .map(|(_, value)| value.as_str())
}

fn remove_ordered(map: &mut Vec<(String, String)>, key: &str) {
    map.retain(|(known, _)| known != key);
}

/// `parseTypedGuardSwitchRequest`. With `words_answer`, the words after
/// the flags answer the question that is open, so the flags are for the
/// work open now rather than for new work.
#[must_use]
pub fn parse_typed_guard_switch_request(prompt: &str, words_answer: bool) -> TypedSwitchRequest {
    let trimmed = focus_js::text::trim(prompt);
    let trailing = re::find(r"[.,;:!?]+$", "", trimmed)
        .map(|found| &trimmed[found.range.clone()])
        .unwrap_or("");
    let text = &trimmed[..trimmed.len() - trailing.len()];
    let Some(command) = re::find(COMMAND_ENTRY, "i", text) else {
        // The person's own words for "no plan stops on this piece of work".
        // A question ("skip plan approval?"), a remark, or anything longer is
        // not a switch: the agent answers it.
        if trailing.contains('?') {
            let asked = re::test(PLAN_APPROVAL_OFF_WORDS, "i", text)
                || re::test(GUARD_POLICY_WORDS, "i", text);
            return TypedSwitchRequest {
                asked,
                ..TypedSwitchRequest::default()
            };
        }
        if re::test(PLAN_APPROVAL_OFF_WORDS, "i", text) {
            return TypedSwitchRequest {
                switches: vec![GuardSwitch {
                    key: "plan-approval".to_owned(),
                    value: "off".to_owned(),
                }],
                settings: vec![TypedSetting {
                    key: "plan-approval".to_owned(),
                    value: "off".to_owned(),
                }],
                ..TypedSwitchRequest::default()
            };
        }
        let lowered = focus_js::text::to_lower_case(text);
        let value = re::find(GUARD_POLICY_WORDS, "i", &lowered)
            .and_then(|found| re::group(&lowered, &found, 1))
            .map(str::to_owned);
        return match value {
            None => TypedSwitchRequest::default(),
            Some(value) => TypedSwitchRequest {
                switches: vec![GuardSwitch {
                    key: "guard-policy".to_owned(),
                    value: value.clone(),
                }],
                settings: vec![TypedSetting {
                    key: "guard-policy".to_owned(),
                    value,
                }],
                ..TypedSwitchRequest::default()
            },
        };
    };
    let tokens = split_kiro_command_args(focus_js::text::trim(&text[command.range.end..]));
    // Workspace commands own the whole invocation: never apply a trailing
    // lowering flag to the active selection before a switch or create command
    // resolves its destination.
    if is_workspace_command(&tokens) {
        return TypedSwitchRequest::default();
    }
    let lower = |index: usize| tokens.get(index).map(|t| focus_js::text::to_lower_case(t));
    let config_form = lower(0).as_deref() == Some("config") && lower(1).as_deref() == Some("set");
    let mut switches: Vec<(String, String)> = Vec::new();
    let mut settings: Vec<(String, String)> = Vec::new();
    let mut space: Option<String> = None;
    let mut intent: Option<String> = None;
    let mut scope: Option<String> = None;
    let mut guard_policy_spelling: Option<String> = None;
    let mut described = false;
    let mut words: Vec<String> = Vec::new();
    let mut unread: Option<String> = None;
    // A switch of theirs never does nothing in silence: where the message
    // holds a contradiction, or a setting with no value, nothing is changed
    // and they hear which part could not be read.
    let lost = |switches: &[(String, String)], settings: &[(String, String)], reason: String| {
        if switches.is_empty() && settings.is_empty() {
            return TypedSwitchRequest::default();
        }
        TypedSwitchRequest {
            error: Some(reason),
            ..TypedSwitchRequest::default()
        }
    };
    let mut index = if config_form { 2 } else { 0 };
    if config_form && tokens.len() < 4 {
        return TypedSwitchRequest::default();
    }
    while index < tokens.len() {
        let token = tokens[index].clone();
        index += 1;
        if !config_form && token == "--" {
            described = index < tokens.len();
            words.extend(tokens[index..].iter().cloned());
            break;
        }
        let config_key: Option<String> = if config_form && index == 3 {
            Some(focus_js::text::to_lower_case(&token))
        } else {
            token.strip_prefix("--").map(focus_js::text::to_lower_case)
        };
        let Some(config_key) = config_key else {
            if !config_form {
                described = true;
                words.push(token);
                continue;
            }
            return TypedSwitchRequest::default();
        };
        // A flag-shaped token this parser does not know: once they have
        // started describing the work it is one of their words; before any
        // description it is left out, with a value-shaped token after it, and
        // named back to them once the switches are carried out.
        if !config_form && !known_flag(&config_key) {
            if described {
                words.push(token);
                continue;
            }
            if unread.is_none() {
                unread = Some(token);
            }
            if tokens
                .get(index)
                .is_some_and(|next| !next.starts_with("--"))
            {
                index += 1;
            }
            continue;
        }
        let value = match tokens.get(index) {
            Some(next) if !next.starts_with("--") => {
                index += 1;
                Some(next.clone())
            }
            _ => None,
        };
        let Some(value) = value.filter(|value| !focus_js::text::trim(value).is_empty()) else {
            // A setting of theirs with nothing after it: there is no value to
            // apply, so nothing changes and they are told which flag is
            // missing one.
            return if TYPED_INTENT_SETTING_KEYS.contains(&setting_name(&config_key)) {
                lost(
                    &switches,
                    &settings,
                    format!(
                        "Nothing changed: \"--{config_key}\" came with no value. Which value did you mean for it?"
                    ),
                )
            } else {
                TypedSwitchRequest::default()
            };
        };
        if config_key == "space" || config_key == "intent" {
            let slot = if config_key == "space" {
                &mut space
            } else {
                &mut intent
            };
            if slot.is_some() {
                return TypedSwitchRequest::default();
            }
            *slot = Some(value);
            continue;
        }
        if !config_form && config_key == "scope" {
            if scope.is_some() {
                return TypedSwitchRequest::default();
            }
            scope = Some(value);
            continue;
        }
        if !config_form && TYPED_REQUEST_FLAGS.contains(&config_key.as_str()) {
            continue;
        }
        let current_key = setting_name(&config_key).to_owned();
        if !TYPED_INTENT_SETTING_KEYS.contains(&current_key.as_str()) {
            return TypedSwitchRequest::default();
        }
        let normalized = focus_js::text::to_lower_case(&value);
        let previous = get_ordered(&settings, &current_key).map(str::to_owned);
        if current_key == "guard-policy"
            && let Some(spelling) = &guard_policy_spelling
            && *spelling != config_key
            && let Some(previous) = &previous
            && *previous != normalized
        {
            // Both names of the same setting, with two different values.
            return lost(
                &switches,
                &settings,
                format!(
                    "Nothing changed: you typed Guard Policy twice in that command, as {previous} and {normalized}. Which did you mean?"
                ),
            );
        }
        if current_key == "guard-policy" {
            guard_policy_spelling = Some(config_key.clone());
        }
        set_ordered(&mut settings, &current_key, &normalized);
        let key = if current_key == "guard-policy" {
            current_key.clone()
        } else if current_key == "summary-confirmation" || current_key == "plan-approval" {
            // The last value wins, so a later on drops an earlier off.
            if normalized != "off" {
                remove_ordered(&mut switches, &current_key);
                continue;
            }
            current_key.clone()
        } else {
            let Some(fence) = current_key.strip_prefix("guard.") else {
                continue;
            };
            if Fence::from_config_key(&current_key).is_none() {
                continue;
            }
            if normalized != "off" {
                remove_ordered(&mut switches, &format!("guard.{fence}"));
                continue;
            }
            format!("guard.{fence}")
        };
        if normalized == "relaxed" || normalized == "off" {
            set_ordered(&mut switches, &key, &normalized);
        }
    }
    // Beside a description, summary confirmation off could land on the active
    // piece of work before the new-work offer, or the message may be a
    // question about the flag: not the person's switch at prompt time. Plan
    // approval off typed for the new work is still the person's: creation
    // honors it for the piece of work this chat creates next.
    let for_new_work = described && !words_answer;
    let new_work_plan_approval_off =
        for_new_work && get_ordered(&settings, "plan-approval") == Some("off");
    let mut new_work_ceremonies = Vec::new();
    for key in ["sensors", "learnings", "summary-confirmation"] {
        if let Some(value) = get_ordered(&settings, key)
            && for_new_work
            && (value == "on" || value == "off")
        {
            new_work_ceremonies.push((key.to_owned(), value.to_owned()));
        }
    }
    for ceremony in ["summary-confirmation", "plan-approval"] {
        if for_new_work && get_ordered(&settings, ceremony) == Some("off") {
            remove_ordered(&mut switches, ceremony);
            remove_ordered(&mut settings, ceremony);
        }
    }
    // Guard Policy typed with the new work is for that work, never for the
    // work open now.
    let typed_policy = get_ordered(&settings, "guard-policy").map(str::to_owned);
    let new_work_guard_policy =
        typed_policy.filter(|policy| for_new_work && (policy == "relaxed" || policy == "off"));
    if new_work_guard_policy.is_some() {
        remove_ordered(&mut switches, "guard-policy");
        remove_ordered(&mut settings, "guard-policy");
    }
    // So is a check turned off with it, when off is the last word typed for it.
    let mut new_work_fences_off = Vec::new();
    for fence in Fence::SWITCHABLE {
        let key = fence.config_key();
        if !for_new_work
            || fence == Fence::PlanApproval
            || get_ordered(&settings, &key) != Some("off")
        {
            continue;
        }
        new_work_fences_off.push(fence);
        remove_ordered(&mut switches, &key);
        remove_ordered(&mut settings, &key);
    }
    TypedSwitchRequest {
        switches: switches
            .into_iter()
            .map(|(key, value)| GuardSwitch { key, value })
            .collect(),
        settings: settings
            .into_iter()
            .map(|(key, value)| TypedSetting { key, value })
            .collect(),
        space,
        intent,
        scope,
        error: None,
        new_work_plan_approval_off,
        new_work_guard_policy,
        new_work_ceremonies,
        new_work_fences_off,
        asked: false,
        words: (!words.is_empty()).then(|| words.join(" ")),
        unread,
    }
}

/// `isTypedGuardSwitchPrompt`.
#[must_use]
pub fn is_typed_guard_switch_prompt(prompt: &str) -> bool {
    !parse_typed_guard_switch_request(prompt, false)
        .switches
        .is_empty()
}

/// `isTypedGuardSwitchQuestion`.
#[must_use]
pub fn is_typed_guard_switch_question(prompt: &str) -> bool {
    parse_typed_guard_switch_request(prompt, false).asked
}

/// `isAidlcCommandPrompt`: a prompt that is a command to Focus, by its
/// slash or Codex entry (`/focus …`, `/focus-<runner> …`, `$focus …`).
/// Other text that starts with a slash is the person's own words.
#[must_use]
pub fn is_command_prompt(prompt: &str) -> bool {
    re::test(r"^[/$]focus(?:[-\s]|$)", "i", focus_js::text::trim(prompt))
}

/// `aidlcEntryWords`: what the person typed after a plain `/focus` or
/// `$focus` entry; `None` for any other prompt, a `/focus-<runner>` entry
/// included.
#[must_use]
pub fn entry_words(prompt: &str) -> Option<String> {
    let text = focus_js::text::trim(prompt);
    let entry = re::find(r"^[/$]focus(?:\s+|$)", "i", text)?;
    Some(focus_js::text::trim(&text[entry.range.end..]).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The corpus (`fixtures/oracle`, families `typed_switch` and `kiro_args`)
    // compares the parser and the splitter with upstream; these name the
    // readings a reader of this module should know.

    #[test]
    fn plain_words_switch_plan_approval_and_the_policy() {
        let request = parse_typed_guard_switch_request("Skip the plan approvals, please.", false);
        assert_eq!(
            request.switches,
            vec![GuardSwitch {
                key: "plan-approval".into(),
                value: "off".into()
            }]
        );
        let request = parse_typed_guard_switch_request("guard policy relaxed", false);
        assert_eq!(request.switches[0].key, "guard-policy");
        assert_eq!(request.switches[0].value, "relaxed");
        assert!(
            parse_typed_guard_switch_request("guard policy strict", false)
                .switches
                .is_empty()
        );
        // Asked about, the words are a question for the agent.
        let asked = parse_typed_guard_switch_request("skip plan approval?", false);
        assert!(asked.asked && asked.switches.is_empty());
        assert!(!parse_typed_guard_switch_request("skip the plan", false).asked);
        assert!(is_typed_guard_switch_question("change control off?"));
        assert!(is_typed_guard_switch_prompt(
            "no more plan approvals for this work"
        ));
        assert!(!is_typed_guard_switch_prompt("approve the plan"));
    }

    #[test]
    fn flags_on_a_typed_line_are_settings_and_the_rest_is_the_persons_words() {
        let request = parse_typed_guard_switch_request(
            "/focus --guard-policy off --depth minimal add the export",
            false,
        );
        // Typed with new work, the policy is for that work: not a switch now,
        // and not among the settings either.
        assert_eq!(
            request.settings,
            vec![TypedSetting {
                key: "depth".into(),
                value: "minimal".into()
            }]
        );
        assert!(request.switches.is_empty());
        assert_eq!(request.new_work_guard_policy.as_deref(), Some("off"));
        assert_eq!(request.words.as_deref(), Some("add the export"));
        // With the words answering an open question, the flags are for the
        // work open now.
        let answer =
            parse_typed_guard_switch_request("/focus --guard-policy off approve the plan", true);
        assert_eq!(answer.switches.len(), 1);
        assert!(answer.new_work_guard_policy.is_none());
        // Settings alone are switches; a flag with no value says so.
        assert_eq!(
            parse_typed_guard_switch_request("/focus --guard.review-freeze off", false).switches[0]
                .key,
            "guard.review-freeze"
        );
        let missing = parse_typed_guard_switch_request("/focus --guard-policy", false);
        assert!(missing.switches.is_empty() && missing.error.is_none());
        let missing = parse_typed_guard_switch_request("/focus --plan-approval off --depth", false);
        assert!(
            missing
                .error
                .as_deref()
                .unwrap()
                .starts_with("Nothing changed: \"--depth\" came with no value.")
        );
        let twice = parse_typed_guard_switch_request(
            "/focus --guard-policy off --change-control relaxed",
            false,
        );
        assert!(
            twice
                .error
                .as_deref()
                .unwrap()
                .contains("you typed Guard Policy twice")
        );
        // An unknown flag before the words is named back; after them it is a word.
        let unread = parse_typed_guard_switch_request("/focus --nope x --plan-approval off", false);
        assert_eq!(unread.unread.as_deref(), Some("--nope"));
        assert_eq!(unread.switches.len(), 1);
        let worded = parse_typed_guard_switch_request("/focus add a --help flag", false);
        assert_eq!(worded.words.as_deref(), Some("add a --help flag"));
        assert!(worded.unread.is_none());
        // Workspace commands own the line; the config form reads one setting.
        assert!(
            parse_typed_guard_switch_request("/focus intent create x --guard-policy off", false)
                .switches
                .is_empty()
        );
        assert_eq!(
            parse_typed_guard_switch_request("/focus config set guard-policy off", false)
                .switches
                .len(),
            1
        );
        assert!(
            parse_typed_guard_switch_request("/focus config set guard-policy", false)
                .switches
                .is_empty()
        );
    }

    #[test]
    fn the_entry_and_the_command_prompt() {
        assert!(is_command_prompt("/focus next"));
        assert!(is_command_prompt(" $focus"));
        assert!(is_command_prompt("/focus-runner go"));
        assert!(!is_command_prompt("/focusx"));
        assert!(!is_command_prompt("/api/users returns 500"));
        assert_eq!(
            entry_words("/focus  use postgres "),
            Some("use postgres".into())
        );
        assert_eq!(entry_words("/FOCUS"), Some(String::new()));
        assert_eq!(entry_words("/focus-runner go"), None);
        assert_eq!(entry_words("focus on it"), None);
        // The bare entry stays, as upstream's `aidlc …` does: a sentence
        // that begins with the English word reads as a command line with
        // words and no flags, which is no switch, no command prompt and
        // no `/focus` line, so the hook records it as the person's words.
        let prose = "focus on the login page, not the settings";
        let request = parse_typed_guard_switch_request(prose, false);
        assert!(request.switches.is_empty() && request.error.is_none());
        assert_eq!(
            request.words.as_deref(),
            Some("on the login page, not the settings")
        );
        assert!(!is_typed_guard_switch_prompt(prose));
        assert!(!is_typed_guard_switch_question(prose));
        assert!(!is_command_prompt(prose));
        assert!(is_typed_guard_switch_prompt("focus --guard-policy relaxed"));
    }

    #[test]
    fn kiro_words_keep_apostrophes_in_words_and_read_escapes() {
        assert_eq!(
            split_kiro_command_args("a 'b c' \"d e\""),
            ["a", "b c", "d e"]
        );
        assert_eq!(
            split_kiro_command_args("users' files it's"),
            ["users'", "files", "it's"]
        );
        assert_eq!(
            split_kiro_command_args(r"a\ b a\;b a\'b"),
            ["a b", "a;b", "a'b"]
        );
        assert_eq!(
            split_kiro_command_args(r"C:\Users\x y"),
            [r"C:\Users\x", "y"]
        );
        // An escaped space in an `--output` path is kept only before an
        // option, where it ended the path.
        assert_eq!(
            split_kiro_command_args(r"--output a\ b --export"),
            ["--output", "a b", "--export"]
        );
        assert_eq!(
            split_kiro_command_args(r"--output a\ --export"),
            ["--output", r"a\", "--export"]
        );
        assert_eq!(split_kiro_command_args("'unterminated"), ["unterminated"]);
        assert!(split_kiro_command_args("   ").is_empty());
    }
}

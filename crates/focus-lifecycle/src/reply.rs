//! What a reply says, as syntax: a cancellation phrase, an exact pick of an
//! offered option, and text that credits the assistant with a decision.
//!
//! Nothing here reads meaning. A reply that is nothing but one offered
//! option, by number, letter or label, is an exact pick; a reply that is
//! only a cancellation phrase is no answer; a decision text that names the
//! agent as its author, or the Stop hook as the one who confirmed, is
//! nobody's decision. Anything else is the conductor's to read.
//!
//! Mirrors `isNonAnswer`, `formatReceivedReply`, `stripRecommendedDecorator`
//! and `exactOptionPick` in `aidlc-reply-reader.ts`, and `stageGateApproval`,
//! `selfAttributedDecisionMarker` and `maskQuotedDecisionExamples` in
//! `aidlc-lib.ts`.

use focus_js::re;
use focus_js::text::trim;

/// The gate's third choice once the revision cap is reached.
pub const ACCEPT_AS_IS_CHOICE: &str = "Accept as-is";
/// The gate's two choices.
pub const APPROVAL_GATE_LABELS: [&str; 2] = ["Approve", "Request Changes"];

const NON_ANSWER: &str = r"^(?:cancel(?:led|ed)?|cancellation|dismiss(?:ed)?|abort(?:ed)?|timed?[ -]?out|timeout|no (?:answer|response)|(?:user|question) (?:cancel(?:led|ed)|dismissed))[.!]?$";

/// `isNonAnswer`: empty, or only a cancellation phrase.
#[must_use]
pub fn is_non_answer(text: Option<&str>) -> bool {
    let text = trim(text.unwrap_or(""));
    text.is_empty() || re::test(NON_ANSWER, "i", text)
}

const RECEIVED_REPLY_DISPLAY_LIMIT: usize = 120;

/// `formatReceivedReply`: the reply on one line, cut to 120 characters,
/// quoted as JSON.
#[must_use]
pub fn format_received_reply(text: Option<&str>) -> String {
    let normalized = re::replace(r"\s+", "g", trim(text.unwrap_or("")), " ");
    let normalized = if normalized.is_empty() {
        "(empty)".to_owned()
    } else {
        normalized
    };
    let units: Vec<u16> = normalized.encode_utf16().collect();
    let display = if units.len() <= RECEIVED_REPLY_DISPLAY_LIMIT {
        normalized
    } else {
        format!(
            "{}...",
            String::from_utf16_lossy(&units[..RECEIVED_REPLY_DISPLAY_LIMIT - 3])
        )
    };
    focus_js::json::stringify(&serde_json::Value::String(display))
}

/// `stripRecommendedDecorator`.
#[must_use]
pub fn strip_recommended_decorator(text: &str) -> String {
    trim(&re::replace(r"\s*\(recommended\)\s*$", "i", text, "")).to_owned()
}

fn clean(value: &str) -> String {
    let stripped = re::replace(r#"^[`*_"'\s]+|[`*_"'\s]+$"#, "g", trim(value), "");
    let stripped = strip_recommended_decorator(&stripped);
    let stripped = re::replace(r"[\s.!]+$", "g", &stripped, "");
    trim(&re::replace(r"\s+", "g", &stripped, " ")).to_lowercase()
}

/// `exactOptionPick`: the index of the one offered option the reply is,
/// by number, letter or label; `None` for anything else.
#[must_use]
pub fn exact_option_pick(text: Option<&str>, labels: &[&str]) -> Option<usize> {
    let reply = clean(text.unwrap_or(""));
    if reply.is_empty() || labels.is_empty() {
        return None;
    }
    let index = |position: usize| (position < labels.len()).then_some(position);
    if let Some(found) = re::find(r"^(?:option\s+)?[([#]?\s*(\d{1,2})\s*[)\]]?$", "", &reply) {
        let number: usize = re::group(&reply, &found, 1)?.parse().ok()?;
        return index(number.checked_sub(1)?);
    }
    if let Some(found) = re::find(r"^(?:option\s+)?[([]?([a-z])[)\]]?$", "", &reply) {
        let letter = re::group(&reply, &found, 1)?.bytes().next()?;
        return index(usize::from(letter - b'a'));
    }
    let unprefixed = re::replace(r"^(?:(?:\d{1,2}|[a-z])[.):]\s+)", "", &reply, "");
    let cleaned: Vec<String> = labels.iter().map(|label| clean(label)).collect();
    let named = cleaned.iter().position(|label| *label == unprefixed)?;
    if cleaned.iter().rposition(|label| *label == unprefixed) != Some(named) {
        return None;
    }
    if let Some(found) = re::find(r"^(\d{1,2}|[a-z])[.):]\s+", "", &reply) {
        let prefix = re::group(&reply, &found, 1)?;
        let prefixed = if prefix.bytes().next().is_some_and(|b| b.is_ascii_digit()) {
            prefix.parse::<usize>().ok()?.checked_sub(1)?
        } else {
            usize::from(prefix.bytes().next()? - b'a')
        };
        if prefixed != named {
            return None;
        }
    }
    Some(named)
}

/// `stageGateApproval`: which approval the input names: `Accept as-is`
/// when the cap is reached and the input says so, else `Approve`.
#[must_use]
pub fn stage_gate_approval(input: Option<&str>, accept_as_is: bool) -> &'static str {
    let labelled = re::test(
        r#"^(?:(?:[A-Za-z]|\d+)[.)])?[\s"'`]*accept\s+as[\s-]+is[\s"'`.!]*(?:\(recommended\)[\s"'`.!]*)?$"#,
        "i",
        trim(input.unwrap_or("")),
    );
    if accept_as_is && labelled {
        ACCEPT_AS_IS_CHOICE
    } else {
        "Approve"
    }
}

/// What kind of decision a text is read for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    Approval,
    Rejection,
    Answer,
}

/// A text that credits someone other than the person with the decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfAttribution {
    pub category: &'static str,
    pub phrase: String,
}

/// `maskQuotedDecisionExamples`: code, quotes and block quotes blanked out,
/// so a quoted example is not read as the text's own claim.
fn mask_quoted(text: &str) -> String {
    let mask = |value: &str| -> String {
        value
            .chars()
            .map(|c| if c == '\n' { '\n' } else { ' ' })
            .collect()
    };
    let mut out = text.to_owned();
    for (pattern, flags) in [
        (r"(```|~~~)[\s\S]*?\1", "g"),
        (r"(^|\n)[ \t]*(?:```|~~~)[^\n]*(?:\n[\s\S]*|$)", "g"),
        (r"``[^`\n]*``|`[^`\n]*`", "g"),
        (r"^ {0,3}>[^\n]*(?:\n(?!\s*$)[^>\n][^\n]*)*", "gm"),
        (r#""[^"\n]*"|“[^”\n]*”|‘[^’\n]*’"#, "g"),
        (r"(^|[\s([{:])'[^'\n]+'(?=$|[\s)\]},.;:!?])", "gm"),
    ] {
        let pattern = re::compile(pattern, flags);
        let mut masked = String::new();
        let mut last = 0;
        for found in pattern.find_iter(&out) {
            masked.push_str(&out[last..found.range().start]);
            masked.push_str(&mask(&out[found.range()]));
            last = found.range().end;
        }
        masked.push_str(&out[last..]);
        out = masked;
    }
    out
}

const IMPLIED_CONFIRMATION: &str = r"\b(?:confirm(?:ed|ation|s)?|approv(?:ed|al)|consent(?:ed)?)\s+(?:by|from)\s+(?:the\s+)?(?:stop[\s-]+)?hook\b|\bby\s+(?:the\s+)?(?:stop[\s-]+)?hook[\s-]+trigger\b";

/// `selfAttributedDecisionMarker`: the first phrase of `text` that names
/// the assistant as the decision's author.
#[must_use]
pub fn self_attributed_decision_marker(
    text: Option<&str>,
    kind: DecisionKind,
) -> Option<SelfAttribution> {
    let original = text.unwrap_or("");
    let candidate = mask_quoted(original);
    let actor = "(?:agent|assistant|conductor|model|ai)";
    let noun = match kind {
        DecisionKind::Approval => "(?:approval|approve|decision|choice|confirmation)",
        DecisionKind::Rejection => {
            "(?:rejection|reject|decision|choice|change[ -]request|request changes)"
        }
        DecisionKind::Answer => "(?:answer|decision|choice|confirmation)",
    };
    let verb = match kind {
        DecisionKind::Approval => "(?:approv(?:e|ed|ing)|choos(?:e|en|ing)|select(?:ed|ing))",
        DecisionKind::Rejection => {
            "(?:reject(?:ed|ing)?|request(?:ed|ing)? changes|choos(?:e|en|ing)|select(?:ed|ing))"
        }
        DecisionKind::Answer => {
            "(?:answer(?:ed|ing)?|respond(?:ed|ing)?|choos(?:e|en|ing)|select(?:ed|ing))"
        }
    };
    let tail = r"(?=(?:\s*(?:[,.;:!?。！？；：，、)）]|$|\b(?:to|because|so|for)\b)|\s+[-–—]))";
    let mut categories: Vec<(&'static str, String)> = vec![
        (
            "non-human-decision",
            format!(r"\b(?:not|isn['’]t)\s+(?:(?:a|the)\s+)?human(?:['’]s)?\s+{noun}\b{tail}"),
        ),
        (
            "model-authored-decision",
            format!(
                r"(?:^|\n)\s*(?:[A-Z]\.\s*)?{actor}[-\s]+initiated\s+(?:(?:this|the|an?)\s+)?{noun}\b{tail}"
            ),
        ),
        (
            "model-authored-decision",
            format!(
                r"(?:^|\n)\s*(?:[A-Z]\.\s*)?{actor}[-\s]+(?:authored|generated|recorded|written)\s+(?:(?:this|the|an?)\s+)?{noun}\b{tail}"
            ),
        ),
        (
            "model-authored-decision",
            format!(
                r"\b{noun}\s+(?:was|is)\s+(?:generated|authored|written|supplied|entered|selected|chosen|made)\s+by\s+(?:(?:an?|the)\s+)?{actor}\b{tail}"
            ),
        ),
        (
            "model-authored-decision",
            format!(r"\bi\s*,?\s+(?:as\s+)?(?:the\s+)?{actor}\s*,?\s+(?:am|have)\s+{verb}\b"),
        ),
        (
            "model-authored-decision",
            format!(r"\b{actor}\s+(?:chose|selected)\s+(?:this\s+)?{noun}\b{tail}"),
        ),
    ];
    if kind == DecisionKind::Rejection {
        categories.push((
            "model-authored-decision",
            format!(r"\b{actor}\s+rejected\s+this\b{tail}"),
        ));
    }
    categories.push((
        "conductor-default",
        r"(?:^|\n)\s*(?:[A-Z]\.\s*)?(?:[^\n]{1,80}?\s[-–—:]\s*)?conductor(?:['’]s)?[ -]+default(?=(?:\s*(?:[,.?!;:。！？；：，、()[\]]|$)|\s+[-–—]))".to_owned(),
    ));
    categories.push(("implied-confirmation", IMPLIED_CONFIRMATION.to_owned()));
    for (category, pattern) in &categories {
        if let Some(found) = re::find(pattern, "i", &candidate) {
            return Some(SelfAttribution {
                category,
                phrase: original[found.range()].to_owned(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_answer_is_empty_or_a_cancellation() {
        assert!(is_non_answer(None));
        assert!(is_non_answer(Some("  ")));
        assert!(is_non_answer(Some("Cancelled.")));
        assert!(is_non_answer(Some("timed out")));
        assert!(is_non_answer(Some("User dismissed")));
        assert!(!is_non_answer(Some("cancel the standing order")));
        assert!(!is_non_answer(Some("Approve")));
        assert_eq!(format_received_reply(Some("  a   b\nc ")), "\"a b c\"");
        assert_eq!(format_received_reply(None), "\"(empty)\"");
        let long = "x".repeat(130);
        assert_eq!(
            format_received_reply(Some(&long)),
            format!("\"{}...\"", "x".repeat(117))
        );
    }

    #[test]
    fn an_exact_pick_is_syntax_only() {
        let labels = &APPROVAL_GATE_LABELS;
        assert_eq!(exact_option_pick(Some("Approve"), labels), Some(0));
        assert_eq!(
            exact_option_pick(Some("  \"request changes\" "), labels),
            Some(1)
        );
        assert_eq!(exact_option_pick(Some("2"), labels), Some(1));
        assert_eq!(exact_option_pick(Some("(b)"), labels), Some(1));
        assert_eq!(exact_option_pick(Some("option 1"), labels), Some(0));
        assert_eq!(exact_option_pick(Some("1. Approve"), labels), Some(0));
        assert_eq!(exact_option_pick(Some("2. Approve"), labels), None);
        assert_eq!(
            exact_option_pick(Some("Approve (Recommended)"), labels),
            Some(0)
        );
        assert_eq!(exact_option_pick(Some("approve it"), labels), None);
        assert_eq!(exact_option_pick(Some("3"), labels), None);
        assert_eq!(exact_option_pick(Some(""), labels), None);
        assert_eq!(exact_option_pick(Some("a"), &["x", "x"]), Some(0));
        assert_eq!(exact_option_pick(Some("x"), &["x", "x"]), None);
        assert_eq!(
            stage_gate_approval(Some("Accept as-is"), true),
            ACCEPT_AS_IS_CHOICE
        );
        assert_eq!(
            stage_gate_approval(Some("3) accept as is (Recommended)"), true),
            ACCEPT_AS_IS_CHOICE
        );
        assert_eq!(stage_gate_approval(Some("Accept as-is"), false), "Approve");
        assert_eq!(stage_gate_approval(Some("Approve"), true), "Approve");
    }

    #[test]
    fn a_self_attributed_decision_is_named() {
        let marker = |text: &str, kind: DecisionKind| {
            self_attributed_decision_marker(Some(text), kind).map(|m| (m.category, m.phrase))
        };
        assert_eq!(
            marker(
                "This is not a human approval, I just went ahead",
                DecisionKind::Approval
            ),
            Some(("non-human-decision", "not a human approval".to_owned()))
        );
        assert_eq!(
            marker("Assistant-initiated approval.", DecisionKind::Approval),
            Some((
                "model-authored-decision",
                "Assistant-initiated approval".to_owned()
            ))
        );
        assert_eq!(
            marker(
                "The decision was made by the conductor for speed",
                DecisionKind::Rejection
            ),
            Some((
                "model-authored-decision",
                "decision was made by the conductor".to_owned()
            ))
        );
        assert_eq!(
            marker("I, as the agent, am approving this", DecisionKind::Approval),
            Some((
                "model-authored-decision",
                "I, as the agent, am approving".to_owned()
            ))
        );
        assert_eq!(
            marker("agent rejected this.", DecisionKind::Rejection),
            Some(("model-authored-decision", "agent rejected this".to_owned()))
        );
        assert_eq!(marker("agent rejected this.", DecisionKind::Approval), None);
        assert_eq!(
            marker("Going with the conductor's default.", DecisionKind::Answer),
            None
        );
        assert_eq!(
            marker("Answer - conductor's default", DecisionKind::Answer),
            Some((
                "conductor-default",
                "Answer - conductor's default".to_owned()
            ))
        );
        assert_eq!(
            marker("conductor default", DecisionKind::Answer),
            Some(("conductor-default", "conductor default".to_owned()))
        );
        assert_eq!(
            marker(
                "user implicitly confirmed by hook trigger",
                DecisionKind::Approval
            ),
            Some(("implied-confirmation", "confirmed by hook".to_owned()))
        );
        // A quoted example, or one in code, is not the text's own claim.
        assert_eq!(
            marker(
                "She wrote \"not a human approval\" in the brief",
                DecisionKind::Approval
            ),
            None
        );
        assert_eq!(
            marker("see `agent rejected this`", DecisionKind::Rejection),
            None
        );
        assert_eq!(
            marker("> agent rejected this\nfine", DecisionKind::Rejection),
            None
        );
        assert_eq!(marker("Approve", DecisionKind::Approval), None);
        assert_eq!(marker("", DecisionKind::Approval), None);
    }
}

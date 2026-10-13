//! Reading entries back.
//!
//! A shard is read as text. Blocks are separated by a `---` rule on its own
//! line, a field is the first line of a block that starts with its bold
//! label, and readers take the first match they find. Nothing here trusts a
//! block to be well formed: the ledger is edited by people and merged by git.
//!
//! Mirrors `findAllEvents` (L33848), `auditBlockField` (L14102), `getField`
//! (L29424), `hasUnsafeSingleLineCharacter` (L29098) and `parseFieldArgs`
//! (L40438) in `aidlc-lib.ts`.

use focus_js::order::cmp_utf16;
use focus_js::re;
use focus_js::text::trim;

use crate::entry::Fields;

/// One block that matched a query, with the timestamp it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub timestamp: String,
    pub block: String,
}

use focus_js::re::escape as escape_regex;

/// Every block of `audit` whose event is `event` (and, when `slug` is given,
/// whose `Bolt slug` is `slug`), oldest first.
///
/// Windows line endings are folded to `\n` first, so a shard edited on
/// Windows reads like any other. Shards are concatenated in file-name order,
/// which is not time order, so the result is sorted by timestamp; blocks with
/// the same timestamp keep the order they were read in. A block with no
/// timestamp line is not an event.
#[must_use]
pub fn find_all_events(audit: &str, event: &str, slug: Option<&str>) -> Vec<Found> {
    let audit = audit.replace("\r\n", "\n");
    // A value sits on its label's own line: `[ \t]*`, never `\s*`, which
    // would read a bare label's value off the next line.
    let event_line = format!(r"^\*\*Event\*\*:[ \t]*{}[ \t]*$", escape_regex(event));
    let slug_line =
        slug.map(|slug| format!(r"^\*\*Bolt slug\*\*:[ \t]*{}[ \t]*$", escape_regex(slug)));
    let mut found: Vec<Found> = Vec::new();
    for block in audit.split("\n---\n") {
        if !re::test(&event_line, "m", block) {
            continue;
        }
        if slug_line
            .as_deref()
            .is_some_and(|line| !re::test(line, "m", block))
        {
            continue;
        }
        let Some(stamp) = re::find(r"^\*\*Timestamp\*\*:[ \t]*(\S+)", "m", block) else {
            continue;
        };
        let Some(timestamp) = re::group(block, &stamp, 1) else {
            continue;
        };
        found.push(Found {
            timestamp: timestamp.to_owned(),
            block: block.to_owned(),
        });
    }
    // Stable: equal timestamps stay in reading order.
    found.sort_by(|a, b| cmp_utf16(&a.timestamp, &b.timestamp));
    found
}

/// The value of the first `**name**:` line of a block, trimmed; `None` when
/// the block has no such line. A leading `- ` on the line is ignored, so the
/// same reader serves the bulleted fields of a state file.
///
/// A field written twice still answers with its first value. A caller that
/// needs the field to be unique must check that itself.
#[must_use]
pub fn block_field<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let label = format!("**{name}**:");
    block
        .split('\n')
        .map(|raw| raw.strip_prefix("- ").unwrap_or(raw))
        .find_map(|line| line.strip_prefix(label.as_str()))
        .map(trim)
}

/// The value of the first `- **field**: value` bullet in `content`, trimmed;
/// `None` when there is none. A bullet with nothing after the colon answers
/// with the empty string, not with the next line.
#[must_use]
pub fn bullet_field<'a>(content: &'a str, field: &str) -> Option<&'a str> {
    let line = format!(r"^- \*\*{}\*\*:[ \t]*(.*)$", escape_regex(field));
    let found = re::find(&line, "m", content)?;
    re::group(content, &found, 1).map(trim)
}

/// Whether a value would break out of the single line it must stay on: any
/// control character, or one of the two Unicode line separators.
#[must_use]
pub fn has_unsafe_single_line_character(value: &str) -> bool {
    value
        .chars()
        .any(|c| c <= '\u{1f}' || c == '\u{7f}' || c == '\u{2028}' || c == '\u{2029}')
}

/// Collect every `--field key=value` pair of a command line. A pair with no
/// `=`, or with nothing before it, is skipped; a key given twice keeps its
/// last value.
#[must_use]
pub fn parse_field_args(args: &[String]) -> Fields {
    let mut fields = Fields::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--field" && index + 1 < args.len() {
            if let Some((key, value)) = args[index + 1]
                .split_once('=')
                .filter(|(key, _)| !key.is_empty())
            {
                fields.set(key, value);
            }
            index += 1;
        }
        index += 1;
    }
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_come_back_oldest_first_whatever_order_they_were_written_in() {
        let audit = "**Event**: A\n**Timestamp**: 2\n---\n**Timestamp**: 1 x\n**Event**: A  \n---\n**Event**: A\n---\n**Event**: AB\n**Timestamp**: 0\n";
        let found = find_all_events(audit, "A", None);
        let found: Vec<(&str, &str)> = found
            .iter()
            .map(|f| (f.timestamp.as_str(), f.block.as_str()))
            .collect();
        assert_eq!(
            found,
            [
                ("1", "**Timestamp**: 1 x\n**Event**: A  "),
                ("2", "**Event**: A\n**Timestamp**: 2"),
            ]
        );
    }

    #[test]
    fn a_slug_narrows_the_query_and_special_characters_are_literal() {
        let audit = "**Event**: A.B\n**Timestamp**: 1\n**Bolt slug**: one\n---\n**Event**: AxB\n**Timestamp**: 2\n**Bolt slug**: one\n---\n**Event**: A.B\n**Timestamp**: 3\n**Bolt slug**: two\n";
        let found = find_all_events(audit, "A.B", Some("one"));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].timestamp, "1");
    }

    #[test]
    fn a_field_is_its_first_line_with_or_without_a_bullet() {
        let block = "## Heading\n- **Stage**:  code-generation \n**Stage**: second\n**Empty**:";
        assert_eq!(block_field(block, "Stage"), Some("code-generation"));
        assert_eq!(block_field(block, "Empty"), Some(""));
        assert_eq!(block_field(block, "Absent"), None);
    }

    #[test]
    fn an_empty_bullet_does_not_read_the_next_line() {
        let state = "- **Scope**:\n- **Depth**: standard \n";
        assert_eq!(bullet_field(state, "Scope"), Some(""));
        assert_eq!(bullet_field(state, "Depth"), Some("standard"));
        assert_eq!(bullet_field(state, "Depth (x)"), None);
    }

    #[test]
    fn field_arguments_keep_the_last_value_and_skip_malformed_pairs() {
        let args: Vec<String> = [
            "--field", "b=1", "--field", "2=x", "--field", "b=3=4", "--field", "=no", "--field",
            "novalue", "x", "--field",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(
            parse_field_args(&args).entries(),
            [("2", "x"), ("b", "3=4")]
        );
    }

    #[test]
    fn control_characters_and_line_separators_are_unsafe_on_one_line() {
        assert!(has_unsafe_single_line_character("a\tb"));
        assert!(has_unsafe_single_line_character("a\u{2028}b"));
        assert!(has_unsafe_single_line_character("a\u{7f}"));
        assert!(!has_unsafe_single_line_character("a\u{85}b é"));
    }
}

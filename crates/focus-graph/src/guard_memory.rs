//! Guard Policy declarations in the method ("memory") files.
//!
//! A team can pin the Guard Policy for everyone by writing a `Mode:` line
//! under `## Guard Policy` in `org.md`, `team.md` or `project.md`. A proposal
//! to relax the policy is checked against those files before it reaches a
//! person: if any layer says strict, the proposal is refused there.
//!
//! What counts is what a reader of the rendered file sees: a `Mode:` line
//! inside an HTML comment, a code block or a code span that runs over lines
//! declares nothing, and a heading inside one opens no section. That is
//! decided by `focus-markdown`.
//!
//! Mirrors `memoryGuardPolicyDeclarations`, `memorySectionBody` and
//! `structuredField` in `aidlc-lib.ts` (L38958, L38506, L38461).

use std::path::{Path, PathBuf};

use focus_js::text::{trim, trim_end};
use focus_js::{Result, bail, re};
use focus_markdown::{Visible, visible_lines};

use crate::fsx::read_text;
use crate::scopes::GuardPolicy;

/// The section heading, and the retired one still read.
pub const GUARD_POLICY_HEADING: &str = "## Guard Policy";
pub const CHANGE_CONTROL_HEADING: &str = "## Change Control";
/// The memory layers that may declare a policy, broadest first.
pub const LAYERS: [&str; 3] = ["org", "team", "project"];

/// One layer's declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub layer: &'static str,
    pub path: PathBuf,
    pub heading: &'static str,
    pub value: GuardPolicy,
}

impl Declaration {
    /// The sentence a proposal to relax the policy gets while this layer
    /// holds strict.
    #[must_use]
    pub fn strict_refusal(&self) -> String {
        format!(
            "Your team set Guard Policy to strict in {} (section: {}), so it stays strict for everyone on this repo. Changing that line there changes it.",
            self.path.display(),
            self.heading.trim_start_matches("## ")
        )
    }
}

/// Every layer under `memory_dir` that declares a Guard Policy mode, org then
/// team then project. A missing file, a missing section and a section without
/// a `Mode:` line all declare nothing.
///
/// # Errors
///
/// A `Mode:` value that is not one of the three, or a file that cannot be
/// read, from disk or as Markdown.
pub fn declarations(memory_dir: &Path) -> Result<Vec<Declaration>> {
    let mut out = Vec::new();
    for layer in LAYERS {
        let path = memory_dir.join(format!("{layer}.md"));
        if !path.exists() {
            continue;
        }
        let declared = declared_mode(&read_text(&path)?)
            .map_err(|refusal| refusal.prefixed(path.display()))?;
        let Some((heading, raw)) = declared else {
            continue;
        };
        let section = heading.trim_start_matches("## ");
        let Some(value) = GuardPolicy::parse(&raw) else {
            bail!(
                "Invalid {section} Mode \"{raw}\" in {} (section: {section}). Expected one of: strict, relaxed, off.",
                path.display()
            );
        };
        out.push(Declaration {
            layer,
            path,
            heading,
            value,
        });
    }
    Ok(out)
}

/// The first `Mode:` value a memory file declares, with the heading it sits
/// under: the current heading is read before the retired one. `None` when
/// the file declares nothing.
///
/// # Errors
///
/// The file cannot be read as Markdown (see `focus_markdown::analyse`).
pub fn declared_mode(content: &str) -> Result<Option<(&'static str, String)>> {
    let lines = visible_lines(content, Visible::default())?;
    for heading in [GUARD_POLICY_HEADING, CHANGE_CONTROL_HEADING] {
        let body = section_body(&lines, heading);
        if trim(&body).is_empty() {
            continue;
        }
        if let Some(raw) = structured_field(&body, "Mode") {
            return Ok(Some((heading, raw)));
        }
    }
    Ok(None)
}

/// The body of one `## Heading` section of a file's visible lines: from the
/// line after the heading to the next `## ` heading. A heading hidden in a
/// comment or a code block neither opens nor closes it, and hidden lines
/// inside it are empty.
#[must_use]
pub fn section_body(lines: &[String], heading: &str) -> String {
    let Some(start) = lines.iter().position(|line| trim_end(line) == heading) else {
        return String::new();
    };
    let end = lines[start + 1..]
        .iter()
        .position(|line| re::test(r"^## [^\n]*$", "", line))
        .map_or(lines.len(), |offset| start + 1 + offset);
    lines[start + 1..end].join("\n")
}

/// A `Field: value` line, with an optional list marker and optional bold
/// around the field name, plus its wrapped continuation lines. `None` when
/// the field is absent or its value is empty.
#[must_use]
pub fn structured_field(section: &str, field: &str) -> Option<String> {
    let escaped: String = field
        .chars()
        .flat_map(|c| {
            let special = ".*+?^${}()|[]\\".contains(c);
            special.then_some('\\').into_iter().chain(Some(c))
        })
        .collect();
    let head =
        format!(r"^([ \t]*)(?:[-*][ \t]*)?(?:\*\*)?{escaped}(?:\*\*)?[ \t]*:[ \t]*(.*?)[ \t]*$");
    // Upstream splits on `/\r?\n/`: a `\r` not before a line feed stays.
    let unix = section.replace("\r\n", "\n");
    let lines: Vec<&str> = unix.split('\n').collect();
    for (idx, line) in lines.iter().enumerate() {
        let Some(found) = re::find(&head, "i", line) else {
            continue;
        };
        let head_indent = re::group(line, &found, 1).unwrap_or("").len();
        let mut parts: Vec<&str> = vec![trim(re::group(line, &found, 2).unwrap_or(""))];
        // A wrapped value continues on lines indented deeper than its head,
        // until a blank line, a shallower line, another structured field, or
        // a line that opens another block.
        for next in &lines[idx + 1..] {
            let indent = next.chars().take_while(|c| matches!(c, ' ' | '\t')).count();
            if indent <= head_indent || !re::test(r"\S", "", next) {
                break;
            }
            if re::test(
                r"^[ \t]*(?:[-*+][ \t]|\d+[.)][ \t]|[>#|]|```|~~~)",
                "",
                next,
            ) {
                break;
            }
            if re::test(
                r"^[ \t]*(?:\*\*)?(?:Mode|Methodology|Ordering)(?:\*\*)?[ \t]*:",
                "i",
                next,
            ) {
                break;
            }
            parts.push(trim(next));
        }
        let value = parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        return Some(value).filter(|value| !value.is_empty());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(org: &str) -> Result<Vec<(&'static str, GuardPolicy)>> {
        let dir = std::env::temp_dir().join(format!(
            "focus-guard-memory-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("org.md"), org).unwrap();
        let out = declarations(&dir)
            .map(|found| found.into_iter().map(|d| (d.heading, d.value)).collect());
        std::fs::remove_dir_all(&dir).unwrap();
        out
    }

    #[test]
    fn a_mode_inside_a_comment_declares_nothing() {
        // The shape upstream ships: the section exists, its only text is a
        // comment that mentions the three modes.
        let stock = "# Org\n\n## Guard Policy\n\n<!-- Mode: strict, relaxed, or off. Changing this line changes it. -->\n\n## Deployment\n\nMode: strict\n";
        assert_eq!(declared(stock).unwrap(), []);
        let multi = "## Guard Policy\n<!-- start\nMode: strict\n-->\n";
        assert_eq!(declared(multi).unwrap(), []);
    }

    #[test]
    fn a_visible_mode_line_declares_in_any_accepted_spelling() {
        for body in [
            "Mode: strict",
            "- Mode: strict",
            "* **Mode**: `strict`",
            "  mode :   STRICT  ",
            "Mode:\n  strict",
            "<!-- note --> Mode: strict",
        ] {
            let file = format!("## Guard Policy\n\n{body}\n\n## Next\n");
            assert_eq!(
                declared(&file).unwrap(),
                [(GUARD_POLICY_HEADING, GuardPolicy::Strict)],
                "{body:?}"
            );
        }
        // The retired heading is still read; the current one wins when both
        // declare.
        let retired = "## Change Control\nMode: relaxed\n";
        assert_eq!(
            declared(retired).unwrap(),
            [(CHANGE_CONTROL_HEADING, GuardPolicy::Relaxed)]
        );
        let both = "## Change Control\nMode: relaxed\n## Guard Policy\nMode: off\n";
        assert_eq!(
            declared(both).unwrap(),
            [(GUARD_POLICY_HEADING, GuardPolicy::Off)]
        );
    }

    #[test]
    fn an_invalid_mode_is_an_error_naming_the_file() {
        let message = declared("## Guard Policy\nMode: lenient\n")
            .unwrap_err()
            .message()
            .to_owned();
        assert!(
            message.starts_with("Invalid Guard Policy Mode \"lenient\" in "),
            "{message}"
        );
        assert!(
            message.ends_with("(section: Guard Policy). Expected one of: strict, relaxed, off."),
            "{message}"
        );
    }

    #[test]
    fn what_a_reader_does_not_see_declares_nothing() {
        for hidden in [
            // A fenced block, a comment that opens mid-line, an HTML block,
            // indented code, a code span over lines, a processing instruction.
            "## Guard Policy\n\n```\nMode: strict\n```\n",
            "```\n## Guard Policy\nMode: strict\n```\n",
            "## Guard Policy\ntext <!-- c\nMode: strict\n--> more\n",
            "## Guard Policy\n\n<div>\nMode: strict\n</div>\n",
            "## Guard Policy\n\n    Mode: strict\n",
            "## Guard Policy\na `b\nMode: strict\nc` d\n",
            "## Guard Policy\n\n<?php\nMode: strict\n?>\n",
            "<div>\n## Guard Policy\nMode: strict\n</div>\n",
        ] {
            assert_eq!(declared(hidden).unwrap(), [], "{hidden:?}");
        }
        // What only looks hidden is read: a comment that closed, `<` inside
        // a code span, a code span on one line.
        for seen in [
            "## Guard Policy\n<!-- one\ntwo --> Mode: off\n",
            "A `Conversation language: <language>` line.\n\n## Guard Policy\nMode: off\n",
            "## Guard Policy\n\n<details>\n\nMode: off\n\n</details>\n",
        ] {
            assert_eq!(
                declared(seen).unwrap(),
                [(GUARD_POLICY_HEADING, GuardPolicy::Off)],
                "{seen:?}"
            );
        }
    }

    /// Deliberate deviation: upstream reads this `Mode:` line and Focus does
    /// not. An escaped backtick is text, so the code span here opens at the
    /// second backtick of the first line and closes on the third line, with
    /// the `Mode:` line inside it: that is what CommonMark renders, and what
    /// both engines do for a span with no escape before it. Upstream's
    /// analysis looks for a span by putting a probe word after each backtick
    /// run; after the escaped one the probe renders as text, and the span is
    /// not recorded. Focus reads the span from the parser. Declared in
    /// `tools/oracle/differential.py` (`verbs/memory_span_after_escape`) and
    /// in the plan (markdown/commonmark).
    #[test]
    fn a_span_opened_after_an_escaped_backtick_hides_what_it_covers() {
        let escaped = "## Guard Policy\na \\` b `\nMode: strict\n` c\n";
        assert_eq!(declared(escaped).unwrap(), []);
        // The same span without the escape, which upstream hides too.
        assert_eq!(
            declared("## Guard Policy\na `b\nMode: strict\n` c\n").unwrap(),
            []
        );
        // Escaped at both ends there is no span, and the line is read.
        assert_eq!(
            declared("## Guard Policy\na \\` b\nMode: strict\n\\` c\n").unwrap(),
            [(GUARD_POLICY_HEADING, GuardPolicy::Strict)]
        );
    }

    /// Deliberate deviation: upstream reads this commented-out `Mode:` line
    /// and Focus does not. Upstream's analysis stops after 256 renders of a
    /// document and reads what it had not reached as text; sixty small
    /// tables are enough to get there, and the comment after them is then
    /// taken for a declaration. The same section at the top of the file
    /// declares nothing in both. Declared in `tools/oracle/differential.py`
    /// (`verbs/memory_past_budget`) and in the plan (markdown/commonmark).
    #[test]
    fn a_comment_at_the_end_of_a_long_file_is_still_a_comment() {
        let section = "## Guard Policy\n\n<!--\nMode: strict\n-->\n";
        let tables: String = (0..60)
            .map(|n| format!("| a{n} | b |\n| --- | --- |\n| `x{n}` | <!-- c --> |\n\n"))
            .collect();
        assert_eq!(declared(section).unwrap(), []);
        assert_eq!(declared(&format!("{tables}{section}")).unwrap(), []);
    }

    /// Deliberate deviation: upstream reads this `Mode:` line and Focus does
    /// not. `<!` followed by a letter opens an HTML block that runs to the
    /// first line with a `>` in it (CommonMark 0.31, start condition 4), so
    /// the line after it is inside the block. Upstream's renderer follows a
    /// text of the specification that wanted the letter in upper case, and
    /// reads a paragraph. Declared in `tools/oracle/differential.py`
    /// (`verbs/memory_lowercase_declaration`) and in the plan
    /// (markdown/commonmark).
    #[test]
    fn a_declaration_in_lower_case_hides_what_follows_it() {
        assert_eq!(
            declared("## Guard Policy\n<!x\nMode: strict\n").unwrap(),
            []
        );
        // In upper case both engines read a block.
        assert_eq!(
            declared("## Guard Policy\n<!X\nMode: strict\n").unwrap(),
            []
        );
        // The block ends on the line that holds `>`.
        assert_eq!(
            declared("## Guard Policy\n<!x y>\nMode: strict\n").unwrap(),
            [(GUARD_POLICY_HEADING, GuardPolicy::Strict)]
        );
    }

    #[test]
    fn a_field_name_is_matched_as_text() {
        assert_eq!(
            structured_field("A.b: 1\nAxb: 2", "A.b").as_deref(),
            Some("1")
        );
        assert_eq!(structured_field("Axb: 2", "A.b"), None);
        assert_eq!(
            structured_field("- **Mode**: strict\r\n  wrapped\r\n", "Mode").as_deref(),
            Some("strict wrapped")
        );
        // A carriage return with no line feed after it is part of its line,
        // and the value pattern does not run over it.
        assert_eq!(structured_field("Mode: strict\r", "Mode"), None);
    }
}

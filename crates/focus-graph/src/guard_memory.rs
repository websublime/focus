//! Guard Policy declarations in the method ("memory") files.
//!
//! A team can pin the Guard Policy for everyone by writing a `Mode:` line
//! under `## Guard Policy` in `org.md`, `team.md` or `project.md`. A proposal
//! to relax the policy is checked against those files before it reaches a
//! person: if any layer says strict, the proposal is refused there.
//!
//! Mirrors `memoryGuardPolicyDeclarations`, `memorySectionBody` and
//! `structuredField` in `aidlc-lib.ts` (L38958, L38506, L38461).
//!
//! # What this reader does not do
//!
//! Upstream decides which lines of a memory file are *visible* with a full
//! CommonMark parse (`Bun.markdown`): text inside an HTML comment, a fenced
//! block or a multi-line code span is not read. Focus has no Markdown block
//! parser yet (it is the largest open risk in the migration plan), so this
//! module reads only files where visibility can be decided line by line, and
//! **refuses** the rest instead of guessing. A file is readable here when,
//! outside comment blocks that open at the start of a line:
//!
//! - no line has a fence (`~~~`, or two or more backticks in a row);
//! - backticks on a line pair up, and none is preceded by a backslash;
//! - every `<` sits inside such a code span;
//!
//! and when the `## Guard Policy` section itself has no line indented four
//! spaces or more. The files upstream ships satisfy all of this. The refusal
//! names the file and the line, so the fix is visible.

use std::path::{Path, PathBuf};

use focus_js::text::{trim, trim_end};
use focus_js::{EngineError, Result, bail, re};

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
/// A `Mode:` value that is not one of the three; or a file this reader cannot
/// read reliably (see the module docs).
pub fn declarations(memory_dir: &Path) -> Result<Vec<Declaration>> {
    let mut out = Vec::new();
    for layer in LAYERS {
        let path = memory_dir.join(format!("{layer}.md"));
        if !path.exists() {
            continue;
        }
        let content = read_text(&path)?;
        let lines =
            visible_lines(&content).map_err(|refusal| EngineError::new(refusal.describe(&path)))?;
        for heading in [GUARD_POLICY_HEADING, CHANGE_CONTROL_HEADING] {
            let body = section_body(&lines, heading)
                .map_err(|refusal| EngineError::new(refusal.describe(&path)))?;
            if trim(&body.join("\n")).is_empty() {
                continue;
            }
            let Some(raw) = structured_field(&body, "Mode") else {
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
                path: path.clone(),
                heading,
                value,
            });
            break;
        }
    }
    Ok(out)
}

/// The first `Mode:` value a memory file declares, with the heading it sits
/// under: the current heading is read before the retired one. `Ok(None)`
/// when the file declares nothing.
///
/// # Errors
///
/// Why the file cannot be read reliably (see the module docs).
pub fn declared_mode(content: &str) -> std::result::Result<Option<(&'static str, String)>, String> {
    let describe = |refusal: Unreadable| format!("line {} {}", refusal.line, refusal.reason);
    let lines = visible_lines(content).map_err(describe)?;
    for heading in [GUARD_POLICY_HEADING, CHANGE_CONTROL_HEADING] {
        let body = section_body(&lines, heading).map_err(describe)?;
        if trim(&body.join("\n")).is_empty() {
            continue;
        }
        if let Some(raw) = structured_field(&body, "Mode") {
            return Ok(Some((heading, raw)));
        }
    }
    Ok(None)
}

/// Why a file is outside what this reader can decide.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Unreadable {
    /// 1-based.
    line: usize,
    reason: &'static str,
}

impl Unreadable {
    fn describe(&self, path: &Path) -> String {
        format!(
            "{}: line {} {}, and deciding what that hides needs a full Markdown reader, which Focus does not have yet. Focus cannot tell whether this file sets a Guard Policy, so it will not judge a relaxed or off proposal against it. Simplify that line, or propose strict.",
            path.display(),
            self.line,
            self.reason
        )
    }
}

/// The file's lines as a reader sees them: a line hidden inside a comment
/// block is empty.
fn visible_lines(content: &str) -> std::result::Result<Vec<String>, Unreadable> {
    let normalized = content
        .strip_prefix('\u{FEFF}')
        .unwrap_or(content)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut out = Vec::new();
    let mut in_comment = false;
    for (idx, line) in normalized.split('\n').enumerate() {
        let refuse = |reason| Unreadable {
            line: idx + 1,
            reason,
        };
        if line.contains(['\u{0}', '\u{1}', '\u{2}']) {
            return Err(refuse("contains a control character"));
        }
        let after_comment = |rest: &str| {
            if rest.contains('<') {
                Err(refuse("has markup after the end of a comment"))
            } else {
                Ok(rest.to_owned())
            }
        };
        if in_comment {
            match line.find("-->") {
                Some(end) => {
                    in_comment = false;
                    out.push(after_comment(&line[end + 3..])?);
                }
                None => out.push(String::new()),
            }
            continue;
        }
        if line.starts_with("<!--") {
            match line.find("-->") {
                Some(end) if end < 4 => {
                    return Err(refuse("opens a comment with a degenerate `<!-->`"));
                }
                Some(end) => out.push(after_comment(&line[end + 3..])?),
                None => {
                    in_comment = true;
                    out.push(String::new());
                }
            }
            continue;
        }
        check_plain(line).map_err(refuse)?;
        out.push(line.to_owned());
    }
    Ok(out)
}

/// A line outside any comment block must not start or continue a construct
/// that hides text.
fn check_plain(line: &str) -> std::result::Result<(), &'static str> {
    if line.contains("~~~") || line.contains("``") {
        return Err("has a code fence or a multi-backtick code span");
    }
    if line.contains("\\`") {
        return Err("has an escaped backtick");
    }
    let mut in_code = false;
    for c in line.chars() {
        match c {
            '`' => in_code = !in_code,
            '<' if !in_code => return Err("has `<` outside a code span"),
            _ => {}
        }
    }
    if in_code {
        return Err("has an unpaired backtick");
    }
    Ok(())
}

/// The lines of one `## Heading` section: from the line after the heading to
/// the next `## ` heading.
fn section_body(lines: &[String], heading: &str) -> std::result::Result<Vec<String>, Unreadable> {
    let Some(start) = lines.iter().position(|line| trim_end(line) == heading) else {
        return Ok(Vec::new());
    };
    let end = lines[start + 1..]
        .iter()
        .position(|line| re::test(r"^## [^\n]*$", "", line))
        .map_or(lines.len(), |offset| start + 1 + offset);
    let body = &lines[start + 1..end];
    for (offset, line) in body.iter().enumerate() {
        let indent: String = line
            .chars()
            .take_while(|c| matches!(c, ' ' | '\t'))
            .collect();
        let deep = indent.contains('\t') || indent.len() >= 4;
        if deep && !trim(line).is_empty() {
            return Err(Unreadable {
                line: start + 2 + offset,
                reason: "is indented four spaces or more inside the Guard Policy section",
            });
        }
    }
    Ok(body.to_vec())
}

/// A `Field: value` line, with an optional list marker and optional bold
/// around the field name, plus its wrapped continuation lines. `None` when
/// the field is absent or its value is empty.
fn structured_field(section: &[String], field: &str) -> Option<String> {
    let head =
        format!(r"^([ \t]*)(?:[-*][ \t]*)?(?:\*\*)?{field}(?:\*\*)?[ \t]*:[ \t]*(.*?)[ \t]*$");
    for (idx, line) in section.iter().enumerate() {
        let Some(found) = re::find(&head, "i", line) else {
            continue;
        };
        let head_indent = re::group(line, &found, 1).unwrap_or("").len();
        let mut parts: Vec<&str> = vec![trim(re::group(line, &found, 2).unwrap_or(""))];
        // A wrapped value continues on lines indented deeper than its head,
        // until a blank line, a shallower line, another structured field, or
        // a line that opens another block.
        for next in &section[idx + 1..] {
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
    fn a_file_that_needs_a_markdown_parser_is_refused_not_guessed() {
        for (file, reason) in [
            (
                "```\n## Guard Policy\nMode: strict\n```\n",
                "line 1 has a code fence",
            ),
            (
                "text <!-- c\n## Guard Policy\n",
                "line 1 has `<` outside a code span",
            ),
            ("a `b\nc` d\n", "line 1 has an unpaired backtick"),
            (
                "## Guard Policy\n    Mode: strict\n",
                "line 2 is indented four spaces",
            ),
            (
                "<div>\n## Guard Policy\n</div>\n",
                "line 1 has `<` outside a code span",
            ),
        ] {
            let message = declared(file).unwrap_err().message().to_owned();
            assert!(message.contains(reason), "{file:?}: {message}");
        }
        // `<` inside a code span is plain text, as in the shipped org.md.
        let fine = "A `Conversation language: <language>` line.\n\n## Guard Policy\nMode: off\n";
        assert_eq!(
            declared(fine).unwrap(),
            [(GUARD_POLICY_HEADING, GuardPolicy::Off)]
        );
    }
}

//! The consolidated summary a person confirms, as the evidence reads it.
//!
//! A stage that asks questions ends them with a summary of what it
//! understood, in the stage's questions file, under the heading
//! `## Consolidated Summary Confirmation`, with one `[Answer]:` line the
//! person's choice goes on. The Markdown is context: the agent can write
//! that text. What a gate weighs is the row `focus engine log` records for
//! the choice, which carries a hash of what the person confirmed. This
//! module reads the choice off the file and takes that hash
//! (`summaryConfirmationAnswer`, `summaryConfirmationContentHash`,
//! `aidlc-lib.ts` L12 764, L12 642).
//!
//! # What is hashed
//!
//! The file as it is, line endings made `\n`, without the one section that
//! may be added after the confirmation and is not part of it: `## Assumption
//! Confirmation`, with the thematic break that may stand before it. A
//! question added after the summary (`## Q<n>`) and `## Requested Changes
//! Feedback` are hashed, so a later confirmation covers them; any other
//! heading after the summary is refused, since nothing says whether the
//! person confirmed what stands under it.
//!
//! # What is kept as upstream has it, and what is not
//!
//! Upstream looks for a raw HTML tag in the line as the file has it, with
//! the columns of the line without a byte order mark, so on the first line
//! of a file that starts with one its columns fall one unit early. That
//! line is read here without the mark. No answer depends on it: the only
//! heading that counts before the summary section is a `#` one, and the
//! first line of a file is before it or is it.
//!
//! # Tests that another test covers
//!
//! Five questions change no answer when taken out, and are kept where
//! upstream has them. A comment's end is looked for after its four opening
//! characters, and the two comments that end inside those four are told
//! apart first. A comment is looked for from where a line's content
//! starts, and only container markers and indentation stand before that.
//! A raw HTML block is read from its first line, whichever line asks, and
//! its lines are asked in order. An `[Answer]:` line's value starts after
//! spaces and tabs, and is trimmed of both anyway. A choice is only
//! collected inside the summary section, so having one says the section
//! was seen.

use std::collections::{HashMap, HashSet};

use focus_graph::vocabulary::heading_key;
use focus_js::text::{trim, trim_end};
use focus_js::{Result, re};
use focus_markdown::{
    Blocks, COMMENT_MARKER, Line, LineKind, SpanKind, Visible, analyse, source_lines,
    visible_lines_of,
};
use sha2::{Digest, Sha256};

/// `SUMMARY_CONFIRMATION_CHECKPOINT`: the section's title, and the
/// checkpoint's name on a row.
pub const CHECKPOINT: &str = "Consolidated Summary Confirmation";
/// `SUMMARY_CONFIRMATION_HASH_SCOPE`: what a row's hash was taken over.
pub const HASH_SCOPE: &str = "confirmed-content-v2";

const BOUNDARIES: Visible = Visible {
    preserve_indented_code: false,
    preserve_comment_boundaries: true,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Atx,
    Setext,
    Html,
}

/// `VisibleMarkdownHeading`: a heading a reader of the rendered file sees.
#[derive(Debug)]
struct Heading {
    title: String,
    level: usize,
    style: Style,
    /// Inside a block quote or a list item. Said of a `#` heading only:
    /// upstream works it out for the other two styles as well, and reads it
    /// for none of them.
    nested: bool,
}

/// `line.slice(start)`, where `start` is a column upstream and Focus count
/// alike.
fn from(line: &str, start: usize) -> &str {
    line.get(start..).unwrap_or("")
}

/// `stripInvisibleCommentMarkers`.
fn without_markers(line: &str) -> String {
    line.replace(COMMENT_MARKER, "")
}

/// `visibleAtxHeading`.
fn atx_heading(line: &str, block: &Line) -> Option<Heading> {
    if block.kind != LineKind::Heading {
        return None;
    }
    let found = re::find(r"^ {0,3}(#{1,6})(?:[ \t]+|$)(.*)$", "", line)?;
    let title = without_markers(re::group(line, &found, 2).unwrap_or(""));
    let title = re::replace(r"[ \t]+#+[ \t]*$", "", &title, "");
    Some(Heading {
        title: trim(&title).to_owned(),
        level: re::group(line, &found, 1).map_or(0, str::len),
        style: Style::Atx,
        nested: !block.containers.is_empty(),
    })
}

/// `visibleSetextHeading`: the underline's line names the title above it.
fn setext_heading(lines: &[String], blocks: &Blocks, at: usize) -> Option<Heading> {
    let block = &blocks.lines[at];
    if block.kind != LineKind::Heading || at == 0 {
        return None;
    }
    let underline = from(&lines[at], block.content_start);
    let found = re::find(r"^(=+|-+)[ \t]*$", "", underline)?;
    let above = from(&lines[at - 1], blocks.lines[at - 1].content_start);
    Some(Heading {
        title: trim(&without_markers(above)).to_owned(),
        level: if re::group(underline, &found, 1).is_some_and(|rule| rule.starts_with('=')) {
            1
        } else {
            2
        },
        style: Style::Setext,
        nested: false,
    })
}

/// The lines of a document's raw HTML blocks with their comments blanked
/// (`HTML_FLOW_WITHOUT_COMMENTS`), each block read once.
struct Flows<'a> {
    lines: &'a [String],
    blocks: &'a Blocks,
    blanked: HashMap<usize, String>,
}

impl Flows<'_> {
    fn in_flow(&self, at: usize, id: Option<usize>) -> bool {
        let line = &self.blocks.lines[at];
        line.kind == LineKind::HtmlFlow && line.block == id
    }

    /// `htmlFlowWithoutComments`: a line of a raw HTML block with the
    /// comments of the block blanked, columns kept; a comment may open on
    /// an earlier line of the block.
    fn without_comments(&mut self, index: usize) -> String {
        if let Some(cached) = self.blanked.get(&index) {
            return cached.clone();
        }
        let id = self.blocks.lines[index].block;
        let mut first = index;
        while first > 0 && self.in_flow(first - 1, id) {
            first -= 1;
        }
        let mut in_comment = false;
        let mut line = first;
        while line < self.lines.len() && self.in_flow(line, id) {
            let mut text = self.lines[line].clone();
            let mut cursor = self.blocks.lines[line].content_start;
            while cursor < text.len() {
                let start = if in_comment {
                    Some(cursor)
                } else {
                    text[cursor..].find("<!--").map(|at| cursor + at)
                };
                let Some(start) = start else { break };
                // `<!-->` and `<!--->` are complete comments.
                let rest = &text[start..];
                let empty = if in_comment {
                    None
                } else if rest.starts_with("<!-->") {
                    Some(5)
                } else if rest.starts_with("<!--->") {
                    Some(6)
                } else {
                    None
                };
                let closing = match empty {
                    Some(length) => Some(start + length - 3),
                    None => {
                        let from = if in_comment { start } else { start + 4 };
                        text[from..].find("-->").map(|at| from + at)
                    }
                };
                let end = closing.map_or(text.len(), |at| at + 3);
                text.replace_range(start..end, &" ".repeat(end - start));
                in_comment = closing.is_none();
                cursor = end;
            }
            self.blanked.insert(line, text);
            line += 1;
        }
        self.blanked
            .get(&index)
            .cloned()
            .unwrap_or_else(|| self.lines[index].clone())
    }

    /// `visibleHtmlHeading`: an `<h1>` to `<h6>` tag the renderer keeps as
    /// markup, in a raw HTML block opened by a tag or inline.
    fn html_heading(&mut self, index: usize) -> Option<Heading> {
        let block = &self.blocks.lines[index];
        // Only a line of a raw HTML block has a kind.
        let flow = matches!(block.html_kind, Some(6 | 7));
        let text = if flow {
            self.without_comments(index)
        } else {
            self.lines[index].clone()
        };
        let spans: Vec<(usize, usize)> = if flow {
            vec![(block.content_start.min(text.len()), text.len())]
        } else {
            block
                .invisible
                .iter()
                .filter(|span| span.kind == SpanKind::HtmlText && span.token_start_line == index)
                .map(|span| (span.start, span.end))
                .collect()
        };
        for (start, end) in spans {
            // Inline positions come from the parser, so escapes, code and
            // link destinations cannot make an HTML heading. Tags of a raw
            // block still need their quoted attributes skipped.
            let within = text.get(start..end).unwrap_or("");
            let tags = re::find_all(
                r#"<(?:[^<>"']|"[^"]*"|'[^']*')*>|<h[1-6]\b[^>]*$"#,
                "i",
                within,
            );
            for tag in tags {
                let found = &within[tag.range()];
                let level = re::find(r"^<h([1-6])\b", "i", found)
                    .and_then(|h| re::group(found, &h, 1).and_then(|digit| digit.parse().ok()));
                if let Some(level) = level {
                    return Some(Heading {
                        title: format!("<h{level}>"),
                        level,
                        style: Style::Html,
                        nested: false,
                    });
                }
            }
        }
        None
    }
}

/// `visibleQuestionId`: a `Q<n>` heading, behind a leading emoji or not.
fn question_id(title: &str) -> Option<String> {
    let key = heading_key(title);
    let found = re::find(r"^Q([1-9][0-9]*)(?:[.:](?:[ \t]+.*)?)?$", "", &key)?;
    Some(format!("Q{}", re::group(&key, &found, 1).unwrap_or("")))
}

/// `assumptionExclusionStart`: a thematic break standing before the
/// `Assumption Confirmation` heading is presentation and is left out with
/// it. Only a rule with a blank line above it is one: a rule right under
/// text is a setext underline.
fn assumption_exclusion_start(lines: &[&str], heading: usize) -> usize {
    let blank = |line: &str| trim(line).is_empty();
    let mut above = heading;
    while above > 0 && blank(lines[above - 1]) {
        above -= 1;
    }
    if above == 0 {
        return heading;
    }
    let rule = above - 1;
    if re::test(r"^ {0,3}(?:-{3,}|\*{3,}|_{3,})\s*$", "", lines[rule])
        && (rule == 0 || blank(lines[rule - 1]))
    {
        rule
    } else {
        heading
    }
}

/// `summaryBoundarySpelling`: a line spelled as a top-level `Q<n>` or
/// `Requested Changes Feedback` heading, whatever the renderer made of it.
fn boundary_spelling(line: &str) -> bool {
    re::find(r"^ {0,3}##[ \t]+(.*?)(?:[ \t]+#+)?[ \t]*$", "", line)
        .and_then(|found| re::group(line, &found, 1))
        .is_some_and(|title| title == "Requested Changes Feedback" || question_id(title).is_some())
}

/// `summaryConfirmationContentHash`: the hash of what the person confirmed
/// in a questions file; the refusal's words when the file cannot be one.
///
/// # Errors
///
/// The file has no summary section, has it twice, repeats a question or the
/// assumption section, or has a heading after the summary that is none of
/// the three that may follow; or its Markdown cannot be read.
pub fn content_hash(content: &str) -> std::result::Result<String, String> {
    let normalized = content.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let blocks = analyse(&normalized).map_err(|err| err.to_string())?;
    let visible = visible_lines_of(&normalized, &blocks, BOUNDARIES);
    let counted = source_lines(&normalized);
    let mut flows = Flows {
        lines: &counted,
        blocks: &blocks,
        blanked: HashMap::new(),
    };
    let mut saw_summary = false;
    let mut assumption_seen = false;
    let mut open_excluded: Option<usize> = None;
    let mut excluded: HashSet<usize> = HashSet::new();
    let mut question_ids: HashSet<String> = HashSet::new();
    let mut close_excluded = |open: &mut Option<usize>, line: usize| {
        if let Some(start) = open.take() {
            excluded.extend(start..line);
        }
    };

    for line in 0..visible.len() {
        let block = &blocks.lines[line];
        let heading = atx_heading(from(&visible[line], block.content_start), block)
            .or_else(|| setext_heading(&visible, &blocks, line))
            .or_else(|| flows.html_heading(line));
        // The exclusion fails closed: a line spelled as a top-level Q<n> or
        // Requested Changes Feedback heading ends it even where the
        // renderer read no heading, so a misread line can only widen what
        // is hashed.
        if open_excluded.is_some() && boundary_spelling(lines[line]) {
            close_excluded(&mut open_excluded, line);
        }
        let Some(heading) = heading else { continue };
        let title = heading.title.as_str();
        let atx_h2 = heading.style == Style::Atx && heading.level == 2 && !heading.nested;

        if title == CHECKPOINT && atx_h2 {
            if saw_summary {
                return Err(format!("duplicate H2 section \"{CHECKPOINT}\""));
            }
            saw_summary = true;
            continue;
        }
        let question = if atx_h2 { question_id(title) } else { None };
        if !saw_summary {
            if let Some(id) = question
                && !question_ids.insert(id.clone())
            {
                return Err(format!("duplicate H2 section \"{id}\""));
            }
            continue;
        }
        if heading_key(title) == "Assumption Confirmation" && atx_h2 {
            if assumption_seen {
                return Err("duplicate H2 section \"Assumption Confirmation\"".to_owned());
            }
            assumption_seen = true;
            // Heading visibility blanks code blocks. Only white space of the
            // source may be crossed to an adjacent thematic break.
            open_excluded = Some(assumption_exclusion_start(&lines, line));
            continue;
        }
        if atx_h2 && (title == "Requested Changes Feedback" || question.is_some()) {
            if let Some(id) = question
                && !question_ids.insert(id.clone())
            {
                return Err(format!("duplicate H2 section \"{id}\""));
            }
            // A question may be added after an assumption decision. It is
            // in a later receipt's hash; only the assumption section stays
            // out.
            close_excluded(&mut open_excluded, line);
            continue;
        }
        let boundary = if assumption_seen {
            "after \"Assumption Confirmation\"; only Q<n> or \"Requested Changes Feedback\" sections may follow"
        } else {
            "after the consolidated summary; only Q<n>, \"Requested Changes Feedback\", or one \"Assumption Confirmation\" section may follow"
        };
        let kind = match heading.style {
            Style::Html => "HTML H",
            Style::Setext => "Setext H",
            Style::Atx => "H",
        };
        return Err(format!(
            "unsupported {kind}{} heading \"{title}\" {boundary}",
            heading.level
        ));
    }
    if !saw_summary {
        return Err(format!("missing required H2 section \"{CHECKPOINT}\""));
    }
    close_excluded(&mut open_excluded, lines.len());
    let confirmed: Vec<&str> = lines
        .iter()
        .enumerate()
        .filter(|(line, _)| !excluded.contains(line))
        .map(|(_, text)| *text)
        .collect();
    let confirmed = confirmed.join("\n");
    let digest = Sha256::digest(trim_end(&confirmed).as_bytes());
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// `summaryConfirmationAnswer`: the choice on the summary section's one
/// `[Answer]:` line, read through the same visibility as the hash; `None`
/// when the file has no summary section, or the section has no such line
/// or more than one.
///
/// # Errors
///
/// The file's Markdown cannot be read.
pub fn answer(content: &str) -> Result<Option<String>> {
    let blocks = analyse(content)?;
    let visible = visible_lines_of(content, &blocks, BOUNDARIES);
    let mut in_summary = false;
    let mut answers: Vec<String> = Vec::new();
    for (index, line) in visible.iter().enumerate() {
        // `visibleH2Title`: the whole line, so a heading inside a container
        // is not one.
        let title = atx_heading(line, &blocks.lines[index])
            .filter(|heading| heading.level == 2 && !heading.nested)
            .map(|heading| heading.title);
        if let Some(title) = title {
            if in_summary {
                break;
            }
            in_summary = title == CHECKPOINT;
            continue;
        }
        if !in_summary {
            continue;
        }
        let shown = without_markers(line);
        if let Some(found) = re::find(r"^\[Answer\]:[ \t]*(.*)$", "", &shown) {
            answers.push(trim(re::group(&shown, &found, 1).unwrap_or("")).to_owned());
        }
    }
    Ok(if in_summary && answers.len() == 1 {
        answers.pop()
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "# Intent Capture Questions\n\n## Q1. Who is it for?\n\n[Answer]: A\n\n## Consolidated Summary Confirmation\n\nA tool for the team.\n\n[Answer]: Looks correct\n";

    /// The comparison with upstream is the oracle corpus (kind `summary`).
    /// This is what the hash is for: the section added after a confirmation
    /// does not change what was confirmed, and anything else does.
    #[test]
    fn the_assumption_section_is_outside_what_was_confirmed() {
        let confirmed = content_hash(FILE).unwrap();
        assert_eq!(answer(FILE).unwrap().as_deref(), Some("Looks correct"));
        for added in [
            "\n## Assumption Confirmation\n\nWe assume Postgres.\n\n[Answer]: Looks correct\n",
            "\n---\n\n## Assumption Confirmation\n\nWe assume Postgres.\n",
            "\n\n***\n## \u{2705} Assumption Confirmation\n",
        ] {
            let with = format!("{FILE}{added}");
            assert_eq!(content_hash(&with).unwrap(), confirmed, "{added:?}");
            assert_eq!(answer(&with).unwrap().as_deref(), Some("Looks correct"));
        }
        // A question added afterwards is confirmed by the next receipt.
        let more = format!("{FILE}\n## Assumption Confirmation\n\nx\n\n## Q2. And then?\n");
        assert_ne!(content_hash(&more).unwrap(), confirmed);
        // The choice itself is part of what is hashed.
        let changed = FILE.replace("Looks correct", "Request changes");
        assert_ne!(content_hash(&changed).unwrap(), confirmed);
        assert_eq!(
            content_hash(&format!("{FILE}\n## Notes\n")).unwrap_err(),
            "unsupported H2 heading \"Notes\" after the consolidated summary; only Q<n>, \"Requested Changes Feedback\", or one \"Assumption Confirmation\" section may follow"
        );
    }
}

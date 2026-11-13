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
//! # Columns
//!
//! Upstream counts a line in UTF-16 units and `focus-markdown` in bytes.
//! Up to where a line's content starts the two agree, because only
//! container markers and indentation stand there. A raw HTML tag is looked
//! for in the line as the file has it, while the columns are those of the
//! line without a byte order mark: on the first line of a file that starts
//! with one, upstream's columns fall one unit early. That reading is kept,
//! unit for unit (`html_heading`), instead of a corrected one.

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
    /// Inside a block quote or a list item, or after other words on its line.
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
        nested: !block.containers.is_empty(),
    })
}

/// A document's lines as upstream indexes them, in UTF-16 units, with the
/// columns of `focus-markdown` (bytes of the line without a byte order
/// mark) taken to units.
struct Units<'a> {
    /// The lines as the file has them: the first keeps a byte order mark.
    raw: Vec<Vec<u16>>,
    /// The lines the columns were counted on.
    counted: &'a [String],
    blocks: &'a Blocks,
    /// `HTML_FLOW_WITHOUT_COMMENTS`.
    flows: HashMap<usize, Vec<u16>>,
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn find_units(text: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    (from..=text.len().checked_sub(needle.len())?).find(|at| text[*at..].starts_with(needle))
}

/// `text.slice(start, end)`, as text: half of a pair reads as U+FFFD, one
/// unit that is neither white space nor markup, like the half itself.
fn slice_units(text: &[u16], start: usize, end: usize) -> String {
    let end = end.min(text.len());
    let start = start.min(end);
    String::from_utf16_lossy(&text[start..end])
}

impl Units<'_> {
    /// A byte column of line `at` as the unit column upstream has for it.
    fn column(&self, at: usize, byte: usize) -> usize {
        let line = &self.counted[at];
        let mut byte = byte.min(line.len());
        while !line.is_char_boundary(byte) {
            byte -= 1;
        }
        line[..byte].encode_utf16().count()
    }

    fn in_flow(&self, at: usize, id: Option<usize>) -> bool {
        let line = &self.blocks.lines[at];
        line.kind == LineKind::HtmlFlow && line.block == id
    }

    /// `htmlFlowWithoutComments`: a line of a raw HTML block with the
    /// comments of the block blanked, columns kept; a comment may open on
    /// an earlier line of the block.
    fn flow_without_comments(&mut self, index: usize) -> Vec<u16> {
        if let Some(cached) = self.flows.get(&index) {
            return cached.clone();
        }
        let id = self.blocks.lines[index].block;
        let mut first = index;
        while first > 0 && self.in_flow(first - 1, id) {
            first -= 1;
        }
        let (open, close) = (units("<!--"), units("-->"));
        let mut in_comment = false;
        let mut line = first;
        while line < self.raw.len() && self.in_flow(line, id) {
            let mut text = self.raw[line].clone();
            let mut cursor = self.column(line, self.blocks.lines[line].content_start);
            while cursor < text.len() {
                let start = if in_comment {
                    Some(cursor)
                } else {
                    find_units(&text, &open, cursor)
                };
                let Some(start) = start else { break };
                // `<!-->` and `<!--->` are complete comments.
                let rest = &text[start..];
                let empty = if in_comment {
                    None
                } else if rest.starts_with(&units("<!-->")) {
                    Some(5)
                } else if rest.starts_with(&units("<!--->")) {
                    Some(6)
                } else {
                    None
                };
                let closing = match empty {
                    Some(length) => Some(start + length - 3),
                    None => find_units(&text, &close, if in_comment { start } else { start + 4 }),
                };
                let end = closing.map_or(text.len(), |at| at + 3);
                for unit in &mut text[start..end] {
                    *unit = u16::from(b' ');
                }
                in_comment = closing.is_none();
                cursor = end;
            }
            self.flows.insert(line, text);
            line += 1;
        }
        self.flows
            .get(&index)
            .cloned()
            .unwrap_or_else(|| self.raw[index].clone())
    }

    /// `visibleHtmlHeading`: an `<h1>` to `<h6>` tag the renderer keeps as
    /// markup, in a raw HTML block opened by a tag or inline.
    fn html_heading(&mut self, index: usize) -> Option<Heading> {
        let block = &self.blocks.lines[index];
        let flow = block.kind == LineKind::HtmlFlow && matches!(block.html_kind, Some(6 | 7));
        let content_start = self.column(index, block.content_start);
        let contained = !block.containers.is_empty();
        let spans: Vec<(usize, Option<usize>)> = if flow {
            vec![(content_start, None)]
        } else {
            block
                .invisible
                .iter()
                .filter(|span| span.kind == SpanKind::HtmlText && span.token_start_line == index)
                .map(|span| {
                    (
                        self.column(index, span.start),
                        Some(self.column(index, span.end)),
                    )
                })
                .collect()
        };
        let text = if flow {
            self.flow_without_comments(index)
        } else {
            self.raw[index].clone()
        };
        for (start, end) in spans {
            // Inline positions come from the parser, so escapes, code and
            // link destinations cannot make an HTML heading. Tags of a raw
            // block still need their quoted attributes skipped.
            let within = slice_units(&text, start, end.unwrap_or(text.len()));
            let tags = re::find_all(
                r#"<(?:[^<>"']|"[^"]*"|'[^']*')*>|<h[1-6]\b[^>]*$"#,
                "i",
                &within,
            );
            for tag in tags {
                let found = &within[tag.range()];
                let Some(level) = re::find(r"^<h([1-6])\b", "i", found)
                    .and_then(|h| re::group(found, &h, 1).and_then(|digit| digit.parse().ok()))
                else {
                    continue;
                };
                let before = start + within[..tag.range().start].encode_utf16().count();
                return Some(Heading {
                    title: format!("<h{level}>"),
                    level,
                    style: Style::Html,
                    nested: contained
                        || !trim(&slice_units(&text, content_start, before)).is_empty(),
                });
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
    let mut read = Units {
        raw: lines.iter().map(|line| units(line)).collect(),
        counted: &counted,
        blocks: &blocks,
        flows: HashMap::new(),
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
            .or_else(|| read.html_heading(line));
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

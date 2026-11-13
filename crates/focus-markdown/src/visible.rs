//! The lines of a document a consumer may read as text.
//!
//! A consumer looks for a heading, a field or an answer line by line. This
//! projection gives it the document with the same number of lines, where a
//! line of a code block or an HTML block is empty, a comment is cut out of
//! its line, and a line that only continues an inline code span or tag is
//! empty too. What is left is what a reader would take for the document's
//! own words.
//!
//! Mirrors `projectVisibleMarkdownLines` and `restoreVisibleMarkdownMarkers`
//! in `aidlc-lib.ts` (L41407, L12486).

use focus_js::{Result, re};

use crate::{Blocks, LineKind, SpanKind, analyse, source_lines};

/// Stands where a comment was cut out of a line.
pub const COMMENT_MARKER: char = '\u{0}';
/// Starts a line that only continues something invisible.
pub const LINE_MARKER: char = '\u{1}';
/// Stands for a NUL the source itself held, so it is not taken for a marker.
const RAW_COMMENT_MARKER_ESCAPE: char = '\u{2}';

/// What to keep that the plain projection drops.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Visible {
    /// Keep the lines of indented code, and lines indented like code.
    pub preserve_indented_code: bool,
    /// Keep [`COMMENT_MARKER`] where each comment starts and ends, and
    /// [`LINE_MARKER`] at the start of a line of an HTML block opened by a
    /// tag (kinds 6 and 7) and of a line that continues an inline construct,
    /// so a consumer can tell "nothing here" from "something hidden here".
    pub preserve_comment_boundaries: bool,
}

/// Drop the markers a consumer did not ask to keep.
fn restore(line: &str) -> String {
    if line.starts_with(LINE_MARKER) {
        return String::new();
    }
    line.chars()
        .filter(|c| *c != COMMENT_MARKER && *c != LINE_MARKER)
        .map(|c| {
            if c == RAW_COMMENT_MARKER_ESCAPE {
                COMMENT_MARKER
            } else {
                c
            }
        })
        .collect()
}

/// The visible lines of `content`.
///
/// # Errors
///
/// The document cannot be read (see [`analyse`]).
pub fn visible_lines(content: &str, options: Visible) -> Result<Vec<String>> {
    Ok(visible_lines_of(content, &analyse(content)?, options))
}

/// The visible lines of `content`, whose structure is `blocks`: what
/// [`analyse`] answered for this same `content`. For a consumer that reads
/// the structure too, so the document is analysed once.
#[must_use]
pub fn visible_lines_of(content: &str, blocks: &Blocks, options: Visible) -> Vec<String> {
    let finish = |visible: String| {
        if options.preserve_comment_boundaries {
            visible
        } else {
            restore(&visible)
        }
    };
    source_lines(content)
        .iter()
        .enumerate()
        .map(|(index, source)| {
            let block = &blocks.lines[index];
            let line = source.replace(COMMENT_MARKER, &RAW_COMMENT_MARKER_ESCAPE.to_string());
            if block.kind == LineKind::CodeFenced
                || (block.kind == LineKind::CodeIndented && !options.preserve_indented_code)
            {
                return String::new();
            }
            let comment_block = block.kind == LineKind::HtmlFlow && block.html_kind == Some(2);
            if block.kind == LineKind::HtmlFlow && !comment_block {
                // Kinds 1 to 5 are verbatim to their end condition; a kind 6
                // or 7 block is markup a consumer may still need to see is there.
                return if block.html_kind.is_some_and(|kind| kind >= 6)
                    && options.preserve_comment_boundaries
                {
                    format!("{LINE_MARKER}{line}")
                } else {
                    String::new()
                };
            }
            // The long-standing lexical rule: a line indented like code is
            // not read, even when the indentation continues a paragraph.
            let start = block.content_start.min(line.len());
            if !options.preserve_indented_code
                && block.kind != LineKind::HtmlFlow
                && !block
                    .invisible
                    .iter()
                    .any(|span| span.token_start_line < index)
                && re::test(r"(?:^|> ?)(?: {4}|\t)", "", &line[..start])
            {
                return String::new();
            }
            let mut visible = String::new();
            if comment_block {
                // The parser decides how far the comment block runs; only its
                // delimiters are looked for here.
                let previous = index.checked_sub(1).map(|above| &blocks.lines[above]);
                let continued = previous.is_some_and(|above| {
                    above.kind == LineKind::HtmlFlow
                        && above.html_kind == Some(2)
                        && above.block == block.block
                });
                let opening = if continued {
                    None
                } else {
                    line[start..].find("<!--").map(|at| start + at)
                };
                let from = opening.map_or(start, |at| at + 4).min(line.len());
                let closing = line[from..].find("-->").map(|at| from + at);
                if let Some(opening) = opening {
                    visible.push_str(&line[..opening]);
                    visible.push(COMMENT_MARKER);
                }
                if let Some(closing) = closing {
                    visible.push(COMMENT_MARKER);
                    // Every further comment on the line: upstream's
                    // `/<!--[\s\S]*?-->/g`, found by hand so that a line of
                    // comments that never close costs one pass, not one for
                    // each of them.
                    let rest = &line[closing + 3..];
                    let mut last = 0;
                    while let Some(opens) = rest[last..].find("<!--").map(|at| last + at)
                        && let Some(closes) = rest[opens + 4..].find("-->").map(|at| opens + 4 + at)
                    {
                        visible.push_str(&rest[last..opens]);
                        visible.push(COMMENT_MARKER);
                        visible.push(COMMENT_MARKER);
                        last = closes + 3;
                    }
                    visible.push_str(&rest[last..]);
                }
                return finish(visible);
            }
            let mut cursor = 0;
            let between = |cursor: usize, upto: usize| -> &str {
                if upto > cursor && upto <= line.len() {
                    &line[cursor..upto]
                } else {
                    ""
                }
            };
            for span in &block.invisible {
                let continued = span.token_start_line < index;
                let continues = span.token_end_line > index;
                match span.kind {
                    SpanKind::CodeText => {
                        // Code on one line stays as written.
                        if !continued && !continues {
                            continue;
                        }
                        if continued && continues {
                            return String::new();
                        }
                        if !continued {
                            visible.push_str(between(cursor, span.start));
                        }
                        visible.push(LINE_MARKER);
                        cursor = span.end;
                    }
                    SpanKind::HtmlComment => {
                        if continued && !visible.starts_with(LINE_MARKER) {
                            visible.insert(0, LINE_MARKER);
                        }
                        visible.push_str(between(cursor, span.start));
                        if !continued {
                            visible.push(COMMENT_MARKER);
                        }
                        if !continues {
                            visible.push(COMMENT_MARKER);
                        }
                        cursor = span.end;
                    }
                    SpanKind::HtmlText => {
                        if continued && !visible.starts_with(LINE_MARKER) {
                            visible.insert(0, LINE_MARKER);
                        }
                    }
                }
            }
            if cursor <= line.len() {
                visible.push_str(&line[cursor..]);
            }
            finish(visible)
        })
        .collect()
}

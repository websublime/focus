//! The structure of a Markdown document, line by line.
//!
//! The engine reads Markdown that people and agents write: memory files,
//! state files, question files, reviews. What it may act on is the text a
//! reader would see as text. A heading inside a code fence is not a heading,
//! a field inside an HTML comment is not a field, and a line of an HTML
//! block is not prose. Deciding that takes the document's block structure,
//! as CommonMark and GFM define it, not a scan for `#` and backticks.
//!
//! [`analyse`] gives that structure: for every source line, the kind of
//! block it belongs to, the block quotes and list items it sits in, where
//! its own content starts, and the stretches of it that are inline code,
//! raw HTML or a comment. [`visible_lines`] projects a document onto the
//! lines a consumer reads, with everything else blanked.
//!
//! The structure comes from `pulldown-cmark`, which reports where each block
//! and inline sits in the source. Upstream has no such parser: its runtime
//! renders Markdown without source positions, so `markdownBlocks` in
//! `aidlc-lib.ts` (L40952) finds each line's block by rendering the document
//! with a probe word in the line and looking for where the word comes out.
//! That approximates the CommonMark structure upstream means to read, and it
//! repairs its renderer where the renderer departs from CommonMark. On the
//! documents people write the two agree line for line: upstream's own golden
//! documents, the Markdown it ships and documents written with care are in
//! the oracle corpus, and this crate is held to upstream's answer there.
//!
//! They part in three ways, each a declared deviation
//! (`tests/deviations.rs` has one example of each):
//!
//! - a line upstream cannot probe, or does not reach before its budget of
//!   256 renders runs out, it leaves `unknown` and reads as text. This crate
//!   has no such kind and places every line;
//! - in nested containers upstream's analysis places some lines where
//!   CommonMark does not, and its renderer reads a few constructs
//!   otherwise than CommonMark 0.31 does. This crate reads what the parser
//!   reads, and a third parser, the referee, reads the same;
//! - in corners of tables the parser itself reads otherwise than GFM does.
//!   That is a limit, not a choice; it is measured and bounded.
//!
//! `tools/oracle/markdown_rule.py` is the rule that tells them apart.

mod analysis;
pub mod oracle;
mod probes;
mod visible;

pub use analysis::analyse;
pub use visible::{COMMENT_MARKER, LINE_MARKER, Visible, visible_lines, visible_lines_of};

/// The kind of block a source line belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    Paragraph,
    Heading,
    ThematicBreak,
    CodeFenced,
    CodeIndented,
    HtmlFlow,
    Definition,
    Table,
}

impl LineKind {
    /// The name upstream gives the kind.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Blank => "blank",
            Self::Paragraph => "paragraph",
            Self::Heading => "heading",
            Self::ThematicBreak => "thematicBreak",
            Self::CodeFenced => "codeFenced",
            Self::CodeIndented => "codeIndented",
            Self::HtmlFlow => "htmlFlow",
            Self::Definition => "definition",
            Self::Table => "table",
        }
    }
}

/// A block quote or a list item a line sits in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Container {
    BlockQuote,
    ListItem {
        ordered: bool,
        /// The number an ordered list starts at.
        start: Option<u64>,
        /// Which item, counted from zero over the document.
        id: usize,
    },
}

/// What an inline stretch of a line is, when it is not text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    CodeText,
    HtmlText,
    HtmlComment,
}

impl SpanKind {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::CodeText => "codeText",
            Self::HtmlText => "htmlText",
            Self::HtmlComment => "htmlComment",
        }
    }
}

/// The part of one line that an inline code span, raw HTML tag or comment
/// covers. Columns are byte offsets into the line, with `start <= end`; the
/// lines are those of the whole construct, which may run over several.
///
/// One kind of span is not reported where it is: a code span in a table
/// cell that holds an escaped pipe is reported as upstream measures it,
/// short by one column for each `\|` (see `analysis.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: SpanKind,
    pub token_start_line: usize,
    pub token_end_line: usize,
}

/// One source line, placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub kind: LineKind,
    /// The block the line belongs to: lines of one leaf block share a
    /// number. `None` for a blank line outside any block, a thematic break
    /// and a link reference definition.
    pub block: Option<usize>,
    /// Outermost first.
    pub containers: Vec<Container>,
    /// For a line of an HTML block, which of CommonMark's seven start
    /// conditions opened the block.
    pub html_kind: Option<u8>,
    /// The byte column after the line's container markers and indentation.
    pub content_start: usize,
    /// Inline code, raw HTML and comments on the line, in order.
    pub invisible: Vec<Span>,
}

/// A link reference definition: its normalised label and the lines it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub label: String,
    pub start_line: usize,
    pub end_line: usize,
}

/// A document's structure. `lines` has one entry for each line of the
/// document as [`source_lines`] splits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocks {
    pub lines: Vec<Line>,
    pub definitions: Vec<Definition>,
    /// The normalised label of every link reference definition, sorted.
    pub labels: Vec<String>,
}

/// The lines of a document as every function here counts them: without a
/// leading byte order mark, and with `\r\n` and a lone `\r` each read as one
/// line break.
#[must_use]
pub fn source_lines(content: &str) -> Vec<String> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    content
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::to_owned)
        .collect()
}

/// A label as CommonMark matches labels: white space collapsed, then case
/// folded the way upstream folds it (lower, then upper).
#[must_use]
pub fn normalize_label(label: &str) -> String {
    let mut collapsed = String::new();
    let mut gap = false;
    for c in label.chars() {
        if matches!(c, '\t' | '\n' | '\r' | ' ') {
            gap = true;
        } else {
            if gap {
                collapsed.push(' ');
            }
            gap = false;
            collapsed.push(c);
        }
    }
    if gap {
        collapsed.push(' ');
    }
    // One leading and one trailing space go, as upstream's `/^ | $/g` takes them.
    let collapsed = collapsed.strip_prefix(' ').unwrap_or(&collapsed);
    let collapsed = collapsed.strip_suffix(' ').unwrap_or(collapsed);
    collapsed.to_lowercase().to_uppercase()
}

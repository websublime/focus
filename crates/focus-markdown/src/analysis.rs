//! From a parse to a line-by-line account of the document.

use std::cell::Cell;
use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Once;

use focus_js::order::cmp_utf16;
use focus_js::{EngineError, Result, re};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

use crate::probes::{CONTAINER_PREFIX, prefix_len, probe_columns};
use crate::{
    Blocks, Container, Definition, Line, LineKind, Span, SpanKind, normalize_label, source_lines,
};

/// The tags that make a line an HTML block of kind 6, in lower case.
const HTML_BLOCK_TAG: &str = r"^</?(?:address|article|aside|base|basefont|blockquote|body|caption|center|col|colgroup|dd|details|dialog|dir|div|dl|dt|fieldset|figcaption|figure|footer|form|frame|frameset|h[1-6]|head|header|hr|html|iframe|legend|li|link|main|menu|menuitem|nav|noframes|ol|optgroup|option|p|param|search|section|summary|table|tbody|td|tfoot|th|thead|title|tr|track|ul)(?:[\t >]|/>|$)";
/// The tags whose HTML block (kind 1) runs to a closing tag, in lower case.
const RAW_TEXT_OPENING: &str = r"^<(script|pre|style|textarea)(?:[\t >]|$)";
const RAW_TEXT_CLOSING: &str = "</(?:script|pre|style|textarea)>";

/// Which of CommonMark's seven HTML block start conditions a first line
/// meets. The parser decided it is an HTML block; this only names the kind.
///
/// Tag names are matched in any case of the ASCII letters and of no others,
/// which is what CommonMark, the parser and upstream's patterns all do. A
/// case-insensitive pattern here would also take `ſ` for `s`, so the line is
/// put in lower case and matched as it is.
fn html_kind(first_line: &str) -> u8 {
    let lowered = first_line.to_ascii_lowercase();
    if re::test(RAW_TEXT_OPENING, "", &lowered) {
        1
    } else if first_line.starts_with("<!--") {
        2
    } else if first_line.starts_with("<?") {
        3
    } else if re::test("^<![A-Za-z]", "", first_line) {
        4
    } else if first_line.starts_with("<![CDATA[") {
        5
    } else if re::test(HTML_BLOCK_TAG, "", &lowered) {
        6
    } else {
        7
    }
}

/// A line of container markers and nothing else, wherever they are indented:
/// quote markers, and list markers with nothing or white space after them.
pub(crate) const MARKERS_ONLY: &str = r"^[ \t]*(?:(?:>|(?:[-*+]|\d{1,9}[.)])(?=[ \t]|$))[ \t]*)*$";

/// Any run of container markers and indentation at the start of a line.
const MARKERS: &str = r"^[ \t]*(?:(?:>|(?:[-*+]|\d{1,9}[.)])(?=[ \t]|$))[ \t]*)*";

/// How a container shows on one line: a quote marker, a list marker (on the
/// item's first line), or the indentation that continues an item.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prefix {
    Quote,
    Marker,
    Indent,
}

/// The column after a line's container prefixes and indentation. A list
/// item's marker appears only on its first line; later lines carry
/// indentation.
fn content_start(line: &str, prefixes: &[Prefix]) -> usize {
    let mut position = 0;
    for prefix in prefixes {
        let pattern = match prefix {
            Prefix::Quote => r"^[ \t]{0,3}>[ \t]?",
            Prefix::Marker => r"^[ \t]{0,3}(?:[-*+]|\d{1,9}[.)])(?:[ \t]+|$)",
            Prefix::Indent => r"^[ \t]*",
        };
        position += prefix_len(pattern, &line[position..]);
    }
    position + prefix_len(r"^[ \t]*", &line[position..])
}

/// The line as upstream hands it to its renderer: NUL becomes U+FFFD, as
/// CommonMark requires before anything else, and the four control characters
/// upstream uses to delimit its rendered tree become a letter. Where upstream
/// would put its probes is worked out on this text.
fn probed_line(raw: &str) -> String {
    raw.chars()
        .map(|c| match c {
            '\0' => '\u{fffd}',
            '\u{1}'..='\u{4}' => 'x',
            other => other,
        })
        .collect()
}

/// The line as the parser is to see it. Two things are changed for the
/// parser's sake, neither of which changes what CommonMark reads:
///
/// - vertical tab and form feed become another control character. To
///   CommonMark they are characters like any other where blocks are
///   concerned: a line of them is not blank, they do not separate a tag's
///   name from its attributes, and like every control character they cannot
///   be part of a link destination. The parser, as of 0.13, takes the two
///   for white space;
/// - a line of spaces and tabs becomes an empty line. Both are blank lines.
///   The parser panics on one that follows a list item holding only a link
///   reference definition and is indented past it.
fn parser_line(probed: &str) -> String {
    if is_blank(probed) {
        return String::new();
    }
    probed
        .chars()
        .map(|c| match c {
            '\u{b}' | '\u{c}' => '\u{5}',
            other => other,
        })
        .collect()
}

/// A blank line, as CommonMark has it: nothing, or spaces and tabs.
fn is_blank(line: &str) -> bool {
    line.bytes().all(|byte| byte == b' ' || byte == b'\t')
}

/// Where the columns of a line that had its NULs widened map back to: a NUL
/// is one byte in the raw line and the three of U+FFFD in the other.
struct Columns {
    /// The column in the widened line of each widened character.
    widened: Vec<usize>,
    /// The raw column just after each of them.
    after: Vec<usize>,
    raw_len: usize,
}

impl Columns {
    fn of(raw: &str) -> Self {
        let (mut widened, mut after) = (Vec::new(), Vec::new());
        for (at, _) in raw.match_indices('\0') {
            widened.push(at + 2 * widened.len());
            after.push(at + 1);
        }
        Self {
            widened,
            after,
            raw_len: raw.len(),
        }
    }

    /// A byte column of the widened line as a byte column of the raw line.
    fn raw(&self, column: usize) -> usize {
        let before = self.widened.partition_point(|at| *at < column);
        let floor = before.checked_sub(1).map_or(0, |last| self.after[last]);
        column
            .saturating_sub(2 * before)
            .max(floor)
            .min(self.raw_len)
    }
}

/// How many UTF-16 code units `text` is.
fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The byte column of `line` that is `units` UTF-16 code units in, or the
/// start of the character that offset falls inside.
fn byte_column(line: &str, units: usize) -> usize {
    let mut seen = 0;
    for (at, c) in line.char_indices() {
        if seen + c.len_utf16() > units {
            return at;
        }
        seen += c.len_utf16();
    }
    line.len()
}

/// A leaf block the parser reported: what it is, where, and inside what.
struct Leaf {
    kind: LineKind,
    lines: Range<usize>,
    path: Vec<Container>,
}

struct Walk {
    starts: Vec<usize>,
    /// Open containers, outermost first.
    path: Vec<Container>,
    /// For each open list: whether it is ordered and where it starts.
    lists: Vec<Option<u64>>,
    /// The first line of every list item, by id.
    item_starts: Vec<usize>,
    leaves: Vec<Leaf>,
    /// Open blocks that hold inline content, innermost last: the index of a
    /// leaf, or `None` for a list item whose text has no paragraph around it.
    holders: Vec<Option<usize>>,
    /// Inline code, raw HTML and comments, each with whether it sits in a
    /// table cell.
    spans: Vec<(SpanKind, Range<usize>, bool)>,
    /// How many tables are open.
    tables: usize,
    /// Whether a code block is open: its text is not inline content.
    in_code: bool,
}

impl Walk {
    fn line_of(&self, offset: usize) -> usize {
        self.starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }

    /// The lines a block's source range covers. The range's last byte is on
    /// the block's last line, whether it is text or the line's own break.
    fn lines_of(&self, range: &Range<usize>) -> Range<usize> {
        let last = range.end.max(range.start + 1) - 1;
        self.line_of(range.start)..self.line_of(last) + 1
    }

    fn leaf(&mut self, kind: LineKind, lines: Range<usize>) -> usize {
        self.leaves.push(Leaf {
            kind,
            lines,
            path: self.path.clone(),
        });
        self.leaves.len() - 1
    }

    /// Inline content met directly inside a list item, with no paragraph
    /// around it: the item is tight, and its text is a paragraph all the
    /// same. Inside a paragraph, a heading or a table the lines are already
    /// the block's.
    fn inline(&mut self, range: &Range<usize>) {
        if self.in_code {
            return;
        }
        let lines = self.lines_of(range);
        if let Some(None) = self.holders.last() {
            let leaf = self.leaf(LineKind::Paragraph, lines.clone());
            if let Some(holder) = self.holders.last_mut() {
                *holder = Some(leaf);
            }
        }
        if let Some(Some(leaf)) = self.holders.last().copied() {
            let held = &mut self.leaves[leaf].lines;
            held.start = held.start.min(lines.start);
            held.end = held.end.max(lines.end);
        }
    }
}

fn parser_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options
}

thread_local! {
    /// Whether this thread is inside the parser, where a panic is caught and
    /// answered, not reported.
    static IN_PARSER: Cell<bool> = const { Cell::new(false) };
}

/// Runs `work`, which is the parser, and answers `None` if it panics.
///
/// The parser is somebody else's code reading text anybody may have written.
/// It has panicked on a sixteen-byte document (see [`parser_line`]); the one
/// known way in is closed, and this is for the ones not known. The engine
/// must be able to say "this file cannot be read" and go on, so the panic is
/// caught here and its message is not printed: the hook that prints is
/// replaced, once, by one that stays quiet for a panic raised on a thread
/// that is inside this function and passes every other on.
pub(crate) fn contained<T>(work: impl FnOnce() -> T) -> Option<T> {
    static QUIET_IN_PARSER: Once = Once::new();
    QUIET_IN_PARSER.call_once(|| {
        let report = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            if !IN_PARSER.with(Cell::get) {
                report(panic);
            }
        }));
    });
    IN_PARSER.with(|inside| inside.set(true));
    let outcome = catch_unwind(AssertUnwindSafe(work));
    IN_PARSER.with(|inside| inside.set(false));
    outcome.ok()
}

/// What one parse of the document reports.
struct Reading {
    walk: Walk,
    /// The lines that are thematic breaks.
    rules: Vec<usize>,
    /// The label and source range of every link reference definition the
    /// parser kept.
    defined: Vec<(String, Range<usize>)>,
}

/// Parses `source`, the document's lines as the parser is to see them.
/// `None` when the parser fails on them.
fn read(source: &[String]) -> Option<Reading> {
    let text = source.join("\n");
    let mut starts = Vec::with_capacity(source.len());
    let mut offset = 0;
    for line in source {
        starts.push(offset);
        offset += line.len() + 1;
    }
    contained(|| walk(&text, starts))
}

fn walk(text: &str, starts: Vec<usize>) -> Reading {
    let parser = Parser::new_ext(text, parser_options());
    let defined: Vec<(String, Range<usize>)> = parser
        .reference_definitions()
        .iter()
        .map(|(label, definition)| (label.to_owned(), definition.span.clone()))
        .collect();

    let mut walk = Walk {
        starts,
        path: Vec::new(),
        lists: Vec::new(),
        item_starts: Vec::new(),
        leaves: Vec::new(),
        holders: Vec::new(),
        spans: Vec::new(),
        tables: 0,
        in_code: false,
    };
    let mut rules: Vec<usize> = Vec::new();
    for (event, range) in parser.into_offset_iter() {
        // A block that starts inside a tight list item ends the run of text
        // before it: text after it is another paragraph of the item.
        let starts_a_block = matches!(
            &event,
            Event::Rule
                | Event::Start(
                    Tag::Paragraph
                        | Tag::Heading { .. }
                        | Tag::BlockQuote(_)
                        | Tag::CodeBlock(_)
                        | Tag::HtmlBlock
                        | Tag::List(_)
                        | Tag::Table(_)
                )
        );
        if starts_a_block && let Some(holder @ Some(_)) = walk.holders.last_mut() {
            *holder = None;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::BlockQuote(_) => walk.path.push(Container::BlockQuote),
                Tag::List(start) => walk.lists.push(start),
                Tag::Item => {
                    let start = walk.lists.last().copied().flatten();
                    let id = walk.item_starts.len();
                    walk.item_starts.push(walk.line_of(range.start));
                    walk.path.push(Container::ListItem {
                        ordered: start.is_some(),
                        start,
                        id,
                    });
                    walk.holders.push(None);
                }
                Tag::Paragraph => {
                    let lines = walk.lines_of(&range);
                    let leaf = walk.leaf(LineKind::Paragraph, lines);
                    walk.holders.push(Some(leaf));
                }
                Tag::Heading { .. } => {
                    let lines = walk.lines_of(&range);
                    let leaf = walk.leaf(LineKind::Heading, lines);
                    walk.holders.push(Some(leaf));
                }
                Tag::Table(_) => {
                    let lines = walk.lines_of(&range);
                    let leaf = walk.leaf(LineKind::Table, lines);
                    walk.holders.push(Some(leaf));
                    walk.tables += 1;
                }
                Tag::CodeBlock(kind) => {
                    let lines = walk.lines_of(&range);
                    let kind = match kind {
                        CodeBlockKind::Fenced(_) => LineKind::CodeFenced,
                        CodeBlockKind::Indented => LineKind::CodeIndented,
                    };
                    walk.leaf(kind, lines);
                    walk.in_code = true;
                }
                Tag::HtmlBlock => {
                    let lines = walk.lines_of(&range);
                    walk.leaf(LineKind::HtmlFlow, lines);
                }
                // A link, an image and emphasis are inline content from
                // where they open to where they close, which may be lines
                // with no text of their own: a title on the next line.
                Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::Emphasis
                | Tag::Strong
                | Tag::Strikethrough => walk.inline(&range),
                _ => {}
            },
            Event::End(end) => match end {
                TagEnd::BlockQuote(_) => {
                    walk.path.pop();
                }
                TagEnd::List(_) => {
                    walk.lists.pop();
                }
                TagEnd::Item => {
                    walk.path.pop();
                    walk.holders.pop();
                }
                TagEnd::Paragraph | TagEnd::Heading(_) => {
                    walk.holders.pop();
                }
                TagEnd::Table => {
                    walk.holders.pop();
                    walk.tables -= 1;
                }
                TagEnd::CodeBlock => walk.in_code = false,
                _ => {}
            },
            Event::Rule => rules.push(walk.line_of(range.start)),
            Event::Code(_) => {
                walk.inline(&range);
                walk.spans
                    .push((SpanKind::CodeText, range, walk.tables > 0));
            }
            Event::InlineHtml(html) => {
                walk.inline(&range);
                let kind = if html.starts_with("<!--") {
                    SpanKind::HtmlComment
                } else {
                    SpanKind::HtmlText
                };
                walk.spans.push((kind, range, walk.tables > 0));
            }
            Event::Text(_) | Event::SoftBreak | Event::HardBreak => walk.inline(&range),
            _ => {}
        }
    }

    Reading {
        walk,
        rules,
        defined,
    }
}

/// The opening line of an HTML block, from where its own text starts.
fn html_opening<'a>(leaf: &Leaf, source: &'a [String], item_starts: &[usize]) -> &'a str {
    let first = leaf.lines.start;
    let prefixes: Vec<Prefix> = leaf
        .path
        .iter()
        .map(|container| match container {
            Container::BlockQuote => Prefix::Quote,
            Container::ListItem { id, .. } if first <= item_starts[*id] => Prefix::Marker,
            Container::ListItem { .. } => Prefix::Indent,
        })
        .collect();
    let line = source[first].as_str();
    &line[content_start(line, &prefixes).min(line.len())..]
}

/// The first HTML block of kind 1 that runs past the line that ends it: the
/// line, the closing tag found there, and the one the parser waits for.
///
/// CommonMark ends such a block at the first line holding `</script>`,
/// `</pre>`, `</style>` or `</textarea>`, in any letter case, whichever tag
/// opened it. The parser, as of 0.13, ends it only at the tag that matches
/// the opening one, in lower case, so `<pre>` followed by `</style>`, or
/// `<SCRIPT>` by `</SCRIPT>`, would hide the rest of the document.
fn overrun(reading: &Reading, source: &[String]) -> Option<(usize, Range<usize>, String)> {
    let closing = re::compile(RAW_TEXT_CLOSING, "");
    reading
        .walk
        .leaves
        .iter()
        .filter(|leaf| leaf.kind == LineKind::HtmlFlow)
        .find_map(|leaf| {
            // In lower case, for the reason given at `html_kind`. The case
            // of ASCII letters does not move a byte.
            let opening =
                html_opening(leaf, source, &reading.walk.item_starts).to_ascii_lowercase();
            let name = re::find(RAW_TEXT_OPENING, "", &opening)
                .and_then(|found| re::group(&opening, &found, 1))?
                .to_owned();
            (leaf.lines.start..leaf.lines.end.saturating_sub(1)).find_map(|index| {
                let found = closing.find(&source[index].to_ascii_lowercase())?;
                Some((index, found.range(), format!("</{name}>")))
            })
        })
}

/// Reads the document until no HTML block overruns its end (see
/// [`overrun`]): the closing tag on the line that should have ended a block
/// is replaced by the one the parser waits for, and the document read again.
/// The line is inside an HTML block either way, so nothing else reads its
/// text. Each pass settles the document up to one more such line, so a
/// document is read once more for each block that ends this way.
fn settled(source: &[String]) -> Option<Reading> {
    let mut parsed = source.to_vec();
    loop {
        let reading = read(&parsed)?;
        match overrun(&reading, &parsed) {
            Some((line, closer, wanted)) => parsed[line].replace_range(closer, &wanted),
            None => return Some(reading),
        }
    }
}

/// The probes of one line, by raw column, each with its place in the order
/// upstream makes them.
struct Probes {
    by_column: Vec<(usize, usize)>,
}

impl Probes {
    fn of(columns: &[usize], line: &Columns) -> Self {
        let mut by_column: Vec<(usize, usize)> = columns
            .iter()
            .enumerate()
            .map(|(order, column)| (line.raw(*column), order))
            .collect();
        by_column.sort_unstable();
        Self { by_column }
    }

    /// The first probe upstream makes that lands in `low..=high`.
    fn first_in(&self, low: usize, high: usize) -> Option<usize> {
        let from = self.by_column.partition_point(|(column, _)| *column < low);
        self.by_column[from..]
            .iter()
            .take_while(|(column, _)| *column <= high)
            .min_by_key(|(_, order)| *order)
            .map(|(column, _)| *column)
    }
}

/// The structure of `content`, one entry for each of its lines.
///
/// # Errors
///
/// The document cannot be read: the parser failed on it, or reported nothing
/// for a line that is neither blank, nor container markers, nor a link
/// reference definition by its looks. Neither is known to happen; a caller
/// refuses the file, it does not guess.
pub fn analyse(content: &str) -> Result<Blocks> {
    let raw = source_lines(content);
    let probed: Vec<String> = raw.iter().map(|line| probed_line(line)).collect();
    let source: Vec<String> = probed.iter().map(|line| parser_line(line)).collect();
    let Some(Reading {
        walk,
        rules,
        defined,
    }) = settled(&source)
    else {
        return Err(EngineError::new(
            "this Markdown cannot be read: the parser failed on it",
        ));
    };
    // The parser's columns, and those of upstream's probes, back to the raw
    // line's.
    let columns: Vec<Columns> = raw.iter().map(|line| Columns::of(line)).collect();

    // Every line starts as blank or unplaced, with its content where the
    // indentation ends.
    let mut lines: Vec<Line> = raw
        .iter()
        .map(|line| Line {
            kind: if is_blank(line) {
                LineKind::Blank
            } else {
                LineKind::Paragraph
            },
            block: None,
            containers: Vec::new(),
            html_kind: None,
            content_start: content_start(line, &[]),
            invisible: Vec::new(),
        })
        .collect();
    let mut placed = vec![false; lines.len()];

    let Walk {
        leaves,
        item_starts,
        spans,
        starts,
        ..
    } = walk;
    for (block, leaf) in leaves.iter().enumerate() {
        for index in leaf.lines.clone() {
            if index >= lines.len() || placed[index] {
                continue;
            }
            // A blank line inside a block that holds no blank lines is the
            // parser's range running over; code and HTML keep theirs.
            let holds_blank = matches!(
                leaf.kind,
                LineKind::CodeFenced | LineKind::CodeIndented | LineKind::HtmlFlow
            );
            if !holds_blank && lines[index].kind == LineKind::Blank {
                continue;
            }
            placed[index] = true;
            let prefixes: Vec<Prefix> = leaf
                .path
                .iter()
                .map(|container| match container {
                    Container::BlockQuote => Prefix::Quote,
                    Container::ListItem { id, .. } if index <= item_starts[*id] => Prefix::Marker,
                    Container::ListItem { .. } => Prefix::Indent,
                })
                .collect();
            let start = content_start(&raw[index], &prefixes);
            let html = (leaf.kind == LineKind::HtmlFlow)
                .then(|| html_kind(html_opening(leaf, &probed, &item_starts)));
            lines[index] = Line {
                kind: leaf.kind,
                block: Some(block),
                containers: leaf.path.clone(),
                html_kind: html,
                content_start: start,
                invisible: Vec::new(),
            };
        }
    }
    for index in rules {
        if index < lines.len() && !placed[index] {
            placed[index] = true;
            lines[index].kind = LineKind::ThematicBreak;
        }
    }

    // Link reference definitions the parser kept, in the order they stand.
    let line_of = |offset: usize| {
        starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    };
    // A label is defined when a reference to it resolves, which is what the
    // parser's own table says; where a definition's label sits on its line
    // is read lexically below, as upstream reads it.
    let mut labels: Vec<String> = Vec::new();
    for (label, _) in &defined {
        let label = normalize_label(label);
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.sort_by(|a, b| cmp_utf16(a, b));
    let mut defined: Vec<Range<usize>> = defined
        .into_iter()
        .map(|(_, span)| {
            line_of(span.start)..(line_of(span.end.max(span.start + 1) - 1) + 1).min(raw.len())
        })
        .collect();
    for span in &defined {
        for index in span.clone() {
            if !placed[index] {
                placed[index] = true;
                lines[index].kind = LineKind::Definition;
            }
        }
    }

    // What is left, the parser reported nothing for. A line of nothing but
    // container markers is blank. A run of lines that opens with a label is
    // a definition of a label already defined: the parser reads it as one,
    // keeps the first and says nothing of the rest. A run has as many
    // definitions as it has lines that open a label. Anything else is
    // something this reading does not understand.
    let label_of = |text: &str| {
        re::find(r"^\[((?:\\.|[^\\\[\]])+)\]:", "", text)
            .and_then(|found| re::group(text, &found, 1))
            .map(normalize_label)
            .filter(|label| !label.is_empty())
    };
    let after_markers = |index: usize| {
        let line = raw[index].as_str();
        &line[prefix_len(CONTAINER_PREFIX, line)..]
    };
    let mut index = 0;
    while index < lines.len() {
        if placed[index] || lines[index].kind == LineKind::Blank {
            index += 1;
        } else if re::test(MARKERS_ONLY, "", &raw[index]) {
            lines[index].kind = LineKind::Blank;
            index += 1;
        } else {
            let start = index;
            index += 1;
            while index < lines.len()
                && !placed[index]
                && lines[index].kind != LineKind::Blank
                && !re::test(MARKERS_ONLY, "", &raw[index])
                && label_of(after_markers(index)).is_none()
            {
                index += 1;
            }
            for line in &mut lines[start..index] {
                line.kind = LineKind::Definition;
            }
            defined.push(start..index);
        }
    }

    defined.sort_by_key(|span| span.start);
    let mut definitions = Vec::new();
    for span in defined {
        let joined = raw[span.clone()].join("\n");
        // Where the label sits is read as upstream reads it, which loses a
        // label nested deeper than its pattern for container markers sees.
        let text = &joined[prefix_len(CONTAINER_PREFIX, &raw[span.start]).min(joined.len())..];
        // Whether there is one at all is asked of the line with every
        // marker and all indentation off it.
        if !placed[span.start] && label_of(&joined[prefix_len(MARKERS, &joined)..]).is_none() {
            return Err(EngineError::new(format!(
                "this Markdown cannot be read: line {} is neither text, nor part of a block, nor a link reference definition",
                span.start + 1
            )));
        }
        definitions.push(Definition {
            label: label_of(text).unwrap_or_default(),
            start_line: span.start,
            end_line: span.end - 1,
        });
    }

    let probes: Vec<Probes> = probe_columns(&probed)
        .iter()
        .zip(&columns)
        .map(|(probes, line)| Probes::of(probes, line))
        .collect();

    // Inline code, raw HTML and comments. Upstream records one only where a
    // probe lands inside it (see `probes`).
    for (kind, range, in_table) in spans {
        let (first, last) = (
            line_of(range.start),
            line_of(range.end.max(range.start + 1) - 1),
        );
        if last >= lines.len() {
            continue;
        }
        let start_column = columns[first].raw(range.start - starts[first]);
        let end_column = columns[last].raw(range.end - starts[last]);
        // The backtick run that opens and closes a code span.
        let run = if kind == SpanKind::CodeText {
            raw[first][start_column..]
                .bytes()
                .take_while(|byte| *byte == b'`')
                .count()
        } else {
            0
        };
        let (content_low, content_high) = (start_column + run, end_column.saturating_sub(run));
        let probe = (first..=last).find_map(|index| {
            let low = match (index == first, run > 0) {
                (false, _) => 0,
                (true, true) => content_low,
                (true, false) => start_column + 1,
            };
            let high = match (index == last, run > 0) {
                (false, _) => usize::MAX,
                (true, true) => content_high,
                (true, false) => end_column.checked_sub(1)?,
            };
            (low <= high)
                .then(|| probes[index].first_in(low, high))
                .flatten()
                .map(|column| (index, column))
        });
        let Some((probe_line, probe_column)) = probe else {
            continue;
        };
        let (mut start_column, mut end_column) = (start_column, end_column);
        if in_table
            && first == last
            && probe_line == first
            && content_low <= probe_column
            && probe_column <= content_high
        {
            // Upstream measures a span from the probe, by the length of the
            // text rendered on each side of it, in UTF-16 code units. In a
            // table cell `\|` renders as `|`, one unit for two, so a code
            // span with an escaped pipe in it comes out that much short on
            // that side. The sensors read these columns, so they are
            // upstream's here, not the span's own. A code span also renders
            // without one space of padding at each end, which the count
            // allows for.
            let line = raw[first].as_str();
            let rendered = |text: &str| units(text) - text.matches("\\|").count();
            let (before, after) = (
                &line[content_low..probe_column],
                &line[probe_column..content_high],
            );
            let padded = run > 0 && before.starts_with(' ') && after.ends_with(' ');
            let at = units(&line[..probe_column]);
            let mut start = byte_column(line, at - (rendered(before) - usize::from(padded)));
            let mut end = byte_column(line, at + (rendered(after) - usize::from(padded)));
            if run > 0 {
                start -= re::find("(`+) ?$", "", &line[..start])
                    .map_or(0, |found| found.end() - found.start());
                end += prefix_len("^ ?(`+)", &line[end..]);
            }
            start_column = start;
            end_column = end;
        }
        let extents: Vec<(usize, usize)> = (first..=last)
            .map(|index| {
                let start = if index == first {
                    start_column
                } else {
                    lines[index].content_start
                };
                let end = if index == last {
                    end_column
                } else {
                    raw[index].len()
                };
                (start, end)
            })
            .collect();
        // The parser ends a tag at the next `>`, and on a later line that
        // may be a quote marker: such a tag ends before its line's text
        // starts. It is no tag, and CommonMark reads none there.
        if extents.iter().any(|(start, end)| start > end) {
            continue;
        }
        for (index, (start, end)) in (first..=last).zip(extents) {
            lines[index].invisible.push(Span {
                start,
                end,
                kind,
                token_start_line: first,
                token_end_line: last,
            });
        }
    }
    for line in &mut lines {
        line.invisible.sort_by_key(|span| span.start);
    }

    Ok(Blocks {
        lines,
        definitions,
        labels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_in_the_parser_is_an_answer_not_a_crash() {
        assert_eq!(contained(|| 7), Some(7));
        let failed: Option<u8> = contained(|| panic!("inside the parser"));
        assert_eq!(failed, None);
        // And the next call is as if nothing had happened.
        assert_eq!(contained(|| 8), Some(8));
    }

    #[test]
    fn columns_come_back_from_a_line_with_widened_characters() {
        // `a`, NUL, `é` (two bytes), NUL, `b`: raw columns 0, 1, 2, 4, 5.
        let raw = "a\0\u{e9}\0b";
        let columns = Columns::of(raw);
        // Widened: a at 0, U+FFFD at 1..4, é at 4..6, U+FFFD at 6..9, b at 9.
        let back: Vec<usize> = [0, 1, 4, 6, 9, 10]
            .iter()
            .map(|at| columns.raw(*at))
            .collect();
        assert_eq!(back, [0, 1, 2, 4, 5, 6]);
        // A column inside a widened character is the column after it.
        assert_eq!(columns.raw(2), 2);
        assert_eq!(Columns::of("plain").raw(3), 3);
    }

    #[test]
    fn a_utf16_offset_is_a_byte_column() {
        let line = "a\u{e9}\u{1f600}b";
        assert_eq!(units(line), 5);
        let columns: Vec<usize> = (0..=6).map(|at| byte_column(line, at)).collect();
        // Offset 3 falls inside the surrogate pair and is its start.
        assert_eq!(columns, [0, 1, 3, 3, 7, 8, 8]);
    }
}

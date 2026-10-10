//! Where Focus reads a document otherwise than upstream, one example of each
//! way, with upstream's answer beside it.
//!
//! Upstream's answers were taken by running its analysis on these documents
//! (`tools/oracle/driver.ts`, kind `markdown`); the third answer mentioned is
//! the referee's (`tools/oracle/referee`): micromark's, or commonmark.js's
//! on a line that is no table. The classes are the ones
//! `tools/oracle/markdown_rule.py` judges, and the plan lists them
//! (docs/plano-de-migracao.md, section 13, M2 slice 5).

use focus_markdown::{Blocks, LineKind, SpanKind, Visible, analyse, visible_lines};

fn kinds(document: &str) -> Vec<LineKind> {
    read(document).lines.iter().map(|line| line.kind).collect()
}

fn read(document: &str) -> Blocks {
    analyse(document).unwrap()
}

fn visible(document: &str) -> Vec<String> {
    visible_lines(document, Visible::default()).unwrap()
}

use LineKind::{Blank, Definition, Heading, HtmlFlow, Paragraph, Table};

/// Deliberate deviation (markdown/commonmark): upstream finds a line's block
/// by where a probe word renders, and a line with nowhere to put a probe (a
/// list marker alone, a quote marker alone) stays `unknown`. Focus has no
/// such kind: every line is placed.
#[test]
fn a_line_upstream_cannot_probe_is_placed() {
    // Upstream: paragraph, blank, unknown, blank, paragraph, unknown, paragraph.
    assert_eq!(
        kinds("1. one\n\n11.\n\n> a\n>\n> b"),
        [Paragraph, Blank, Blank, Blank, Paragraph, Blank, Paragraph]
    );
}

/// Deliberate deviation (markdown/commonmark): upstream stops probing a
/// document after 256 renders and leaves the rest unplaced, and an unplaced
/// line is read as text whatever it is inside. Sixty small tables are enough
/// to use the budget up; a comment after them is then read. Focus reads the
/// end of a document as it reads its start.
#[test]
fn a_long_document_is_read_to_its_end() {
    let tables: String = (0..60)
        .map(|n| format!("| a{n} | b |\n| --- | --- |\n| `x{n}` | <!-- c --> |\n\n"))
        .collect();
    let document = format!("{tables}## Guard Policy\n\n<!--\nMode: strict\n-->\n");
    let lines = visible(&document);
    // Upstream: the line reads `Mode: strict`, and so does its Guard Policy.
    assert_eq!(lines[lines.len() - 3], "");
    assert_eq!(lines[lines.len() - 6], "## Guard Policy");
    let structure = read(&document);
    assert!(
        structure.lines[..240]
            .iter()
            .enumerate()
            .all(|(index, line)| (line.kind == Table) == (index % 4 != 3))
    );
    // Upstream: the last table's rows are unplaced, with nothing recorded in them.
    assert_eq!(structure.lines[238].invisible.len(), 2);
}

/// Deliberate deviation (markdown/commonmark): a paragraph followed by a
/// line of hyphens is a setext heading, whatever its text looks like.
/// Upstream's probe lands in what reads like a definition's label, renders
/// nowhere, and the two lines come out as a definition and a rule; it also
/// lists `ANSWER` as a defined label. The referee reads a heading.
#[test]
fn a_heading_whose_text_reads_like_a_label_is_a_heading() {
    let structure = read("> [Answer]:\n> ---");
    assert_eq!(kinds("> [Answer]:\n> ---"), [Heading, Heading]);
    assert!(structure.definitions.is_empty() && structure.labels.is_empty());
}

/// Deliberate deviation (markdown/commonmark): `<!` followed by a letter
/// opens an HTML block (CommonMark 0.31, start condition 4). Upstream's
/// renderer follows an older text that wanted the letter in upper case, so
/// it reads a paragraph holding a tag, and the line stays visible. The
/// referee reads an HTML block.
#[test]
fn a_declaration_in_lower_case_is_an_html_block() {
    let document = "<!x y>\nstill text\n";
    assert_eq!(kinds(document), [HtmlFlow, Paragraph, Blank]);
    assert_eq!(read(document).lines[0].html_kind, Some(4));
    // Upstream: `<!x y>`.
    assert_eq!(visible(document), ["", "still text", ""]);
}

/// Deliberate deviation (markdown/commonmark): a link destination in angle
/// brackets cannot hold `<`, so this is no definition and `[Answer]` no
/// link. Upstream's renderer accepts it, reads a definition and lists
/// `ANSWER` as defined. Focus and the referee read a paragraph, with the
/// comment in it cut from what is visible.
#[test]
fn a_destination_with_an_angle_bracket_in_it_defines_nothing() {
    let document = "[Answer]: <!--<!---->\n\n[Answer]";
    let structure = read(document);
    assert_eq!(kinds(document), [Paragraph, Blank, Paragraph]);
    assert!(structure.definitions.is_empty() && structure.labels.is_empty());
    assert_eq!(structure.lines[0].invisible[0].kind, SpanKind::HtmlComment);
    // Upstream: the whole line.
    assert_eq!(visible(document)[0], "[Answer]: ");
}

/// Deliberate deviation (markdown/commonmark): a code span that closes at
/// the very start of a line still covers that line. Upstream measures a span
/// from its rendered text, where the line break before the closing backticks
/// is gone, and ends the span a line early: the last line stays visible.
#[test]
fn a_code_span_that_closes_at_the_start_of_a_line_covers_it() {
    let document = "a `` b\nMode: strict\n`` c";
    let structure = read(document);
    let ends: Vec<usize> = structure
        .lines
        .iter()
        .map(|line| line.invisible[0].token_end_line)
        .collect();
    // Upstream: the span ends on the second line and the third has none.
    assert_eq!(ends, [2, 2, 2]);
    // Upstream: `a `, ``, `` `` c``.
    assert_eq!(visible(document), ["a ", "", ""]);
}

/// Deliberate deviation (markdown/commonmark): an escaped backtick is text,
/// so the span opens at the next one. Upstream probes after every backtick
/// run, escaped or not; the probe after the escaped one renders as text, and
/// the span that follows is not recorded: all three lines stay visible.
#[test]
fn a_code_span_opened_after_an_escaped_backtick_is_a_code_span() {
    let document = "a \\` b `\nMode: strict\n` c";
    // Upstream: the three lines as they are.
    assert_eq!(visible(document), ["a \\` b ", "", ""]);
}

/// Deliberate deviation (markdown/commonmark): a tag indented four columns
/// under a paragraph is a tag in that paragraph. Upstream renders the line
/// with a no-break space in place of its last column of indentation, to stop
/// its renderer opening an HTML block there, and then probes that line: the
/// probe no longer lands inside the tag and the tag is not recorded. A span
/// that carries on onto such a line is reported one column short for the
/// same reason (`repair column` in the rule).
#[test]
fn a_tag_indented_under_a_paragraph_is_recorded_where_it_is() {
    let structure = read("para\n    <div>\nmore");
    assert_eq!(
        kinds("para\n    <div>\nmore"),
        [Paragraph, Paragraph, Paragraph]
    );
    // Upstream: no span.
    let tag = structure.lines[1].invisible[0];
    assert_eq!((tag.start, tag.end, tag.kind), (4, 9, SpanKind::HtmlText));
    // Upstream: the span starts at column 3 on the second line.
    let carried = read("para `x\n    <h2> a` b").lines[1].invisible[0];
    assert_eq!((carried.start, carried.end), (4, 11));
}

/// Deliberate deviation (markdown/commonmark): a blank line is nothing, or
/// spaces and tabs. A no-break space is a character, and a line holding one
/// is a paragraph. Upstream calls a line blank when JavaScript's `trim`
/// leaves nothing of it, which takes every Unicode space with it.
#[test]
fn a_line_with_a_no_break_space_is_not_blank() {
    // Upstream: paragraph, blank, blank, blank, paragraph.
    assert_eq!(
        kinds("a\n\n\u{a0}\n\nb"),
        [Paragraph, Blank, Paragraph, Blank, Paragraph]
    );
}

/// Not a deviation, a repair: CommonMark ends an HTML block opened by
/// `<script>`, `<pre>`, `<style>` or `<textarea>` at the first line holding
/// the closing tag of any of the four, in any letter case. The parser, as of
/// 0.13, waits for the tag that matches the opening one, in lower case, and
/// would hide the rest of the document. Upstream and the referee read these
/// as Focus does.
#[test]
fn a_raw_text_block_ends_at_any_of_the_four_closing_tags() {
    assert_eq!(
        kinds("<pre>\ncode\n</style>\n\nafter"),
        [HtmlFlow, HtmlFlow, HtmlFlow, Blank, Paragraph]
    );
    assert_eq!(
        kinds("<SCRIPT>\nx\n</SCRIPT>\nafter\n\n<style>\n</TextArea> y\nz"),
        [
            HtmlFlow, HtmlFlow, HtmlFlow, Paragraph, Blank, HtmlFlow, HtmlFlow, Paragraph
        ]
    );
    // One closing tag inside another block's last line ends only its own.
    assert_eq!(
        kinds("- <pre>\n  </script>\n- <textarea>\n  x\n\n  </pre>\n\ntext"),
        [
            HtmlFlow, HtmlFlow, HtmlFlow, HtmlFlow, HtmlFlow, HtmlFlow, Blank, Paragraph
        ]
    );
}

/// A second definition of a label is a definition all the same: the parser
/// keeps the first and reports nothing for the rest, and Focus must not take
/// the silence for text.
#[test]
fn a_label_defined_twice_is_two_definitions() {
    let structure = read("[a]: /one\n[A]: /two\n  'title'\n\n[a]");
    assert_eq!(
        kinds("[a]: /one\n[A]: /two\n  'title'\n\n[a]"),
        [Definition, Definition, Definition, Blank, Paragraph]
    );
    let listed: Vec<(&str, usize, usize)> = structure
        .definitions
        .iter()
        .map(|definition| {
            (
                definition.label.as_str(),
                definition.start_line,
                definition.end_line,
            )
        })
        .collect();
    assert_eq!(listed, [("A", 0, 0), ("A", 1, 2)]);
    assert_eq!(structure.labels, ["A"]);
}

/// Known limit (markdown/reading): the parser's reading of a table differs
/// from GFM's in corners. Here upstream and the referee are right and Focus
/// is not; `differential.py --markdown` counts such lines and refuses a rise.
/// The tests pin what Focus does today, so a parser release that moves one
/// shows here.
#[test]
fn where_the_parser_reads_a_table_otherwise_than_gfm() {
    // A header row with no pipe over a delimiter row with one: a table of
    // one column in GFM.
    assert_eq!(kinds("a\n|-"), [Paragraph, Paragraph]);
    // A delimiter row that starts like a list item: GFM reads the item.
    assert_eq!(kinds("a | b\n- | -"), [Table, Table]);
}

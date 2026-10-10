//! Readings the corpus cannot hold Focus to by itself.
//!
//! The corpus judges a line that differs from upstream's with a referee
//! (`tests/oracle_corpus.rs`), so where upstream's own answer is the odd one
//! out, both it and the referee's would pass. These are the places where
//! Focus is meant to give upstream's answer all the same; the values are
//! upstream's, taken from `tools/oracle/driver.ts`.

use focus_markdown::{LineKind, analyse};

/// Upstream measures a span from a probe word, by the length of the text
/// rendered on each side of it. A table cell renders `\|` as `|`, so a code
/// span holding one is reported that much short. The sensors read these
/// columns (they blank code before looking for tags), so Focus reports
/// upstream's, not the span's own: `` `a\|b` `` runs from column 2 to 8 and
/// is reported as 2 to 6.
#[test]
fn a_code_span_in_a_table_cell_is_measured_as_upstream_measures_it() {
    let document = "| a | b |\n| --- | --- |\n| `a\\|b` | `x\\|y\\|z` |\n| ` p\\|q ` | `\\|` |\n";
    let structure = analyse(document).unwrap();
    let columns = |line: usize| -> Vec<(usize, usize)> {
        structure.lines[line]
            .invisible
            .iter()
            .map(|span| (span.start, span.end))
            .collect()
    };
    assert_eq!(columns(2), [(2, 6), (11, 17)]);
    assert_eq!(columns(3), [(2, 7), (13, 15)]);
    // The lengths are in UTF-16 code units, as upstream counts them: here
    // the span ends before the first `é`, eight units and eight bytes in,
    // not two bytes further on.
    let accented = analyse("| a |\n|---|\n| `x\\|\\|\u{e9}\u{e9}` |").unwrap();
    let span = accented.lines[2].invisible[0];
    assert_eq!((span.start, span.end), (2, 8));
    // And rendered code loses a space of padding at each end, which moves
    // the start when the escaped pipe comes before the probe.
    let padded = analyse("| a |\n|---|\n| ` \\|a ` |").unwrap();
    let span = padded.lines[2].invisible[0];
    assert_eq!((span.start, span.end), (5, 9));
    // Outside a table a span is where it is.
    let plain = analyse("see `a\\|b` and ` p\\|q `\n").unwrap();
    let spans: Vec<(usize, usize)> = plain.lines[0]
        .invisible
        .iter()
        .map(|span| (span.start, span.end))
        .collect();
    assert_eq!(spans, [(4, 10), (15, 23)]);
}

/// Blank lines between lines of indented code are code; blank lines after
/// the last one are not, and the parser ends the block there. The corpus
/// cannot tell if that changes: a line of white space reads the same either
/// way.
#[test]
fn blank_lines_after_indented_code_are_not_code() {
    use LineKind::{Blank, CodeIndented, Paragraph};
    let kinds: Vec<LineKind> = analyse("    code\n\n    more\n    \n\ntext")
        .unwrap()
        .lines
        .iter()
        .map(|line| line.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            CodeIndented,
            CodeIndented,
            CodeIndented,
            Blank,
            Blank,
            Paragraph
        ]
    );
}

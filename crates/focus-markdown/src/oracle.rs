//! The oracle protocol for Markdown.
//!
//! A `markdown` case is a document. The answer has one record for each line:
//! its kind, and what a consumer reads off the line's structure and off the
//! three projections. Columns are UTF-16 code units, as upstream counts
//! them.
//!
//! The answer is not required to be upstream's. The driver answers `null`
//! for a line upstream did not place, and puts a third parser's reading next
//! to its own; each line is then judged by the rule in
//! `tools/oracle/markdown_rule.py`, which `tests/oracle_corpus.rs` repeats.

use serde_json::{Value, json};

use crate::visible::visible_lines_of;
use crate::{Blocks, Container, LineKind, Visible, analyse, source_lines};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 1] = ["markdown"];

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    match case.get("kind").and_then(Value::as_str).unwrap_or_default() {
        "markdown" => markdown(case["content"].as_str().unwrap_or_default()),
        other => json!({ "unsupported": other }),
    }
}

/// A byte column of `line` as a count of UTF-16 code units.
fn units(line: &str, column: usize) -> usize {
    line[..column.min(line.len())].encode_utf16().count()
}

fn shape(containers: &[Container]) -> String {
    containers
        .iter()
        .map(|container| match container {
            Container::BlockQuote => ">",
            Container::ListItem { ordered: true, .. } => "o",
            Container::ListItem { .. } => "u",
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `true` when `text` is `reference`, else the text.
fn same_or(text: &str, reference: &str) -> Value {
    if text == reference {
        Value::Bool(true)
    } else {
        Value::String(text.to_owned())
    }
}

fn markdown(content: &str) -> Value {
    let raw = source_lines(content);
    let blocks: Blocks = match analyse(content) {
        Ok(blocks) => blocks,
        Err(refusal) => return json!({ "error": refusal.message() }),
    };
    let plain = visible_lines_of(content, &blocks, Visible::default());
    let boundaries = visible_lines_of(
        content,
        &blocks,
        Visible {
            preserve_comment_boundaries: true,
            ..Visible::default()
        },
    );
    let indented = visible_lines_of(
        content,
        &blocks,
        Visible {
            preserve_indented_code: true,
            ..Visible::default()
        },
    );
    let lines: Vec<Value> = blocks
        .lines
        .iter()
        .enumerate()
        .map(|(index, line)| {
            let code = matches!(line.kind, LineKind::CodeFenced | LineKind::CodeIndented);
            let structured = matches!(
                line.kind,
                LineKind::Heading | LineKind::Paragraph | LineKind::Table | LineKind::HtmlFlow
            );
            // An HTML block is known by the line it starts on.
            let group = (line.kind == LineKind::HtmlFlow).then(|| {
                blocks
                    .lines
                    .iter()
                    .position(|other| other.kind == LineKind::HtmlFlow && other.block == line.block)
            });
            let spans: Vec<Value> = line
                .invisible
                .iter()
                .map(|span| {
                    json!([
                        units(&raw[index], span.start),
                        units(&raw[index], span.end),
                        span.kind.name(),
                        span.token_start_line,
                        span.token_end_line
                    ])
                })
                .collect();
            json!([
                line.kind.name(),
                line.html_kind,
                (!code).then(|| units(&raw[index], line.content_start)),
                structured.then(|| shape(&line.containers)),
                group,
                spans,
                // A projection that leaves the line as it is answers `true`,
                // and so does one that answers what the plain one does.
                same_or(&plain[index], &raw[index]),
                same_or(&boundaries[index], &plain[index]),
                same_or(&indented[index], &plain[index]),
            ])
        })
        .collect();
    let definitions: Vec<Value> = blocks
        .definitions
        .iter()
        .map(|definition| json!([definition.label, definition.start_line, definition.end_line]))
        .collect();
    json!({ "lines": lines, "definitions": definitions, "labels": blocks.labels })
}

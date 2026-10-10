//! Replays the Markdown share of the oracle corpus.
//!
//! Deliberate deviation: these cases are not held to equality with upstream.
//! Upstream's runtime renders Markdown without source positions, so its
//! analysis finds each line's block by rendering the document with probe
//! words in it. That approximates the CommonMark structure it means to read:
//! some lines it cannot place, and in nested containers it places some where
//! CommonMark does not. Focus reads the structure from a parser. Each line of
//! each document is therefore judged, by the rule `tools/oracle/markdown_rule.py`
//! states and this file repeats for the vendored corpus:
//!
//! - `same`: Focus's record of the line is upstream's;
//! - `unplaced`: upstream answered nothing for the line, and the referee
//!   reads it as Focus does;
//! - `no text`: the line is white space and container markers, and upstream
//!   and Focus differ only on whether it is blank or inside a code or HTML
//!   block, or upstream did not place it and Focus reads it as one of those;
//! - `upstream alone`: the two differ, and the referee recorded next to
//!   upstream's answer reads the line as Focus does: micromark, a third
//!   parser, or, on a line none of the three calls a table, commonmark.js,
//!   the reference implementation;
//! - `repair column`: a code span or tag carries on onto a line that opens
//!   with an indented HTML tag, and upstream reports it as starting one
//!   column short there (it renders such a line with a no-break space in
//!   its last column of indentation).
//!
//! Anything else fails. The corpus is upstream's own golden documents, the
//! Markdown it ships, and documents written the way a careful author writes
//! them; `differential.py --markdown` judges careless ones live. Nothing here
//! needs upstream or Bun.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use focus_js::re;
use focus_markdown::oracle::{KINDS, run_case};
use focus_markdown::source_lines;
use serde_json::Value;

/// Quote markers, and list markers with nothing or white space after them.
const MARKERS_ONLY: &str = r"^[ \t]*(?:(?:>|(?:[-*+]|\d{1,9}[.)])(?=[ \t]|$))[ \t]*)*$";
const RAW_OR_BLANK: [&str; 4] = ["blank", "codeFenced", "codeIndented", "htmlFlow"];

/// What the referee also reports of a line: kind, containers, content start
/// and HTML block.
fn structure(record: &Value) -> Value {
    Value::Array(vec![
        record[0].clone(),
        record[3].clone(),
        record[2].clone(),
        record[4].clone(),
    ])
}

/// A construct as the referee names it: its kind and the lines it runs over.
type Name = (String, u64, u64);

fn name(kind: &Value, first: &Value, last: &Value) -> Name {
    (
        kind.as_str().unwrap().to_owned(),
        first.as_u64().unwrap(),
        last.as_u64().unwrap(),
    )
}

/// How many spans of a line belong to each construct.
fn constructs(record: &Value) -> BTreeMap<Name, usize> {
    let mut counted = BTreeMap::new();
    for span in record[5].as_array().unwrap() {
        *counted
            .entry(name(&span[2], &span[3], &span[4]))
            .or_insert(0) += 1;
    }
    counted
}

struct Referee<'a> {
    lines: &'a Value,
    /// The reference implementation's reading, where it is not the referee's.
    second: &'a Value,
    seen: BTreeMap<Name, usize>,
    starts: BTreeMap<Name, BTreeSet<u64>>,
    ends: BTreeMap<Name, BTreeSet<u64>>,
}

impl<'a> Referee<'a> {
    fn of(answer: &'a Value) -> Self {
        let mut referee = Self {
            lines: &answer["referee"]["lines"],
            second: &answer["referee"]["second"],
            seen: BTreeMap::new(),
            starts: BTreeMap::new(),
            ends: BTreeMap::new(),
        };
        for token in answer["referee"]["tokens"].as_array().unwrap() {
            let name = name(&token[0], &token[1], &token[2]);
            *referee.seen.entry(name.clone()).or_insert(0) += 1;
            referee
                .starts
                .entry(name.clone())
                .or_default()
                .insert(token[3].as_u64().unwrap());
            referee
                .ends
                .entry(name)
                .or_default()
                .insert(token[4].as_u64().unwrap());
        }
        referee
    }

    /// Whether the referee sees every construct `one` records, and fewer
    /// than `other` records wherever `other` records more.
    fn sees(&self, one: &BTreeMap<Name, usize>, other: &BTreeMap<Name, usize>) -> bool {
        let seen = |name: &Name| self.seen.get(name).copied().unwrap_or(0);
        one.iter().all(|(name, count)| *count <= seen(name))
            && other.iter().all(|(name, count)| {
                *count <= one.get(name).copied().unwrap_or(0) || *count > seen(name)
            })
    }

    /// Whether the two records differ only in where a construct starts on its
    /// first line or ends on its last, and the referee has it where Focus does.
    fn settles_columns(&self, index: u64, theirs: &Value, mine: &Value) -> bool {
        let (theirs, mine) = (theirs[5].as_array().unwrap(), mine[5].as_array().unwrap());
        let mut settled = false;
        for (a, b) in theirs.iter().zip(mine) {
            if a == b {
                continue;
            }
            let name = name(&b[2], &b[3], &b[4]);
            let has = |columns: &BTreeMap<Name, BTreeSet<u64>>, column: &Value| {
                columns
                    .get(&name)
                    .is_some_and(|known| known.contains(&column.as_u64().unwrap()))
            };
            let start_settled = a[0] == b[0]
                || (b[3] == index && has(&self.starts, &b[0]) && !has(&self.starts, &a[0]));
            let end_settled = a[1] == b[1]
                || (b[4] == index && has(&self.ends, &b[1]) && !has(&self.ends, &a[1]));
            if a[2] != b[2] || a[3] != b[3] || a[4] != b[4] || !start_settled || !end_settled {
                return false;
            }
            settled = true;
        }
        settled && theirs.len() == mine.len()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    Same,
    Unplaced,
    NoText,
    UpstreamAlone,
    RepairColumn,
}

/// Whether the two records differ only in where one span starts, upstream
/// having it one column before the line's text, on a line that the span
/// carries on onto and that opens with `<`. The projections may differ with
/// it: they keep the text before the span.
fn repair_column(text: &str, index: usize, theirs: &Value, mine: &Value) -> bool {
    let (theirs, mine) = (theirs.as_array().unwrap(), mine.as_array().unwrap());
    let (spans, focus_spans) = (theirs[5].as_array().unwrap(), mine[5].as_array().unwrap());
    if theirs[..5] != mine[..5] || spans.len() != focus_spans.len() {
        return false;
    }
    let differing: Vec<(&Value, &Value)> = spans
        .iter()
        .zip(focus_spans)
        .filter(|(a, b)| a != b)
        .collect();
    let [(a, b)] = differing[..] else {
        return false;
    };
    let start = b[0].as_u64().unwrap();
    a.as_array().unwrap()[1..] == b.as_array().unwrap()[1..]
        && a[0].as_u64().unwrap() + 1 == start
        && b[3].as_u64().unwrap() < index as u64
        && b[0] == mine[2]
        && text.encode_utf16().nth(start as usize) == Some(u16::from(b'<'))
}

fn judge(
    text: &str,
    index: usize,
    theirs: &Value,
    mine: &Value,
    referee: &Referee,
) -> Result<Verdict, String> {
    let raw_or_blank = |record: &Value| RAW_OR_BLANK.contains(&record[0].as_str().unwrap());
    let no_text = re::test(MARKERS_ONLY, "", text);
    let focus = structure(mine);
    // The referee's reading is listed only where it is not upstream's, and
    // the second opinion only where it is not the referee's.
    let upstream = (!theirs.is_null()).then(|| structure(theirs));
    let key = index.to_string();
    let said = referee.lines.get(&key).or(upstream.as_ref()).unwrap();
    let other = referee.second.get(&key).unwrap_or(said);
    let tabular = [Some(mine), Some(said), upstream.as_ref()]
        .iter()
        .flatten()
        .any(|record| record[0] == "table");
    let backed = *said == focus || (!tabular && *other == focus);
    if theirs.is_null() {
        // Nothing of upstream's to hold Focus to: the referee's reading is.
        return if backed {
            Ok(Verdict::Unplaced)
        } else if no_text && raw_or_blank(mine) {
            Ok(Verdict::NoText)
        } else {
            Err(format!("upstream nothing, focus {focus} referee {said}"))
        };
    }
    if theirs == mine {
        return Ok(Verdict::Same);
    }
    if no_text && raw_or_blank(theirs) && raw_or_blank(mine) {
        return Ok(Verdict::NoText);
    }
    let upstream = structure(theirs);
    if upstream != focus {
        return if backed {
            Ok(Verdict::UpstreamAlone)
        } else {
            Err(format!("upstream {upstream} focus {focus} referee {said}"))
        };
    }
    let (of_upstream, of_focus) = (constructs(theirs), constructs(mine));
    let settled = if of_upstream == of_focus {
        theirs.as_array().unwrap()[..5] == mine.as_array().unwrap()[..5]
            && referee.settles_columns(index as u64, theirs, mine)
    } else {
        referee.sees(&of_focus, &of_upstream)
    };
    if settled {
        Ok(Verdict::UpstreamAlone)
    } else if repair_column(text, index, theirs, mine) {
        Ok(Verdict::RepairColumn)
    } else {
        Err(format!("same structure; upstream {theirs} focus {mine}"))
    }
}

/// A definition as an answer lists it: label, first line, last line.
type Entry = (String, usize, usize);

fn definitions(answer: &Value) -> BTreeSet<Entry> {
    answer["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry[0].as_str().unwrap().to_owned(),
                entry[1].as_u64().unwrap() as usize,
                entry[2].as_u64().unwrap() as usize,
            )
        })
        .collect()
}

#[test]
fn every_line_is_upstreams_or_one_it_misplaced() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/oracle");
    let cases = fs::read_to_string(dir.join("cases.ndjson")).unwrap();
    let expected = fs::read_to_string(dir.join("expected.ndjson")).unwrap();
    let cases: Vec<&str> = cases.lines().filter(|l| !l.is_empty()).collect();
    let expected: Vec<&str> = expected.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(
        cases.len(),
        expected.len(),
        "cases.ndjson and expected.ndjson are out of step"
    );

    let mut documents = 0;
    let mut verdicts: BTreeMap<Verdict, usize> = BTreeMap::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut spans: BTreeMap<String, usize> = BTreeMap::new();
    let mut hidden = 0;
    let mut failures = Vec::new();
    for (line, want) in cases.iter().zip(&expected) {
        let case: Value = focus_js::json::parse(line).unwrap();
        if !KINDS.contains(&case["kind"].as_str().unwrap_or_default()) {
            continue;
        }
        documents += 1;
        let id = case["id"].as_str().unwrap();
        let upstream = &serde_json::from_str::<Value>(want).unwrap()["out"];
        let focus = run_case(&case);
        assert!(
            focus.get("error").is_none(),
            "{id}: focus refused the document: {focus}"
        );
        let source = source_lines(case["content"].as_str().unwrap());
        let (theirs, mine) = (
            upstream["lines"].as_array().unwrap(),
            focus["lines"].as_array().unwrap(),
        );
        assert_eq!(
            theirs.len(),
            mine.len(),
            "{id}: the two count lines differently"
        );
        assert_eq!(
            source.len(),
            mine.len(),
            "{id}: an answer does not have one record per line"
        );
        let referee = Referee::of(upstream);
        // Lines the two did not answer alike.
        let mut loose = BTreeSet::new();
        for (index, (theirs, mine)) in theirs.iter().zip(mine).enumerate() {
            *kinds
                .entry(mine[0].as_str().unwrap().to_owned())
                .or_insert(0) += 1;
            for span in mine[5].as_array().unwrap() {
                *spans
                    .entry(span[2].as_str().unwrap().to_owned())
                    .or_insert(0) += 1;
            }
            hidden += usize::from(mine[6] != Value::Bool(true));
            match judge(&source[index], index, theirs, mine, &referee) {
                Ok(verdict) => {
                    *verdicts.entry(verdict).or_insert(0) += 1;
                    if verdict != Verdict::Same {
                        loose.insert(index);
                    }
                }
                Err(why) => {
                    loose.insert(index);
                    failures.push(format!("{id} line {}: {why}", index + 1));
                }
            }
        }
        // Definitions are compared where their lines were, a definition
        // together with the ones it touches; labels when every line was.
        let (theirs, mine) = (definitions(upstream), definitions(&focus));
        let defined: BTreeSet<usize> = theirs
            .union(&mine)
            .flat_map(|(_, first, last)| *first..=*last)
            .collect();
        for (label, first, last) in theirs.symmetric_difference(&mine) {
            let (mut first, mut last) = (*first, *last);
            while first > 0 && defined.contains(&(first - 1)) {
                first -= 1;
            }
            while defined.contains(&(last + 1)) {
                last += 1;
            }
            if !(first..=last).any(|index| loose.contains(&index)) {
                failures.push(format!(
                    "{id}: definition `{label}` differs on lines both placed alike"
                ));
            }
        }
        if loose.is_empty() && upstream["labels"] != focus["labels"] {
            failures.push(format!(
                "{id}: labels: upstream {} focus {}",
                upstream["labels"], focus["labels"]
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} Markdown lines or documents differ from upstream with no verdict to explain it; first:\n{}",
        failures.len(),
        failures
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    // A corpus that shows little would prove little.
    let count = |verdict| verdicts.get(&verdict).copied().unwrap_or(0);
    let lines: usize = verdicts.values().sum();
    assert!(documents >= 700, "too few Markdown documents: {documents}");
    assert!(lines >= 20_000, "too few lines: {lines}");
    assert!(
        count(Verdict::Same) * 100 >= lines * 97,
        "too few lines answered as upstream answers: {verdicts:?}"
    );
    assert!(
        count(Verdict::Unplaced) >= 100 && count(Verdict::UpstreamAlone) >= 10,
        "the corpus no longer shows where upstream's analysis falls short: {verdicts:?}"
    );
    // These documents were written with care, and on them the two analyses
    // part on about one line in a thousand. More than that is Focus drifting.
    assert!(
        count(Verdict::UpstreamAlone) * 400 <= lines,
        "too many lines where only the referee reads as Focus does: {verdicts:?}"
    );
    for kind in [
        "blank",
        "paragraph",
        "heading",
        "thematicBreak",
        "codeFenced",
        "codeIndented",
        "htmlFlow",
        "definition",
        "table",
    ] {
        assert!(
            kinds.get(kind).copied().unwrap_or(0) >= 100,
            "too few `{kind}` lines: {kinds:?}"
        );
    }
    for kind in ["codeText", "htmlText", "htmlComment"] {
        assert!(
            spans.get(kind).copied().unwrap_or(0) >= 100,
            "too few `{kind}` spans: {spans:?}"
        );
    }
    assert!(
        hidden >= 2_000,
        "too few lines a projection changes: {hidden}"
    );
}

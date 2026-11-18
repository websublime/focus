//! What a reviewer wrote, checked before it is recorded: the review states
//! one verdict, its reviewer and its pass, in lines a reader of the
//! rendered review sees as such, and it is one section with nothing of the
//! document's own rank after it (`validateReviewAppendix`, `aidlc-lib.ts`
//! L16 107). And a review file's own `#` and `##` headings made `###`, so a
//! reviewer who wrote `## What I verified` is not sent back for it
//! (`demoteReviewHeadings`, `aidlc-log.ts` L868).
//!
//! The review is read through `focus-markdown`: the lines that count are
//! the ones [`focus_markdown::rendered`] gives, and the headings the ones
//! the analysis places.

use focus_js::{num, re, text as js_text};
use focus_markdown::rendered::{self, MARK_CLOSE, MARK_OPEN};
use focus_markdown::{LineKind, analyse};

/// What the request the review answers expects of it.
#[derive(Debug, Clone, Copy)]
pub struct Expected<'a> {
    /// `READY` or `NOT-READY`: what `--verdict` said.
    pub verdict: &'a str,
    pub reviewer: &'a str,
    pub iteration: f64,
    /// The challenge of a request of the retired appendix protocol.
    pub review_challenge: Option<&'a str>,
    /// The review is a file of its own, not an appendix: it need not open
    /// with the heading.
    pub standalone: bool,
}

/// Whether a review can be recorded, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validity {
    Valid,
    Invalid {
        reason: &'static str,
        /// The reason is a heading of the document's own rank.
        heading: bool,
    },
}

const fn invalid(reason: &'static str) -> Validity {
    Validity::Invalid {
        reason,
        heading: false,
    }
}

/// `renderedReviewFields`: the values of the rendered lines that are a
/// field of this label: the label in bold at the start of the line, and
/// nothing else in it.
fn fields(rendered: &str, label: &str) -> Vec<String> {
    let pattern = format!("^{MARK_OPEN}{label}:{MARK_CLOSE}[ \\t]+(.+?)[ \\t]*$");
    re::find_all(&pattern, "gm", rendered)
        .iter()
        .filter_map(|found| re::group(rendered, found, 1))
        .map(str::to_owned)
        .collect()
}

/// The values, each once, in the order first written.
fn distinct(values: Vec<String>) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for value in values {
        if !seen.contains(&value) {
            seen.push(value);
        }
    }
    seen
}

/// `validateReviewAppendix`.
#[must_use]
pub fn validate_review_appendix(appendix: &[u8], expected: &Expected<'_>) -> Validity {
    let Ok(text) = std::str::from_utf8(appendix) else {
        return invalid("the review is not valid UTF-8");
    };
    // (The decoder upstream takes a leading byte order mark for itself.)
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let normalized = re::replace(r"\r\n?", "g", text, "\n");
    let opening = re::find(r"^(?:[ \t]*\n)*## Review[ \t]*\n", "", &normalized);
    if opening.is_none() && !expected.standalone {
        return invalid(
            "the appended bytes must begin with only blank lines followed by an exact `## Review` heading",
        );
    }
    let section = opening
        .as_ref()
        .map_or(normalized.as_str(), |found| &normalized[found.end()..]);
    let Some(authority) = rendered::render(section) else {
        return invalid("the reviewer appendix could not be parsed as Markdown");
    };
    if authority.headings.iter().any(|heading| heading.level <= 2) {
        return Validity::Invalid {
            reason: "the reviewer appendix must be terminal and contain no later rendered H1 or H2 heading",
            heading: true,
        };
    }
    if authority
        .html_blocks
        .iter()
        .any(|block| rendered::html_carries_h1_h2(block))
    {
        return Validity::Invalid {
            reason: "the reviewer appendix must be terminal and contain no rendered HTML H1 or H2 heading",
            heading: true,
        };
    }
    // Two Review sections are two reviews: the record reads the first
    // section's findings only, so the second's would be lost. The opening
    // heading, cut before rendering, counts as the first.
    let sections = authority
        .headings
        .iter()
        .filter(|heading| {
            let plain: String = heading
                .children
                .chars()
                .filter(|character| *character != MARK_OPEN && *character != MARK_CLOSE)
                .collect();
            js_text::to_lower_case(js_text::trim(&plain)) == "review"
        })
        .count();
    if sections + usize::from(opening.is_some()) >= 2 {
        return invalid("the review has two Review sections; keep one and write it whole");
    }
    // Inside the one section a line repeated word for word is one line; two
    // different values are still two lines, and refused.
    let verdicts = distinct(fields(&authority.text, "Verdict"));
    let reviewers = distinct(fields(&authority.text, "Reviewer"));
    let iterations = distinct(fields(&authority.text, "Iteration"));
    let challenges = fields(&authority.text, "Request Challenge");
    if verdicts.len() != 1 || verdicts[0] != expected.verdict {
        return invalid(
            "the reviewer appendix must contain exactly one canonical verdict line matching --verdict",
        );
    }
    if reviewers.len() != 1 || reviewers[0] != expected.reviewer {
        return invalid(
            "the reviewer appendix must contain exactly one Reviewer line matching the requested reviewer",
        );
    }
    if iterations.len() != 1 || iterations[0] != num::to_string(expected.iteration) {
        return invalid(
            "the reviewer appendix must contain exactly one Iteration line matching the request",
        );
    }
    match expected.review_challenge {
        None if !challenges.is_empty() => invalid(
            "the reviewer appendix must omit Request Challenge when the request did not issue one",
        ),
        Some(challenge) if challenges.len() != 1 || challenges[0] != challenge => invalid(
            "the reviewer appendix must contain exactly one Request Challenge line matching the request",
        ),
        _ => Validity::Valid,
    }
}

/// A review file with its headings changed, and a line naming each one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Demoted {
    pub bytes: Vec<u8>,
    pub changed: Vec<String>,
}

/// `demoteReviewHeadings`: a review file's top-level `#` and `##` heading
/// lines (outside code, quotes and lists, other than an opening
/// `## Review`) made `###`, with every other byte kept; `None` when there
/// is none, or the file is not text.
#[must_use]
pub fn demote_review_headings(body: &[u8]) -> Option<Demoted> {
    let text = std::str::from_utf8(body).ok()?;
    let (bom, rest) = match text.strip_prefix('\u{feff}') {
        Some(rest) => ("\u{feff}", rest),
        None => ("", text),
    };
    // The lines, and each one's own line ending.
    let mut source: Vec<String> = Vec::new();
    let mut endings: Vec<&str> = Vec::new();
    let mut from = 0;
    for found in re::find_all(r"\r\n|\r|\n", "g", rest) {
        source.push(rest[from..found.start()].to_owned());
        endings.push(&rest[found.start()..found.end()]);
        from = found.end();
    }
    source.push(rest[from..].to_owned());
    let blocks = analyse(&source.join("\n")).ok()?;
    if blocks.lines.len() != source.len() {
        return None;
    }
    let opening = source
        .iter()
        .position(|line| !js_text::trim(line).is_empty());
    let mut changed = Vec::new();
    for index in 0..source.len() {
        let placed = &blocks.lines[index];
        if placed.kind != LineKind::Heading || !placed.containers.is_empty() {
            continue;
        }
        if Some(index) == opening && re::test(r"^## Review[ \t]*$", "", &source[index]) {
            continue;
        }
        let demoted = re::replace(r"^( {0,3})#{1,2}(?=[ \t]|$)", "", &source[index], "$1###");
        if demoted == source[index] {
            continue;
        }
        let said = re::replace(
            r"[\u0000-\u001f\u007f]",
            "g",
            js_text::trim(&source[index]),
            "?",
        );
        changed.push(format!(
            "line {}: {}",
            index + 1,
            js_text::slice_utf16(&said, 0, 120)
        ));
        source[index] = demoted;
    }
    if changed.is_empty() {
        return None;
    }
    let mut out = String::from(bom);
    for (index, line) in source.iter().enumerate() {
        out.push_str(line);
        if let Some(ending) = endings.get(index) {
            out.push_str(ending);
        }
    }
    Some(Demoted {
        bytes: out.into_bytes(),
        changed,
    })
}

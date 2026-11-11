//! What the Plan Approval guard reads of the Testing Posture contract
//! (`aidlc-testing-posture.ts`): the pure readers of a developer handoff
//! and of the Code Generation stage's own files. The contract itself (the
//! methodology resolved, the plan profile, the approval fingerprint and its
//! receipt) is M5; what is here reads text and decides nothing about
//! authority.
//!
//! - [`prompt_testing_contract_markers`]: the `FOCUS-TESTING-CONTRACT`
//!   lines of a handoff, by hash;
//! - [`questions_file_approved`] and
//!   [`questions_file_has_pending_plan_approval`]: the answer under the
//!   latest `Plan Approval` section of `code-generation-questions.md`, read
//!   through the document's visible lines;
//! - [`plan_review_appendix`]: the terminal `## Review` appendix of a plan,
//!   which the approval excludes, located as the reviewer protocol locates
//!   it: the exact heading line, confirmed to be the document's last h1 or
//!   h2.
//!
//! Held to upstream's answers by the oracle corpus (`contract_markers`,
//! `questions_file`, `review_appendix`).

use focus_js::re;
use focus_js::text::trim;
use focus_markdown::{Blocks, LineKind, Visible, analyse, source_lines, visible_lines};

const CONTRACT_MARKER: &str =
    r"^[ \t]*FOCUS-TESTING-CONTRACT[ \t]*:[ \t]*(sha256:[0-9a-f]{64})[ \t]*$";
const MARKDOWN_HEADING: &str = r"^(#{1,6})[ \t]+(.+?)[ \t]*#*[ \t]*$";
const ANSWER_TAG: &str = r"^\[Answer\]:[ \t]*(.*)$";
/// The recorded fingerprint tag, in every format the tool has written
/// (bare, v2, v3), so an older file is read and reported rather than
/// looking like a line the parser does not understand.
const FINGERPRINT_TAG: &str =
    r"^\[Approval Fingerprint\]:[ \t]*(sha256:(?:v[23]:)?[0-9a-f]{64})?[ \t]*$";
const PLANNED_SOURCE_TAG: &str =
    r"^\[Planned Source\]:[ \t]*([0-9a-f]{40}|[0-9a-f]{64}|unbindable)?[ \t]*$";
/// "Plan approval off" is the engine's own record for a plan built without
/// asking. It grants nothing alone: every reader also needs the engine's
/// receipt.
const APPROVE_PLAN: &str = r#"^(?:(?:[A-Z][.)][ \t]*)?["']?Approve Plan["']?|Plan approval off)$"#;
const QUESTION_PREFIX: &str = r"^(?:(?:q(?:uestion)?[ \t]*)?\d+[ \t]*[:.)-][ \t]*)";
const NUMBERED_QUESTION_HEADING: &str = r"^(?:q(?:uestion)?[ \t]*)?\d+[ \t]*[.:)-]?[ \t]*$";

/// `promptTestingContractMarkers`: the distinct contract hashes a handoff
/// carries, in encounter order.
#[must_use]
pub fn prompt_testing_contract_markers(text: &str) -> Vec<String> {
    let mut hashes: Vec<String> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(found) = re::find(CONTRACT_MARKER, "", line)
            && let Some(hash) = re::group(line, &found, 1)
            && !hashes.iter().any(|seen| seen == hash)
        {
            hashes.push(hash.to_owned());
        }
    }
    hashes
}

/// `isPlanApprovalLabel`: the heading text, less a trailing `?` or `:` and
/// one pair of emphasis markers, is "plan approval".
fn is_plan_approval_label(value: &str) -> bool {
    let mut normalized = trim(&re::replace(r"[?:][ \t]*$", "", trim(value), "")).to_owned();
    for marker in ["**", "__", "*", "_"] {
        if normalized.starts_with(marker)
            && normalized.ends_with(marker)
            && normalized.len() > marker.len() * 2
        {
            normalized =
                trim(&normalized[marker.len()..normalized.len() - marker.len()]).to_owned();
            break;
        }
    }
    normalized.to_lowercase() == "plan approval"
}

/// What the latest `Plan Approval` section of a questions file records.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanApprovalSection {
    pub found: bool,
    pub answer: Option<String>,
    pub answer_line: Option<usize>,
    pub fingerprint: Option<String>,
    pub planned_source: Option<String>,
}

/// `latestPlanApproval`: the section opened by the last heading whose text
/// is the label `Plan Approval` (`## Plan Approval`, `## Q1: Plan Approval`,
/// or a numbered heading with the label on the next line), running until a
/// heading at its depth or shallower, or the next numbered question. A
/// deeper heading opens a subsection whose tags fill an empty slot but never
/// replace one the section already carried.
#[must_use]
pub fn latest_plan_approval(body: &str) -> PlanApprovalSection {
    let Ok(visible) = visible_lines(body, Visible::default()) else {
        return PlanApprovalSection::default();
    };
    let opened = || PlanApprovalSection {
        found: true,
        ..PlanApprovalSection::default()
    };
    let mut section = PlanApprovalSection::default();
    let mut in_plan_approval = false;
    let mut awaiting_numbered_question_text = false;
    let mut plan_approval_depth = 0;
    let mut in_subsection = false;
    for (index, line) in visible.iter().enumerate() {
        if let Some(heading) = re::find(MARKDOWN_HEADING, "", line) {
            let depth = re::group(line, &heading, 1).map_or(0, str::len);
            let heading_text = trim(re::group(line, &heading, 2).unwrap_or(""));
            if is_plan_approval_label(&re::replace(QUESTION_PREFIX, "i", heading_text, "")) {
                in_plan_approval = true;
                plan_approval_depth = depth;
                in_subsection = false;
                awaiting_numbered_question_text = false;
                section = opened();
                continue;
            }
            if in_plan_approval
                && depth > plan_approval_depth
                // A numbered heading is the next question, however deeply it
                // was nested, so it ends the section rather than opening a
                // subsection.
                && !re::test(QUESTION_PREFIX, "i", heading_text)
                && !re::test(NUMBERED_QUESTION_HEADING, "i", heading_text)
            {
                in_subsection = true;
                awaiting_numbered_question_text = false;
                continue;
            }
            in_plan_approval = false;
            awaiting_numbered_question_text =
                re::test(NUMBERED_QUESTION_HEADING, "i", heading_text);
            if awaiting_numbered_question_text {
                plan_approval_depth = depth;
            }
            continue;
        }
        if awaiting_numbered_question_text && !trim(line).is_empty() {
            awaiting_numbered_question_text = false;
            // The numbered form puts the label on the line after the heading,
            // so the section it opens has that heading's depth.
            if is_plan_approval_label(line) {
                in_plan_approval = true;
                in_subsection = false;
                section = opened();
            }
        }
        if !in_plan_approval {
            continue;
        }
        if let Some(answer) = re::find(ANSWER_TAG, "", line)
            && !(in_subsection && section.answer.is_some())
        {
            section.answer = Some(trim(re::group(line, &answer, 1).unwrap_or("")).to_owned());
            section.answer_line = Some(index);
        }
        if let Some(fingerprint) = re::find(FINGERPRINT_TAG, "", line)
            && !(in_subsection && section.fingerprint.is_some())
        {
            section.fingerprint = re::group(line, &fingerprint, 1).map(str::to_owned);
        }
        if let Some(planned) = re::find(PLANNED_SOURCE_TAG, "", line)
            && !(in_subsection && section.planned_source.is_some())
        {
            section.planned_source = re::group(line, &planned, 1).map(str::to_owned);
        }
    }
    section
}

/// `questionsFileApproved`: the latest Plan Approval answer is an explicit
/// "Approve Plan" (or the engine's own "Plan approval off").
#[must_use]
pub fn questions_file_approved(body: &str) -> bool {
    let latest = latest_plan_approval(body);
    latest.found
        && latest
            .answer
            .as_deref()
            .is_some_and(|answer| re::test(APPROVE_PLAN, "i", answer))
}

/// `questionsFileHasPendingPlanApproval`: the latest Plan Approval answer
/// is still the blank (`[Answer]:` alone or underscores).
#[must_use]
pub fn questions_file_has_pending_plan_approval(body: &str) -> bool {
    let latest = latest_plan_approval(body);
    latest.found
        && latest
            .answer
            .as_deref()
            .is_some_and(|answer| re::test(r"^_*$", "", answer))
}

/// The last line of the leaf block that starts at `index`.
fn block_end(blocks: &Blocks, index: usize) -> usize {
    let line = &blocks.lines[index];
    let mut last = index;
    while line.block.is_some()
        && blocks
            .lines
            .get(last + 1)
            .is_some_and(|next| next.kind == line.kind && next.block == line.block)
    {
        last += 1;
    }
    last
}

/// Whether the heading whose block starts at `index` is an h1 or an h2,
/// read from the source: an ATX heading by its marks; a setext heading is
/// always one or the other.
fn heading_is_h1_h2(blocks: &Blocks, source: &[String], index: usize) -> bool {
    let start = blocks.lines[index].content_start.min(source[index].len());
    let text = &source[index][start..];
    match re::find(r"^(#{1,6})(?:[ \t]|$)", "", text) {
        Some(found) => re::group(text, &found, 1).is_some_and(|marks| marks.len() <= 2),
        None => true,
    }
}

/// `renderedHtmlCarriesH1H2`: an HTML block carries an `<h1>` or `<h2>`
/// tag outside its comments and raw-text elements.
fn html_carries_h1_h2(html: &str) -> bool {
    let visible = re::replace(r"<!--[\s\S]*?(?:-->|$)", "g", html, "");
    let visible = re::replace(
        r"<(script|style|textarea|title|xmp|iframe|noembed|noframes|plaintext|template)\b[\s\S]*?(?:</\1\s*>|$)",
        "gi",
        &visible,
        "",
    );
    re::test(r"<h[12](?=[\s/>]|$)", "i", &visible)
}

/// `terminalReviewAppendixBoundary`: the byte offset where a plan's
/// terminal `## Review` appendix begins, when the document has one: an
/// exact `## Review` line at column 0 that is a level-2 heading, with no h1
/// or h2 (as Markdown, or in an HTML block) after it. The offset is the
/// trimmed boundary: the separator white space before the heading reduced
/// to one line end.
///
/// Upstream confirms the heading by rendering the document; this reads the
/// same structure from `focus-markdown` and each heading's level from its
/// own line.
#[must_use]
pub fn terminal_review_appendix_offset(body: &str) -> Option<usize> {
    let source = source_lines(body);
    // Line starts in the original text, which keeps its own line ends.
    let mut line_starts = vec![0usize];
    let bytes = body.as_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        if bytes[offset] == b'\r' {
            if bytes.get(offset + 1) == Some(&b'\n') {
                offset += 1;
            }
            line_starts.push(offset + 1);
        } else if bytes[offset] == b'\n' {
            line_starts.push(offset + 1);
        }
        offset += 1;
    }
    let candidates: Vec<(usize, usize)> = (0..line_starts.len())
        .filter_map(|line| {
            let start = line_starts[line];
            let end = line_starts.get(line + 1).copied().unwrap_or(body.len());
            let raw = body[start..end].trim_end_matches(['\r', '\n']);
            re::test(r"^## Review[ \t]*$", "", raw).then_some((line, start))
        })
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let blocks = analyse(body).ok()?;
    if blocks.lines.len() != source.len() || line_starts.len() != source.len() {
        return None;
    }
    // The candidate that is the document's last h1 or h2. A candidate line
    // is an ATX heading of level 2 whenever it is a heading at all, so the
    // last h1 or h2 either is one of them or rules them all out.
    let mut terminal: Option<usize> = None;
    let mut index = 0;
    while index < blocks.lines.len() {
        let last = block_end(&blocks, index);
        match blocks.lines[index].kind {
            LineKind::Heading if heading_is_h1_h2(&blocks, &source, index) => {
                terminal = candidates
                    .iter()
                    .position(|(candidate, _)| *candidate == index);
            }
            LineKind::HtmlFlow if html_carries_h1_h2(&source[index..=last].join("\n")) => {
                terminal = None;
            }
            _ => {}
        }
        index = last + 1;
    }
    let (_, heading_start) = candidates[terminal?];
    let prefix = &body[..heading_start];
    let Some(trailing) = re::find(r"\s+$", "", prefix) else {
        return Some(heading_start);
    };
    let content_end = trailing.start();
    let retained = re::find(r"^[ \t]*(?:\r\n|\n|\r)", "", &prefix[content_end..]);
    Some(content_end + retained.map_or(0, |found| found.end()))
}

/// `contentBeforeTerminalReviewAppendix`: the plan with a terminal
/// `## Review` appendix removed.
#[must_use]
pub fn content_before_terminal_review_appendix(body: &str) -> &str {
    match terminal_review_appendix_offset(body) {
        Some(offset) => &body[..offset],
        None => body,
    }
}

/// `planReviewAppendix`: the terminal `## Review` appendix of a plan, or
/// `""` when it carries none.
#[must_use]
pub fn plan_review_appendix(plan: &str) -> &str {
    &plan[content_before_terminal_review_appendix(plan).len()..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_markers_are_distinct_hashes() {
        let hash = "a".repeat(64);
        let text = format!(
            "FOCUS-TESTING-CONTRACT: sha256:{hash}\n  FOCUS-TESTING-CONTRACT :sha256:{hash}  \r\nFOCUS-TESTING-CONTRACT: sha256:{}\nFOCUS-TESTING-CONTRACT: nope\n",
            "b".repeat(64)
        );
        assert_eq!(
            prompt_testing_contract_markers(&text),
            vec![
                format!("sha256:{hash}"),
                format!("sha256:{}", "b".repeat(64))
            ]
        );
    }

    #[test]
    fn the_latest_plan_approval_section_is_read() {
        let body = "## Q1: Plan Approval\n\n[Answer]: Request Changes\n\n## Plan Approval\n\n[Approval Fingerprint]: sha256:v3:0000000000000000000000000000000000000000000000000000000000000000\n[Planned Source]: unbindable\n\n[Answer]: a) \"Approve Plan\"\n";
        let latest = latest_plan_approval(body);
        assert!(latest.found);
        assert_eq!(latest.answer.as_deref(), Some("a) \"Approve Plan\""));
        assert_eq!(latest.planned_source.as_deref(), Some("unbindable"));
        assert!(questions_file_approved(body));
        assert!(!questions_file_has_pending_plan_approval(body));
        // A deeper heading is a subsection whose tags only fill empty slots;
        // a numbered heading ends the section.
        let nested = "## Plan Approval\n\n[Answer]: ___\n\n### Notes\n\n[Answer]: Approve Plan\n";
        assert!(questions_file_has_pending_plan_approval(nested));
        assert!(questions_file_approved(
            "## Plan Approval\n\n### Notes\n\n[Answer]: Approve Plan\n"
        ));
        let next = "## Plan Approval\n\n### Q2\nOther\n\n[Answer]: Approve Plan\n";
        assert!(!questions_file_approved(next));
        let numbered = "## 3.\n**Plan Approval**\n\n[Answer]: Plan approval off\n";
        assert!(questions_file_approved(numbered));
        // The numbered form's section has the numbered heading's depth.
        assert!(questions_file_approved(
            "#### Plan Approval\n\n## 2.\nPlan Approval\n\n### Notes\n\n[Answer]: Approve Plan\n"
        ));
        // Emphasis marks alone are no label.
        assert!(!latest_plan_approval("## ****\n[Answer]: Approve Plan\n").found);
        // Inside a fence it is not a heading.
        assert!(!questions_file_approved(
            "```\n## Plan Approval\n[Answer]: Approve Plan\n```\n"
        ));
        assert!(!latest_plan_approval("## Approval\n[Answer]: Approve Plan\n").found);
    }

    #[test]
    fn the_terminal_review_appendix_is_the_last_h2() {
        let plan = "# Plan\n\nbody\n\n\n## Review\n\nverdict\n";
        assert_eq!(plan_review_appendix(plan), "\n\n## Review\n\nverdict\n");
        assert_eq!(
            content_before_terminal_review_appendix(plan),
            "# Plan\n\nbody\n"
        );
        assert_eq!(
            plan_review_appendix("# Plan\n\n## Review\n\n## After\n"),
            ""
        );
        assert_eq!(plan_review_appendix("# Plan\n\n## Review\n\nB\n--\n"), "");
        assert_eq!(plan_review_appendix("# Plan\n\n## Review\n\nB\n==\n"), "");
        assert_eq!(
            plan_review_appendix("# Plan\n\n## Review\n\n### C\n"),
            "\n## Review\n\n### C\n"
        );
        assert_eq!(
            plan_review_appendix("# Plan\n\n## Review\n\n<h2>z</h2>\n"),
            ""
        );
        assert_eq!(
            plan_review_appendix("# Plan\n\n## Review\n\n<div><h20>z</h20></div>\n"),
            "\n## Review\n\n<div><h20>z</h20></div>\n"
        );
        assert_eq!(
            plan_review_appendix("# Plan\n\n## Review\n\n<!-- <h1>z</h1> -->\n"),
            "\n## Review\n\n<!-- <h1>z</h1> -->\n"
        );
        assert_eq!(plan_review_appendix("```\n## Review\n```\n"), "");
        assert_eq!(plan_review_appendix("## Review\n"), "## Review\n");
        assert_eq!(plan_review_appendix("text\n## Review\n"), "## Review\n");
        assert_eq!(
            plan_review_appendix("x\r\n\r\n## Review\r\n"),
            "\r\n## Review\r\n"
        );
        assert_eq!(
            content_before_terminal_review_appendix("x\r\n\r\n## Review\r\n"),
            "x\r\n"
        );
        assert_eq!(plan_review_appendix("## Review ##\n"), "");
    }
}

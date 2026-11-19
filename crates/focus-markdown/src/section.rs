//! A section of a document, taken by its heading as upstream takes it
//! (`extractMarkdownSection`, `aidlc-lib.ts` L40 466).
//!
//! This is the reader upstream calls "pure string operations against
//! well-formed markdown": the heading is found by pattern on the lines as
//! written, after the lines of fenced code are blanked, and the section
//! runs to the next `## ` line. A comment stays part of what is returned,
//! and a heading inside one is a heading. It is ported as it reads, for the
//! callers upstream gives it; nothing new should be written on it (see
//! [`crate::analysis`] for a document read as its reader sees it).

use focus_js::re;

/// `stripFencedCodeBlocks`: the text with the lines of fenced code, and the
/// fences, made empty lines. A fence is a line that starts with three
/// backticks, and nothing else.
#[must_use]
pub fn strip_fenced_code_blocks(content: &str) -> String {
    let mut in_fence = false;
    let lines: Vec<&str> = content
        .split('\n')
        .map(|line| {
            if line.starts_with("```") {
                in_fence = !in_fence;
                ""
            } else if in_fence {
                ""
            } else {
                line
            }
        })
        .collect();
    lines.join("\n")
}

/// `extractMarkdownSection`: what stands between the line `heading` (its
/// full form, `## Review`) and the next `## ` line, or the end. Empty when
/// the heading is not there; the first one counts when it is there twice.
#[must_use]
pub fn extract_markdown_section(content: &str, heading: &str) -> String {
    let stripped = strip_fenced_code_blocks(content);
    let line = format!("^{}[ \\t]*$", re::escape(heading));
    let Some(found) = re::find(&line, "m", &stripped) else {
        return String::new();
    };
    let after = found.end();
    let start = if stripped[after..].starts_with('\n') {
        after + 1
    } else {
        after
    };
    let rest = &stripped[start..];
    let end = re::find(r"^## [^\n]*$", "m", rest).map_or(rest.len(), |next| next.start());
    rest[..end].to_owned()
}

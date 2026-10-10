//! Where upstream's analysis looks.
//!
//! Upstream finds a line's block by inserting a probe word into the line and
//! seeing where it renders. A line gets one probe where a word cannot change
//! the block structure, and more inside the constructs it wants columns for:
//! after each `<!--`, after each backtick run, inside a tag that opens the
//! line, and inside every HTML heading tag.
//!
//! This crate reads structure from a parser and needs no probes to place a
//! line. It needs to know where they would be for one reason: upstream
//! records an inline raw HTML tag only when a probe happens to land inside
//! it, and a tag it did not record hides nothing. Which tags count is
//! therefore decided by this placement, and to agree with upstream on what
//! is visible the placement has to be the same. It is lexical, so it ports
//! exactly.
//!
//! Mirrors `markdownProbeColumn`, `markdownFallbackProbeColumn` and
//! `markdownProbes` in `aidlc-lib.ts` (L40665 to L40790).

use focus_js::re;
use focus_js::text::{trim, trim_end};

pub(crate) const CONTAINER_PREFIX: &str =
    r"^(?:[ \t]{0,3}(?:>[ \t]?|(?:[-*+]|\d{1,9}[.)])(?:[ \t]+|$)))*[ \t]*";
pub(crate) const FENCE: &str = r"^(`{3,}|~{3,})";
pub(crate) const SETEXT_UNDERLINE: &str = r"^(?:=+|-+)[ \t]*$";

/// Block syntax whose shape a probe word would change: rules, setext
/// underlines, fences and table delimiter rows.
const STRUCTURE_LINE: &str = r"^(?:(?:\*[ \t]*){3,}|(?:-[ \t]*){3,}|(?:_[ \t]*){3,}|=+[ \t]*|-+[ \t]*|`{3,}.*|~{3,}.*|\|?[ \t]*:?-+:?[ \t]*(?:\|[ \t]*:?-+:?[ \t]*)*\|?[ \t]*)$";

/// The length of what `pattern` matches at the start of `text`; zero when it
/// does not match.
pub(crate) fn prefix_len(pattern: &str, text: &str) -> usize {
    re::find(pattern, "", text).map_or(0, |found| found.end())
}

fn letter_or_digit(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric();
    }
    let mut buffer = [0_u8; 4];
    re::test(r"[\p{L}\p{N}]", "u", c.encode_utf8(&mut buffer))
}

/// The column where a probe word cannot change block structure: after the
/// first letter or digit outside container markers, tags, entities and
/// escapes. A line whose text is all inside brackets takes it after a closing
/// bracket, then inside a label, then inside a link destination.
fn probe_column(line: &str, from: usize) -> Option<usize> {
    let (mut in_tag, mut in_entity) = (false, false);
    let (mut brackets, mut destination) = (0_usize, 0_usize);
    let (mut after_label, mut in_label, mut in_destination) = (None, None, None);
    let char_at = |index: usize| line[index..].chars().next();
    let mut index = from;
    while let Some(character) = char_at(index) {
        let next = index + character.len_utf8();
        if in_tag {
            if character == '>' {
                in_tag = false;
            }
        } else if character == '<' {
            in_tag = true;
        } else if character == '\\' {
            index = match char_at(next) {
                Some(escaped) => next + escaped.len_utf8(),
                None => next,
            };
            continue;
        } else if character == '&' {
            in_entity = true;
        } else if in_entity && (character.is_ascii_alphanumeric() || character == '#') {
            // An entity name is not source text.
        } else {
            in_entity = false;
            if destination > 0 {
                if character == '(' {
                    destination += 1;
                } else if character == ')' {
                    destination -= 1;
                } else if in_destination.is_none() && letter_or_digit(character) {
                    in_destination = Some(next);
                }
            } else if character == '[' {
                brackets += 1;
            } else if character == ']' {
                brackets = brackets.saturating_sub(1);
                if char_at(next) == Some('(') {
                    destination = 1;
                    index = next + 1;
                    continue;
                }
                if brackets == 0 && char_at(next).is_none_or(|c| c == ' ' || c == '\t') {
                    after_label.get_or_insert(next);
                }
            } else if letter_or_digit(character) {
                if brackets == 0 {
                    return Some(next);
                }
                in_label.get_or_insert(next);
            }
        }
        index = next;
    }
    after_label.or(in_label).or(in_destination)
}

/// Where a line with no letter or digit outside markup takes its probe: at
/// its content, inside a table row's first cell, or after a closing tag.
fn fallback_column(line: &str, start: usize) -> Option<usize> {
    let content = trim_end(&line[start..]);
    if content.is_empty() || re::test(STRUCTURE_LINE, "", content) {
        return None;
    }
    if content.starts_with('|') {
        return Some(start + prefix_len(r"^\|[ \t]*", content));
    }
    if content.starts_with('<') {
        return Some(start + content.len());
    }
    Some(start)
}

/// The byte columns of every probe upstream puts on each line.
pub(crate) fn probe_columns(source: &[String]) -> Vec<Vec<usize>> {
    let mut all = Vec::with_capacity(source.len());
    for (index, line) in source.iter().enumerate() {
        let mut columns = Vec::new();
        let start = prefix_len(CONTAINER_PREFIX, line);
        let rest = &line[start..];
        let atx = re::find(r"^#{1,6}(?=[ \t]|$)[ \t]*", "", rest);
        let opening = re::find(
            r"^<(?:([A-Za-z][A-Za-z0-9-]*)(?=[\s/>]|$)|\?|![A-Za-z]|!\[CDATA\[)",
            "",
            rest,
        );
        if let Some(atx) = &atx {
            columns.push(start + atx.end());
        } else if let Some(column) = probe_column(line, start) {
            columns.push(column);
        } else {
            let next_is_underline = source.get(index + 1).is_some_and(|next| {
                let quoted = prefix_len(r"^(?:[ \t]{0,3}>[ \t]?)*[ \t]{0,3}", next);
                re::test(SETEXT_UNDERLINE, "", &next[quoted..])
            });
            if !trim(line).is_empty() && next_is_underline {
                columns.push(trim_end(line).len());
            } else if opening.is_none()
                && let Some(column) = fallback_column(line, start)
            {
                columns.push(column);
            }
        }
        let mut offset = 0;
        while let Some(found) = line[offset..].find("<!--") {
            let at = offset + found;
            // `<!-->` and `<!--->` are complete comments a probe would reopen.
            if !re::test(r"^<!---?>", "", &line[at..]) {
                columns.push(at + 4);
            }
            offset = at + 4;
        }
        if !re::test(FENCE, "", rest) {
            let runs = re::compile("`+ ?", "");
            columns.extend(runs.find_iter(line).map(|run| run.end()));
        }
        if let Some(opening) = &opening {
            columns.push(start + opening.end());
        }
        let headings = re::compile(r"<h[1-6](?=[\s/>]|$)", "i");
        for tag in headings.find_iter(line) {
            if tag.start() != start {
                columns.push(tag.end());
            }
        }
        all.push(columns);
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(lines: &[&str]) -> Vec<Vec<usize>> {
        probe_columns(
            &lines
                .iter()
                .map(|line| (*line).to_owned())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn a_line_is_probed_after_its_first_letter_outside_markup() {
        assert_eq!(columns(&["plain text"]), [[1]]);
        assert_eq!(columns(&["> - **bold** text"]), [[7]]);
        assert_eq!(columns(&["<b>tag</b> after"]), [[4, 2]]);
        assert_eq!(columns(&["&amp; \\x y"]), [[10]]);
        assert_eq!(columns(&["## Heading"]), [[3]]);
        assert_eq!(columns(&["[label]: /url"]), [[11]]);
        assert_eq!(columns(&["[only] "]), [[6]]);
    }

    #[test]
    fn comments_code_and_heading_tags_get_probes_of_their_own() {
        assert_eq!(columns(&["a <!-- b --> c"]), [[1, 6]]);
        assert_eq!(columns(&["<!-->"]), [[5]]);
        assert_eq!(columns(&["a `b` c"]), [[1, 3, 6]]);
        assert_eq!(columns(&["x <h2 class=y>"]), [[1, 5]]);
        assert_eq!(
            columns(&["---", "***", "```rust"]),
            [vec![], vec![], vec![4]]
        );
        assert_eq!(columns(&["!!!", "==="]), [vec![3], vec![]]);
        assert_eq!(columns(&["| ! | ? |"]), [[2]]);
    }
}

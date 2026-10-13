//! A stage's diary, counted.
//!
//! Each stage keeps a `memory.md`: what the agent interpreted, where it
//! deviated, which trade-offs it made and what it left open, one entry per
//! line under four headings. The runtime graph records how many entries each
//! heading holds, and a stage approved with none is an event in itself.
//!
//! Mirrors `parseMemoryHeadings` in `aidlc-lib.ts` (L36920).

use std::path::{Path, PathBuf};

use focus_js::re;
use focus_js::text::trim;
use focus_workspace::project::joined;

/// How many entries a diary holds under each of its four headings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemoryCounts {
    pub interpretations: u64,
    pub deviations: u64,
    pub tradeoffs: u64,
    pub open_questions: u64,
}

impl MemoryCounts {
    /// Every entry, under whichever heading.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.interpretations + self.deviations + self.tradeoffs + self.open_questions
    }
}

/// Where the diary of the stage `slug` of `phase` is, under the record's
/// files: `<record>/<phase>/<slug>/memory.md`, put together as upstream's
/// `join` does (`memoryFilePath`). The phase and the slug come from the
/// stage graph, and a stage graph is a file someone may have edited: a
/// phase that looks absolute still goes under the record, and `..` is
/// settled by reading the path.
#[must_use]
pub fn diary_path(record_files: &Path, phase: &str, slug: &str) -> PathBuf {
    joined(record_files, &[phase, slug, "memory.md"])
}

/// What a line that is one of these exactly does to the count. Upstream
/// looks a line up in a plain object with `in`, so the name of anything
/// every object inherits is found there too, and what it finds is no
/// heading: the entries after such a line are counted nowhere, until the
/// next heading. A line that says `constructor` and nothing else is rare
/// enough, but the count is compared byte for byte.
const INHERITED: [&str; 12] = [
    "constructor",
    "__proto__",
    "hasOwnProperty",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toLocaleString",
    "toString",
    "valueOf",
    "__defineGetter__",
    "__defineSetter__",
    "__lookupGetter__",
    "__lookupSetter__",
];

/// Count the entries of a diary.
///
/// An entry is a line with something on it, under one of the four headings
/// written exactly (`## Interpretations`, `## Deviations`, `## Tradeoffs`,
/// `## Open questions`), that is not a quotation, not a comment closed on its
/// own line, and not inside a code fence. Any other second-level heading
/// ends the section. A heading that is missing counts as zero: three of four
/// written under pressure is no reason to fail.
#[must_use]
pub fn parse_memory_headings(raw: &str) -> MemoryCounts {
    let normalized = raw
        .strip_prefix('\u{feff}')
        .unwrap_or(raw)
        .replace("\r\n", "\n");
    let mut counts = MemoryCounts::default();
    let mut current: Option<fn(&mut MemoryCounts) -> &mut u64> = None;
    let mut in_fence = false;
    for line in normalized.split('\n') {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let heading: Option<fn(&mut MemoryCounts) -> &mut u64> = match line {
            "## Interpretations" => Some(|counts| &mut counts.interpretations),
            "## Deviations" => Some(|counts| &mut counts.deviations),
            "## Tradeoffs" => Some(|counts| &mut counts.tradeoffs),
            "## Open questions" => Some(|counts| &mut counts.open_questions),
            _ => None,
        };
        if heading.is_some() {
            current = heading;
            continue;
        }
        if INHERITED.contains(&line) || line.starts_with("## ") {
            current = None;
            continue;
        }
        let Some(counted) = current else {
            continue;
        };
        let trimmed = trim(line);
        if trimmed.is_empty()
            || trimmed.starts_with('>')
            || re::test(r"^<!--.*-->\s*$", "", trimmed)
        {
            continue;
        }
        *counted(&mut counts) += 1;
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    // The paths are a Unix machine's.
    #[cfg(unix)]
    #[test]
    fn a_diary_path_is_joined_as_upstream_joins_it() {
        let record = Path::new("/project/focus/intents/one");
        let at = |phase: &str, slug: &str| diary_path(record, phase, slug);
        assert_eq!(
            at("inception", "requirements-analysis"),
            Path::new("/project/focus/intents/one/inception/requirements-analysis/memory.md")
        );
        // A phase that looks absolute does not start over at the root.
        assert_eq!(
            at("/abs", "stage"),
            Path::new("/project/focus/intents/one/abs/stage/memory.md")
        );
        // `..` is settled by reading: it takes back the part before it.
        assert_eq!(
            at("inception", ".."),
            Path::new("/project/focus/intents/one/memory.md")
        );
        assert_eq!(at("..", ".."), Path::new("/project/focus/memory.md"));
        assert_eq!(
            at("a/./b//c", "."),
            Path::new("/project/focus/intents/one/a/b/c/memory.md")
        );
    }

    #[test]
    fn entries_are_counted_under_their_headings() {
        let diary = "\u{feff}# Diary\r\n\r\n## Interpretations\r\n- one\r\n\r\n> quoted\r\n<!-- hidden -->\r\n- two\r\n```\r\n- in a fence\r\n## Deviations\r\n```\r\n- three\r\n## Notes\r\n- not counted\r\n## Open questions\r\n  - four  \r\n## Tradeoffs\r\n";
        let counts = parse_memory_headings(diary);
        assert_eq!(
            counts,
            MemoryCounts {
                interpretations: 3,
                deviations: 0,
                tradeoffs: 0,
                open_questions: 1
            }
        );
        assert_eq!(counts.total(), 4);
    }

    #[test]
    fn a_heading_is_one_only_when_written_exactly() {
        // A trailing space, another case, or a deeper level: none is the
        // heading, and the first two end the section as headings of their own.
        let counts = parse_memory_headings(
            "## Deviations\n- a\n## Deviations \n- b\n## deviations\n- c\n## Deviations\n### Deviations\n- d\n",
        );
        assert_eq!(counts.deviations, 3);
        // A comment that goes on is an entry, line by line.
        assert_eq!(
            parse_memory_headings("## Tradeoffs\n<!-- a\nb -->\n").tradeoffs,
            2
        );
    }

    #[test]
    fn a_line_naming_what_every_object_inherits_ends_the_section() {
        let counts = parse_memory_headings(
            "## Deviations\n- a\nconstructor\n- lost\n- lost too\n## Tradeoffs\n- b\n",
        );
        assert_eq!((counts.deviations, counts.tradeoffs), (1, 1));
        // Only when it is the whole line.
        assert_eq!(
            parse_memory_headings("## Deviations\n constructor\ntoString()\n").deviations,
            2
        );
    }
}

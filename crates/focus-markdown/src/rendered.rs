//! A document as a reader of its rendering takes it for authority: what its
//! top-level paragraphs say in plain words and in bold, its headings, and
//! its blocks of HTML.
//!
//! A review states its verdict, its reviewer and its pass in lines of its
//! own (`**Verdict:** READY`). Only a line a reader sees as such a line
//! counts: one in a paragraph of the document itself, not in a quote, a
//! list, a table, a code block or a block of HTML, and with its label in
//! bold and nothing else. Upstream decides that by rendering the review
//! with its runtime's Markdown renderer and callbacks that write marks
//! (`renderReviewMarkdownAuthority`, `aidlc-lib.ts` L16 259): a bold run
//! comes out between U+0001 and U+0002, and everything that is not plain
//! text or bold (a heading, code, a rule, emphasis, a link, an image) as
//! U+0003, so a pattern anchored at the start of a rendered line finds the
//! lines and nothing else. This writes the same text from the structure the
//! parser reads.
//!
//! What the renderer upstream uses does with each construct was asked of
//! it, construct by construct, and is held in the oracle corpus
//! (`review_text`): a line break of either kind ends the rendered line;
//! entities and escapes are decoded; raw HTML inside a paragraph is plain
//! text; a bare address is not a link; U+0000 reads U+FFFD; one byte order
//! mark at the very start is not part of the text.

use focus_js::re;
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use crate::analysis::contained;

/// A bold run opens and closes.
pub const MARK_OPEN: char = '\u{1}';
pub const MARK_CLOSE: char = '\u{2}';
/// Something that is neither plain text nor bold.
pub const NON_AUTHORITY: char = '\u{3}';

/// `renderedHtmlCarriesH1H2`: a block of HTML carries an `<h1>` or `<h2>`
/// tag outside its comments and raw-text elements.
#[must_use]
pub fn html_carries_h1_h2(html: &str) -> bool {
    let visible = re::replace(r"<!--[\s\S]*?(?:-->|$)", "g", html, "");
    let visible = re::replace(
        r"<(script|style|textarea|title|xmp|iframe|noembed|noframes|plaintext|template)\b[\s\S]*?(?:</\1\s*>|$)",
        "gi",
        &visible,
        "",
    );
    re::test(r"<h[12](?=[\s/>]|$)", "i", &visible)
}

/// A heading, with its content as rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    pub level: u8,
    pub children: String,
}

/// What [`render`] gives.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rendered {
    /// The rendered text: a line for each line of a top-level paragraph,
    /// and a line holding U+0003 for each other block that shows.
    pub text: String,
    /// Every heading, in order, wherever it sits.
    pub headings: Vec<Heading>,
    /// Every block of HTML, as written, wherever it sits.
    pub html_blocks: Vec<String>,
}

/// `escapeReviewRendererText`: text that carries one of the three marks
/// says so in letters, so it can never be taken for a mark.
fn escaped(text: &str, restore: &[(char, char)], out: &mut String) {
    for character in text.chars() {
        let character = restore
            .iter()
            .find(|(stand_in, _)| *stand_in == character)
            .map_or(character, |(_, original)| *original);
        match character {
            '\u{1}'..='\u{3}' => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            other => out.push(other),
        }
    }
}

/// The section as the parser is to see it, and the characters that stand
/// in for others in it. As in the analysis: U+0000 reads U+FFFD; a line of
/// spaces and tabs is an empty line; and vertical tab and form feed, which
/// the parser takes for white space and CommonMark does not, are given to
/// it as characters the text does not use, and put back in what is
/// rendered.
fn prepared(section: &str) -> (String, Vec<(char, char)>) {
    let mut unused = ('\u{e000}'..='\u{f8ff}').filter(|candidate| !section.contains(*candidate));
    let mut restore = Vec::new();
    for original in ['\u{b}', '\u{c}'] {
        if section.contains(original)
            && let Some(stand_in) = unused.next()
        {
            restore.push((stand_in, original));
        }
    }
    let lines: Vec<String> = section
        .split('\n')
        .map(|line| {
            if line.bytes().all(|byte| byte == b' ' || byte == b'\t') {
                return String::new();
            }
            line.chars()
                .map(|character| match character {
                    '\0' => '\u{fffd}',
                    other => restore
                        .iter()
                        .find(|(_, original)| *original == other)
                        .map_or(other, |(stand_in, _)| *stand_in),
                })
                .collect()
        })
        .collect();
    (lines.join("\n"), restore)
}

/// A block that shows as U+0003, on a line of its own when it ends one;
/// nothing inside a quote, a list or a table.
fn block(text: &mut String, hidden: usize, ends_line: bool) {
    if hidden == 0 {
        text.push(NON_AUTHORITY);
        if ends_line {
            text.push('\n');
        }
    }
}

/// The rendering of `section` (with `\n` line ends) that a review's
/// authority is read from. `None` when the parser fails on it.
#[must_use]
pub fn render(section: &str) -> Option<Rendered> {
    // (The renderer upstream uses takes one leading byte order mark for
    // itself.)
    let section = section.strip_prefix('\u{feff}').unwrap_or(section);
    let (text, restore) = prepared(section);
    contained(|| walk(&text, &restore))
}

#[allow(clippy::too_many_lines)]
fn walk(text: &str, restore: &[(char, char)]) -> Rendered {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let mut rendered = Rendered::default();
    // How deep inside a quote, a list or a table: no text there shows.
    let mut hidden = 0usize;
    // The inline content being rendered, innermost last; empty outside a
    // heading and a paragraph.
    let mut inline: Vec<String> = Vec::new();
    let mut html: Option<String> = None;
    let mut in_code = false;
    for event in Parser::new_ext(text, options) {
        match event {
            Event::Start(
                Tag::BlockQuote(_)
                | Tag::List(_)
                | Tag::Table(_)
                | Tag::FootnoteDefinition(_)
                | Tag::DefinitionList
                | Tag::MetadataBlock(_),
            ) => hidden += 1,
            Event::End(
                TagEnd::BlockQuote(_)
                | TagEnd::List(_)
                | TagEnd::Table
                | TagEnd::FootnoteDefinition
                | TagEnd::DefinitionList
                | TagEnd::MetadataBlock(_),
            ) => hidden = hidden.saturating_sub(1),
            Event::Start(Tag::Heading { .. } | Tag::Paragraph) => inline.push(String::new()),
            Event::End(TagEnd::Heading(level)) => {
                // A heading is one wherever it sits: the renderer's callback
                // is called for it inside a quote or a list too.
                rendered.headings.push(Heading {
                    level: level as u8,
                    children: inline.pop().unwrap_or_default(),
                });
                inline.clear();
                block(&mut rendered.text, hidden, true);
            }
            Event::End(TagEnd::Paragraph) => {
                let children = inline.pop().unwrap_or_default();
                inline.clear();
                if hidden == 0 {
                    rendered.text.push_str(&children);
                    rendered.text.push('\n');
                }
            }
            Event::Start(Tag::HtmlBlock) => html = Some(String::new()),
            Event::End(TagEnd::HtmlBlock) => {
                let raw = html.take().unwrap_or_default();
                block(&mut rendered.text, hidden, raw.ends_with('\n'));
                rendered.html_blocks.push(raw);
            }
            Event::Start(Tag::CodeBlock(_)) => in_code = true,
            Event::End(TagEnd::CodeBlock) => {
                in_code = false;
                block(&mut rendered.text, hidden, true);
            }
            Event::Rule => block(&mut rendered.text, hidden, true),
            Event::Html(raw) | Event::Text(raw) if html.is_some() => {
                if let Some(held) = &mut html {
                    held.push_str(&raw);
                }
            }
            _ if in_code || inline.is_empty() => {}
            // Raw HTML inside a paragraph is handed to the renderer's text
            // callback: it is plain text.
            Event::Text(raw) | Event::InlineHtml(raw) | Event::Html(raw) => {
                if let Some(current) = inline.last_mut() {
                    escaped(&raw, restore, current);
                }
            }
            Event::Code(_) => {
                if let Some(current) = inline.last_mut() {
                    current.push(NON_AUTHORITY);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(current) = inline.last_mut() {
                    current.push('\n');
                }
            }
            Event::Start(
                Tag::Strong
                | Tag::Emphasis
                | Tag::Strikethrough
                | Tag::Link { .. }
                | Tag::Image { .. },
            ) => inline.push(String::new()),
            Event::End(TagEnd::Strong) => {
                let children = inline.pop().unwrap_or_default();
                if let Some(current) = inline.last_mut() {
                    current.push(MARK_OPEN);
                    current.push_str(&children);
                    current.push(MARK_CLOSE);
                }
            }
            Event::End(TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link) => {
                let children = inline.pop().unwrap_or_default();
                if let Some(current) = inline.last_mut() {
                    current.push(NON_AUTHORITY);
                    current.push_str(&children);
                    current.push(NON_AUTHORITY);
                }
            }
            Event::End(TagEnd::Image) => {
                inline.pop();
                if let Some(current) = inline.last_mut() {
                    current.push(NON_AUTHORITY);
                }
            }
            _ => {}
        }
    }
    rendered
}

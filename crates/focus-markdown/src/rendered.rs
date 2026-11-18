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
//! comes out between U+0001 and U+0002, and everything else that is not
//! plain text (code, emphasis, struck text, a link, an image) as U+0003, so
//! a pattern anchored at the start of a rendered line finds the lines and
//! nothing else. This writes the same lines from the structure the parser
//! reads, struck text apart.
//!
//! # Struck text is not read
//!
//! The renderer upstream uses strikes text between runs of one or two
//! tildes that flank it as `*` flanks emphasis, inside a word too. The
//! parser asks more of a single tilde, and loses a pair of tildes that has
//! an unmatched single one between them, so its reading cannot be made the
//! renderer's from outside it. A tilde is therefore plain text here, which
//! changes no line this module's consumer finds: a field's label is bold
//! closed with nothing between its marks, so no struck text undoes it, and
//! a line that struck text opens starts with a tilde in place of U+0003,
//! neither of which is a label. What does change is a value that holds a
//! tilde, which reads `a~b~c` here and `a`, U+0003, `b`, U+0003, `c`
//! upstream. The consumer refuses to compare such a value with one it could
//! equal either way (a deliberate deviation, declared there).
//!
//! # What upstream renders and this does not
//!
//! Upstream also writes a line holding U+0003 for each heading, code block,
//! rule and block of HTML. No pattern finds a line that does not start
//! with U+0001, and such a line never joins another: the renderer hands
//! the callback a block of HTML that always ends in a line end, the last
//! line of a document without one included. So the lines are left out, and
//! with them any way of being wrong about them.
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
    /// The rendered text: a line for each line of a top-level paragraph.
    pub text: String,
    /// Every heading, in order, wherever it sits.
    pub headings: Vec<Heading>,
    /// Every block of HTML, as written, wherever it sits.
    pub html_blocks: Vec<String>,
}

/// What stands in for what in the text the parser is given, so that it
/// reads the text as the renderer upstream uses does.
struct StandIns {
    /// A stand-in and the character it stands for.
    swapped: Vec<(char, char)>,
}

/// White space to the renderer upstream uses, where blocks are concerned
/// characters like any other: the parser takes them for white space in both.
/// Each is given to it as a space character no block is made of.
const INLINE_SPACE: [char; 2] = ['\u{b}', '\u{c}'];
/// White space to the parser where it reads emphasis, and to the renderer
/// upstream uses characters like any other.
const NOT_SPACE: [char; 3] = ['\u{85}', '\u{2028}', '\u{2029}'];

impl StandIns {
    fn of(section: &str) -> Self {
        let unused = |candidate: &char| !section.contains(*candidate);
        let mut private = ('\u{e000}'..='\u{f8ff}').filter(unused);
        let mut spaces = ('\u{2000}'..='\u{200a}').filter(unused);
        let mut swapped = Vec::new();
        for original in INLINE_SPACE {
            if section.contains(original)
                && let Some(stand_in) = spaces.next().or_else(|| private.next())
            {
                swapped.push((stand_in, original));
            }
        }
        for original in NOT_SPACE {
            if section.contains(original)
                && let Some(stand_in) = private.next()
            {
                swapped.push((stand_in, original));
            }
        }
        Self { swapped }
    }

    /// A line as the parser is to see it. U+0000 reads U+FFFD, and a line
    /// of spaces and tabs is an empty line: the parser panics on one that
    /// follows a list item holding only a link reference definition.
    fn line(&self, line: &str) -> String {
        if line.bytes().all(|byte| byte == b' ' || byte == b'\t') {
            return String::new();
        }
        line.chars()
            .map(|character| match character {
                '\0' => '\u{fffd}',
                other => self
                    .swapped
                    .iter()
                    .find(|(_, original)| *original == other)
                    .map_or(other, |(stand_in, _)| *stand_in),
            })
            .collect()
    }

    /// What the parser gave back, as it was written.
    fn restored(&self, text: &str) -> String {
        text.chars()
            .map(|character| {
                self.swapped
                    .iter()
                    .find(|(stand_in, _)| *stand_in == character)
                    .map_or(character, |(_, original)| *original)
            })
            .collect()
    }
}

/// `escapeReviewRendererText`: text that carries one of the three marks
/// says so in letters, so it can never be taken for a mark.
fn escaped(text: &str, out: &mut String) {
    for character in text.chars() {
        match character {
            '\u{1}'..='\u{3}' => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            other => out.push(other),
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
    let stand_ins = StandIns::of(section);
    let lines: Vec<String> = section
        .split('\n')
        .map(|line| stand_ins.line(line))
        .collect();
    let text = lines.join("\n");
    contained(|| walk(&text, &stand_ins))
}

#[allow(clippy::too_many_lines)]
fn walk(text: &str, stand_ins: &StandIns) -> Rendered {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    let mut rendered = Rendered::default();
    // How deep inside a quote or a list: no paragraph there shows. (A
    // table's cells hold no paragraph, and a tight list's items none.)
    let mut hidden = 0usize;
    // The inline content being rendered, innermost last; empty outside a
    // heading and a paragraph.
    let mut inline: Vec<String> = Vec::new();
    let mut html: Option<String> = None;
    for event in Parser::new_ext(text, options) {
        match event {
            Event::Start(Tag::BlockQuote(_) | Tag::List(_)) => hidden += 1,
            Event::End(TagEnd::BlockQuote(_) | TagEnd::List(_)) => {
                hidden = hidden.saturating_sub(1);
            }
            Event::Start(Tag::Heading { .. } | Tag::Paragraph) => inline.push(String::new()),
            Event::End(TagEnd::Heading(level)) => {
                // A heading is one wherever it sits: the renderer's callback
                // is called for it inside a quote or a list too.
                rendered.headings.push(Heading {
                    level: level as u8,
                    children: inline.pop().unwrap_or_default(),
                });
            }
            Event::End(TagEnd::Paragraph) => {
                let children = inline.pop().unwrap_or_default();
                if hidden == 0 {
                    rendered.text.push_str(&children);
                    rendered.text.push('\n');
                }
            }
            Event::Start(Tag::HtmlBlock) => html = Some(String::new()),
            Event::End(TagEnd::HtmlBlock) => {
                let raw = html.take().unwrap_or_default();
                rendered.html_blocks.push(stand_ins.restored(&raw));
            }
            Event::Html(raw) if html.is_some() => {
                if let Some(held) = &mut html {
                    held.push_str(&raw);
                }
            }
            // From here on, inline content: rendered into the paragraph or
            // the heading that holds it, and nowhere when none does (a code
            // block's text, a table's cells, the items of a tight list).
            // Raw HTML inside a paragraph is handed to the renderer's text
            // callback: it is plain text.
            Event::Text(raw) | Event::InlineHtml(raw) | Event::Html(raw) => {
                if let Some(current) = inline.last_mut() {
                    escaped(&stand_ins.restored(&raw), current);
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
            Event::Start(Tag::Strong | Tag::Emphasis | Tag::Link { .. } | Tag::Image { .. }) => {
                inline.push(String::new())
            }
            Event::End(TagEnd::Strong) => {
                let children = inline.pop().unwrap_or_default();
                if let Some(current) = inline.last_mut() {
                    current.push(MARK_OPEN);
                    current.push_str(&children);
                    current.push(MARK_CLOSE);
                }
            }
            Event::End(TagEnd::Emphasis | TagEnd::Link) => {
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

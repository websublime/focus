//! Reading a POSIX command line as words and as segments.
//!
//! Mirrors `shellWords`, `shellCommandSegments`, `shellWordAt` and
//! `withoutRedirections` in `review-freeze-command.ts`. Characters are read
//! one at a time as the original reads code units; a special character is
//! always ASCII, so the two readings agree.

use focus_js::re;
use focus_js::text::is_js_whitespace;

/// `/\s/.test(ch)`.
pub(crate) fn blank(ch: char) -> bool {
    is_js_whitespace(ch)
}

fn operator(ch: char) -> bool {
    ";|&()<>".contains(ch)
}

/// Whether the next character is one a backslash escapes inside `"…"`:
/// `$`, `` ` ``, `"`, `\` and newline. A backslash before anything else is
/// kept, so a quoted Windows path keeps its separators.
fn double_quote_escapes(next: Option<char>) -> bool {
    next.is_some_and(|next| "$`\"\\\n".contains(next))
}

fn rest_from(chars: &[char], at: usize) -> String {
    chars[at..].iter().collect()
}

/// `shellWords`: the words of a segment, quotes and escapes removed, with
/// descriptor redirections (`2>&1`, `>&-`) dropped as syntax.
#[must_use]
pub fn shell_words(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    let mut word_started = false;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let push = |words: &mut Vec<String>, word: &mut String, word_started: &mut bool| {
        if *word_started {
            words.push(std::mem::take(word));
        } else {
            word.clear();
        }
        *word_started = false;
    };
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if escaped {
            word.push(ch);
            word_started = true;
            escaped = false;
            i += 1;
            continue;
        }
        if ch == '\\' && quote == Some('"') && !double_quote_escapes(chars.get(i + 1).copied()) {
            word.push(ch);
            i += 1;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            word_started = true;
            i += 1;
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            } else {
                word.push(ch);
            }
            i += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            word_started = true;
            i += 1;
            continue;
        }
        if ch == '<' || ch == '>' || (!word_started && ch.is_ascii_digit()) {
            // Shell descriptors are syntax, not argv. In particular, keeping
            // a trailing "2" or "1" would change a mutator's apparent
            // destination.
            let rest = rest_from(&chars, i);
            if let Some(found) =
                re::find(r"^\d*[<>]&[ \t]*(?:\d+|-)(?=$|[ \t\n;|&()<>])", "", &rest)
            {
                push(&mut words, &mut word, &mut word_started);
                i += rest[found.range()].chars().count();
                continue;
            }
        }
        if blank(ch) || operator(ch) {
            push(&mut words, &mut word, &mut word_started);
            i += 1;
            continue;
        }
        word.push(ch);
        word_started = true;
        i += 1;
    }
    push(&mut words, &mut word, &mut word_started);
    words
}

/// `shellCommandSegments`: the command split at `;`, newline, `|`, `||`,
/// `&` and `&&` outside quotes, a descriptor redirection left whole.
#[must_use]
pub fn shell_command_segments(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut segments = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if ch == '\\' && quote == Some('"') && !double_quote_escapes(chars.get(i + 1).copied()) {
            i += 1;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            i += 1;
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            }
            i += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            i += 1;
            continue;
        }
        // Descriptor duplication/closure is one redirection operator, not a
        // background separator followed by a command named "1" or "-". Only
        // shell blanks/newlines delimit it: Unicode whitespace can be part of
        // a filename.
        if ch == '>' || ch == '<' {
            let rest = rest_from(&chars, i);
            if let Some(found) = re::find(r"^[<>]&[ \t]*(?:\d+|-)(?=$|[ \t\n;|&()<>])", "", &rest) {
                i += rest[found.range()].chars().count();
                continue;
            }
        }
        if ch != ';' && ch != '\n' && ch != '|' && ch != '&' {
            i += 1;
            continue;
        }
        segments.push(chars[start..i].iter().collect());
        if (ch == '|' || ch == '&') && chars.get(i + 1) == Some(&ch) {
            i += 1;
        }
        start = i + 1;
        i += 1;
    }
    segments.push(chars[start..].iter().collect());
    segments
}

/// `shellWordAt`: the word that starts at `start` (an index into `chars`),
/// quotes removed, and the index after it; `None` when there is no word or
/// a quote never closes.
pub(crate) fn shell_word_at(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut i = start;
    while i < chars.len() {
        let ch = chars[i];
        if escaped {
            word.push(ch);
            escaped = false;
            i += 1;
            continue;
        }
        // Same double-quote rule as shell_words: inside "..." a backslash
        // escapes only $ ` " \ and newline.
        if ch == '\\' && quote == Some('"') && !double_quote_escapes(chars.get(i + 1).copied()) {
            word.push(ch);
            i += 1;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            i += 1;
            continue;
        }
        if let Some(open) = quote {
            if ch == open {
                quote = None;
            } else {
                word.push(ch);
            }
            i += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            i += 1;
            continue;
        }
        if blank(ch) || operator(ch) {
            break;
        }
        word.push(ch);
        i += 1;
    }
    (quote.is_none() && !word.is_empty()).then_some((word, i))
}

/// `withoutRedirections`: a segment with its redirections removed, so
/// `cd 2>/dev/null` is read as a bare `cd`. Quotes are respected: a `>`
/// inside a quoted word is not one.
#[must_use]
pub fn without_redirections(segment: &str) -> String {
    let chars: Vec<char> = segment.chars().collect();
    let mut out = String::new();
    let mut quote: Option<char> = None;
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if let Some(open) = quote {
            out.push(ch);
            if ch == open {
                quote = None;
            } else if ch == '\\' && open == '"' {
                i += 1;
                if let Some(next) = chars.get(i) {
                    out.push(*next);
                }
            }
            i += 1;
            continue;
        }
        if ch == '\\' {
            out.push(ch);
            i += 1;
            if let Some(next) = chars.get(i) {
                out.push(*next);
            }
            i += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            out.push(ch);
            i += 1;
            continue;
        }
        if ch != '>' && ch != '<' {
            out.push(ch);
            i += 1;
            continue;
        }
        // A descriptor number or `&` written against the operator belongs
        // to it.
        out = re::replace(r"(?:^|(?<=\s))(?:\d+|&)$", "", &out, "");
        while chars.get(i + 1).is_some_and(|next| ">|&<".contains(*next)) {
            i += 1;
        }
        while chars
            .get(i + 1)
            .is_some_and(|next| *next == ' ' || *next == '\t')
        {
            i += 1;
        }
        // The operator's word, quotes and all.
        let mut word_quote: Option<char> = None;
        while i + 1 < chars.len() {
            let next = chars[i + 1];
            if word_quote.is_none() && (blank(next) || ";|&<>".contains(next)) {
                break;
            }
            i += 1;
            if let Some(open) = word_quote {
                if next == open {
                    word_quote = None;
                }
            } else if next == '\'' || next == '"' {
                word_quote = Some(next);
            } else if next == '\\' {
                i += 1;
            }
        }
        out.push(' ');
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_drop_quotes_and_descriptor_redirections() {
        assert_eq!(
            shell_words("rm -rf 'a b' \"c\\$d\" 2>&1 e"),
            ["rm", "-rf", "a b", "c$d", "e"]
        );
        assert_eq!(shell_words("echo x>f 1>&- g"), ["echo", "x", "f", "g"]);
        assert_eq!(shell_words("\"C:\\path\\x\""), ["C:\\path\\x"]);
        assert_eq!(shell_words("a\\ b ''"), ["a b", ""]);
    }

    #[test]
    fn segments_split_at_operators_outside_quotes() {
        assert_eq!(
            shell_command_segments("a; b && c | d || 'e;f' &g"),
            ["a", " b ", " c ", " d ", " 'e;f' ", "g"]
        );
        assert_eq!(shell_command_segments("a 2>&1 b"), ["a 2>&1 b"]);
        assert_eq!(shell_command_segments("a >&- &b"), ["a >&- ", "b"]);
    }

    #[test]
    fn redirections_are_removed_with_their_descriptors() {
        assert_eq!(without_redirections("cd 2>/dev/null dir"), "cd   dir");
        assert_eq!(without_redirections("cd >\"a b\" 2>&1"), "cd    ");
        assert_eq!(without_redirections("echo '>' x"), "echo '>' x");
    }
}

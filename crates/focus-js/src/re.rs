//! ECMAScript regular expressions.
//!
//! Upstream holds roughly 1,900 regex literals and builds another hundred at
//! runtime. Their semantics are not those of the `regex` crate: `\s` covers a
//! different set, `$` under `m` also stops at `\r`, U+2028 and U+2029, `.`
//! excludes all four line terminators, and lookbehind and backreferences are in
//! use. Porting each pattern by hand would re-derive those rules at every call
//! site, so patterns are ported verbatim and run on `regress`, an engine that
//! targets the ECMAScript grammar.
//!
//! One caveat: upstream's patterns are mostly not in `u` mode and so match
//! UTF-16 code units, while `regress` matches code points here. The two agree
//! unless a pattern consumes half of a surrogate pair, which none of the ported
//! patterns can do on valid UTF-8 input.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

pub use regress::Match;
use regress::Regex;

thread_local! {
    static CACHE: RefCell<HashMap<(String, &'static str), Rc<Regex>>> = RefCell::new(HashMap::new());
}

/// Compile `pattern` with JS `flags` (`"m"`, `"i"`, `"s"`, `"u"`, `"v"`, or a
/// mix), memoised per thread.
///
/// # Panics
///
/// Panics when the pattern does not compile. Every pattern is a constant from
/// upstream, optionally with an identifier spliced in, so a failure is a
/// porting bug rather than a runtime condition.
#[must_use]
pub fn compile(pattern: &str, flags: &'static str) -> Rc<Regex> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(found) = cache.get(&(pattern.to_owned(), flags)) {
            return Rc::clone(found);
        }
        let regex = Regex::with_flags(pattern, flags)
            .unwrap_or_else(|err| panic!("invalid ported regex /{pattern}/{flags}: {err}"));
        let regex = Rc::new(regex);
        cache.insert((pattern.to_owned(), flags), Rc::clone(&regex));
        regex
    })
}

/// `text.match(/pattern/flags)` for a non-global regex: the first match.
#[must_use]
pub fn find(pattern: &str, flags: &'static str, text: &str) -> Option<Match> {
    compile(pattern, flags).find(text)
}

/// `/pattern/flags.test(text)`.
#[must_use]
pub fn test(pattern: &str, flags: &'static str, text: &str) -> bool {
    find(pattern, flags, text).is_some()
}

/// Capture group `idx` of `m` as a slice of `text`; `None` when the group did
/// not participate (JS `undefined`).
#[must_use]
pub fn group<'t>(text: &'t str, m: &Match, idx: usize) -> Option<&'t str> {
    m.group(idx).map(|range: Range<usize>| &text[range])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_class_is_ecmascript() {
        assert!(test(r"^\s$", "", "\u{FEFF}"));
        assert!(!test(r"^\s$", "", "\u{0085}"));
    }

    #[test]
    fn multiline_anchors_stop_at_every_line_terminator() {
        // `$` under `m` matches before `\r`, which the `regex` crate does not do.
        let m = find(r"^key:\s*(.+?)\s*$", "m", "key: value\r\nnext: x").unwrap();
        assert_eq!(group("key: value\r\nnext: x", &m, 1), Some("value"));
    }

    #[test]
    fn dot_excludes_carriage_return() {
        assert!(!test(r"^a.b$", "", "a\rb"));
        assert!(test(r"^a.b$", "s", "a\rb"));
    }

    #[test]
    fn lookbehind_and_unicode_sets_are_supported() {
        // The two least common constructs upstream uses: lookbehind, and the
        // `v` flag with a property of strings (`aidlc-artifact-vocabulary.ts`).
        let m = find(r"(?<=a)-(?!c)", "", "x-c a-c a-b").unwrap();
        assert_eq!(m.range(), 9..10);
        let heading = "\u{2139}\u{FE0F} Sources";
        let m = find(
            r"^\p{RGI_Emoji}(?:\u200D?\p{RGI_Emoji})*[ \t]+",
            "v",
            heading,
        )
        .unwrap();
        assert_eq!(&heading[m.range().end..], "Sources");
    }

    #[test]
    fn unmatched_group_is_none() {
        let text = "b";
        let m = find(r"(a)?b", "", text).unwrap();
        assert_eq!(group(text, &m, 1), None);
    }
}

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
//!
//! And one correction. Under `i` without `u`, JavaScript compares characters
//! by their upper-case form (the full mapping, and only when it is a single
//! code unit), except that a character outside ASCII is never equal to one
//! inside it. `regress` 0.12 compares otherwise in four places, all measured
//! against Bun by the oracle corpus (kind `icase`), which has every
//! character with a case as a pattern, alone and as a class:
//!
//! - 65 characters of the Basic Multilingual Plane that to JavaScript are
//!   equal to nothing but themselves (`misfolded`). It takes dotless `ı`,
//!   long `ſ` and the kelvin sign for `i`, `s` and `k`, in a literal, a
//!   class, a negated class and a backreference alike; the small and capital
//!   forms of the 54 Greek letters with an iota subscript for each other;
//!   and, in a class only, the ohm and angstrom signs, capital `ẞ`, the
//!   theta symbol, two Greek letters of the extended block and the two
//!   ligatures of long s for the letters they resemble.
//! - Beyond that plane it pairs a capital letter with its small form.
//!   JavaScript sees two code units there, and a code unit has no case.
//! - `\W` inside a class takes `s` and `k` with it, because `ſ` and the
//!   kelvin sign are in `\W`.
//! - A class range that reaches outside ASCII is closed over the same
//!   pairings.
//!
//! So a pattern compiled here with `i` and without `u` or `v` is never shown
//! the 65: the text it searches has each replaced by a stand-in of the same
//! length in UTF-8 that has no case at all (see [`Pattern`]). Offsets do not
//! move. That is exact as long as the pattern cannot tell one caseless
//! character outside ASCII from another and stays out of the other three
//! places, so a pattern that does not is refused when it is compiled
//! (`unguardable`). None of upstream's 226 such patterns is.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

pub use regress::Match;
use regress::Regex;

thread_local! {
    static CACHE: RefCell<HashMap<(String, &'static str), Rc<Pattern>>> = RefCell::new(HashMap::new());
}

/// What a searched text gets in place of a character `regress` would
/// misfold: a character with no case, as many bytes long.
const STAND_INS: [char; 2] = ['\u{138}', '\u{2129}'];

/// The characters `regress` compares otherwise than JavaScript under `i`
/// without `u` (see the module docs). To JavaScript each is equal to itself
/// alone.
fn misfolded(c: char) -> bool {
    // Taken for `i`, `s` and `k`.
    matches!(c, '\u{131}' | '\u{17f}' | '\u{212a}')
        // The Greek letters with an iota subscript, small and capital.
        || matches!(c, '\u{1f80}'..='\u{1faf}' | '\u{1fb3}' | '\u{1fbc}' | '\u{1fc3}' | '\u{1fcc}')
        || matches!(c, '\u{1ff3}' | '\u{1ffc}')
        // Paired with another letter in a class only.
        || matches!(c, '\u{3f4}' | '\u{1e9e}' | '\u{1fd3}' | '\u{1fe3}' | '\u{2126}' | '\u{212b}')
        || matches!(c, '\u{fb05}' | '\u{fb06}')
}

fn stand_in(c: char) -> char {
    if !misfolded(c) {
        c
    } else if c.len_utf8() == 2 {
        STAND_INS[0]
    } else {
        STAND_INS[1]
    }
}

/// Why a case-insensitive pattern outside the Unicode modes cannot be run
/// with the correction; `None` when it can. It cannot when:
///
/// - it names a character the correction replaces or puts in, or one beyond
///   the Basic Multilingual Plane;
/// - one of its classes has a range that reaches outside ASCII, or holds
///   `\W`;
/// - it has a backreference, and a group that is more than a choice of ASCII
///   words: what such a group captures could hold two characters the
///   correction makes one.
fn unguardable(pattern: &str) -> Option<String> {
    let chars: Vec<char> = pattern.chars().collect();
    // One atom at `at`: its code point when it stands for a single
    // character, and where the next atom starts.
    let atom = |at: usize| -> (Option<char>, usize) {
        if chars[at] != '\\' || at + 1 >= chars.len() {
            return (Some(chars[at]), at + 1);
        }
        let digits = match chars[at + 1] {
            'u' => 4,
            'x' => 2,
            _ => return (None, at + 2),
        };
        let hex: String = chars[at + 2..].iter().take(digits).collect();
        match u32::from_str_radix(&hex, 16) {
            Ok(code) if hex.len() == digits => (char::from_u32(code), at + 2 + digits),
            _ => (None, at + 2),
        }
    };
    let escape = |at: usize| {
        (chars[at] == '\\')
            .then(|| chars.get(at + 1).copied())
            .flatten()
    };
    // `(` opens a capturing group unless `?` follows, and `(?<name>` does.
    let capturing = |at: usize| match (chars.get(at + 1), chars.get(at + 2), chars.get(at + 3)) {
        (Some('?'), Some('<'), Some(c)) => *c != '=' && *c != '!',
        (Some('?'), ..) => false,
        _ => true,
    };
    let (mut at, mut in_class) = (0, false);
    // Where each capturing group opens, and whether a backreference was seen.
    let (mut groups, mut refers) = (Vec::new(), false);
    while at < chars.len() {
        let (named, next) = atom(at);
        if let Some(c) = named
            && (misfolded(c) || STAND_INS.contains(&c) || u32::from(c) > 0xFFFF)
        {
            return Some(format!("names U+{:04X}", u32::from(c)));
        }
        if !in_class {
            in_class = chars[at] == '[';
            refers |= matches!(escape(at), Some('1'..='9' | 'k'));
            if chars[at] == '(' && capturing(at) {
                groups.push(at);
            }
        } else if chars[at] == ']' {
            in_class = false;
        } else if escape(at) == Some('W') {
            return Some(r"has \W in a class".to_owned());
        } else if chars.get(next) == Some(&'-')
            && next + 1 < chars.len()
            // A range ends no lower than it starts, so its end tells.
            && atom(next + 1).0.is_some_and(|end| !end.is_ascii())
        {
            return Some("has a class range outside ASCII".to_owned());
        }
        at = next;
    }
    let words = |open: &usize| {
        chars[open + 1..]
            .iter()
            .position(|c| !(c.is_ascii_alphanumeric() || *c == '|'))
            .is_some_and(|end| chars[open + 1 + end] == ')')
    };
    if refers && !groups.iter().all(words) {
        return Some("has a backreference to more than a choice of ASCII words".to_owned());
    }
    None
}

/// A compiled pattern.
pub struct Pattern {
    regex: Regex,
    /// Whether the pattern is case-insensitive without being in a Unicode
    /// mode, so that the text it searches is first cleared of the characters
    /// the engine would misfold.
    guarded: bool,
}

impl Pattern {
    /// The text the engine is to search: `text` itself, unless the pattern is
    /// guarded and the text holds a character the engine would misfold.
    fn searched<'t>(&self, text: &'t str) -> std::borrow::Cow<'t, str> {
        if !self.guarded || !text.contains(misfolded) {
            return std::borrow::Cow::Borrowed(text);
        }
        text.chars().map(stand_in).collect()
    }

    /// The first match in `text`. Its ranges index `text`.
    #[must_use]
    pub fn find(&self, text: &str) -> Option<Match> {
        self.regex.find(&self.searched(text))
    }

    /// Every match in `text`, in order, as `String.prototype.matchAll` with a
    /// global pattern finds them. Their ranges index `text`.
    #[must_use]
    pub fn find_iter(&self, text: &str) -> std::vec::IntoIter<Match> {
        let found: Vec<Match> = self.regex.find_iter(&self.searched(text)).collect();
        found.into_iter()
    }
}

/// Compile `pattern` with JS `flags` (`"m"`, `"i"`, `"s"`, `"u"`, `"v"`, or a
/// mix), memoised per thread.
///
/// # Panics
///
/// Panics when the pattern does not compile. Every pattern is a constant from
/// upstream, optionally with an identifier spliced in, so a failure is a
/// porting bug rather than a runtime condition.
///
/// Panics too when a case-insensitive pattern outside the Unicode modes
/// could tell a character the engine misfolds from its stand-in (see the
/// module docs): the correction would change what such a pattern matches.
/// None of upstream's does.
#[must_use]
pub fn compile(pattern: &str, flags: &'static str) -> Rc<Pattern> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(found) = cache.get(&(pattern.to_owned(), flags)) {
            return Rc::clone(found);
        }
        let regex = Regex::with_flags(pattern, flags)
            .unwrap_or_else(|err| panic!("invalid ported regex /{pattern}/{flags}: {err}"));
        let guarded = flags.contains('i') && !flags.contains(['u', 'v']);
        if guarded && let Some(why) = unguardable(pattern) {
            panic!(
                "ported regex /{pattern}/{flags} {why}, and this engine ignores case otherwise than JavaScript there"
            );
        }
        let compiled = Rc::new(Pattern { regex, guarded });
        cache.insert((pattern.to_owned(), flags), Rc::clone(&compiled));
        compiled
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
    fn a_letter_outside_ascii_is_never_an_ascii_letter() {
        // Dotless i, long s and the kelvin sign: the engine alone would take
        // each for the ASCII letter it upper-cases or folds to.
        for (pattern, text) in [
            ("i", "\u{131}"),
            ("S", "\u{17f}"),
            ("k", "\u{212a}"),
            ("[a-z]", "\u{131}\u{17f}\u{212a}"),
            (r"\w", "\u{17f}\u{212a}"),
            (r"(s)\1", "s\u{17f}"),
            ("</(?:script|style)>", "</\u{17f}tyle>"),
            // A Greek letter with an iota subscript and its capital form.
            ("\u{3b1}", "\u{1f80}"),
            // In a class the engine pairs more: the ohm and angstrom signs,
            // capital sharp s, the theta symbol, and two Greek letters that
            // Unicode has twice.
            ("[\u{3c9}]", "\u{2126}"),
            ("[\u{e5}]", "\u{212b}"),
            ("[\u{df}]", "\u{1e9e}"),
            ("[\u{3b8}]", "\u{3f4}"),
            ("[\u{390}\u{3b0}]", "\u{1fd3}\u{1fe3}"),
        ] {
            assert!(!test(pattern, "i", text), "/{pattern}/i on {text:?}");
        }
        // A negated class takes them, and offsets are the text's own.
        let text = "a\u{17f}b\u{212a}S";
        let found: Vec<_> = compile("[^a-z]", "i")
            .find_iter(text)
            .map(|m| m.range())
            .collect();
        assert_eq!(found, [1..3, 4..7]);
        assert_eq!(find("s", "i", text).unwrap().range(), 7..8);
        // In a Unicode mode JavaScript folds them as the engine does.
        assert!(test("s", "iu", "\u{17f}") && test("k", "iu", "\u{212a}"));
        // Without `i` nothing is replaced.
        assert!(test("\u{17f}", "", "\u{17f}"));
        // Other letters keep their case pairs, and punctuation outside ASCII
        // in a pattern is no obstacle.
        assert!(test("\u{e9}t\u{e9}", "i", "\u{c9}T\u{c9}"));
        assert!(test("[\u{3c9}]", "i", "\u{3a9}"));
        assert!(test(r"couldn[’']t", "i", "COULDN\u{2019}T"));
    }

    #[test]
    fn a_pattern_the_correction_would_change_is_refused() {
        let names = |code: &str| Some(format!("names U+{code}"));
        assert_eq!(unguardable(r"[a-zſ]"), names("017F"));
        assert_eq!(unguardable("x\u{1f88}"), names("1F88"));
        assert_eq!(unguardable(r"Ω"), names("2126"));
        assert_eq!(unguardable("\u{138}"), names("0138"));
        assert_eq!(unguardable(r"\x{2129}"), None);
        assert_eq!(unguardable("a\u{2129}"), names("2129"));
        // Beyond the Basic Multilingual Plane the engine pairs letters that
        // to JavaScript are two code units each.
        assert_eq!(unguardable("\u{10400}"), names("10400"));
        assert_eq!(unguardable("[\u{1f600}]"), names("1F600"));
        let range = Some("has a class range outside ASCII".to_owned());
        assert_eq!(unguardable(r"[à-ÿ]"), range);
        assert_eq!(unguardable("[a-\u{e9}]"), range);
        assert_eq!(
            unguardable(r"[^\W\d_]").as_deref(),
            Some(r"has \W in a class")
        );
        let refers = Some("has a backreference to more than a choice of ASCII words".to_owned());
        assert_eq!(unguardable(r"(\S+) \1"), refers);
        assert_eq!(unguardable(r"(a|b)(.)\1"), refers);
        assert_eq!(unguardable(r"(?<tag>a)\k<tag>"), refers);
        assert_eq!(unguardable(r"[a-z](\S+)\1"), refers);
        for fine in [
            r"[a-z0-9_-]+",
            r"[\w-]\-[a-z]",
            r"a-é",
            r"[-–—:]",
            r"\[à-ÿ\]x",
            r"\W+[\s\S][^\s\w]",
            r"<(script|style)\b[\s\S]*?(?:<\/\1\s*>|$)",
            r"(?<=a)(?:\S+)(?!b)(x|y1)\1",
            r"(?<!a)(x)\1[\u00e9-]",
            r"(\S+)[\1]\(",
        ] {
            assert_eq!(unguardable(fine), None, "{fine}");
        }
    }

    #[test]
    fn the_characters_replaced_are_the_ones_measured() {
        // The list is what the comparison with Bun found (module docs). The
        // corpus cannot hold a pattern that names one, since such a pattern
        // is refused, so the list is pinned here: 65 characters, each
        // refused by name and replaced by a caseless character as long.
        let listed: Vec<char> = (0..=0x10_FFFF)
            .filter_map(char::from_u32)
            .filter(|c| misfolded(*c))
            .collect();
        assert_eq!(listed.len(), 65);
        let greek = listed
            .iter()
            .filter(|c| ('\u{1f80}'..='\u{1ffc}').contains(*c));
        assert_eq!(greek.count(), 56);
        for c in listed {
            let put = stand_in(c);
            assert!(STAND_INS.contains(&put) && put.len_utf8() == c.len_utf8());
            assert!(unguardable(&c.to_string()).is_some(), "{c:?}");
            assert!(
                !test("[a-z\u{df}\u{e5}\u{3b8}\u{3c9}]", "i", &c.to_string()),
                "{c:?}"
            );
        }
        for put in STAND_INS {
            assert_eq!(
                put.to_lowercase()
                    .chain(put.to_uppercase())
                    .collect::<String>(),
                format!("{put}{put}")
            );
            assert_eq!(stand_in(put), put);
        }
    }

    #[test]
    #[should_panic(expected = "names U+017F")]
    fn compiling_a_refused_pattern_panics() {
        let _ = compile(r"[a-zſ]", "i");
    }

    #[test]
    fn a_pattern_is_refused_only_where_the_correction_applies() {
        // With `u`, or without `i`, the engine compares as JavaScript does.
        for flags in ["", "m", "iu", "iv"] {
            let _ = compile(r"[à-ÿſ]", flags);
        }
    }

    #[test]
    fn unmatched_group_is_none() {
        let text = "b";
        let m = find(r"(a)?b", "", text).unwrap();
        assert_eq!(group(text, &m, 1), None);
    }
}

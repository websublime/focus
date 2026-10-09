//! Ordering behaviour: default `sort()`, `localeCompare`, `Object.keys`.

use std::cmp::Ordering;

use serde_json::{Map, Value};

/// The order `Array.prototype.sort()` gives strings with no comparator:
/// UTF-16 code units.
///
/// This differs from Rust's `str` ordering (UTF-8 bytes, i.e. code points) only
/// when one string has an astral character where the other has one in
/// U+E000..=U+FFFF. Upstream sorts directory listings this way before emitting
/// them into compiled artifacts, so the difference is observable.
#[must_use]
pub fn cmp_utf16(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Printable ASCII in the order of the CLDR root collation, the order
/// `localeCompare` gives under an English locale. A letter's two cases share
/// one position (they differ only at the third level).
const ROOT_ORDER: &str = " _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$0123456789abcdefghijklmnopqrstuvwxyz";

/// Whether `localeCompare` is reproduced exactly for `s`: printable ASCII only.
#[must_use]
pub fn is_collation_safe(s: &str) -> bool {
    s.bytes().all(|b| (0x20..0x7f).contains(&b))
}

fn primary(b: u8) -> usize {
    ROOT_ORDER
        .bytes()
        .position(|c| c == b.to_ascii_lowercase())
        .unwrap_or(usize::MAX)
}

/// `a.localeCompare(b)` for the engine's own identifiers.
///
/// Exact when both strings are printable ASCII ([`is_collation_safe`]), which
/// covers every grammar whose order reaches engine output: stage slugs, scope
/// names, rule paths. The comparison has the collation's two relevant levels:
/// first the characters ignoring case, in root order (punctuation, then
/// digits, then letters; a prefix sorts before its extensions); then, for
/// strings equal that far, lowercase before uppercase at the first difference.
///
/// Anything else falls back to code-unit order, which is **not** ICU order; a
/// caller that can see such input and whose output depends on the order needs
/// a real collator (the migration plan tracks the upstream call sites).
#[must_use]
pub fn locale_compare(a: &str, b: &str) -> Ordering {
    if !is_collation_safe(a) || !is_collation_safe(b) {
        return cmp_utf16(a, b);
    }
    a.bytes()
        .map(primary)
        .cmp(b.bytes().map(primary))
        .then_with(|| {
            a.bytes()
                .map(|c| c.is_ascii_uppercase())
                .cmp(b.bytes().map(|c| c.is_ascii_uppercase()))
        })
}

/// A canonical array index in `0..=2^32-2`, the keys `Object.keys` lists first.
///
/// Canonical means the decimal form with no sign, no leading zeros and no
/// whitespace: `"7"` is an index, `"07"` and `"+7"` are ordinary keys.
#[must_use]
pub fn array_index(key: &str) -> Option<u32> {
    let canonical = key == "0" || (!key.starts_with('0') && !key.is_empty());
    if !canonical || !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    key.parse::<u32>().ok().filter(|n| *n != u32::MAX)
}

/// Rebuild `map` in the order a JS object would enumerate the same insertions:
/// integer-like keys ascending first, then the rest in insertion order.
#[must_use]
pub fn js_key_order(map: Map<String, Value>) -> Map<String, Value> {
    if !map.keys().any(|k| array_index(k).is_some()) {
        return map;
    }
    let (mut indexed, named): (Vec<_>, Vec<_>) =
        map.into_iter().partition(|(k, _)| array_index(k).is_some());
    indexed.sort_by_key(|(k, _)| array_index(k));
    indexed.into_iter().chain(named).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sort_is_code_unit_order() {
        // U+FF5E sorts before U+1F600 by code point, after it by UTF-16 unit
        // (0xFF5E > the 0xD83D lead surrogate).
        let bmp = "\u{FF5E}";
        let astral = "\u{1F600}";
        assert_eq!(bmp.cmp(astral), Ordering::Less);
        assert_eq!(cmp_utf16(bmp, astral), Ordering::Greater);
        assert_eq!(cmp_utf16("a", "ab"), Ordering::Less);
    }

    #[test]
    fn collation_safe_alphabet() {
        assert!(is_collation_safe("phases/construction.md"));
        assert!(is_collation_safe("Snake_case+v2@x"));
        assert!(!is_collation_safe("caf\u{e9}"));
        assert!(!is_collation_safe("tab\there"));
    }

    #[test]
    fn locale_compare_follows_root_collation() {
        use Ordering::{Equal, Greater, Less};
        // Each answer is what Bun's `a.localeCompare(b)` returned.
        for (a, b, expected) in [
            ("a", "B", Less),
            ("A", "a", Greater),
            ("ab", "Ab", Less),
            ("aB", "Ab", Less),
            ("a-b", "ab", Less),
            ("a_b", "a-b", Less),
            ("Ab", "aC", Less),
            ("ab", "a", Greater),
            ("A", "ab", Less),
            ("x+1", "x@1", Greater),
            ("v1.2", "v1-2", Greater),
            ("9", "a", Less),
            ("same", "same", Equal),
        ] {
            assert_eq!(locale_compare(a, b), expected, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn array_index_is_canonical_only() {
        assert_eq!(array_index("0"), Some(0));
        assert_eq!(array_index("42"), Some(42));
        assert_eq!(array_index("4294967294"), Some(4_294_967_294));
        assert_eq!(array_index("4294967295"), None);
        assert_eq!(array_index("07"), None);
        assert_eq!(array_index("+7"), None);
        assert_eq!(array_index(""), None);
        assert_eq!(array_index("1a"), None);
    }

    #[test]
    fn object_keys_list_indices_first() {
        let mut map = Map::new();
        for key in ["10", "9", "abc", "2x", "1"] {
            map.insert(key.to_owned(), Value::Null);
        }
        let ordered: Vec<_> = js_key_order(map).keys().cloned().collect();
        assert_eq!(ordered, ["1", "9", "10", "abc", "2x"]);
    }
}

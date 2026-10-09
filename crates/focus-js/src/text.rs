//! String behaviour: whitespace, trimming, line splitting, `parseInt`.

/// ECMAScript `WhiteSpace` + `LineTerminator`: what `\s`, `trim`, `trimStart`
/// and `trimEnd` treat as whitespace.
///
/// This is not `char::is_whitespace`: JS includes U+FEFF and excludes U+0085.
#[must_use]
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim`.
#[must_use]
pub fn trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `String.prototype.trimStart`.
#[must_use]
pub fn trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_whitespace)
}

/// `String.prototype.trimEnd`.
#[must_use]
pub fn trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

/// `s.split(/\r?\n/)`.
///
/// A `\r` is dropped only when it directly precedes the `\n` that ends the
/// line; a lone `\r`, or one at the very end of the input, stays in the piece.
#[must_use]
pub fn split_lines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(idx) = rest.find('\n') {
        let line = &rest[..idx];
        out.push(line.strip_suffix('\r').unwrap_or(line));
        rest = &rest[idx + 1..];
    }
    out.push(rest);
    out
}

/// `parseInt(s, 10)`: skip leading whitespace, take an optional sign and the
/// longest run of ASCII digits, ignore whatever follows. `None` is `NaN`.
#[must_use]
pub fn parse_int(s: &str) -> Option<f64> {
    let s = trim_start(s);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = digits
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(digits.len());
    if end == 0 {
        return None;
    }
    // Digit-by-digit accumulation in f64 matches JS for every value an f64
    // represents exactly; beyond 2^53 both round, though not always alike.
    let magnitude = digits[..end]
        .bytes()
        .fold(0.0_f64, |acc, b| acc * 10.0 + f64::from(b - b'0'));
    Some(if negative { -magnitude } else { magnitude })
}

/// `typeof v` for a JSON value, with upstream's `describe()` refinements
/// (`null` and `array` named explicitly).
#[must_use]
pub fn describe(v: &serde_json::Value) -> &'static str {
    use serde_json::Value;
    match v {
        Value::Null => "null",
        Value::Array(_) => "array",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_follows_ecmascript_not_unicode_white_space() {
        // In the JS set, absent from Rust's char::is_whitespace.
        assert!(is_js_whitespace('\u{FEFF}'));
        assert!(!'\u{FEFF}'.is_whitespace());
        // In Rust's set, absent from the JS one.
        assert!(!is_js_whitespace('\u{0085}'));
        assert!('\u{0085}'.is_whitespace());
        assert_eq!(trim("\u{FEFF}\u{00A0} a b \u{2028}"), "a b");
    }

    #[test]
    fn split_lines_drops_cr_only_before_lf() {
        assert_eq!(split_lines("a\r\nb\nc"), ["a", "b", "c"]);
        assert_eq!(split_lines("a\rb"), ["a\rb"]);
        assert_eq!(split_lines("a\r\r\nb\r"), ["a\r", "b\r"]);
        assert_eq!(split_lines(""), [""]);
        assert_eq!(split_lines("a\n"), ["a", ""]);
    }

    #[test]
    fn parse_int_matches_js() {
        assert_eq!(parse_int("42"), Some(42.0));
        assert_eq!(parse_int("  7abc"), Some(7.0));
        assert_eq!(parse_int("-3.9"), Some(-3.0));
        assert_eq!(parse_int("+5"), Some(5.0));
        assert_eq!(parse_int("abc"), None);
        assert_eq!(parse_int(""), None);
        assert_eq!(parse_int("-"), None);
        assert_eq!(parse_int("050"), Some(50.0));
    }
}

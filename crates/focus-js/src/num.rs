//! Number behaviour: `Number(s)`, `String(n)`, `toFixed`, `Math.round`.
//!
//! The engine prints numbers into JSON and into Markdown tables that agents
//! copy verbatim, so the text of a number is part of the contract.

use crate::text::trim;

/// `String(x)` / `x.toString()`: the shortest decimal that reads back as `x`,
/// laid out as ECMAScript's `Number::toString` lays it out.
///
/// Rust's `{}` prints the same digits but a different layout: `1.0` where JS
/// prints `1`, and no exponent form at all (`1e21` prints as 22 digits).
#[must_use]
pub fn to_string(x: f64) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x == 0.0 {
        return "0".to_owned();
    }
    if x < 0.0 {
        return format!("-{}", to_string(-x));
    }
    if x.is_infinite() {
        return "Infinity".to_owned();
    }
    // `{:e}` is the shortest round-trip form: `d[.ddd]e<exp>`.
    let scientific = format!("{x:e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("{:e} always prints an exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let digits = even_on_tie(x, digits, exponent);
    let exponent: i32 = exponent.parse().expect("{:e} prints a decimal exponent");
    let k = i32::try_from(digits.len()).expect("at most 17 digits");
    // x = 0.<digits> * 10^n
    let n = exponent + 1;

    if k <= n && n <= 21 {
        return format!("{digits}{}", "0".repeat(usize_of(n - k)));
    }
    if 0 < n && n <= 21 {
        let (int, frac) = digits.split_at(usize_of(n));
        return format!("{int}.{frac}");
    }
    if -6 < n && n <= 0 {
        return format!("0.{}{digits}", "0".repeat(usize_of(-n)));
    }
    let sign = if n - 1 < 0 { '-' } else { '+' };
    let magnitude = (n - 1).abs();
    if k == 1 {
        return format!("{digits}e{sign}{magnitude}");
    }
    let (first, rest) = digits.split_at(1);
    format!("{first}.{rest}e{sign}{magnitude}")
}

/// When `x` lies exactly halfway between two shortest candidates, ECMAScript
/// takes the one whose last digit is even; Rust takes the upper one.
/// `2126906910518472.25` prints as `...472.2` in JS and `...472.3` from `{:e}`.
fn even_on_tie(x: f64, digits: String, exponent: &str) -> String {
    // 800 fractional digits hold any f64's significand exactly.
    let exact: String = format!("{x:.800e}")
        .chars()
        .take_while(|c| *c != 'e')
        .filter(char::is_ascii_digit)
        .collect();
    let (kept, dropped) = exact.split_at(digits.len());
    let is_tie = dropped.starts_with('5') && dropped[1..].bytes().all(|b| b == b'0');
    let last = kept.as_bytes()[kept.len() - 1];
    if !is_tie || last == b'9' {
        return digits;
    }
    // The two candidates are `kept` and `kept` with its last digit one higher.
    let lower = kept.to_owned();
    let upper = format!("{}{}", &kept[..kept.len() - 1], char::from(last + 1));
    let even = if (last - b'0').is_multiple_of(2) {
        lower
    } else {
        upper
    };
    let reads_back = format!("{}.{}e{exponent}", &even[..1], &even[1..])
        .parse::<f64>()
        .is_ok_and(|parsed| parsed == x);
    if reads_back { even } else { digits }
}

fn usize_of(n: i32) -> usize {
    usize::try_from(n).expect("checked non-negative by the caller")
}

/// `x.toFixed(digits)`.
///
/// Rounds the exact binary value half **up** in magnitude, where Rust's
/// `{:.N}` rounds half to even: `0.125.toFixed(2)` is `0.13`, `{:.2}` gives
/// `0.12`.
#[must_use]
pub fn to_fixed(x: f64, digits: usize) -> String {
    if x.is_nan() {
        return "NaN".to_owned();
    }
    if x.abs() >= 1e21 {
        return to_string(x);
    }
    // 1074 fractional digits hold any f64 exactly, so this expansion is the
    // number itself and the rounding below never sees a rounded input.
    let exact = format!("{:.1080}", x.abs());
    let (int, frac) = exact.split_once('.').expect("precision prints a point");
    let round_up = frac.as_bytes()[digits] >= b'5';
    let mut kept: Vec<u8> = int.bytes().chain(frac.bytes().take(digits)).collect();
    if round_up {
        let mut idx = kept.len();
        loop {
            if idx == 0 {
                kept.insert(0, b'1');
                break;
            }
            idx -= 1;
            if kept[idx] == b'9' {
                kept[idx] = b'0';
            } else {
                kept[idx] += 1;
                break;
            }
        }
    }
    let split = kept.len() - digits;
    let mut out = String::new();
    if x < 0.0 {
        out.push('-');
    }
    out.push_str(std::str::from_utf8(&kept[..split]).expect("ASCII digits"));
    if digits > 0 {
        out.push('.');
        out.push_str(std::str::from_utf8(&kept[split..]).expect("ASCII digits"));
    }
    out
}

/// `Number(s)` for a string: `NaN` unless the whole trimmed text is a numeric
/// literal. The empty string is `0`.
///
/// Rust's `str::parse::<f64>` is not this: it accepts `inf` and `nan`, and
/// rejects hex and surrounding whitespace.
#[must_use]
pub fn from_str(s: &str) -> f64 {
    let s = trim(s);
    if s.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(body) = s.strip_prefix(prefix) {
            if body.is_empty() || !body.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            return body.chars().fold(0.0, |acc, c| {
                acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
            });
        }
    }
    let (negative, body) = match s.as_bytes()[0] {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let magnitude = if body == "Infinity" {
        f64::INFINITY
    } else if is_decimal_literal(body) {
        body.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        return f64::NAN;
    };
    if negative { -magnitude } else { magnitude }
}

/// `digits [. digits?] | . digits`, then an optional exponent.
fn is_decimal_literal(s: &str) -> bool {
    let bytes = s.as_bytes();
    let run = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let int = run(0);
    let mut at = int;
    let mut frac = 0;
    if bytes.get(at) == Some(&b'.') {
        frac = run(at + 1);
        at += 1 + frac;
    }
    if int == 0 && frac == 0 {
        return false;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        at += 1;
        if matches!(bytes.get(at), Some(b'+' | b'-')) {
            at += 1;
        }
        let exp = run(at);
        if exp == 0 {
            return false;
        }
        at += exp;
    }
    at == bytes.len()
}

/// `Math.round(x)`: the nearest integer, a tie going towards positive
/// infinity. Rust's `f64::round` sends ties away from zero (`-2.5` gives `-3`,
/// JS gives `-2`).
#[must_use]
pub fn round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let floor = x.floor();
    if x - floor >= 0.5 { floor + 1.0 } else { floor }
}

/// `Number.isInteger(x)`.
#[must_use]
pub fn is_integer(x: f64) -> bool {
    x.is_finite() && x.trunc() == x
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every expectation below was printed by Bun 1.4 for the same input.

    #[test]
    fn to_string_layouts() {
        for (x, expected) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.3, "0.3"),
            (100.0, "100"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123_456_789_012_345_680_000.0, "123456789012345680000"),
            (1.5e22, "1.5e+22"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.25e-7, "1.25e-7"),
            (0.1 + 0.2, "0.30000000000000004"),
            (62.749_999_999_999_99, "62.74999999999999"),
            (20.499_999_999_999_996, "20.499999999999996"),
            (5e-324, "5e-324"),
            // Exactly halfway between two shortest candidates: the even one.
            (2_126_906_910_518_472.3, "2126906910518472.2"),
            (241_343_026_456_369.63, "241343026456369.62"),
            (851_118_995_491_804.3, "851118995491804.2"),
            (738_735_360_588_770.3, "738735360588770.2"),
            (f64::MAX, "1.7976931348623157e+308"),
        ] {
            assert_eq!(to_string(x), expected, "{x:e}");
        }
        assert_eq!(to_string(f64::NAN), "NaN");
        assert_eq!(to_string(f64::NEG_INFINITY), "-Infinity");
    }

    #[test]
    fn to_fixed_rounds_ties_up_in_magnitude() {
        for (x, digits, expected) in [
            (0.125, 2, "0.13"),
            (0.375, 2, "0.38"),
            (-0.125, 2, "-0.13"),
            (0.3, 2, "0.30"),
            (0.299, 2, "0.30"),
            (0.4004, 2, "0.40"),
            (1.0, 2, "1.00"),
            (0.005, 2, "0.01"),
            (1.005, 2, "1.00"),
            (0.995, 2, "0.99"),
            (9.999, 2, "10.00"),
            (-0.001, 2, "-0.00"),
            (-0.0, 2, "0.00"),
            (2.5, 0, "3"),
            (20.499_999_999_999_996, 9, "20.500000000"),
            (62.749_999_999_999_99, 9, "62.750000000"),
            (1e21, 2, "1e+21"),
        ] {
            assert_eq!(to_fixed(x, digits), expected, "{x} to {digits}");
        }
    }

    #[test]
    fn from_str_is_number_of_string() {
        for (s, expected) in [
            ("0.30", 0.3),
            (" 12 ", 12.0),
            ("", 0.0),
            ("  ", 0.0),
            ("1.", 1.0),
            (".5", 0.5),
            ("+.5e1", 5.0),
            ("-1e-2", -0.01),
            ("0x1f", 31.0),
            ("0b101", 5.0),
            ("0o17", 15.0),
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
        ] {
            assert_eq!(from_str(s), expected, "{s:?}");
        }
        for s in [
            "abc", "1,5", "1e", ".", "+", "inf", "nan", "NaN", "infinity", "1_000", "0x", "-0x1f",
            "1 2", "1e+", "--1",
        ] {
            assert!(from_str(s).is_nan(), "{s:?}");
        }
    }

    #[test]
    fn round_sends_ties_towards_positive_infinity() {
        for (x, expected) in [
            (2.5, 3.0),
            (-2.5, -2.0),
            (20.5, 21.0),
            (20.499_999_999_999_996, 20.0),
            (0.499_999_999_999_999_94, 0.0),
            (-0.5, -0.0),
            (62.75, 63.0),
        ] {
            assert_eq!(round(x), expected, "{x}");
        }
    }
}

//! `Date.parse`, for the timestamps the engine writes.
//!
//! JavaScript's `Date.parse` reads the ECMAScript date-time format and, when
//! a text is not in it, hands the text to a second, lenient parser whose
//! rules are the runtime's own and whose answer can depend on the machine's
//! time zone. Upstream calls `Date.parse` on timestamps read back from the
//! audit ledger, which people edit and git merges, so in principle any text
//! reaches it.
//!
//! This module answers for one shape only: an instant with its zone written
//! out, on a date the calendar has.
//!
//! ```text
//! YYYY-MM-DDTHH:MM[:SS[.fraction]](Z|+HH:MM|-HH:MM)
//! ```
//!
//! with a fraction of one to nine digits. That is the shape of every
//! timestamp the engine writes, and on it the answer is the same on every
//! runtime and in every zone. For anything else [`parse_instant`] answers
//! `None`, which means "not known here", never "not a date": Bun reads
//! `2026-02-29T00:00:00Z` as the first of March, takes a lower-case `t`,
//! reads a text with no zone in local time, and reads a fraction of ten
//! digits or more by rules of its own (`.0674309079` is 674 milliseconds to
//! it). A caller that needs the answer for such a text has to refuse.
//!
//! One other thing is known: a text with no ASCII digit in it is not a date
//! to any parser ([`Instant::NotADate`]). That covers what people actually
//! leave where a timestamp goes, a word or nothing at all.
//!
//! The oracle corpus compares this with Bun's `Date.parse` on generated
//! texts of the shape (kind `instant`).

/// Days from 1970-01-01 to the given date of the proleptic Gregorian
/// calendar.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    }
}

/// `count` ASCII digits at `at`, as a number.
fn digits(text: &[u8], at: usize, count: usize) -> Option<i64> {
    let field = text.get(at..at + count)?;
    field.iter().all(u8::is_ascii_digit).then(|| {
        field
            .iter()
            .fold(0, |sum, digit| sum * 10 + i64::from(digit - b'0'))
    })
}

/// What `Date.parse` answers for a text, as far as this module knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instant {
    /// Milliseconds since the epoch.
    At(i64),
    /// `NaN`: the text has no digit, so it is a date to no parser.
    NotADate,
    /// A text this module does not read. JavaScript may read it as a date
    /// or not, by rules of the runtime's own.
    Unread,
}

/// `Date.parse(text)`, as far as it is the same everywhere.
#[must_use]
pub fn parse(text: &str) -> Instant {
    match parse_instant(text) {
        Some(ms) => Instant::At(ms),
        None if !text.bytes().any(|byte| byte.is_ascii_digit()) => Instant::NotADate,
        None => Instant::Unread,
    }
}

/// The milliseconds since the epoch that `Date.parse(text)` answers, for a
/// text of the one shape this module knows (see the module docs); `None`
/// for every other text.
#[must_use]
pub fn parse_instant(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    let separators = [(4, b'-'), (7, b'-'), (10, b'T'), (13, b':')];
    if separators
        .iter()
        .any(|(at, mark)| bytes.get(*at) != Some(mark))
    {
        return None;
    }
    let (year, month, day) = (
        digits(bytes, 0, 4)?,
        digits(bytes, 5, 2)?,
        digits(bytes, 8, 2)?,
    );
    let (hour, minute) = (digits(bytes, 11, 2)?, digits(bytes, 14, 2)?);
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
    {
        return None;
    }
    let mut at = 16;
    let (mut second, mut millis) = (0, 0);
    if bytes.get(at) == Some(&b':') {
        second = digits(bytes, at + 1, 2).filter(|second| *second <= 59)?;
        at += 3;
        if bytes.get(at) == Some(&b'.') {
            let fraction = bytes[at + 1..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if fraction == 0 || fraction > 9 {
                return None;
            }
            // Past the third digit a fraction is cut, not rounded.
            for place in 0..3 {
                let digit = if place < fraction {
                    i64::from(bytes[at + 1 + place] - b'0')
                } else {
                    0
                };
                millis = millis * 10 + digit;
            }
            at += 1 + fraction;
        }
    }
    let offset_minutes = match bytes.get(at)? {
        b'Z' => {
            at += 1;
            0
        }
        sign @ (b'+' | b'-') => {
            if bytes.get(at + 3) != Some(&b':') {
                return None;
            }
            let (hours, minutes) = (digits(bytes, at + 1, 2)?, digits(bytes, at + 4, 2)?);
            if hours > 23 || minutes > 59 {
                return None;
            }
            at += 6;
            (hours * 60 + minutes) * if *sign == b'+' { 1 } else { -1 }
        }
        _ => return None,
    };
    if at != bytes.len() {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    Some((seconds - offset_minutes * 60) * 1_000 + millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_instant_with_its_zone_is_read() {
        // Bun's answers.
        for (text, ms) in [
            ("2026-01-05T10:00:00Z", 1_767_607_200_000),
            ("2026-01-05T10:00Z", 1_767_607_200_000),
            ("2026-01-05T10:00:00.5Z", 1_767_607_200_500),
            ("2026-01-05T10:00:00.123456789Z", 1_767_607_200_123),
            ("2026-01-05T10:00:00.099999999Z", 1_767_607_200_099),
            ("2026-01-05T10:00:00.9999Z", 1_767_607_200_999),
            ("2026-01-05T10:00:00+01:00", 1_767_603_600_000),
            ("2026-01-05T23:59:59+23:59", 1_767_571_259_000),
            ("2026-01-05T10:00:00-00:00", 1_767_607_200_000),
            ("2024-02-29T00:00:00Z", 1_709_164_800_000),
            ("2000-02-29T00:00:00Z", 951_782_400_000),
            ("1970-01-01T00:00:00Z", 0),
            ("1969-12-31T23:59:59.999Z", -1),
            ("0000-01-01T00:00:00Z", -62_167_219_200_000),
            ("9999-12-31T23:59:59.999Z", 253_402_300_799_999),
        ] {
            assert_eq!(parse_instant(text), Some(ms), "{text}");
        }
    }

    #[test]
    fn a_text_with_no_digit_is_no_date() {
        for text in [
            "",
            "yesterday",
            "soon",
            "z-last",
            "T::Z",
            "\u{0665}\u{ff12}",
            "Jan",
        ] {
            assert_eq!(parse(text), Instant::NotADate, "{text:?}");
        }
        assert_eq!(parse("1970-01-01T00:00:01Z"), Instant::At(1_000));
        // With a digit and not of the shape: JavaScript's to say.
        for text in [
            "2026-01-05",
            "2026-01-05T10:00:00",
            "Jan 5 2026",
            "5",
            "2026-02-29T00:00:00Z",
        ] {
            assert_eq!(parse(text), Instant::Unread, "{text:?}");
        }
    }

    #[test]
    fn every_other_text_is_not_known_here() {
        for text in [
            "",
            "yesterday",
            // Read by Bun, each in a way of its own: the first of March, a
            // lower-case form, local time, a date alone, the end of a day.
            "2026-02-29T00:00:00Z",
            "1900-02-29T00:00:00Z",
            "2100-02-29T00:00:00Z",
            "2026-01-05t10:00:00z",
            "2026-01-05T10:00:00",
            "2026-01-05",
            "2026-01-05T24:00:00Z",
            "2026-01-05T10:00:00+0100",
            "2026-01-05T10:00:00.0674309079Z",
            "+002026-01-05T10:00:00Z",
            // Not dates to Bun either.
            "2026-13-01T00:00:00Z",
            "2026-01-32T00:00:00Z",
            "2026-01-05T23:60:00Z",
            "2026-01-05T23:59:60Z",
            "2026-01-05T10:00:00+24:00",
            "2026-01-05T10:00:00.Z",
            "2026-01-05T10:00:00Zjunk",
            "2026-01-05T10:00:00+01",
            "2026-1-5T10:00:00Z",
            "2026-01-05T10:00:00 Z",
            "2026-01-05T10:00:00\u{2212}01:00",
            // Digits, but not ASCII ones.
            "\u{ff12}\u{ff10}\u{ff12}\u{ff16}-01-05T10:00:00Z",
            "2026-01-05T10:00:00.\u{0665}Z",
        ] {
            assert_eq!(parse_instant(text), None, "{text:?}");
        }
    }
}

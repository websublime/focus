//! The ledger's timestamps.
//!
//! Every entry carries the instant it was written as UTC to the second:
//! `2026-10-09T16:22:42Z`. Timestamps in this form sort as text, which is
//! what lets readers order entries from several shards without parsing them.
//! The instant is a parameter, never read from the system here, so anything
//! that writes the ledger can be run against a fixed clock.
//!
//! Mirrors `isoTimestamp` in `aidlc-lib.ts` (L37961).

use std::time::{SystemTime, UNIX_EPOCH};

/// `at` as `YYYY-MM-DDTHH:MM:SSZ`, fractions of a second dropped. An instant
/// before 1970 is written as 1970-01-01T00:00:00Z.
#[must_use]
pub fn iso_timestamp(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (days, in_day) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        in_day / 3_600,
        in_day % 3_600 / 60,
        in_day % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01 (Howard Hinnant's
/// `civil_from_days`, for non-negative day counts).
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(seconds: u64) -> String {
        iso_timestamp(UNIX_EPOCH + Duration::from_secs(seconds))
    }

    #[test]
    fn known_instants_are_written_as_javascript_writes_them() {
        // Each checked against `new Date(ms).toISOString()` under Bun.
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_791_562_962), "2026-10-09T16:22:42Z");
        assert_eq!(at(4_102_444_799), "2099-12-31T23:59:59Z");
        assert_eq!(at(4_107_542_400), "2100-03-01T00:00:00Z");
    }

    #[test]
    fn fractions_of_a_second_are_dropped_not_rounded() {
        let instant = UNIX_EPOCH + Duration::from_millis(1_791_562_962_999);
        assert_eq!(iso_timestamp(instant), "2026-10-09T16:22:42Z");
    }
}

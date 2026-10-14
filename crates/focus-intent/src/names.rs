//! What a new record is called: its slug, its directory name and its id.
//!
//! Mirrors `slugify`, `dateStamp`, `uuidv7`, `resolveUniqueIntentDir` and
//! `RESERVED_RECORD_NAMES` in `aidlc-lib.ts`.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use focus_js::re;
use focus_js::{EngineError, Result, bail};

/// Names a record may never take: `help`, the intent and space verbs, and
/// the verbs reserved for later, which the router reads before a name.
pub const RESERVED_RECORD_NAMES: [&str; 11] = [
    "help",
    "list",
    "switch",
    "create",
    "archive",
    "unarchive",
    "add-repo",
    "remove-repo",
    "rename",
    "show",
    "birth",
];

/// `slugify`: free text as a slug. Lowercased; every run of anything but
/// `a-z0-9` is one hyphen; no hyphen at either end; at most `max_length`
/// characters, with no hyphen left at the end; starting with a letter, as
/// `intent-<slug>` when it does not; `intent` when nothing is left.
///
/// Upstream lowercases with JavaScript's `toLowerCase`; Rust's follows the
/// same Unicode mapping, and a character the two could map differently is
/// not a letter or digit either way, so it is a hyphen in both.
#[must_use]
pub fn slugify(text: &str, max_length: usize) -> String {
    let lowered = text.to_lowercase();
    let hyphenated = re::replace(r"[^a-z0-9]+", "g", &lowered, "-");
    let trimmed = re::replace(r"^-+|-+$", "g", &hyphenated, "");
    // ASCII by now, so a character is a UTF-16 unit.
    let capped: String = trimmed.chars().take(max_length).collect();
    let mut slug = re::replace(r"-+$", "g", &capped, "");
    if !slug.starts_with(|c: char| c.is_ascii_lowercase()) {
        slug = re::replace(r"-+$", "g", &format!("intent-{slug}"), "");
    }
    if slug.is_empty() {
        slug = "intent".to_owned();
    }
    slug
}

/// `dateStamp`: `YYMMDD` in UTC.
#[must_use]
pub fn date_stamp(at: SystemTime) -> String {
    let stamp = focus_audit::time::iso_timestamp(at);
    // `2026-10-14T…` → `261014`.
    format!("{}{}{}", &stamp[2..4], &stamp[5..7], &stamp[8..10])
}

/// `uuidv7`: a time-ordered id, as upstream mints one from a random v4: the
/// millisecond timestamp in the first 48 bits, the version nibble `7`, and
/// the random tail with its variant bits.
#[must_use]
pub fn uuid_v7(at: SystemTime) -> String {
    let mut bytes = [0_u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        let nanos = at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let mixed = nanos ^ (u128::from(std::process::id()) << 64);
        bytes = mixed.to_le_bytes();
    }
    // The variant bits a v4 carries, which the tail keeps.
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    let millis = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let ts_hex = format!("{millis:012x}");
    let ts_hex = &ts_hex[ts_hex.len() - 12..];
    let body = format!("{ts_hex}7{}", &hex[13..]);
    format!(
        "{}-{}-{}-{}-{}",
        &body[0..8],
        &body[8..12],
        &body[12..16],
        &body[16..20],
        &body[20..32]
    )
}

/// `resolveUniqueIntentDir`: `base`, or the first of `base-2`, `base-3`, …
/// that is free under `intents_root`.
///
/// # Errors
///
/// A thousand names are taken: that many same-day records with one label is
/// a runaway caller, which is said rather than looped on.
pub fn unique_intent_dir(intents_root: &Path, base: &str) -> Result<String> {
    if !intents_root.join(base).exists() {
        return Ok(base.to_owned());
    }
    const MAX_DIR_COLLISIONS: u32 = 1000;
    for n in 2..MAX_DIR_COLLISIONS {
        let candidate = format!("{base}-{n}");
        if !intents_root.join(&candidate).exists() {
            return Ok(candidate);
        }
    }
    bail!(
        "Could not find a free intent record dir for \"{base}\" after {MAX_DIR_COLLISIONS} attempts in {}. This many same-day intents with the same label indicates a bug or a runaway caller — pass a distinct --label.",
        intents_root.display()
    )
}

/// The refusal of a reserved name, as creation words it.
#[must_use]
pub fn reserved_name_refusal(slug: &str) -> EngineError {
    EngineError::new(format!(
        "\"{slug}\" is a reserved name and cannot be an intent label. Pick a label that describes the work."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_is_lowercase_hyphenated_and_capped() {
        assert_eq!(
            slugify("Add Login!! to the app", 24),
            "add-login-to-the-app"
        );
        assert_eq!(slugify("  --Simple calc--  ", 24), "simple-calc");
        assert_eq!(slugify("1st thing", 24), "intent-1st-thing");
        assert_eq!(slugify("***", 24), "intent");
        assert_eq!(slugify("", 24), "intent");
        // Capped, then a hyphen the cap leaves at the end is dropped.
        assert_eq!(slugify("abcdefghij klmnopqrstuvwxyz", 11), "abcdefghij");
        assert_eq!(
            slugify("abcdefghij klmnopqrstuvwxyz", 24),
            "abcdefghij-klmnopqrstuvw"
        );
        // Idempotent.
        assert_eq!(slugify("add-login-to-the-app", 24), "add-login-to-the-app");
        // Anything beyond ASCII is a hyphen.
        assert_eq!(slugify("Añadir çà", 24), "a-adir");
        assert_eq!(slugify("-9-", 24), "intent-9");
        assert_eq!(slugify("-", 24), "intent");
    }

    #[test]
    fn the_date_stamp_is_six_digits_utc() {
        let at = UNIX_EPOCH + std::time::Duration::from_secs(1_792_540_800);
        assert_eq!(focus_audit::time::iso_timestamp(at), "2026-10-21T00:00:00Z");
        assert_eq!(date_stamp(at), "261021");
    }

    #[test]
    fn a_v7_id_carries_the_time_and_the_version() {
        let at = UNIX_EPOCH + std::time::Duration::from_millis(0x0199_bd0a_1234);
        let id = uuid_v7(at);
        assert_eq!(id.len(), 36);
        assert!(id.starts_with("0199bd0a-1234-7"), "{id}");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"), "{id}");
        assert_ne!(uuid_v7(at), id);
    }

    #[test]
    fn a_taken_dir_name_gets_a_counter() {
        let root = std::env::temp_dir().join(format!("focus-names-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("261014-x")).unwrap();
        std::fs::create_dir_all(root.join("261014-x-2")).unwrap();
        assert_eq!(unique_intent_dir(&root, "261014-y").unwrap(), "261014-y");
        assert_eq!(unique_intent_dir(&root, "261014-x").unwrap(), "261014-x-3");
        let _ = std::fs::remove_dir_all(&root);
    }
}

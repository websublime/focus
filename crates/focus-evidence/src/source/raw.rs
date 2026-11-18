//! Hashes this process took both ways: over a file's raw bytes, and over
//! its text with each CRLF read as LF.
//!
//! A value recorded before line endings were read as LF was taken over the
//! raw bytes. Only content read in this process is remembered, so a
//! recorded raw value maps to the current form of the same content and to
//! nothing else (`RAW_FINGERPRINTS` and `CURRENT_FINGERPRINTS`,
//! `aidlc-lib.ts` L15 780).

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// The two maps: raw to current, and current to raw.
#[derive(Default)]
struct Forms {
    by_raw: HashMap<String, String>,
    by_current: HashMap<String, String>,
}

static FORMS: Mutex<Option<Forms>> = Mutex::new(None);

fn with<T>(read: impl FnOnce(&mut Forms) -> T) -> T {
    let mut forms = FORMS.lock().unwrap_or_else(PoisonError::into_inner);
    read(forms.get_or_insert_with(Forms::default))
}

/// `rememberRawFingerprint`.
pub fn remember(raw: &str, current: &str) {
    if raw == current {
        return;
    }
    with(|forms| {
        forms.by_raw.insert(raw.to_owned(), current.to_owned());
        forms.by_current.insert(current.to_owned(), raw.to_owned());
    });
}

/// `currentFingerprintForm`: a recorded value as it reads now; the value
/// itself unless it was taken over raw line endings of content read here.
#[must_use]
pub fn current_form(value: &str) -> String {
    with(|forms| forms.by_raw.get(value).cloned()).unwrap_or_else(|| value.to_owned())
}

/// `rawFingerprintForm`: the raw-bytes form of a current value this process
/// computed, when it differs.
#[must_use]
pub fn raw_form(value: &str) -> Option<String> {
    with(|forms| forms.by_current.get(value).cloned())
}

/// `committedTextBytes`: the bytes with each CRLF read as LF, when the
/// bytes are text with CRLFs; the bytes as they are otherwise (a lone CR,
/// a NUL, or too many control characters say they are not text).
#[must_use]
pub fn committed_text_bytes(bytes: &[u8]) -> Vec<u8> {
    if !bytes.contains(&13)
        || focus_workspace::env::var("TEST_RAW_LINE_ENDINGS").as_deref() == Some("1")
    {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut printable: u64 = 0;
    let mut nonprintable: i64 = 0;
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == 13 {
            if at + 1 == bytes.len() || bytes[at + 1] != 10 {
                return bytes.to_vec();
            }
            at += 1;
            continue;
        }
        if byte == 0 {
            return bytes.to_vec();
        }
        if byte == 127 || (byte < 32 && !matches!(byte, 10 | 8 | 9 | 27 | 12)) {
            nonprintable += 1;
        } else if byte != 10 {
            printable += 1;
        }
        out.push(byte);
        at += 1;
    }
    // A trailing DOS end-of-file mark is not counted against the text.
    if bytes.last() == Some(&26) {
        nonprintable -= 1;
    }
    if i64::try_from(printable >> 7).unwrap_or(i64::MAX) >= nonprintable {
        out
    } else {
        bytes.to_vec()
    }
}

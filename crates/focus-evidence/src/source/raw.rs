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

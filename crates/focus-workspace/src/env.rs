//! Focus's environment variables.
//!
//! Callers name a variable without its prefix (`var("PROJECT_DIR")` reads
//! `FOCUS_PROJECT_DIR`), which keeps the prefix in one place and makes every
//! variable Focus reads findable by searching for `env::var`.

use std::ffi::OsString;

use crate::names::ENV_PREFIX;

/// The full name of a Focus variable: `full_name("PROJECT_DIR")` is
/// `FOCUS_PROJECT_DIR`.
#[must_use]
pub fn full_name(name: &str) -> String {
    format!("{ENV_PREFIX}{name}")
}

/// A Focus variable, when set to valid Unicode. An empty value counts as set.
#[must_use]
pub fn var(name: &str) -> Option<String> {
    std::env::var(full_name(name)).ok()
}

/// A Focus variable holding a path, when set and not empty.
#[must_use]
pub fn path(name: &str) -> Option<OsString> {
    std::env::var_os(full_name(name)).filter(|value| !value.is_empty())
}

/// Whether a Focus switch is on: set to exactly `1`.
#[must_use]
pub fn is_on(name: &str) -> bool {
    var(name).as_deref() == Some("1")
}

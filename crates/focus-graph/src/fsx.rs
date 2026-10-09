//! Filesystem reads with the semantics the TypeScript engine gets from Node.

use std::fs;
use std::path::Path;

use focus_js::order::cmp_utf16;
use focus_js::{EngineError, Result};

/// `readFileSync(path, "utf-8")`: invalid UTF-8 becomes U+FFFD instead of an
/// error, so a stray byte in an authored file surfaces as a schema error with
/// the file's name on it.
pub fn read_text(path: &Path) -> Result<String> {
    let bytes =
        fs::read(path).map_err(|err| EngineError::new(format!("{}: {err}", path.display())))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `readdirSync(dir).sort()`: entry names in UTF-16 code-unit order. Directory
/// order is filesystem-dependent; sorting is what makes compiled output
/// identical across machines.
pub fn read_dir_sorted(dir: &Path) -> Result<Vec<String>> {
    let entries =
        fs::read_dir(dir).map_err(|err| EngineError::new(format!("{}: {err}", dir.display())))?;
    let mut names: Vec<String> = entries
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort_by(|a, b| cmp_utf16(a, b));
    Ok(names)
}

/// A harness-relative path as upstream prints it: POSIX separators.
#[must_use]
pub fn posix_join(parts: &[&str]) -> String {
    parts.join("/")
}

//! Writing an entry: what the ledger accepts and the bytes it becomes.
//!
//! An entry is an event type and a set of named fields. It is written as one
//! Markdown block, and the block's shape is what every reader depends on: one
//! `**Timestamp**:` line, one `**Event**:` line, one line per field, and a
//! `---` rule that ends it. Validation and rendering exist to keep that true
//! whatever a caller passes: a field can neither add a second event line nor
//! spill onto a second physical line.
//!
//! Mirrors `validateAuditEntry` and `renderAuditBlock` in `aidlc-audit.ts`
//! (L673, L690) and `redactProjectDirPrefix` in `aidlc-lib.ts` (L38175).

use std::path::Path;

use focus_js::order::array_index;
use focus_js::text::is_js_whitespace;
use focus_js::{Result, bail, json, re};
use serde_json::Value;

use crate::events::{EVENTS, heading};

/// The first line of a shard. Written once, when the file is created.
pub const SHARD_TITLE: &str = "# Focus Audit Log\n";

/// What replaces the project's own location wherever a field value names it.
pub const PROJECT_DIR_TOKEN: &str = "<project-dir>";

/// The shape of a field name: one Markdown label on one physical line.
const FIELD_KEY_PATTERN: &str = r"^[A-Za-z][A-Za-z0-9 ._()/-]*$";

/// The fields of an entry, with the semantics of the JavaScript object
/// upstream keeps them in: setting a name again replaces its value where it
/// stands, and names that are array indices are enumerated first, in
/// ascending order, ahead of the rest in insertion order. The second rule
/// only shows in which name a refusal reports first, since no index is a
/// valid field name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Fields(Vec<(String, String)>);

impl Fields {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Assign `value` to `key`.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let (key, value) = (key.into(), value.into());
        match self.0.iter_mut().find(|(name, _)| *name == key) {
            Some(slot) => slot.1 = value,
            None => self.0.push((key, value)),
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The fields in the order a JavaScript object enumerates them.
    #[must_use]
    pub fn entries(&self) -> Vec<(&str, &str)> {
        let mut indexed: Vec<(u32, &str, &str)> = Vec::new();
        let mut named: Vec<(&str, &str)> = Vec::new();
        for (key, value) in &self.0 {
            match array_index(key) {
                Some(index) => indexed.push((index, key, value)),
                None => named.push((key, value)),
            }
        }
        indexed.sort_by_key(|(index, _, _)| *index);
        indexed
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .chain(named)
            .collect()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for Fields {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(pairs: I) -> Self {
        let mut fields = Self::new();
        for (key, value) in pairs {
            fields.set(key, value);
        }
        fields
    }
}

/// One entry to append: an event type and its fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub event_type: String,
    pub fields: Fields,
}

impl Entry {
    #[must_use]
    pub fn new(event_type: impl Into<String>, fields: Fields) -> Self {
        Self {
            event_type: event_type.into(),
            fields,
        }
    }
}

/// Refuse an entry the ledger must not hold.
///
/// # Errors
///
/// The event type is not in the vocabulary; a field is named `Event`, which
/// would write a second event line and let one block answer for two events;
/// or a field name is not a single Markdown label.
pub fn validate(entry: &Entry) -> Result<()> {
    if heading(&entry.event_type).is_none() {
        let known: Vec<&str> = EVENTS.iter().map(|(name, _)| *name).collect();
        bail!(
            "Invalid event type: {}. Must be one of: {}",
            entry.event_type,
            known.join(", ")
        );
    }
    for (key, _) in entry.fields.entries() {
        if key == "Event" {
            bail!(
                "Reserved field key: {key}. The emitter writes **{key}**: itself; a caller-supplied value would forge a second matching line and spoof multiline event queries."
            );
        }
        if !re::test(FIELD_KEY_PATTERN, "", key) {
            bail!(
                "Invalid audit field key: {}. Field keys must match /{FIELD_KEY_PATTERN}/ so they remain one Markdown label on one physical line.",
                json::stringify(&Value::String(key.to_owned()))
            );
        }
    }
    Ok(())
}

/// The block an entry is written as. `entry` has passed [`validate`].
///
/// The writer owns the timestamp and event lines, so a field named
/// `Timestamp` is dropped rather than written a second time. Every line
/// terminator in a value becomes the two characters `\n`, and the project's
/// own location becomes [`PROJECT_DIR_TOKEN`].
#[must_use]
pub fn render_block(entry: &Entry, timestamp: &str, project_dir: &Path) -> String {
    let title = heading(&entry.event_type).unwrap_or(&entry.event_type);
    let mut block = format!(
        "\n## {title}\n**Timestamp**: {timestamp}\n**Event**: {}\n",
        entry.event_type
    );
    for (key, value) in entry.fields.entries() {
        if key == "Timestamp" || key == "Event" {
            continue;
        }
        let value = escape_line_terminators(&redact_project_dir_prefix(value, project_dir));
        block.push_str(&format!("**{key}**: {value}\n"));
    }
    block.push_str("\n---\n");
    block
}

fn escape_line_terminators(value: &str) -> String {
    value
        .replace("\r\n", "\\n")
        .replace(['\r', '\n', '\u{2028}', '\u{2029}'], "\\n")
}

/// Replace the project's location with [`PROJECT_DIR_TOKEN`] wherever `value`
/// names it as a path prefix: followed by a separator, by white space, or by
/// nothing. Both the path as given and the path with links resolved are
/// recognised, each with either separator and, for a drive-letter path, with
/// either case of the letter.
#[must_use]
pub fn redact_project_dir_prefix(value: &str, project_dir: &Path) -> String {
    let mut variants: Vec<String> = Vec::new();
    let add = |variants: &mut Vec<String>, variant: String| {
        if !variants.contains(&variant) {
            variants.push(variant);
        }
    };
    add(&mut variants, lexical_absolute(project_dir));
    if let Ok(real) = std::fs::canonicalize(project_dir) {
        add(
            &mut variants,
            without_verbatim_prefix(&real.to_string_lossy()),
        );
    }
    for variant in variants.clone() {
        add(&mut variants, variant.replace('\\', "/"));
        add(&mut variants, variant.replace('/', "\\"));
    }
    for variant in variants.clone() {
        if !re::test(r"^[a-zA-Z]:(?:[\\/]|$)", "", &variant) {
            continue;
        }
        let (letter, rest) = variant.split_at(1);
        add(&mut variants, format!("{}{rest}", letter.to_uppercase()));
        add(&mut variants, format!("{}{rest}", letter.to_lowercase()));
    }
    // Longest first, so a path is not redacted through a shorter spelling of
    // one of its own prefixes. Stable, as upstream's sort is.
    variants.sort_by_key(|variant| std::cmp::Reverse(variant.encode_utf16().count()));

    let mut redacted = value.to_owned();
    for variant in variants.iter().filter(|variant| !variant.is_empty()) {
        let mut offset = 0;
        while offset < redacted.len() {
            let Some(found) = redacted[offset..].find(variant.as_str()) else {
                break;
            };
            let index = offset + found;
            let end = index + variant.len();
            let ends_the_path = redacted[end..]
                .chars()
                .next()
                .is_none_or(|next| next == '/' || next == '\\' || is_js_whitespace(next));
            if !ends_the_path {
                offset = end;
                continue;
            }
            redacted.replace_range(index..end, PROJECT_DIR_TOKEN);
            offset = index + PROJECT_DIR_TOKEN.len();
        }
    }
    redacted
}

/// `path.resolve`: absolute, with `.` and `..` settled by reading the path,
/// not the disk.
pub(crate) fn lexical_absolute_path(path: &Path) -> std::path::PathBuf {
    focus_workspace::project::settled(path)
}

fn lexical_absolute(path: &Path) -> String {
    lexical_absolute_path(path).to_string_lossy().into_owned()
}

/// Windows spells a canonical path `\\?\C:\...`; people and tools spell it
/// `C:\...`, and that is the spelling to recognise in a value.
fn without_verbatim_prefix(path: &str) -> String {
    if let Some(share) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{share}")
    } else {
        path.strip_prefix(r"\\?\").unwrap_or(path).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(event_type: &str, pairs: &[(&str, &str)]) -> Entry {
        Entry::new(event_type, pairs.iter().copied().collect())
    }

    #[test]
    fn a_block_has_one_timestamp_one_event_and_one_line_per_field() {
        let entry = entry(
            "ERROR_LOGGED",
            &[
                ("Tool", "focus-state"),
                ("Timestamp", "forged"),
                ("Note", "a\r\nb\rc\nd\u{2028}e"),
            ],
        );
        validate(&entry).unwrap();
        assert_eq!(
            render_block(&entry, "2026-10-09T16:22:42Z", Path::new("/nowhere")),
            "\n## Error Logged\n**Timestamp**: 2026-10-09T16:22:42Z\n**Event**: ERROR_LOGGED\n**Tool**: focus-state\n**Note**: a\\nb\\nc\\nd\\ne\n\n---\n"
        );
    }

    #[test]
    fn refusals_name_the_type_or_the_field() {
        let unknown = validate(&entry("NOPE", &[])).unwrap_err();
        assert!(
            unknown
                .message()
                .starts_with("Invalid event type: NOPE. Must be one of: STAGE_STARTED, "),
            "{unknown}"
        );
        assert!(
            validate(&entry("ERROR_LOGGED", &[("Event", "HUMAN_TURN")]))
                .unwrap_err()
                .message()
                .starts_with("Reserved field key: Event.")
        );
        // An index-like name is enumerated first, so it is the one reported.
        assert_eq!(
            validate(&entry("ERROR_LOGGED", &[("9x", "a"), ("1", "b")]))
                .unwrap_err()
                .message(),
            "Invalid audit field key: \"1\". Field keys must match /^[A-Za-z][A-Za-z0-9 ._()/-]*$/ so they remain one Markdown label on one physical line."
        );
    }

    #[test]
    fn assigning_a_field_again_keeps_its_place() {
        let mut fields = Fields::new();
        fields.set("b", "1");
        fields.set("a", "2");
        fields.set("b", "3");
        assert_eq!(fields.entries(), [("b", "3"), ("a", "2")]);
        assert_eq!(fields.get("a"), Some("2"));
    }

    #[test]
    fn the_project_location_is_redacted_only_as_a_path_prefix() {
        let project = std::env::temp_dir().join(format!("focus-redact-{}", std::process::id()));
        std::fs::create_dir_all(&project).unwrap();
        let here = lexical_absolute(&project);
        let value = format!("at {here}/x and {here}x and {here}");
        assert_eq!(
            redact_project_dir_prefix(&value, &project),
            format!("at <project-dir>/x and {here}x and <project-dir>")
        );
        std::fs::remove_dir_all(&project).unwrap();
    }
}

//! A source listing: every path of a source tree with what it held.
//!
//! A key is `<repo>\0<path>`, with an empty repo for a project that is one
//! repository. An entry is `<mode> <object id>` (six digits, then forty or
//! sixty-four lowercase hex digits), or the object id alone in a listing
//! recorded before modes were kept.
//!
//! The text form is one row per path, tab-separated, sorted by key, each
//! row ended by a line feed (`serializeSourceListing`, `aidlc-lib.ts`
//! L24 376). It is what a snapshot file holds and what its hash is taken
//! over, so it is reproduced to the byte.

use std::collections::HashMap;

use focus_js::order::cmp_utf16;
use focus_workspace::project::is_valid_repo_name;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// `GIT_OBJECT_ID_RE`: forty or sixty-four lowercase hex digits.
fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// `^\d{6}$`: a mode, as six digits.
fn is_mode(text: &str) -> bool {
    text.len() == 6 && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// The mode and the object id of an entry that carries its mode. Neither
/// part holds a space, so which space parts them is not a question.
fn modern(entry: &str) -> Option<(&str, &str)> {
    let (mode, oid) = entry.split_once(' ')?;
    (is_mode(mode) && is_object_id(oid)).then_some((mode, oid))
}

/// A listing, in the order its paths were first set: upstream's is a `Map`,
/// and the first entry it cannot write is the one its error names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    entries: Vec<(String, String)>,
    index: HashMap<String, usize>,
}

impl Listing {
    /// An empty listing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a key's entry. A key set again keeps its place.
    pub fn set(&mut self, key: impl Into<String>, entry: impl Into<String>) {
        let key = key.into();
        if let Some(&at) = self.index.get(&key) {
            self.entries[at].1 = entry.into();
        } else {
            self.index.insert(key.clone(), self.entries.len());
            self.entries.push((key, entry.into()));
        }
    }

    /// The entry of a key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&str> {
        self.index.get(key).map(|&at| self.entries[at].1.as_str())
    }

    /// Whether a key is listed.
    #[must_use]
    pub fn has(&self, key: &str) -> bool {
        self.index.contains_key(key)
    }

    /// How many paths are listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is listed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The keys and entries, in the order the keys were first set.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, entry)| (key.as_str(), entry.as_str()))
    }
}

impl<K: Into<String>, E: Into<String>> FromIterator<(K, E)> for Listing {
    fn from_iter<I: IntoIterator<Item = (K, E)>>(pairs: I) -> Self {
        let mut listing = Self::new();
        for (key, entry) in pairs {
            listing.set(key, entry);
        }
        listing
    }
}

/// `sourcePathKey`: the key of a path in a repo.
///
/// # Errors
///
/// A NUL in either part: the key could not be split again.
pub fn path_key(repo: &str, path: &str) -> Result<String, String> {
    if repo.contains('\0') || path.contains('\0') {
        return Err("Source path keys cannot contain NUL bytes".to_owned());
    }
    Ok(format!("{repo}\0{path}"))
}

/// `splitSourcePathKey`: the repo and the path of a key, when it has
/// exactly one NUL, a path, and a repo that is empty or a repo's name.
#[must_use]
pub fn split_path_key(key: &str) -> Option<(&str, &str)> {
    // A second NUL is refused wherever it falls: in the path here, or in a
    // repo that is then not a name.
    let (repo, path) = key.split_once('\0')?;
    if path.contains('\0') || path.is_empty() || (!repo.is_empty() && !is_valid_repo_name(repo)) {
        return None;
    }
    Some((repo, path))
}

/// `sourceListingFieldEncode`: the four characters that would make a row
/// ambiguous, escaped with a backslash.
fn encode_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// `sourceListingFieldDecode`: `None` for a backslash that escapes nothing
/// the encoder writes.
fn decode_field(value: &str) -> Option<String> {
    let mut decoded = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            decoded.push(c);
            continue;
        }
        decoded.push(match chars.next()? {
            '\\' => '\\',
            't' => '\t',
            'n' => '\n',
            'r' => '\r',
            _ => return None,
        });
    }
    Some(decoded)
}

/// `serializeSourceListing`: the text a snapshot holds.
///
/// # Errors
///
/// A key that is not a path key, or an entry that is neither form: the
/// first one in the listing's order, in upstream's words.
pub fn serialize(listing: &Listing) -> Result<String, String> {
    let mut rows: Vec<(&str, String)> = Vec::with_capacity(listing.len());
    for (key, entry) in listing.iter() {
        let Some((repo, path)) = split_path_key(key) else {
            return Err("Invalid canonical source-listing path key".to_owned());
        };
        let suffix = if let Some((mode, oid)) = modern(entry) {
            format!("{mode}\t{oid}")
        } else if is_object_id(entry) {
            entry.to_owned()
        } else {
            return Err(format!(
                "Invalid source-listing entry for {}",
                focus_js::json::stringify(&Value::String(path.to_owned()))
            ));
        };
        rows.push((
            key,
            format!("{}\t{}\t{suffix}", encode_field(repo), encode_field(path)),
        ));
    }
    rows.sort_by(|a, b| cmp_utf16(a.0, b.0));
    let mut text = String::new();
    for (_, line) in rows {
        text.push_str(&line);
        text.push('\n');
    }
    Ok(text)
}

/// `parseSourceListing`: the listing a snapshot's text holds; `None` for
/// text that is not one (a row with the wrong number of fields, a field
/// that does not decode, a repo that is not a name, a mode or an object id
/// of the wrong shape, a path listed twice, a last row without its line
/// feed).
///
/// # Errors
///
/// A path with a NUL in it, in upstream's words: its key cannot be made,
/// and upstream throws where every other fault answers "not a listing".
pub fn parse(serialized: &str) -> Result<Option<Listing>, String> {
    if serialized.is_empty() {
        return Ok(Some(Listing::new()));
    }
    let Some(body) = serialized.strip_suffix('\n') else {
        return Ok(None);
    };
    let mut listing = Listing::new();
    for line in body.split('\n') {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != 3 && fields.len() != 4 {
            return Ok(None);
        }
        let (Some(repo), Some(path)) = (decode_field(fields[0]), decode_field(fields[1])) else {
            return Ok(None);
        };
        let mode = (fields.len() == 4).then(|| fields[2]);
        let oid = fields[fields.len() - 1];
        if path.is_empty()
            || (!repo.is_empty() && !is_valid_repo_name(&repo))
            || mode.is_some_and(|mode| !is_mode(mode))
            || !is_object_id(oid)
        {
            return Ok(None);
        }
        let key = path_key(&repo, &path)?;
        if listing.has(&key) {
            return Ok(None);
        }
        listing.set(
            key,
            match mode {
                Some(mode) => format!("{mode} {oid}"),
                None => oid.to_owned(),
            },
        );
    }
    Ok(Some(listing))
}

/// `sourceListingSha256`: the hash a snapshot is addressed and verified by.
#[must_use]
pub fn sha256(serialized: &str) -> String {
    let digest = Sha256::digest(serialized.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `sourceListingEntriesEqual`: whether two entries say the path held the
/// same thing. Two entries with modes are equal only when identical; an
/// entry without a mode equals one with the same object id.
/// An entry recorded over raw line endings of a file this process read is
/// that file ([`super::raw`]).
#[must_use]
pub fn entries_equal(left: Option<&str>, right: Option<&str>) -> bool {
    if left == right {
        return true;
    }
    let (Some(left), Some(right)) = (left, right) else {
        return false;
    };
    if super::raw::current_form(left) == super::raw::current_form(right) {
        return true;
    }
    let (left_modern, right_modern) = (modern(left), modern(right));
    if left_modern.is_some() && right_modern.is_some() {
        return false;
    }
    // An entry that is neither form has no object id; taking the entry
    // itself for one would change nothing, since the other side's is one.
    let left_oid = left_modern
        .map(|(_, oid)| oid)
        .or_else(|| is_object_id(left).then_some(left));
    let right_oid = right_modern
        .map(|(_, oid)| oid)
        .or_else(|| is_object_id(right).then_some(right));
    left_oid.is_some() && left_oid == right_oid
}

/// `renderSourcePathKeys`: each key as `repo/path` (or `path` in a project
/// that is one repository), once, sorted. A key that is not a path key is
/// kept as it is.
#[must_use]
pub fn render_path_keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut known = std::collections::HashSet::new();
    for key in keys {
        if known.insert(key) {
            seen.push(key);
        }
    }
    let mut rendered: Vec<String> = seen
        .into_iter()
        .map(|key| match split_path_key(key) {
            None => key.to_owned(),
            Some(("", path)) => path.to_owned(),
            Some((repo, path)) => format!("{repo}/{path}"),
        })
        .collect();
    rendered.sort_by(|a, b| cmp_utf16(a, b));
    rendered
}

/// `sourceListingChangedPaths`: the paths whose entry differs between two
/// listings, or that only one of them has, rendered and sorted.
#[must_use]
pub fn changed_paths(before: &Listing, after: &Listing) -> Vec<String> {
    let mut changed: Vec<&str> = Vec::new();
    for (key, entry) in before.iter() {
        if !entries_equal(after.get(key), Some(entry)) {
            changed.push(key);
        }
    }
    for (key, _) in after.iter() {
        if !before.has(key) {
            changed.push(key);
        }
    }
    render_path_keys(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    /// Two paths that are both absent hold the same thing. No listing
    /// compares one, so the corpus cannot say it.
    #[test]
    fn two_absent_entries_are_equal() {
        assert!(entries_equal(None, None));
        assert!(!entries_equal(None, Some(OID)));
        assert!(!entries_equal(Some(OID), None));
    }

    #[test]
    fn a_key_set_again_keeps_its_place_and_takes_the_new_entry() {
        let mut listing = Listing::new();
        listing.set("\0b", OID);
        listing.set("\0a", OID);
        listing.set("\0b", format!("100644 {OID}"));
        let entries: Vec<_> = listing.iter().collect();
        assert_eq!(entries[0], ("\0b", format!("100644 {OID}").as_str()));
        assert_eq!(listing.len(), 2);
        assert_eq!(listing.get("\0a"), Some(OID));
        assert!(!listing.has("\0c") && !listing.is_empty());
    }

    #[test]
    fn a_listing_reads_back_as_it_was_written() {
        let listing: Listing = [
            (path_key("", "a\tb").unwrap(), format!("100755 {OID}")),
            (path_key("api", "src/x.rs").unwrap(), OID.to_owned()),
        ]
        .into_iter()
        .collect();
        let text = serialize(&listing).unwrap();
        assert_eq!(
            text,
            format!("\ta\\tb\t100755\t{OID}\napi\tsrc/x.rs\t{OID}\n")
        );
        assert_eq!(parse(&text).unwrap().unwrap(), listing);
        assert_eq!(
            sha256(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}

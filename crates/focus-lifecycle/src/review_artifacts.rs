//! What a review is of: the declared artifacts of a stage, read once and
//! fingerprinted as they are on disk.
//!
//! A review is requested of a set of files and its verdict is evidence for
//! those files only. The request's row carries their fingerprint; whoever
//! weighs the verdict later takes the fingerprint again and compares. This
//! module takes it (`reviewArtifactSnapshot`, `reviewArtifactFingerprint`,
//! `aidlc-lib.ts` L16 004, L16 072) and says whether a request's still
//! describes the files (`reviewRequestArtifactsCurrent`,
//! `reviewAppendedAfterRequest`, L18 964).
//!
//! # What is fingerprinted
//!
//! Every artifact the stage declares, required or optional, under its
//! logical name: the path within the record (or the code knowledge base),
//! which is the same in a bolt's worktree and after its merge. Each is
//! `missing`, `not-file`, or the hash of its bytes, CRLF read as LF where
//! Git would convert the file. A stage's questions, when the person
//! confirms a summary in them, are hashed without that answer
//! ([`focus_evidence::summary::review_input_fingerprint`]). A missing file
//! is an entry, so creating it after the review changes the fingerprint.
//!
//! # What is read, and how
//!
//! Each file is read through one identity: no link on the way from its
//! boundary down, a regular file with one name, unchanged between being
//! found and being read to the end ([`focus_workspace::files`]). When all
//! are read, each is looked at again, so the set is one moment's and not a
//! file from before an edit beside a file from after it. Anything else
//! (a link, a second name, a path that leaves its boundary, a file that
//! moved) is no snapshot at all, never a fingerprint of something else.
//!
//! Upstream keeps every file open until the second look and compares the
//! open descriptor's identity as well as the name's. Here a file is closed
//! when it is read; the name's identity is compared, which is what a
//! replaced or rewritten file changes.
//!
//! # The appendix view
//!
//! Reviews were once appended to the reviewed file as a terminal
//! `## Review` section. A snapshot still says where such a section starts
//! in the stage's `review_artifact`, and the fingerprints the set has
//! without it, so a review requested under that protocol can be
//! recognised. It is read, never written.
//!
//! # Tests that change no answer
//!
//! The target of the appendix view is the first entry marked as the
//! stage's `review_artifact`: the entries are already in the order
//! upstream sorts those entries by. A path is built under its boundary, so
//! the test that it is lexically inside it can only fail for a unit or a
//! repository whose name climbs out (`..`), and the test on the resolved
//! path only where a link was followed, which the walk refuses first.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use focus_evidence::review::RequestBinding;
use focus_evidence::source::raw;
use focus_evidence::summary::review_input_fingerprint;
use focus_graph::runtime::Stage;
use focus_guards::posture::terminal_review_appendix_boundary;
use focus_js::json;
use focus_js::order::locale_compare;
use focus_workspace::files::{self, Identity};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::artifacts::{ReviewEntry, ReviewScope, review_entries};
use crate::validity::committed_text_bytes;
use crate::workflow::Workflow;

/// `ReviewArtifactSnapshot`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The fingerprint of every declared artifact as it is on disk.
    pub fingerprint: String,
    /// The logical path of the stage's `review_artifact`.
    pub review_artifact: String,
    /// The fingerprints without a terminal `## Review` section on the
    /// review artifact, at the two boundaries an appender may have used:
    /// the heading line, and that line with the white space before it
    /// reduced to one line end. Only [`Self::fingerprint`] when the
    /// artifact has no such section.
    pub body_fingerprints: Vec<String>,
    /// The section's bytes from its heading line; empty when there is none.
    pub appendix: Vec<u8>,
}

/// What a caller asks of a fingerprint.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options<'a> {
    /// `requireRequiredArtifacts`: no fingerprint while an artifact the
    /// stage owes is not a file.
    pub require_required_artifacts: bool,
    /// See [`ReviewScope::merged_bolt_units`].
    pub merged_bolt_units: Option<&'a BTreeSet<String>>,
}

/// What a name held when it was read.
#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Missing,
    NotFile,
    File(Vec<u8>),
}

/// `ReviewArtifactContent`.
struct Content {
    entry: ReviewEntry,
    state: State,
    /// What the name was when it was read, for a name that held something.
    identity: Option<Identity>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `path.resolve` of a path that is already absolute: `.` dropped and `..`
/// taken with the name before it, by the names alone.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `inspectArtifactPath`: `Some(None)` when a name on the way from
/// `boundary` down to `path` is not there, `Some(Some(real))` (the
/// boundary, resolved) when all are and none is a link, and `None` when
/// the path cannot be trusted: it leaves the boundary, the boundary cannot
/// be resolved, a name on the way is a link, or one cannot be looked at.
fn inspect(boundary: &Path, path: &Path) -> Option<Option<PathBuf>> {
    let boundary = lexical(boundary);
    let path = lexical(path);
    let rel = path.strip_prefix(&boundary).ok()?.to_path_buf();
    let real = std::fs::canonicalize(&boundary).ok()?;
    let mut cursor = boundary;
    for segment in rel.components() {
        cursor.push(segment.as_os_str());
        match std::fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => return None,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(None),
            Err(_) => return None,
        }
    }
    Some(Some(real))
}

/// Whether `path`, resolved, is inside `real` (`pathContainedBy`).
fn resolves_inside(real: &Path, path: &Path) -> bool {
    std::fs::canonicalize(path).is_ok_and(|resolved| resolved.starts_with(real))
}

/// `readStableReviewArtifacts`: every entry read, in the order of their
/// logical paths; `None` when one of them cannot be trusted or the set
/// changed while it was read.
fn read_stable(mut entries: Vec<ReviewEntry>) -> Option<Vec<Content>> {
    entries.sort_by(|a, b| locale_compare(&a.logical_path, &b.logical_path));
    let mut contents: Vec<Content> = Vec::with_capacity(entries.len());
    for entry in entries {
        let missing = |entry: ReviewEntry| Content {
            entry,
            state: State::Missing,
            identity: None,
        };
        // Upstream joins a path by its names, so a unit named `a/../b` has
        // the files of `b` under its own logical name.
        let Some(path) = entry.path.as_deref().map(lexical) else {
            contents.push(missing(entry));
            continue;
        };
        let entry = ReviewEntry {
            path: Some(path.clone()),
            ..entry
        };
        let Some(real) = inspect(&entry.boundary, &path)? else {
            contents.push(missing(entry));
            continue;
        };
        let before = std::fs::symlink_metadata(&path).ok()?;
        let identity = Some(files::identity(&before));
        if !before.is_file() {
            contents.push(Content {
                entry,
                state: State::NotFile,
                identity,
            });
            continue;
        }
        if files::hard_links(&before) != 1 {
            return None;
        }
        let body = files::read_regular(&path, "review artifact").ok()?;
        if !resolves_inside(&real, &path) {
            return None;
        }
        contents.push(Content {
            entry,
            state: State::File(body),
            identity,
        });
    }
    // The second look: what was not there is still not there, and what was
    // is what it was.
    for content in &contents {
        let Some(path) = &content.entry.path else {
            continue;
        };
        let inspected = inspect(&content.entry.boundary, path)?;
        match (&content.identity, inspected) {
            (None, None) => {}
            (Some(identity), Some(real)) => {
                let now = std::fs::symlink_metadata(path).ok()?;
                if files::identity(&now) != *identity || !resolves_inside(&real, path) {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(contents)
}

/// `reviewArtifactContentsFingerprint`: the fingerprint of the set, with
/// the bytes of `appendix`'s artifact cut at its offset when one is named.
/// `None` when an owed artifact is not a file and `require_required` asks
/// for them all, when the named artifact is not the review artifact of the
/// set or the offset is past its end, and when a questions file cannot be
/// read as Markdown.
fn contents_fingerprint(
    contents: &[Content],
    require_required: bool,
    appendix: Option<(&str, usize)>,
) -> Option<String> {
    let mut manifest: Vec<Value> = Vec::with_capacity(contents.len());
    // The same manifest over raw bytes, as it was recorded before line
    // endings were read as LF; remembered when it differs.
    let mut raw_manifest: Vec<Value> = Vec::with_capacity(contents.len());
    let mut raw_differs = false;
    let mut matched = appendix.is_none();
    for content in contents {
        let entry = &content.entry;
        let owed = entry.required && require_required;
        let body = match &content.state {
            State::File(body) => body,
            other => {
                if owed {
                    return None;
                }
                let value = json!([
                    entry.logical_path,
                    if *other == State::Missing {
                        "missing"
                    } else {
                        "not-file"
                    }
                ]);
                manifest.push(value.clone());
                raw_manifest.push(value);
                continue;
            }
        };
        if entry.summary_input {
            let value = json!([
                entry.logical_path,
                format!(
                    "summary-input:sha256:{}",
                    review_input_fingerprint(body).ok()?
                )
            ]);
            manifest.push(value.clone());
            raw_manifest.push(value);
            continue;
        }
        let mut fingerprinted: &[u8] = body;
        if let Some((artifact, offset)) = appendix
            && artifact == entry.logical_path
        {
            if !entry.appendix_target || offset > body.len() {
                return None;
            }
            fingerprinted = &body[..offset];
            matched = true;
        }
        let text = committed_text_bytes(fingerprinted);
        manifest.push(json!([
            entry.logical_path,
            format!("sha256:{}", sha256_hex(&text))
        ]));
        raw_manifest.push(json!([
            entry.logical_path,
            format!("sha256:{}", sha256_hex(fingerprinted))
        ]));
        raw_differs |= text != fingerprinted;
    }
    if !matched {
        return None;
    }
    let print = |manifest: Vec<Value>| {
        format!(
            "sha256:{}",
            sha256_hex(json::stringify(&Value::Array(manifest)).as_bytes())
        )
    };
    let fingerprint = print(manifest);
    if raw_differs {
        raw::remember(&print(raw_manifest), &fingerprint);
    }
    Some(fingerprint)
}

/// The stage's entries read; `None` when there is no record, a folder
/// upstream lists cannot be listed, the record's repositories cannot be
/// read, or the files cannot be read as one set.
fn contents(wf: &Workflow, stage: &Stage, scope: ReviewScope<'_>) -> Option<Vec<Content>> {
    read_stable(review_entries(wf, stage, scope).ok()?.ok()??)
}

/// `reviewArtifactSnapshot`: the review's input as it is on disk, with the
/// appendix view of the stage's `review_artifact`. `None` when the set has
/// no fingerprint (see [`fingerprint`]) or the review artifact is not a
/// file.
#[must_use]
pub fn snapshot(
    wf: &Workflow,
    stage: &Stage,
    unit: Option<&str>,
    options: Options<'_>,
) -> Option<Snapshot> {
    let contents = contents(
        wf,
        stage,
        ReviewScope {
            unit,
            state: None,
            merged_bolt_units: options.merged_bolt_units,
        },
    )?;
    let require = options.require_required_artifacts;
    let fingerprint = contents_fingerprint(&contents, require, None)?;
    let target = contents
        .iter()
        .find(|content| content.entry.appendix_target)?;
    let State::File(body) = &target.state else {
        return None;
    };
    let review_artifact = target.entry.logical_path.clone();
    let Some(boundary) = terminal_review_appendix_boundary(body) else {
        return Some(Snapshot {
            body_fingerprints: vec![fingerprint.clone()],
            fingerprint,
            review_artifact,
            appendix: Vec::new(),
        });
    };
    let mut offsets = vec![boundary.trimmed];
    if boundary.raw != boundary.trimmed {
        offsets.push(boundary.raw);
    }
    let body_fingerprints = offsets
        .into_iter()
        .map(|offset| contents_fingerprint(&contents, require, Some((&review_artifact, offset))))
        .collect::<Option<Vec<String>>>()?;
    Some(Snapshot {
        fingerprint,
        review_artifact,
        body_fingerprints,
        appendix: body[boundary.raw..].to_vec(),
    })
}

/// `reviewArtifactFingerprint`: the content identity a review's receipt
/// covers. `state` is the state to read the scope from, as
/// [`ReviewScope::state`] has it. `None` when there is no record, the
/// files cannot be read as one set, or an owed artifact is not a file and
/// the caller asked for them all.
#[must_use]
pub fn fingerprint(
    wf: &Workflow,
    stage: &Stage,
    unit: Option<&str>,
    state: Option<Option<&str>>,
    options: Options<'_>,
) -> Option<String> {
    let contents = contents(
        wf,
        stage,
        ReviewScope {
            unit,
            state,
            merged_bolt_units: options.merged_bolt_units,
        },
    )?;
    contents_fingerprint(&contents, options.require_required_artifacts, None)
}

/// `reviewRequestArtifactsCurrent`: whether the request's fingerprint
/// still describes the bytes on disk. A request under the appendix
/// protocol fingerprinted the bytes before the section it allowed, so it
/// is compared with the body view; a later one, with the whole set.
#[must_use]
pub fn request_artifacts_current(binding: &RequestBinding, snapshot: &Snapshot) -> bool {
    // A request recorded over raw line endings reads in its current form.
    let requested = raw::current_form(&binding.artifact_fingerprint);
    if binding.legacy_appendix.is_some() {
        snapshot.body_fingerprints.contains(&requested)
    } else {
        snapshot.fingerprint == requested
    }
}

/// `reviewAppendedAfterRequest`: whether the review artifact carries a
/// `## Review` section that provably came after the request: the bytes
/// before it are the requested bytes, and the request saw no section. A
/// request that fingerprinted the whole set saw one when the set is still
/// current with the section there; a request under the appendix protocol
/// wrote down whether it did.
#[must_use]
pub fn appended_after_request(binding: &RequestBinding, snapshot: &Snapshot) -> bool {
    if snapshot.appendix.is_empty() {
        return false;
    }
    let requested = raw::current_form(&binding.artifact_fingerprint);
    if !snapshot.body_fingerprints.contains(&requested) {
        return false;
    }
    match &binding.legacy_appendix {
        None => snapshot.fingerprint != requested,
        Some(legacy) => !legacy.prior_appendix,
    }
}

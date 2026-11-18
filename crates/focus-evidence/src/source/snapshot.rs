//! The listing a stage starts from, kept beside the record.
//!
//! A row that opens work on the source (the creation of a piece of work, a
//! jump, the start of a stage that produces code) carries a `Source
//! Baseline`: the hash of the project's source listing at that moment. The
//! listing itself is written once, under the hash it has, in the engine's
//! directory of the record
//! (`.focus-engine/source-review/<stage>/baseline-<hash>.tsv`), so that a
//! later reading can say which paths moved, and not only that something
//! did (`sourceBaselineAuditFields`, `writeBaselineSourceSnapshot`,
//! `aidlc-lib.ts` L25 555).
//!
//! # Tests that another test covers
//!
//! Two questions of the writer change no answer when taken out, and are
//! kept where upstream has them. The record's directory is made before
//! anything else, and no row reaches the writer without a record there
//! already. The snapshot's place is checked for a link twice, before its
//! directory is made and after: nothing but another process can put a link
//! there in between, and the first check is the one that keeps the
//! directory from being made through a link.

use std::path::{Path, PathBuf};

use focus_js::{EngineError, Result, bail, re};
use focus_workspace::records::Selection;
use focus_workspace::{Layout, atomic, files, links, observer};
use sha2::{Digest, Sha256};

use super::listing::{self, Listing};
use super::raw;
use super::walk::{Options, Stop, WorkspaceSource};

/// The field's name on a row.
pub const BASELINE_FIELD: &str = "Source Baseline";
/// `UNBINDABLE_FINGERPRINT`: the source could not be read into a listing,
/// in a project that is a git checkout.
pub const UNBINDABLE: &str = "unbindable";
/// The directory of the snapshots, under the engine's directory of a record.
const SNAPSHOT_DIR: &str = "source-review";

/// `writeSourceSnapshot`: `serialized` at `path`, which lies under
/// `record_root` and is never written through a link. A file already there
/// with the same bytes is the snapshot; with other bytes it is evidence a
/// row already names, and is not replaced.
fn write_snapshot(record_root: &Path, path: &Path, serialized: &str) -> Result<String> {
    let hash = listing::sha256(serialized);
    let made = |dir: &Path| {
        std::fs::create_dir_all(dir)
            .map_err(|err| EngineError::new(format!("{}: {err}", dir.display())))
    };
    let no_link = || -> Result<()> {
        let anchor = std::fs::canonicalize(record_root)
            .map_err(|err| EngineError::new(format!("{}: {err}", record_root.display())))?;
        let rel = path.strip_prefix(record_root).unwrap_or(path);
        links::refuse_link_in_chain(&anchor, rel)
    };
    made(record_root)?;
    no_link()?;
    if let Some(parent) = path.parent() {
        made(parent)?;
    }
    no_link()?;
    if path.exists() {
        let existing = std::fs::read(path)
            .map_err(|err| EngineError::new(format!("{}: {err}", path.display())))?;
        if existing == serialized.as_bytes() {
            return Ok(format!("sha256:{hash}"));
        }
        // A different payload at the same address is corruption or a
        // collision of the prefix: evidence already named is never destroyed.
        bail!(
            "Source snapshot address collision or corruption at {}",
            path.display()
        );
    }
    observer::refuse_write(observer::WRITE_FILE_ATOMIC)?;
    atomic::write_atomic(path, serialized.as_bytes())
        .map_err(|err| EngineError::new(format!("{}: {err}", path.display())))?;
    Ok(format!("sha256:{hash}"))
}

/// `writeBaselineSourceSnapshot`: the listing a stage starts from, written
/// under its own hash; the answer is the value of the row's field.
///
/// # Errors
///
/// No piece of work is selected, or the stage's name is not one; the
/// snapshot's place is behind a link; a different snapshot is already
/// there; the process is an observer.
pub fn write_baseline(
    layout: &Layout,
    selection: &Selection,
    stage_slug: &str,
    listing: &Listing,
) -> Result<String> {
    let record = selection
        .record_dir(layout)
        .filter(|_| re::test("^[a-z][a-z0-9-]*$", "", stage_slug));
    let Some(record) = record else {
        bail!("Cannot write source baseline without a valid active record and stage slug");
    };
    let serialized = listing::serialize(listing).map_err(EngineError::new)?;
    let hash = listing::sha256(&serialized);
    let path = selection
        .engine_dir(layout)
        .join(SNAPSHOT_DIR)
        .join(stage_slug)
        .join(format!("baseline-{}.tsv", &hash[..12]));
    write_snapshot(&record, &path, &serialized)
}

/// The directory a record's snapshots were kept in before the engine's
/// directory held them (`LEGACY_SOURCE_REVIEW_DIR`).
const LEGACY_SNAPSHOT_DIR: &str = ".focus-source-review";
/// `WORKSPACE_SNAPSHOT_HEADER`: the first word of a workspace snapshot.
const WORKSPACE_HEADER: &str = "workspace";

/// `validSourceSnapshotFingerprint`: the hash of a `sha256:` fingerprint.
fn fingerprint_hash(fingerprint: &str) -> Option<&str> {
    fingerprint
        .strip_prefix("sha256:")
        .filter(|hash| re::test("^[0-9a-f]{64}$", "", hash))
}

/// `sourceSnapshotReadPath`: where a snapshot of a stage is read from. A
/// snapshot a row names from before the engine's directory held them is
/// still found: the new place when the file is there, else the earlier
/// one. Nothing is written by this.
fn read_path(
    layout: &Layout,
    selection: &Selection,
    stage_slug: &str,
    file_name: &str,
) -> Option<(PathBuf, PathBuf)> {
    if !re::test("^[a-z][a-z0-9-]*$", "", stage_slug) {
        return None;
    }
    let record = selection.record_dir(layout)?;
    let path = selection
        .engine_dir(layout)
        .join(SNAPSHOT_DIR)
        .join(stage_slug)
        .join(file_name);
    if path.exists() {
        return Some((record, path));
    }
    let legacy = record
        .join(LEGACY_SNAPSHOT_DIR)
        .join(stage_slug)
        .join(file_name);
    Some(if legacy.exists() {
        (record, legacy)
    } else {
        (record, path)
    })
}

/// A snapshot's bytes, read as a file of the record that decides
/// something: a regular file with one name, reached through no link.
fn read_bytes(record: &Path, path: &Path) -> Option<Vec<u8>> {
    let anchor = std::fs::canonicalize(record).ok()?;
    links::refuse_link_in_chain(&anchor, path.strip_prefix(record).ok()?).ok()?;
    files::read_regular(path, "source snapshot").ok()
}

/// `readBaselineSourceSnapshot`: the listing a stage started from, read
/// back only when its bytes are the ones the row's fingerprint names. A
/// checkout with CRLF line ends holds the same listing.
#[must_use]
pub fn read_baseline(
    layout: &Layout,
    selection: &Selection,
    stage_slug: &str,
    fingerprint: &str,
) -> Option<Listing> {
    let hash = fingerprint_hash(fingerprint)?;
    let (record, path) = read_path(
        layout,
        selection,
        stage_slug,
        &format!("baseline-{}.tsv", &hash[..12]),
    )?;
    let bytes = read_bytes(&record, &path)?;
    let text = raw::committed_text_bytes(&bytes);
    let digest = |bytes: &[u8]| -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    if digest(&text) != hash && digest(&bytes) != hash {
        return None;
    }
    listing::parse(&String::from_utf8_lossy(&text))
        .ok()
        .flatten()
}

/// The name of the snapshot kept for a workspace fingerprint
/// (`workspaceSourceSnapshotName`).
fn workspace_snapshot_name(fingerprint: &str) -> Option<String> {
    re::test("^[0-9a-f]{64}$", "", fingerprint)
        .then(|| format!("{WORKSPACE_HEADER}-{}.tsv", &fingerprint[..12]))
}

/// `writeWorkspaceSourceSnapshot`: the listing behind a workspace
/// fingerprint, kept so that a later change can be told as paths. Nothing
/// when it is already there. `false` when it could not be kept: the
/// listing is for what a person is told, and no decision reads it.
#[must_use]
pub fn write_workspace_snapshot(
    layout: &Layout,
    selection: &Selection,
    stage_slug: &str,
    state: &WorkspaceSource,
) -> bool {
    let (Some(record), Some(name)) = (
        selection
            .record_dir(layout)
            .filter(|_| re::test("^[a-z][a-z0-9-]*$", "", stage_slug)),
        workspace_snapshot_name(&state.fingerprint),
    ) else {
        return false;
    };
    let Ok(listed) = listing::serialize(&state.listing) else {
        return false;
    };
    let serialized = format!("{WORKSPACE_HEADER}\t{}\t-\n{listed}", state.fingerprint);
    let path = selection
        .engine_dir(layout)
        .join(SNAPSHOT_DIR)
        .join(stage_slug)
        .join(name);
    write_snapshot(&record, &path, &serialized).is_ok()
}

/// `readWorkspaceSourceSnapshot`: the listing kept for a workspace
/// fingerprint; `None` when none was, or the file is not that snapshot.
#[must_use]
pub fn read_workspace_snapshot(
    layout: &Layout,
    selection: &Selection,
    stage_slug: &str,
    fingerprint: &str,
) -> Option<Listing> {
    let name = workspace_snapshot_name(fingerprint)?;
    let (record, path) = read_path(layout, selection, stage_slug, &name)?;
    let bytes = read_bytes(&record, &path)?;
    let serialized = String::from_utf8_lossy(&bytes);
    let (header, listed) = serialized.split_once('\n')?;
    if header != format!("{WORKSPACE_HEADER}\t{fingerprint}\t-") {
        return None;
    }
    listing::parse(listed).ok().flatten()
}

/// `workspaceSourceChangedPaths`: the paths that changed between a
/// recorded workspace fingerprint and the source as it is, when the
/// recorded listing was kept; `None` when only the two fingerprints can be
/// compared.
#[must_use]
pub fn workspace_changed_paths(
    layout: &Layout,
    selection: &Selection,
    stage_slug: &str,
    recorded_fingerprint: &str,
    current: Option<&WorkspaceSource>,
) -> Option<Vec<String>> {
    let current = current?;
    let recorded = read_workspace_snapshot(layout, selection, stage_slug, recorded_fingerprint)?;
    Some(listing::changed_paths(
        &listing::recorded_under_current_boundary(&recorded, &current.listing),
        &current.listing,
    ))
}

/// `sourceBaselineAuditFields`: the value of `Source Baseline` for a row
/// that opens work on the source, with the listing kept beside the record.
///
/// A project whose source cannot be read into a listing is `unbindable`
/// when it is a git checkout, and starts from an empty listing when it is
/// not. `repos` are the member repositories of the piece of work, as its
/// row of the registry lists them (a creation gives its own, since its row
/// is listed last).
///
/// # Errors
///
/// As [`write_baseline`]; and the tree holds something this port does not
/// read, which is refused by name instead of being answered for.
pub fn baseline_field(
    layout: &Layout,
    selection: &Selection,
    stage_slug: &str,
    repos: &[String],
    options: &Options,
) -> Result<String> {
    let is_git = |dir: &Path| dir.join(".git").exists();
    let has_git_checkout = if repos.is_empty() {
        is_git(&layout.project)
    } else {
        repos.iter().any(|name| is_git(&layout.project.join(name)))
    };
    match super::state::workspace_source_state(&layout.project, repos, options) {
        Ok(source) => write_baseline(layout, selection, stage_slug, &source.listing),
        Err(Stop::Failed(_)) if has_git_checkout => Ok(UNBINDABLE.to_owned()),
        Err(Stop::Failed(_)) => write_baseline(layout, selection, stage_slug, &Listing::new()),
        Err(Stop::NotPorted(what)) => bail!(
            "The source of this piece of work is behind {what}, and Focus does not read that yet: the {BASELINE_FIELD} of this row cannot be taken."
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use focus_harness::Harness;

    use super::*;
    use crate::source::walk::{Budgets, Names};

    fn layout(name: &str) -> Layout {
        let dir =
            std::env::temp_dir().join(format!("source-snapshot-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Layout::new(fs::canonicalize(&dir).unwrap(), Harness::Claude)
    }

    fn selection(intent: Option<&str>) -> Selection {
        Selection {
            space: "default".to_owned(),
            intent: intent.map(str::to_owned),
            session_id: None,
            binding: None,
        }
    }

    fn options() -> Options {
        Options {
            budgets: Budgets::from(|_| None),
            raw_line_endings: false,
            names: Names::default(),
        }
    }

    /// No command reaches these: a row that carries a baseline belongs to a
    /// piece of work, and its stage is one of the graph's.
    #[test]
    fn a_baseline_needs_a_record_and_a_stage_name() {
        let layout = layout("needs");
        let refusal = "Cannot write source baseline without a valid active record and stage slug";
        for (intent, stage) in [
            (None, "code-generation"),
            (Some("260105-add-login"), "Code"),
            (Some("260105-add-login"), ""),
            (Some("260105-add-login"), "code/generation"),
            (Some("260105-add-login"), "1-code"),
        ] {
            let answer = write_baseline(&layout, &selection(intent), stage, &Listing::new());
            assert_eq!(
                answer.unwrap_err().to_string(),
                refusal,
                "{intent:?} {stage}"
            );
        }
        assert!(!layout.project.join("focus").exists());
        let _ = fs::remove_dir_all(&layout.project);
    }

    /// Deliberate deviation: a member repository that is a delegated
    /// worktree is read by upstream through the worktree's own boundary.
    /// Focus does not read that boundary yet, and refuses the row whose
    /// baseline it would be instead of answering with another listing.
    #[test]
    fn a_delegated_worktree_among_the_repositories_is_refused_by_name() {
        let layout = layout("worktree");
        fs::create_dir_all(layout.project.join("app/.focus")).unwrap();
        fs::write(layout.project.join("app/a.ts"), "a\n").unwrap();
        fs::write(layout.project.join("app/.focus/worktree-meta.json"), "{}\n").unwrap();
        let answer = baseline_field(
            &layout,
            &selection(Some("260105-add-login")),
            "code-generation",
            &["app".to_owned()],
            &options(),
        );
        assert_eq!(
            answer.unwrap_err().to_string(),
            "The source of this piece of work is behind a delegated worktree's source boundary, and Focus does not read that yet: the Source Baseline of this row cannot be taken."
        );
        assert!(!layout.project.join("focus").exists());
        let _ = fs::remove_dir_all(&layout.project);
    }
}

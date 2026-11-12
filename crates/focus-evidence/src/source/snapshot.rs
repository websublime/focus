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

use std::path::Path;

use focus_js::{EngineError, Result, bail, re};
use focus_workspace::records::{Selection, read_intent_registry, row_names_record};
use focus_workspace::{Layout, atomic, links, observer};
use serde_json::Value;

use super::listing::{self, Listing};
use super::walk::{self, Options, Stop};

/// The field's name on a row.
pub const BASELINE_FIELD: &str = "Source Baseline";
/// `UNBINDABLE_FINGERPRINT`: the source could not be read into a listing,
/// in a project that is a git checkout.
pub const UNBINDABLE: &str = "unbindable";
/// The directory of the snapshots, under the engine's directory of a record.
const SNAPSHOT_DIR: &str = "source-review";

/// `intentRepos`: the member repositories of the selected piece of work,
/// as its row of the registry lists them; none for a project that is one
/// repository, or when no piece of work is selected.
#[must_use]
pub fn intent_repos(layout: &Layout, selection: &Selection) -> Vec<String> {
    let Some(dir_name) = selection.intent.as_deref() else {
        return Vec::new();
    };
    read_intent_registry(layout, &selection.space)
        .iter()
        .find(|row| row_names_record(row, dir_name))
        .and_then(|row| row.get("repos"))
        .and_then(Value::as_array)
        .map(|repos| {
            repos
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

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

/// `sourceBaselineAuditFields`: the value of `Source Baseline` for a row
/// that opens work on the source, with the listing kept beside the record.
///
/// A project whose source cannot be read into a listing is `unbindable`
/// when it is a git checkout, and starts from an empty listing when it is
/// not. `repos` are the member repositories of the piece of work (see
/// [`intent_repos`]; a creation gives its own, since its row is listed
/// last).
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
    match walk::workspace_source(&layout.project, repos, true, options) {
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

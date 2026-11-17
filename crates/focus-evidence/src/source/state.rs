//! The source of a project as a decision reads it now, and whether a
//! recorded fingerprint describes it (`workspaceSourceState`,
//! `sameWorkspaceSource`, `aidlc-lib.ts` L24 122, L20 817).
//!
//! A fingerprint is recorded once and compared for as long as the record
//! lives, and the walk that takes it has changed since some were recorded:
//! files left out by name, the output directories of .NET projects left
//! out, line ends read as LF. A recorded value is the current source's when
//! it is the current fingerprint, or what one of those earlier walks gives
//! for the same files:
//!
//! - the fingerprint with the files since left out by name, which the walk
//!   that left some out keeps beside its own;
//! - the fingerprint over raw line ends of content read in this process
//!   ([`super::raw`]);
//! - the fingerprint with the .NET output directories walked. That walk is
//!   redone, once for each current value and only when no other comparison
//!   holds, and gives what was recorded both before and after the names
//!   were left out.
//!
//! What a walk leaves for a later comparison is kept for the process, as
//! upstream keeps it: by the current fingerprint, replaced by a later walk
//! that gives the same one.
//!
//! Upstream also keeps one state for the calls of a transition that may
//! share a look at the source (`withWorkspaceSourceStateCache`). That is a
//! saving, not an answer: here each call walks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use super::raw;
use super::walk::{self, Options, Stop, WorkspaceSource};

/// The walk to redo for a source that held a .NET output directory.
struct Earlier {
    project: PathBuf,
    repos: Vec<String>,
    options: Options,
    /// What the redone walk gave: nothing until it is asked for.
    values: Option<Vec<String>>,
}

/// What walks left for comparisons, by the current fingerprint.
#[derive(Default)]
struct Kept {
    /// `legacyWorkspaceSourceAliases`.
    legacy: HashMap<String, String>,
    /// `earlierBoundarySources`.
    earlier: HashMap<String, Earlier>,
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

fn with<T>(read: impl FnOnce(&mut Kept) -> T) -> T {
    let mut kept = KEPT.lock().unwrap_or_else(PoisonError::into_inner);
    read(kept.get_or_insert_with(Kept::default))
}

/// `workspaceSourceState`: the project's source now, over the roof and the
/// repositories `repos` names (none: the project is one repository), with
/// what a later comparison needs kept.
///
/// # Errors
///
/// As [`walk::workspace_source`].
pub fn workspace_source_state(
    project: &Path,
    repos: &[String],
    options: &Options,
) -> Result<WorkspaceSource, Stop> {
    let walked = walk::workspace_source(project, repos, true, options)?;
    with(|kept| {
        match &walked.legacy {
            Some(legacy) => kept
                .legacy
                .insert(walked.fingerprint.clone(), legacy.clone()),
            None => kept.legacy.remove(&walked.fingerprint),
        };
        if walked.dotnet_output_seen {
            kept.earlier.insert(
                walked.fingerprint.clone(),
                Earlier {
                    project: project.to_path_buf(),
                    repos: repos.to_vec(),
                    options: options.clone(),
                    values: None,
                },
            );
        } else {
            kept.earlier.remove(&walked.fingerprint);
        }
    });
    Ok(walked)
}

/// `earlierBoundaryWorkspaceSources`: what the walk that still took the
/// .NET output directories gives for the source `current` was taken of.
fn earlier_boundary_sources(current: &str) -> Vec<String> {
    let asked = with(|kept| {
        kept.earlier.get(current).map(|entry| {
            (
                entry.values.clone(),
                entry.project.clone(),
                entry.repos.clone(),
                entry.options.clone(),
            )
        })
    });
    let Some((values, project, repos, options)) = asked else {
        return Vec::new();
    };
    if let Some(values) = values {
        return values;
    }
    let values = match walk::workspace_source(&project, &repos, false, &options) {
        Ok(walked) => std::iter::once(walked.fingerprint)
            .chain(walked.legacy)
            .collect(),
        Err(_) => Vec::new(),
    };
    with(|kept| {
        if let Some(entry) = kept.earlier.get_mut(current) {
            entry.values = Some(values.clone());
        }
    });
    values
}

/// `sameWorkspaceSource`: whether a recorded workspace fingerprint
/// describes the current source. Nothing recorded and nothing current are
/// the same; one without the other is not.
#[must_use]
pub fn same_workspace_source(recorded: Option<&str>, current: Option<&str>) -> bool {
    if recorded == current {
        return true;
    }
    let (Some(recorded), Some(current)) = (recorded, current) else {
        return false;
    };
    with(|kept| {
        kept.legacy
            .get(current)
            .is_some_and(|legacy| legacy == recorded)
    }) || raw::current_form(recorded) == current
        || earlier_boundary_sources(current)
            .iter()
            .any(|earlier| earlier == recorded)
}

/// The earlier walk's value kept beside `current`, when there is one
/// (`_legacyWorkspaceSourceFingerprintForTests`).
#[must_use]
pub fn legacy_alias(current: &str) -> Option<String> {
    with(|kept| kept.legacy.get(current).cloned())
}

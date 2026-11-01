//! The record's engine folder as the hooks write it, and the records of
//! later milestones that a hook meets there: each is refused by name while
//! Focus does not read it.

use std::path::{Path, PathBuf};

use focus_js::{Result, bail};
use focus_workspace::atomic::write_atomic;
use focus_workspace::links::refuse_link_in_chain;
use focus_workspace::names::{ENGINE_DIR, WORKSPACE_DIR};
use focus_workspace::observer;
use focus_workspace::records::Selection;
use focus_workspace::{Layout, env};

/// `writeRecordFileNoFollow`: a framework-owned file under `record_root`,
/// written through no symlink: a link planted at the folder, at a folder
/// under it, or at the file refuses the write.
///
/// # Errors
///
/// A link is on the way, or the file cannot be written.
pub fn write_record_file_no_follow(
    record_root: &Path,
    relative: &str,
    data: &[u8],
) -> Result<PathBuf> {
    observer::refuse_write(observer::WRITE_FILE_ATOMIC)?;
    let anchor = std::fs::canonicalize(record_root)
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", record_root.display())))?;
    let rel = Path::new(relative);
    refuse_link_in_chain(&anchor, rel)?;
    let target = anchor.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", parent.display())))?;
    }
    // The parents exist now; none of them, nor the leaf, may be a link.
    refuse_link_in_chain(&anchor, rel)?;
    write_atomic(&target, data)
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", target.display())))?;
    Ok(target)
}

/// `writeEngineFileNoFollow`: `<record>/.focus-engine/<name>`, the record
/// root created when missing.
///
/// # Errors
///
/// See [`write_record_file_no_follow`].
pub fn write_engine_file_no_follow(
    layout: &Layout,
    selection: &Selection,
    name: &str,
    data: &str,
) -> Result<PathBuf> {
    let record_root = selection.docs_root(layout);
    std::fs::create_dir_all(&record_root)
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", record_root.display())))?;
    write_record_file_no_follow(
        &record_root,
        &format!("{ENGINE_DIR}/{name}"),
        data.as_bytes(),
    )
}

/// The orchestrator's active-directive marker (M5): a hook that would read
/// or refresh it refuses by name while it is there.
///
/// # Errors
///
/// The marker exists.
pub fn refuse_directive_marker(layout: &Layout, selection: &Selection, hook: &str) -> Result<()> {
    let marker = selection.engine_dir(layout).join("active-directive.json");
    if marker.exists() {
        bail!(
            "{} is the orchestrator's directive marker, which {hook} keeps in step with the state: Focus does not read it yet.",
            marker.display()
        );
    }
    Ok(())
}

/// The background-subagent in-flight ledger (M5): a hook that would take a
/// finished subagent out of it refuses by name while it is there.
///
/// # Errors
///
/// The ledger exists.
pub fn refuse_subagent_ledger(layout: &Layout) -> Result<()> {
    let ledger = layout
        .project
        .join(WORKSPACE_DIR)
        .join(".focus-subagent-inflight");
    if ledger.exists() {
        bail!(
            "{} is the ledger of background subagents in flight, which a finished subagent is taken out of: Focus does not keep that ledger yet.",
            ledger.display()
        );
    }
    Ok(())
}

/// The summary-authorization registry of a record (M5): a write under the
/// record would carry the id of the active authorization for its stage.
///
/// # Errors
///
/// The registry holds anything.
pub fn refuse_summary_authorization(layout: &Layout, selection: &Selection) -> Result<()> {
    let registry = selection.engine_dir(layout).join("summary-authorization");
    if std::fs::read_dir(&registry).is_ok_and(|mut entries| entries.next().is_some()) {
        bail!(
            "{} holds summary authorizations, which an artifact row carries the id of: Focus does not read them yet.",
            registry.display()
        );
    }
    Ok(())
}

/// Whether the hook may write at all: an observer process must not.
#[must_use]
pub fn observing() -> bool {
    observer::observing().is_some()
}

/// The project a hook acts on: `FOCUS_PROJECT_DIR`, else `CLAUDE_PROJECT_DIR`,
/// else the directory the command said (`resolveProjectDirFromHook`).
#[must_use]
pub fn project_dir_from_env() -> Option<PathBuf> {
    env::path("PROJECT_DIR")
        .or_else(|| std::env::var_os("CLAUDE_PROJECT_DIR").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    const RECORD: &str = "focus/spaces/default/intents/260105-add-login";

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("focus-hooks-engine-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join(RECORD)).unwrap();
            Self(Layout::new(root, Harness::Claude))
        }

        fn selection(&self) -> Selection {
            Selection {
                space: "default".to_owned(),
                intent: Some("260105-add-login".to_owned()),
                session_id: None,
                binding: None,
            }
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.project.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    #[test]
    fn an_engine_file_is_written_under_the_record() {
        let tree = Tree::new("write");
        let written =
            write_engine_file_no_follow(&tree.0, &tree.selection(), "recovery.md", "# Crumb\n")
                .unwrap();
        // The path comes back through the record's real path (`/private/var`
        // for a macOS temp dir).
        assert_eq!(
            written,
            std::fs::canonicalize(tree.0.project.join(RECORD))
                .unwrap()
                .join(".focus-engine/recovery.md")
        );
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "# Crumb\n");
        write_engine_file_no_follow(&tree.0, &tree.selection(), "recovery.md", "# Again\n")
            .unwrap();
        assert_eq!(std::fs::read_to_string(&written).unwrap(), "# Again\n");
    }

    #[cfg(unix)]
    #[test]
    fn an_engine_file_is_not_written_through_a_link() {
        let tree = Tree::new("link");
        let elsewhere = tree.0.project.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(
            &elsewhere,
            tree.0.project.join(RECORD).join(".focus-engine"),
        )
        .unwrap();
        let err = write_engine_file_no_follow(&tree.0, &tree.selection(), "recovery.md", "x")
            .unwrap_err();
        assert!(err.to_string().contains(".focus-engine"), "{err}");
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
    }

    #[test]
    fn each_record_of_a_later_milestone_is_refused_by_name_while_there() {
        let tree = Tree::new("later");
        let selection = tree.selection();
        assert!(refuse_directive_marker(&tree.0, &selection, "a test").is_ok());
        assert!(refuse_subagent_ledger(&tree.0).is_ok());
        assert!(refuse_summary_authorization(&tree.0, &selection).is_ok());

        tree.write(
            &format!("{RECORD}/.focus-engine/active-directive.json"),
            "{}",
        );
        let err = refuse_directive_marker(&tree.0, &selection, "a test")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(
                "active-directive.json is the orchestrator's directive marker, which a test keeps"
            ),
            "{err}"
        );

        tree.write("focus/.focus-subagent-inflight", "{}");
        let err = refuse_subagent_ledger(&tree.0).unwrap_err().to_string();
        assert!(
            err.contains("focus-subagent-inflight is the ledger of background subagents"),
            "{err}"
        );

        // The registry refuses only with something in it.
        std::fs::create_dir_all(
            tree.0
                .project
                .join(RECORD)
                .join(".focus-engine/summary-authorization"),
        )
        .unwrap();
        assert!(refuse_summary_authorization(&tree.0, &selection).is_ok());
        tree.write(
            &format!("{RECORD}/.focus-engine/summary-authorization/x.json"),
            "{}",
        );
        let err = refuse_summary_authorization(&tree.0, &selection)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("summary-authorization holds summary authorizations"),
            "{err}"
        );
    }
}

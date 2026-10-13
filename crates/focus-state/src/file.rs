//! The state file on disk.
//!
//! A record's `focus-state.md` is read whole and written whole, atomically,
//! behind the observer barrier (`writeStateFile`). A state file that is not
//! writable is a deliberate write barrier the tool honours: an atomic write
//! renames over a read-only target without noticing, so the target's
//! permission is checked first and the write refused as a plain write
//! would have been.
//!
//! Upstream also keeps, beside the state in the record's engine directory, a
//! record of the last writes that moved the state while a step was issued
//! (`state-writes.json`), for the orchestrator to name what put a step out
//! of date. It is kept only while an active-directive marker exists there.
//! The orchestrator is M5; until then a record carrying that marker is
//! refused here, before anything is written, rather than written without
//! the record upstream would keep.
//!
//! Mirrors `readStateFile` and `writeStateFile` in `aidlc-lib.ts`.

use std::fs;
use std::path::Path;

use focus_js::{Result, bail};
use focus_workspace::atomic::write_atomic;
use focus_workspace::names::ENGINE_DIR;
use focus_workspace::observer;

/// The marker the orchestrator leaves beside the state while a step is
/// issued, and beside which the state-write record is kept.
const ACTIVE_DIRECTIVE_MARKER: &str = "active-directive.json";

/// `readStateFile`: the file's text, bytes that are not UTF-8 replaced.
///
/// # Errors
///
/// The file is not there (`State file not found: <path>`), or cannot be read.
pub fn read_state_file(path: &Path) -> Result<String> {
    if !path.exists() {
        bail!("State file not found: {}", path.display());
    }
    let bytes = fs::read(path)
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// `writeStateFile`: `content` written atomically to `path`, the record's
/// directory created when the file is new. An existing file that is not
/// writable refuses the write.
///
/// # Errors
///
/// An observer is running; the file exists and is not writable; the
/// record carries an active-directive marker (see the module docs); or the
/// write fails.
pub fn write_state_file(path: &Path, content: &str) -> Result<()> {
    observer::refuse_write(observer::WRITE_STATE_FILE)?;
    if path.exists() {
        refuse_read_only(path)?;
    } else if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", parent.display())))?;
    }
    let marker = path
        .parent()
        .unwrap_or(Path::new("."))
        .join(ENGINE_DIR)
        .join(ACTIVE_DIRECTIVE_MARKER);
    if marker.symlink_metadata().is_ok() {
        bail!(
            "{} says the engine has a step in hand, and upstream records beside it every write that moves the state, which Focus does not keep yet. Remove the marker, or write the state with the engine's own tools.",
            marker.display()
        );
    }
    write_atomic(path, content.as_bytes())
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))
}

/// `accessSync(path, W_OK)`: refused as Node refuses it, with the code and
/// the path, when the file cannot be written.
fn refuse_read_only(path: &Path) -> Result<()> {
    let writable = fs::OpenOptions::new().append(true).open(path);
    match writable {
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            bail!("EACCES: permission denied, access '{}'", path.display())
        }
        Err(err) => bail!("{}: {err}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("focus-state-file-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn the_file_is_written_whole_and_its_directory_made_once() {
        let root = dir("write");
        let path = root.join("intents/one/focus-state.md");
        assert!(
            read_state_file(&path)
                .unwrap_err()
                .message()
                .starts_with("State file not found: ")
        );
        write_state_file(&path, "- **Scope**: feature\n").unwrap();
        assert_eq!(read_state_file(&path).unwrap(), "- **Scope**: feature\n");
        write_state_file(&path, "- **Scope**: bugfix\n").unwrap();
        assert_eq!(read_state_file(&path).unwrap(), "- **Scope**: bugfix\n");
        // Bytes that are not UTF-8 are read as U+FFFD.
        fs::write(&path, b"- **Scope**: f\xffx\n").unwrap();
        assert_eq!(read_state_file(&path).unwrap(), "- **Scope**: f\u{fffd}x\n");
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_file_refuses_the_write() {
        use std::os::unix::fs::PermissionsExt;
        let root = dir("readonly");
        let path = root.join("focus-state.md");
        fs::write(&path, "x").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        // Root writes anything; the refusal is the permission's.
        if fs::OpenOptions::new().append(true).open(&path).is_err() {
            let refused = write_state_file(&path, "y").unwrap_err();
            assert_eq!(
                refused.message(),
                format!("EACCES: permission denied, access '{}'", path.display())
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), "x");
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    /// Deliberate deviation. Beside a state whose record carries the
    /// orchestrator's active-directive marker, upstream records each write
    /// that moves the state; Focus refuses the write until it keeps that
    /// record (M5).
    #[test]
    fn a_record_with_an_active_directive_refuses_the_write() {
        let root = dir("marker");
        let path = root.join("focus-state.md");
        fs::write(&path, "x").unwrap();
        fs::create_dir_all(root.join(".focus-engine")).unwrap();
        fs::write(root.join(".focus-engine/active-directive.json"), "{}").unwrap();
        let refused = write_state_file(&path, "y").unwrap_err();
        assert!(
            refused.message().contains("active-directive.json"),
            "{refused}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "x");
        let _ = fs::remove_dir_all(root);
    }
}

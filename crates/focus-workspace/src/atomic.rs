//! Atomic file replacement.
//!
//! A reader of the target sees the previous contents or the new ones, never a
//! partial file, and a crash mid-write leaves the previous contents in place.
//! That is all this gives: two writers that each read, modify and write back
//! still lose one update unless they also hold the workspace lock.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How long a replacement keeps retrying on Windows, where a rename over a
/// file another process has open is refused. Upstream measured 1,879 of 2,000
/// renames refused while a second process read the target in a loop, and
/// settled on the same bound.
const REPLACE_RETRY: Duration = Duration::from_secs(10);

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Replace the contents of `path` with `data`. The directory it goes into
/// must exist: like upstream's `writeFileAtomic`, this creates none, so a
/// caller that names a place that is not there is told so instead of having
/// it made.
///
/// The bytes go to a sibling temporary file created exclusively (so two
/// writers never share one), are flushed to disk, and are renamed over the
/// target. The temporary sits beside the target so the rename stays on one
/// filesystem, which is what makes it atomic.
///
/// # Errors
///
/// Any I/O failure. The temporary file is removed; the target is untouched.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    write_atomic_with(path, data, true)
}

/// [`write_atomic`] without the flush to disk: for a per-user runtime
/// record whose loss costs nothing and whose writer runs on a budget (the
/// process map), where upstream's write never reaches the disk either.
///
/// # Errors
///
/// As [`write_atomic`].
pub fn write_atomic_unsynced(path: &Path, data: &[u8]) -> io::Result<()> {
    write_atomic_with(path, data, false)
}

fn write_atomic_with(path: &Path, data: &[u8], sync: bool) -> io::Result<()> {
    let tmp = loop {
        let candidate = temp_sibling(path);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(mut file) => {
                let written = file
                    .write_all(data)
                    .and_then(|()| if sync { file.sync_all() } else { Ok(()) });
                if let Err(err) = written {
                    let _ = fs::remove_file(&candidate);
                    return Err(err);
                }
                break candidate;
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    };
    replace(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

fn temp_sibling(path: &Path) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos());
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".{}.{}-{nanos}.tmp",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    path.with_file_name(name)
}

fn replace(tmp: &Path, path: &Path) -> io::Result<()> {
    if !cfg!(windows) {
        return fs::rename(tmp, path);
    }
    let deadline = Instant::now() + REPLACE_RETRY;
    loop {
        match fs::rename(tmp, path) {
            Ok(()) => return Ok(()),
            // A directory in the way is not a reader that will let go.
            Err(err) if is_sharing_refusal(&err) && Instant::now() < deadline && !path.is_dir() => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(err) => return Err(err),
        }
    }
}

/// Whether a rename failed the way Windows fails it while another process
/// holds the target or the temporary open. `ERROR_ACCESS_DENIED` (5),
/// `ERROR_SHARING_VIOLATION` (32) and `ERROR_LOCK_VIOLATION` (33) are read as
/// raw codes: the standard library gives the last two no kind of their own.
fn is_sharing_refusal(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(5 | 32 | 33))
        || matches!(
            err.kind(),
            io::ErrorKind::PermissionDenied
                | io::ErrorKind::ResourceBusy
                | io::ErrorKind::AlreadyExists
                | io::ErrorKind::DirectoryNotEmpty
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_contents_and_leaves_no_temporary() {
        let dir = std::env::temp_dir().join(format!("focus-atomic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let target = dir.join("nested").join("data.json");

        // A directory that is not there is an error, and stays not there.
        assert!(write_atomic(&target, b"first").is_err());
        assert!(!dir.exists());
        fs::create_dir_all(dir.join("nested")).unwrap();
        write_atomic(&target, b"first").unwrap();
        write_atomic(&target, b"second").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second");
        let names: Vec<_> = fs::read_dir(target.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["data.json"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_write_leaves_the_target_and_no_temporary() {
        let dir = std::env::temp_dir().join(format!("focus-atomic-fail-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // The target is a directory, so the final rename cannot succeed.
        let target = dir.join("occupied");
        fs::create_dir_all(target.join("child")).unwrap();

        assert!(write_atomic(&target, b"data").is_err());
        let names: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["occupied"]);
        fs::remove_dir_all(&dir).unwrap();
    }
}

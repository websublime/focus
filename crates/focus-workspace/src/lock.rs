//! The workspace lock.
//!
//! Anything that reads project state, decides, and writes it back holds this
//! lock for the whole sequence, so two engine processes in one project (two
//! chat sessions, a hook and a command) never interleave.
//!
//! It is an operating-system advisory lock on one file
//! (`focus/.focus-lock`), taken through `std::fs::File::lock`: `flock` on
//! Linux and macOS, `LockFileEx` on Windows. The kernel releases it when the
//! process ends, however it ends, so there is no stale lock to detect and
//! nothing to reap.
//!
//! This is Focus's own design, not a port. Upstream locks with a directory it
//! creates and stamps with the owner's process id, start time and a token,
//! and recovers from crashes by proving the owner dead (through `bun:ffi` on
//! some platforms). The two locks do not see each other: a project is driven
//! by one engine or the other, not both.
//!
//! Holding the lock is a value. A function that must run under it takes
//! `&WorkspaceLock`, so the compiler checks what upstream checks with a
//! re-entrancy counter.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use focus_js::{EngineError, Result, bail, json};
use serde_json::{Map, Value};

use crate::atomic::write_atomic;
use crate::env;
use crate::links::refuse_link_on_the_way;
use crate::project::Layout;

/// How long an acquire waits for another process before giving up.
/// `FOCUS_LOCK_TIMEOUT_MS` overrides it.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const POLL: Duration = Duration::from_millis(20);

/// Lock files this process holds. A second acquire from the same process
/// would wait on itself until the timeout, so it is refused at once instead.
static HELD: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Proof that this process holds the workspace lock. Dropping it releases the
/// lock.
#[derive(Debug)]
pub struct WorkspaceLock {
    file: File,
    path: PathBuf,
    owner_path: PathBuf,
}

impl WorkspaceLock {
    /// Take the lock for `layout`'s project, waiting up to the configured
    /// timeout. `purpose` names what the holder is doing, for the message a
    /// process that times out behind it prints.
    ///
    /// # Errors
    ///
    /// The lock file cannot be created, another process held the lock for the
    /// whole wait, or this process already holds it.
    pub fn acquire(layout: &Layout, purpose: &str) -> Result<Self> {
        let timeout = env::var("LOCK_TIMEOUT_MS")
            .and_then(|raw| raw.trim().parse::<u64>().ok())
            .map_or(DEFAULT_TIMEOUT, Duration::from_millis);
        Self::acquire_within(layout, purpose, timeout)
    }

    /// [`acquire`](Self::acquire) with an explicit wait.
    ///
    /// # Errors
    ///
    /// As [`acquire`](Self::acquire).
    pub fn acquire_within(layout: &Layout, purpose: &str, timeout: Duration) -> Result<Self> {
        let path = layout.lock_file();
        let owner_path = layout.lock_owner_file();
        // The lock file is the first thing a command writes in a project.
        refuse_link_on_the_way(&layout.project, &path)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|err| EngineError::new(format!("{}: {err}", parent.display())))?;
        }
        let identity = fs::canonicalize(path.parent().unwrap_or(Path::new(".")))
            .map(|dir| dir.join(path.file_name().unwrap_or_default()))
            .unwrap_or_else(|_| path.clone());
        {
            let mut held = HELD
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if held.contains(&identity) {
                bail!(
                    "This process already holds the workspace lock at {}. Pass the held lock to the code that needs it instead of taking it again.",
                    path.display()
                );
            }
            held.push(identity.clone());
        }
        let release_claim = || {
            HELD.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .retain(|held| *held != identity);
        };

        let file = lock_file(&path, &owner_path, timeout).inspect_err(|_| release_claim())?;
        // Best effort: the note only improves the message another process
        // prints if it times out waiting.
        let _ = write_atomic(&owner_path, owner_note(purpose).as_bytes());
        Ok(Self {
            file,
            path: identity,
            owner_path,
        })
    }
}

impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.owner_path);
        let _ = self.file.unlock();
        HELD.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|held| *held != self.path);
    }
}

fn lock_file(path: &Path, owner_path: &Path, timeout: Duration) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|err| EngineError::new(format!("{}: {err}", path.display())))?;
    let deadline = Instant::now() + timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(POLL);
            }
            Err(TryLockError::WouldBlock) => bail!(
                "Timed out after {} waiting for the workspace lock at {}{}. Wait for that process to finish, or stop it; the lock frees itself when its holder exits.",
                seconds(timeout),
                path.display(),
                describe_owner(owner_path)
            ),
            Err(TryLockError::Error(err)) => bail!(
                "Cannot lock {}: {err}. The workspace lock needs a filesystem that supports file locks.",
                path.display()
            ),
        }
    }
}

fn seconds(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis.is_multiple_of(1000) {
        format!("{}s", millis / 1000)
    } else {
        format!("{millis}ms")
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn owner_note(purpose: &str) -> String {
    let mut note = Map::new();
    note.insert("pid".to_owned(), Value::from(std::process::id()));
    note.insert("purpose".to_owned(), Value::from(purpose));
    note.insert("since".to_owned(), Value::from(unix_now()));
    format!("{}\n", json::stringify(&Value::Object(note)))
}

/// `, held by pid 123 (compile) for 4s`, or nothing when the holder left no
/// readable note.
fn describe_owner(owner_path: &Path) -> String {
    let Some(note) = fs::read_to_string(owner_path)
        .ok()
        .and_then(|text| json::parse(&text).ok())
    else {
        return String::new();
    };
    let (Some(pid), Some(purpose), Some(since)) = (
        note.get("pid").and_then(Value::as_u64),
        note.get("purpose").and_then(Value::as_str),
        note.get("since").and_then(Value::as_u64),
    ) else {
        return String::new();
    };
    format!(
        ", held by pid {pid} ({purpose}) for {}s",
        unix_now().saturating_sub(since)
    )
}

#[cfg(test)]
mod tests {
    use focus_harness::Harness;

    use super::*;

    fn layout(name: &str) -> Layout {
        let dir = std::env::temp_dir().join(format!("focus-lock-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Layout::new(dir, Harness::Claude)
    }

    #[test]
    fn the_lock_excludes_another_holder_until_dropped() {
        let layout = layout("exclusive");
        let lock = WorkspaceLock::acquire(&layout, "first").unwrap();

        // A separate open of the same file stands in for another process:
        // file locks belong to the open file, not to the process.
        let refused = lock_file(
            &layout.lock_file(),
            &layout.lock_owner_file(),
            Duration::from_millis(60),
        )
        .unwrap_err();
        let message = refused.message();
        assert!(
            message.starts_with("Timed out after 60ms waiting for the workspace lock at "),
            "{message}"
        );
        assert!(
            message.contains(&format!(
                ", held by pid {} (first) for ",
                std::process::id()
            )),
            "{message}"
        );

        drop(lock);
        assert!(!layout.lock_owner_file().exists());
        let again = lock_file(
            &layout.lock_file(),
            &layout.lock_owner_file(),
            Duration::ZERO,
        );
        assert!(again.is_ok());
        // Windows does not remove a directory holding an open, locked file.
        drop(again);
        fs::remove_dir_all(&layout.project).unwrap();
    }

    #[test]
    fn a_waiter_gets_the_lock_when_the_holder_lets_go() {
        let layout = layout("handover");
        let lock = WorkspaceLock::acquire(&layout, "holder").unwrap();
        let (path, owner) = (layout.lock_file(), layout.lock_owner_file());
        let waiter =
            std::thread::spawn(move || lock_file(&path, &owner, Duration::from_secs(10)).is_ok());
        std::thread::sleep(Duration::from_millis(80));
        drop(lock);
        assert!(waiter.join().unwrap());
        fs::remove_dir_all(&layout.project).unwrap();
    }

    #[test]
    fn taking_the_lock_twice_in_one_process_fails_at_once() {
        let layout = layout("reentrant");
        let _lock = WorkspaceLock::acquire(&layout, "outer").unwrap();
        let started = Instant::now();
        let second = WorkspaceLock::acquire(&layout, "inner").unwrap_err();
        assert!(
            second
                .message()
                .starts_with("This process already holds the workspace lock at ")
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(_lock);
        // The refused attempt must not have cleared the first holder's claim
        // early, and the claim is gone now.
        let third = WorkspaceLock::acquire(&layout, "after");
        assert!(third.is_ok());
        drop(third);
        fs::remove_dir_all(&layout.project).unwrap();
    }
}

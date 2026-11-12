//! Opt-in phase trace for hook processes (`aidlc-hook-trace.ts`).
//! Diagnostics only: it never changes what a hook decides or prints, and it
//! is off unless `FOCUS_HOOK_TRACE_DIR` names an absolute directory.
//!
//! Each traced process appends one JSON line per phase to
//! `<dir>/hook-<pid>.ndjson`, opening and closing the file on every call,
//! so the trace keeps no handle open, takes no lock and reads no file. A
//! trace path that exists as anything but a regular file is skipped; a
//! failed write is dropped silently.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};

use focus_js::json::stringify;
use serde_json::{Map, Value, json};

fn trace_directory() -> Option<PathBuf> {
    let value = focus_workspace::env::var("HOOK_TRACE_DIR")?;
    let value = focus_js::text::trim(&value);
    let path = Path::new(value);
    (!value.is_empty() && path.is_absolute()).then(|| path.to_path_buf())
}

/// `hookTraceEnabled`.
#[must_use]
pub fn enabled() -> bool {
    trace_directory().is_some()
}

/// `hookTracePath`.
#[must_use]
pub fn path(directory: &Path, pid: u32) -> PathBuf {
    directory.join(format!("hook-{pid}.ndjson"))
}

/// The moment this process started tracing, for `sinceStartMs`.
fn started() -> Instant {
    static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *STARTED.get_or_init(Instant::now)
}

/// `hookTrace`: append one phase line for this process. Never fails.
pub fn trace(phase: &str, detail: &[(&str, Value)]) {
    let Some(directory) = trace_directory() else {
        return;
    };
    let since = started().elapsed().as_millis();
    let _ = (|| -> std::io::Result<()> {
        make_private_dir(&directory)?;
        let path = path(&directory, std::process::id());
        if let Ok(existing) = std::fs::symlink_metadata(&path)
            && !existing.is_file()
        {
            return Ok(());
        }
        let mut line = Map::new();
        line.insert(
            "at".to_owned(),
            json!(focus_audit::time::iso_timestamp(SystemTime::now())),
        );
        line.insert("sinceStartMs".to_owned(), json!(since));
        line.insert("pid".to_owned(), json!(std::process::id()));
        line.insert("ppid".to_owned(), json!(parent_pid()));
        line.insert("phase".to_owned(), json!(phase));
        for (key, value) in detail {
            line.insert((*key).to_owned(), value.clone());
        }
        let mut file = open_private(&path)?;
        writeln!(file, "{}", stringify(&Value::Object(line)))
    })();
}

#[cfg(unix)]
fn make_private_dir(directory: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
    {
        Ok(()) => Ok(()),
        Err(err) if directory.is_dir() => {
            let _ = err;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(not(unix))]
fn make_private_dir(directory: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(directory)
}

#[cfg(unix)]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
}

/// The parent's pid (`process.ppid`), where the system hands it over; `0`
/// elsewhere (the trace is diagnostics, and the engine forbids unsafe code).
fn parent_pid() -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::process::parent_id()
    }
    #[cfg(not(unix))]
    {
        0
    }
}

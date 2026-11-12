//! Which conversation a process belongs to, by its ancestry.
//!
//! A harness runs the engine's commands from a shell it spawns, and gives
//! that shell no session id. The session-start hook therefore records the
//! harness process and each of its ancestors as belonging to the session
//! (`focus/.focus-sessions/pids/<pid>`), and a command that is named no
//! session walks its own ancestors up to the first one recorded. A record
//! carries the process's start time, so a pid reused by a later process is
//! not mistaken for its owner.
//!
//! Mirrors `writeSessionPidAncestry`, `resolveSessionIdFromAncestry` and
//! their helpers in `aidlc-lib.ts`. Process identity comes from `/proc` on
//! Linux and from `ps` on macOS, as upstream reads it; on Windows, where
//! upstream calls the kernel, Focus reads the process table through
//! `sysinfo` and compares start times at the second (see [`parent_edge`]).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use focus_js::json;
use serde_json::{Value, json};

use crate::atomic::write_atomic_unsynced;
use crate::names::SESSIONS_DIR;
use crate::records::valid_session_id;
use crate::{Layout, observer};

/// `SESSION_ANCESTRY_BUDGET_MS`: how long a walk may take.
const BUDGET: Duration = Duration::from_millis(50);
/// `SESSION_ANCESTRY_MAX_DEPTH`.
const MAX_DEPTH: usize = 64;
/// `SESSION_ANCESTRY_CACHE_MS`: a miss is remembered this long.
const MISS_CACHE: Duration = Duration::from_millis(1000);

/// One process as the map identifies it: its parent, and when it started
/// (`None` where the platform gives no start time).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub ppid: u32,
    pub start_time: Option<String>,
}

/// One record of the map: which session owns the pid, started when; a
/// retired record names no session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub session_id: Option<String>,
    pub start_time: Option<String>,
}

/// `sessionPidMapDir`.
#[must_use]
pub fn map_dir(layout: &Layout) -> PathBuf {
    layout.workspace_root().join(SESSIONS_DIR).join("pids")
}

fn entry_path(layout: &Layout, pid: u32) -> Option<PathBuf> {
    (pid > 1).then(|| map_dir(layout).join(pid.to_string()))
}

/// `readSessionPidEntry`: a record as written; `None` for one malformed.
#[must_use]
pub fn read_entry(layout: &Layout, pid: u32) -> Option<Entry> {
    let raw = std::fs::read_to_string(entry_path(layout, pid)?).ok()?;
    let Value::Object(map) = json::parse(&raw).ok()? else {
        return None;
    };
    let session_id = match map.get("sessionId") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) => Some(valid_session_id(Some(id))?.to_owned()),
        Some(_) => return None,
    };
    let start_time = match map.get("startTime") {
        None | Some(Value::Null) => None,
        Some(Value::String(time)) => Some(time.clone()),
        Some(_) => return None,
    };
    Some(Entry {
        session_id,
        start_time,
    })
}

/// `writeSessionPidRecord`: best-effort, atomically, so a reader never
/// takes a refresh in progress for bad JSON.
fn write_record(layout: &Layout, pid: u32, entry: &Entry) {
    forget_miss(layout);
    let Some(path) = entry_path(layout, pid) else {
        return;
    };
    // An observer writes nothing, as upstream's primitive refuses it.
    if observer::refuse_write(observer::WRITE_FILE_ATOMIC).is_err()
        || std::fs::create_dir_all(map_dir(layout)).is_err()
    {
        return;
    }
    let record = json!({ "sessionId": entry.session_id, "startTime": entry.start_time });
    let _ = write_atomic_unsynced(&path, format!("{}\n", json::stringify(&record)).as_bytes());
}

/// What a walk remembers: a miss, for a moment; a write forgets it.
static MISSES: Mutex<Vec<(PathBuf, Instant)>> = Mutex::new(Vec::new());

fn forget_miss(layout: &Layout) {
    if let Ok(mut misses) = MISSES.lock() {
        misses.retain(|(project, _)| *project != layout.project);
    }
}

/// `writeSessionPidEntry`: one pid's ownership, when the process is alive
/// and the budget holds. On Windows a record without a start time cannot
/// establish ownership, so the previous claim is retired instead.
pub fn write_entry(
    layout: &Layout,
    pid: u32,
    session_id: &str,
    deadline: Instant,
    identity: Option<&Identity>,
) {
    forget_miss(layout);
    if Instant::now() >= deadline
        || pid <= 1
        || valid_session_id(Some(session_id)).is_none()
        || !is_alive(pid)
    {
        return;
    }
    let resolved = match identity {
        Some(identity) => Some(identity.clone()),
        None => self::identity(pid, deadline),
    };
    let start_time = resolved.and_then(|identity| identity.start_time);
    if cfg!(windows) && (start_time.is_none() || Instant::now() >= deadline) {
        write_record(
            layout,
            pid,
            &Entry {
                session_id: None,
                start_time: None,
            },
        );
        return;
    }
    write_record(
        layout,
        pid,
        &Entry {
            session_id: Some(session_id.to_owned()),
            start_time,
        },
    );
}

/// `gcSessionPidEntries`: records of processes that are gone, or whose
/// pid now belongs to another process. A live process whose identity
/// cannot be read in time is left alone.
fn gc(layout: &Layout, deadline: Instant, skip: &HashSet<u32>) {
    let Ok(names) = std::fs::read_dir(map_dir(layout)) else {
        return;
    };
    for name in names.flatten() {
        let name = name.file_name().to_string_lossy().into_owned();
        if name.is_empty() || !name.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if Instant::now() >= deadline {
            break;
        }
        if skip.contains(&pid) {
            continue;
        }
        let entry = read_entry(layout, pid);
        let stale = match entry {
            None => true,
            Some(_) if !is_alive(pid) => true,
            Some(Entry {
                start_time: Some(recorded),
                ..
            }) => match identity(pid, deadline) {
                None => continue,
                Some(identity) => identity.start_time.as_deref() != Some(recorded.as_str()),
            },
            Some(_) => false,
        };
        if stale {
            let _ = std::fs::remove_file(map_dir(layout).join(&name));
        }
    }
}

/// On Windows, whether `parent` can be the ancestor of `child` by their
/// start times: `Verified` when it started first, `Rejected` when it did
/// not, `Unknown` when either time is missing. Focus reads the times at the
/// second, so a parent started in the same second as its child is not
/// verified either (upstream reads the kernel's 100 ns clock and never
/// meets that case).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Verified,
    Rejected,
    Unknown,
}

#[must_use]
pub fn parent_edge(parent: Option<&Identity>, child: Option<&Identity>) -> Edge {
    let (Some(parent), Some(child)) = (
        parent.and_then(|p| p.start_time.as_deref()),
        child.and_then(|c| c.start_time.as_deref()),
    ) else {
        return Edge::Unknown;
    };
    let (Some(parent), Some(child)) = (win_time(parent), win_time(child)) else {
        return Edge::Unknown;
    };
    if parent < child {
        Edge::Verified
    } else {
        Edge::Rejected
    }
}

/// A Windows start time as written: `win32:` and the instant.
fn win_time(text: &str) -> Option<u64> {
    text.strip_prefix("win32:")
        .and_then(|digits| digits.parse().ok())
}

/// `writeSessionPidAncestry`: map the harness process and each ancestor
/// of this one to `session_id`. The hook process itself is left out: the
/// tool processes that follow are its siblings, not its descendants.
pub fn write_ancestry(layout: &Layout, session_id: &str) {
    write_ancestry_within(layout, session_id, BUDGET);
}

/// [`write_ancestry`] with its own budget, for a test on a loaded host.
pub fn write_ancestry_within(layout: &Layout, session_id: &str, budget: Duration) {
    forget_miss(layout);
    if valid_session_id(Some(session_id)).is_none() {
        return;
    }
    let deadline = Instant::now() + budget;
    let windows = cfg!(windows);
    let mut child: Option<Identity> = None;
    let mut seen: HashSet<u32> = HashSet::new();
    let Some(mut pid) = parent_pid() else {
        return;
    };
    for depth in 0..MAX_DEPTH {
        if pid <= 1 || seen.contains(&pid) {
            break;
        }
        seen.insert(pid);
        let mut found: Option<Identity> = None;
        let mut edge = Edge::Unknown;
        if windows {
            if depth == 0 {
                child = identity(std::process::id(), deadline);
                if child.as_ref().is_some_and(|me| me.ppid != pid) {
                    break;
                }
            }
            found = child.as_ref().and_then(|_| identity(pid, deadline));
            edge = parent_edge(found.as_ref(), child.as_ref());
            if edge == Edge::Rejected {
                break;
            }
        }
        // The known parent slot is retired before it is looked up, so an old
        // session never returns when the lookup recovers.
        write_record(
            layout,
            pid,
            &Entry {
                session_id: None,
                start_time: None,
            },
        );
        if Instant::now() >= deadline {
            break;
        }
        if windows && edge != Edge::Verified {
            break;
        }
        if !windows {
            found = identity(pid, deadline);
        }
        let Some(identity) = found else {
            break;
        };
        write_entry(layout, pid, session_id, deadline, Some(&identity));
        pid = identity.ppid;
        child = Some(identity);
    }
    gc(layout, deadline, &seen);
}

/// `resolveSessionIdFromAncestry`: the session of the nearest recorded
/// ancestor of this process; every failure is a silent miss, so a command
/// keeps the cursor's behaviour when the map says nothing.
#[must_use]
pub fn resolve(layout: &Layout) -> Option<String> {
    resolve_within(layout, BUDGET)
}

/// [`resolve`] with its own budget, for a test on a loaded host.
#[must_use]
pub fn resolve_within(layout: &Layout, budget: Duration) -> Option<String> {
    if let Ok(misses) = MISSES.lock()
        && misses
            .iter()
            .any(|(project, at)| *project == layout.project && at.elapsed() < MISS_CACHE)
    {
        return None;
    }
    let resolved = resolve_uncached(layout, budget);
    if let Ok(mut misses) = MISSES.lock() {
        misses.retain(|(project, _)| *project != layout.project);
        if resolved.is_none() {
            misses.push((layout.project.clone(), Instant::now()));
        }
    }
    resolved
}

fn resolve_uncached(layout: &Layout, budget: Duration) -> Option<String> {
    if !map_dir(layout).exists() {
        return None;
    }
    let deadline = Instant::now() + budget;
    let windows = cfg!(windows);
    let mut child = if windows {
        identity(std::process::id(), deadline)
    } else {
        None
    };
    let parent = parent_pid()?;
    if windows && child.as_ref().is_none_or(|me| me.ppid != parent) {
        return None;
    }
    let mut seen: HashSet<u32> = HashSet::new();
    let mut pid = parent;
    for _ in 0..MAX_DEPTH {
        if pid <= 1 || seen.contains(&pid) || Instant::now() >= deadline {
            return None;
        }
        seen.insert(pid);
        let found = identity(pid, deadline)?;
        if !is_alive(pid) {
            return None;
        }
        // A parent pid can outlive its owner: the walk never joins a
        // replacement born after (or at the same instant as) its child.
        if windows && parent_edge(Some(&found), child.as_ref()) != Edge::Verified {
            return None;
        }
        if let Some(entry) = read_entry(layout, pid)
            && (entry.start_time.is_none() || entry.start_time == found.start_time)
        {
            return if windows && entry.start_time.is_none() {
                None
            } else {
                entry.session_id
            };
        }
        pid = found.ppid;
        child = Some(found);
    }
    None
}

/// This process's parent pid (`process.ppid`).
///
/// Where the system hands it over it costs nothing, as upstream's does. It
/// must not cost a `ps`: the walk's budget is already running when this is
/// asked, and on a loaded macOS host one `ps` can spend all of it, which
/// turned a recorded conversation into a silent miss.
#[must_use]
pub fn parent_pid() -> Option<u32> {
    #[cfg(unix)]
    {
        Some(std::os::unix::process::parent_id())
    }
    #[cfg(not(unix))]
    {
        identity(std::process::id(), Instant::now() + BUDGET).map(|me| me.ppid)
    }
}

/// `processIdentity`: a process's parent and start time, on this platform.
#[must_use]
pub fn identity(pid: u32, deadline: Instant) -> Option<Identity> {
    if pid <= 1 || Instant::now() >= deadline {
        return None;
    }
    platform::identity(pid, deadline)
}

/// `processIsAlive`.
#[must_use]
pub fn is_alive(pid: u32) -> bool {
    pid > 1 && platform::is_alive(pid)
}

#[cfg(target_os = "linux")]
mod platform {
    use super::Identity;
    use std::time::Instant;

    /// `/proc/<pid>/stat`: the parent is the second field after the
    /// command's closing parenthesis, the start time the twentieth.
    pub fn identity(pid: u32, _deadline: Instant) -> Option<Identity> {
        let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let close = raw.rfind(')')?;
        let fields: Vec<&str> = raw[close + 1..].split_whitespace().collect();
        let ppid: u32 = fields.get(1)?.parse().ok()?;
        let start_time = (*fields.get(19)?).to_owned();
        if start_time.is_empty() {
            return None;
        }
        Some(Identity {
            ppid,
            start_time: Some(start_time),
        })
    }

    pub fn is_alive(pid: u32) -> bool {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::Identity;
    use std::process::Command;
    use std::time::Instant;

    /// `ps -o ppid=,lstart= -p <pid>`, as upstream reads it there.
    pub fn identity(pid: u32, deadline: Instant) -> Option<Identity> {
        if Instant::now() >= deadline {
            return None;
        }
        let output = Command::new("ps")
            .args(["-o", "ppid=,lstart=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.trim();
        let (ppid, started) = line.split_once(char::is_whitespace)?;
        let ppid: u32 = ppid.parse().ok()?;
        let started = started.trim();
        if started.is_empty() {
            return None;
        }
        Some(Identity {
            ppid,
            start_time: Some(started.to_owned()),
        })
    }

    pub fn is_alive(pid: u32) -> bool {
        Command::new("ps")
            .args(["-p", &pid.to_string()])
            .output()
            .is_ok_and(|output| output.status.success())
    }
}

#[cfg(windows)]
mod platform {
    use super::Identity;
    use std::time::Instant;

    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    fn process(pid: u32) -> Option<System> {
        let mut system = System::new();
        let updated = system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
            true,
            ProcessRefreshKind::nothing(),
        );
        (updated > 0).then_some(system)
    }

    /// The process table: the parent, and the start time at the second,
    /// written as `win32:<seconds since the epoch>`.
    pub fn identity(pid: u32, deadline: Instant) -> Option<Identity> {
        if Instant::now() >= deadline {
            return None;
        }
        let system = process(pid)?;
        let found = system.process(Pid::from_u32(pid))?;
        let ppid = found.parent()?.as_u32();
        if ppid == pid {
            return None;
        }
        let started = found.start_time();
        if started == 0 {
            return None;
        }
        Some(Identity {
            ppid,
            start_time: Some(format!("win32:{started}")),
        })
    }

    pub fn is_alive(pid: u32) -> bool {
        process(pid).is_some()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    use super::Identity;
    use std::time::Instant;

    pub fn identity(_pid: u32, _deadline: Instant) -> Option<Identity> {
        None
    }

    pub fn is_alive(_pid: u32) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("focus-ancestry-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(Layout::new(root, Harness::Claude))
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    #[test]
    fn a_record_is_read_as_written_or_not_at_all() {
        let tree = Tree::new("records");
        assert_eq!(read_entry(&tree.0, 4242), None);
        write_record(
            &tree.0,
            4242,
            &Entry {
                session_id: Some("s1".to_owned()),
                start_time: Some("414".to_owned()),
            },
        );
        assert_eq!(
            std::fs::read_to_string(map_dir(&tree.0).join("4242")).unwrap(),
            "{\"sessionId\":\"s1\",\"startTime\":\"414\"}\n"
        );
        assert_eq!(
            read_entry(&tree.0, 4242),
            Some(Entry {
                session_id: Some("s1".to_owned()),
                start_time: Some("414".to_owned())
            })
        );
        for raw in [
            "",
            "{",
            "[]",
            "{\"sessionId\":5}",
            "{\"sessionId\":\"../x\"}",
            "{\"startTime\":1}",
        ] {
            std::fs::write(map_dir(&tree.0).join("4242"), raw).unwrap();
            assert_eq!(read_entry(&tree.0, 4242), None, "{raw}");
        }
        std::fs::write(
            map_dir(&tree.0).join("4242"),
            "{\"sessionId\":null,\"startTime\":null}",
        )
        .unwrap();
        assert_eq!(
            read_entry(&tree.0, 4242),
            Some(Entry {
                session_id: None,
                start_time: None
            })
        );
        // Pid 1 and below have no record.
        write_record(
            &tree.0,
            1,
            &Entry {
                session_id: Some("s1".to_owned()),
                start_time: None,
            },
        );
        assert!(!map_dir(&tree.0).join("1").exists());
    }

    #[test]
    fn the_windows_edge_orders_start_times() {
        let at = |time: &str| Identity {
            ppid: 1,
            start_time: Some(time.to_owned()),
        };
        let none = Identity {
            ppid: 1,
            start_time: None,
        };
        assert_eq!(
            parent_edge(Some(&at("win32:10")), Some(&at("win32:11"))),
            Edge::Verified
        );
        assert_eq!(
            parent_edge(Some(&at("win32:11")), Some(&at("win32:11"))),
            Edge::Rejected
        );
        assert_eq!(
            parent_edge(Some(&at("win32:12")), Some(&at("win32:11"))),
            Edge::Rejected
        );
        assert_eq!(
            parent_edge(Some(&at("414")), Some(&at("win32:11"))),
            Edge::Unknown
        );
        assert_eq!(
            parent_edge(Some(&none), Some(&at("win32:11"))),
            Edge::Unknown
        );
        assert_eq!(parent_edge(None, None), Edge::Unknown);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn this_process_is_found_through_its_ancestors() {
        let tree = Tree::new("walk");
        // Nothing recorded: a miss, remembered for a moment.
        let patient = Duration::from_secs(10);
        assert_eq!(resolve_within(&tree.0, patient), None);
        let me = std::process::id();
        let parent = parent_pid().unwrap();
        assert!(is_alive(me));
        assert!(is_alive(parent));
        assert!(identity(me, Instant::now() + BUDGET).is_some());
        // The session-start hook records the harness and its ancestors; a
        // command run from one of them finds the session.
        write_ancestry_within(&tree.0, "s1", patient);
        let listing: Vec<String> = std::fs::read_dir(map_dir(&tree.0))
            .map(|dir| {
                dir.flatten()
                    .map(|e| {
                        format!(
                            "{}={}",
                            e.file_name().to_string_lossy(),
                            std::fs::read_to_string(e.path()).unwrap_or_default()
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            read_entry(&tree.0, parent)
                .and_then(|entry| entry.session_id)
                .as_deref(),
            Some("s1"),
            "parent {parent}, me {me}: {listing:?}"
        );
        assert_eq!(resolve_within(&tree.0, patient).as_deref(), Some("s1"));
        // A record whose start time is another process's is not the owner.
        let started = identity(parent, Instant::now() + BUDGET)
            .unwrap()
            .start_time;
        write_record(
            &tree.0,
            parent,
            &Entry {
                session_id: Some("s2".to_owned()),
                start_time: Some("0".to_owned()),
            },
        );
        assert_ne!(resolve_within(&tree.0, patient).as_deref(), Some("s2"));
        // A retired record (no session) stops the walk at the nearest
        // ancestor; a record with no start time is taken as it is.
        write_record(
            &tree.0,
            parent,
            &Entry {
                session_id: None,
                start_time: None,
            },
        );
        assert_eq!(resolve_within(&tree.0, patient), None);
        write_record(
            &tree.0,
            parent,
            &Entry {
                session_id: Some("s3".to_owned()),
                start_time: started,
            },
        );
        assert_eq!(resolve_within(&tree.0, patient).as_deref(), Some("s3"));
        // The collector removes a record of a process that is gone, and one
        // whose pid now belongs to another process (this one, started at
        // another time than the record says); a dead pid gets no record.
        write_record(
            &tree.0,
            4_000_000,
            &Entry {
                session_id: Some("s1".to_owned()),
                start_time: Some("1".to_owned()),
            },
        );
        write_record(
            &tree.0,
            me,
            &Entry {
                session_id: Some("s9".to_owned()),
                start_time: Some("0".to_owned()),
            },
        );
        write_ancestry_within(&tree.0, "s1", patient);
        assert!(!map_dir(&tree.0).join("4000000").exists());
        assert!(!map_dir(&tree.0).join(me.to_string()).exists());
        write_entry(&tree.0, 4_000_000, "s1", Instant::now() + patient, None);
        assert!(!map_dir(&tree.0).join("4000000").exists());
        assert_eq!(resolve_within(&tree.0, patient).as_deref(), Some("s1"));
    }
}

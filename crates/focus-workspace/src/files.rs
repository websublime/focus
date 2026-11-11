//! Opening a file whose path is not trusted.
//!
//! The workspace is committed and shared: anyone who can land a commit can
//! put a link, a pipe or a device where the engine expects a file. So the
//! files the engine's decisions rest on are never opened by name and then
//! believed. Three things hold for every open here:
//!
//! - **The last component is never followed.** The path itself is opened
//!   (`O_NOFOLLOW`; on Windows the reparse point, not its target), so a link
//!   is seen for what it is.
//! - **Opening never waits.** A FIFO with no writer blocks in `open` until
//!   one appears; `O_NONBLOCK` returns at once and the kind is checked on the
//!   descriptor.
//! - **What was opened is what the path still names.** The check is made on
//!   the open descriptor and compared with the path's identity afterwards, so
//!   a file swapped in between is noticed.
//!
//! Callers still check the directories above the file for links
//! ([`crate::links`]): these guards cover the last component only.
//!
//! Mirrors `readAppendOnlyFileNoFollowOrThrow` (L33474) and
//! `readRegularFileNoFollowOrThrow` (L33251) in `aidlc-lib.ts`, and the open
//! in `appendAuditBlockAtPath` (`aidlc-audit.ts`, L878).

use std::fs::{File, Metadata, OpenOptions};
use std::io::Read;
use std::path::Path;

use focus_js::{Result, bail};

/// Windows: open a reparse point (a link, a junction) itself, not its target.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Make `options` open the path itself, without following a link at its last
/// component and without waiting for the other end of a pipe.
fn guard(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = options;
}

/// Whether opening failed because the last component is a link.
fn refused_a_link(error: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        error.raw_os_error() == Some(libc::ELOOP)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

/// `sameDirectoryIdentity`: both paths are directories, neither a link, and
/// the same one: by inode and device, or by what Windows keeps for them.
#[must_use]
pub fn same_directory(left: &Path, right: &Path) -> bool {
    let (Ok(actual), Ok(expected)) = (
        std::fs::symlink_metadata(left),
        std::fs::symlink_metadata(right),
    ) else {
        return false;
    };
    if !actual.is_dir() || !expected.is_dir() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        actual.ino() != 0 && (actual.ino(), actual.dev()) == (expected.ino(), expected.dev())
    }
    #[cfg(windows)]
    {
        match (
            same_file::Handle::from_path(left),
            same_file::Handle::from_path(right),
        ) {
            (Ok(actual), Ok(expected)) => actual == expected,
            _ => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        left == right
    }
}

/// How many names a file has (`nlink`); one where the host does not say.
#[must_use]
pub fn hard_links(meta: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.nlink()
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        1
    }
}

/// Nothing at `path`, or a regular file with one name: what a write may
/// replace without reaching another file. `lstat` with `throwIfNoEntry:
/// false` as Bun answers it: a name that is not there reads as nothing,
/// and every other failure as not this, a path that goes through a file
/// among them.
#[must_use]
pub fn absent_or_plain_file(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(meta) => meta.is_file() && hard_links(&meta) == 1,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Whether `path`, followed to the file it names, is the file `opened` is.
#[must_use]
pub fn names_the_same_file(opened: &File, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (opened.metadata(), std::fs::metadata(path)) {
            (Ok(held), Ok(named)) => (held.dev(), held.ino()) == (named.dev(), named.ino()),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        let held = opened.try_clone().and_then(same_file::Handle::from_file);
        match (held, same_file::Handle::from_path(path)) {
            (Ok(held), Ok(named)) => held == named,
            _ => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (opened, path);
        true
    }
}

fn is_link(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
}

/// Read a file the engine only ever appends to, such as an audit shard.
/// `None` when it is not a regular file, is a link, vanished, or stopped
/// being what the path names while it was opened.
///
/// Growing while it is read is normal for a live ledger and is not a failure:
/// every parser of an append-only file discards a block that is not yet
/// complete. A second hard link is not a failure either: a backup that
/// hard-links the project leaves live files with two names, and a hard link
/// cannot redirect a path that has been checked for links.
#[must_use]
pub fn read_append_only(path: &Path) -> Option<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    guard(&mut options);
    let mut file = options.open(path).ok()?;
    if !file.metadata().ok()?.is_file() || is_link(path) {
        return None;
    }
    if !names_the_same_file(&file, &std::fs::canonicalize(path).ok()?) {
        return None;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// What kind of thing was found where a regular file was expected.
fn kind_of(meta: &Metadata) -> &'static str {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let kind = meta.file_type();
        if kind.is_fifo() {
            return "a FIFO / named pipe";
        }
        if kind.is_socket() {
            return "a socket";
        }
        if kind.is_char_device() {
            return "a character device";
        }
        if kind.is_block_device() {
            return "a block device";
        }
    }
    if meta.is_dir() {
        "a directory"
    } else {
        "not a regular file"
    }
}

/// What must not change between opening a file and finishing reading it.
#[derive(PartialEq, Eq)]
struct Snapshot {
    len: u64,
    modified: Option<std::time::SystemTime>,
    /// Unix: the link count and the inode change time.
    links_and_change: Option<(u64, i64, i64)>,
}

impl Snapshot {
    fn of(meta: &Metadata) -> Self {
        #[cfg(unix)]
        let links_and_change = {
            use std::os::unix::fs::MetadataExt;
            Some((meta.nlink(), meta.ctime(), meta.ctime_nsec()))
        };
        #[cfg(not(unix))]
        let links_and_change = None;
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
            links_and_change,
        }
    }

    fn links(&self) -> Option<u64> {
        self.links_and_change.map(|(links, _, _)| links)
    }
}

/// Read a file that must be exactly what it looks like: a regular file with
/// one name, reached through no link, that did not change while it was read.
/// `what` names it in the refusal ("active workflow state").
///
/// # Errors
///
/// The file cannot be opened; it is a link, a pipe, a socket, a device or a
/// directory; it has a second hard link (which can bring content from
/// elsewhere on the disk into the workspace); or it was replaced or changed
/// between being opened and being read to the end.
pub fn read_regular(path: &Path, what: &str) -> Result<Vec<u8>> {
    let shown = path.display();
    let a_link = || format!("{what} is a symlink, which is not followed: {shown}");
    let mut options = OpenOptions::new();
    options.read(true);
    guard(&mut options);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if refused_a_link(&error) => bail!("{}", a_link()),
        Err(error) => bail!("{what} could not be opened: {shown} ({error})"),
    };
    let Ok(opened) = file.metadata() else {
        bail!("{what} changed while opening: {shown}");
    };
    if opened.file_type().is_symlink() {
        bail!("{}", a_link());
    }
    if !opened.is_file() {
        bail!(
            "{what} is not a regular file ({}): {shown}. Only regular files are read — a FIFO, socket, or device file can block forever or never reach EOF, so it is refused before any read.",
            kind_of(&opened)
        );
    }
    let before = Snapshot::of(&opened);
    match before.links() {
        // No name left: an atomic replace or a removal landed after the open.
        Some(0) => bail!("{what} was replaced while it was being read: {shown}"),
        Some(links) if links != 1 => bail!(
            "{what} is multiply linked (a hardlink) and is not trusted: {shown}. A hardlink can alias content from elsewhere on the filesystem into this directory. Replace it with an independent copy — cp <file> <file>.copy && mv <file>.copy <file> — and re-run."
        ),
        _ => {}
    }
    if is_link(path) {
        bail!("{}", a_link());
    }
    let still_named = |file: &File| {
        std::fs::canonicalize(path).is_ok_and(|real| names_the_same_file(file, &real))
    };
    if !still_named(&file) {
        bail!("{what} changed while opening: {shown}");
    }
    let mut bytes = Vec::new();
    let read = file.read_to_end(&mut bytes).is_ok();
    let unchanged = read
        && still_named(&file)
        && file
            .metadata()
            .is_ok_and(|after| Snapshot::of(&after) == before)
        && u64::try_from(bytes.len()).is_ok_and(|len| len == before.len);
    if !unchanged {
        bail!("{what} changed while reading: {shown}");
    }
    Ok(bytes)
}

/// Open a file to append to it, creating it when it is not there. The path's
/// last component is not followed and the open does not wait on a pipe.
///
/// # Errors
///
/// The operating system refuses the open.
pub fn open_to_append(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).append(true).create(true);
    guard(&mut options);
    options.open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_folder_is_itself_and_a_plain_file_has_one_name() {
        let dir = scratch("identity");
        std::fs::create_dir_all(dir.join("a")).unwrap();
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("f"), "x").unwrap();
        std::fs::write(dir.join("twice"), "x").unwrap();
        std::fs::hard_link(dir.join("twice"), dir.join("again")).unwrap();
        std::os::unix::fs::symlink("a", dir.join("to-a")).unwrap();
        std::os::unix::fs::symlink("f", dir.join("to-f")).unwrap();

        assert!(same_directory(&dir.join("a"), &dir.join("a")));
        assert!(same_directory(&dir.join("a"), &dir.join("b/../a")));
        assert!(!same_directory(&dir.join("a"), &dir.join("b")));
        // A link to the folder is not the folder, and a file is no folder.
        assert!(!same_directory(&dir.join("to-a"), &dir.join("a")));
        assert!(!same_directory(&dir.join("a"), &dir.join("to-a")));
        assert!(!same_directory(&dir.join("f"), &dir.join("f")));
        assert!(!same_directory(&dir.join("missing"), &dir.join("missing")));

        assert!(absent_or_plain_file(&dir.join("missing")));
        assert!(absent_or_plain_file(&dir.join("a/missing")));
        assert!(absent_or_plain_file(&dir.join("f")));
        assert!(!absent_or_plain_file(&dir.join("twice")));
        assert!(!absent_or_plain_file(&dir.join("a")));
        assert!(!absent_or_plain_file(&dir.join("to-f")));
        // A path through a file is a failure, not an absence.
        assert!(!absent_or_plain_file(&dir.join("f/x")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("focus-files-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_regular_file_is_read_and_a_directory_is_named_as_one() {
        let dir = scratch("regular");
        std::fs::write(dir.join("state.md"), "state\n").unwrap();
        assert_eq!(
            read_regular(&dir.join("state.md"), "state").unwrap(),
            b"state\n"
        );
        assert_eq!(read_append_only(&dir.join("state.md")).unwrap(), b"state\n");

        let missing = read_regular(&dir.join("gone.md"), "state").unwrap_err();
        assert!(
            missing.message().starts_with("state could not be opened: "),
            "{missing}"
        );
        assert_eq!(read_append_only(&dir.join("gone.md")), None);
        assert_eq!(read_append_only(&dir), None);
        // Opening a directory to read is refused by Windows itself.
        if cfg!(unix) {
            let refused = read_regular(&dir, "state").unwrap_err();
            assert!(
                refused
                    .message()
                    .starts_with("state is not a regular file (a directory): "),
                "{refused}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn appending_creates_the_file_and_keeps_what_it_held() {
        use std::io::Write;
        let dir = scratch("append");
        let path = dir.join("shard.md");
        open_to_append(&path).unwrap().write_all(b"one\n").unwrap();
        let mut file = open_to_append(&path).unwrap();
        assert!(names_the_same_file(&file, &path));
        file.write_all(b"two\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"one\ntwo\n");
        std::fs::write(dir.join("other.md"), "x").unwrap();
        assert!(!names_the_same_file(&file, &dir.join("other.md")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_link_a_pipe_and_a_second_hard_link_are_refused_by_the_strict_reader() {
        let dir = scratch("strict");
        std::fs::write(dir.join("real.md"), "state\n").unwrap();
        std::os::unix::fs::symlink(dir.join("real.md"), dir.join("link.md")).unwrap();
        let linked = read_regular(&dir.join("link.md"), "state").unwrap_err();
        assert!(
            linked
                .message()
                .starts_with("state is a symlink, which is not followed: "),
            "{linked}"
        );
        assert_eq!(read_append_only(&dir.join("link.md")), None);

        let made = std::process::Command::new("mkfifo")
            .arg(dir.join("pipe.md"))
            .status()
            .unwrap();
        assert!(made.success());
        let pipe = read_regular(&dir.join("pipe.md"), "state").unwrap_err();
        assert!(
            pipe.message()
                .starts_with("state is not a regular file (a FIFO / named pipe): "),
            "{pipe}"
        );

        std::fs::hard_link(dir.join("real.md"), dir.join("alias.md")).unwrap();
        let aliased = read_regular(&dir.join("real.md"), "state").unwrap_err();
        assert!(
            aliased
                .message()
                .starts_with("state is multiply linked (a hardlink) and is not trusted: "),
            "{aliased}"
        );
        // The append-only reader is the one that tolerates a backup's hard link.
        assert_eq!(read_append_only(&dir.join("real.md")).unwrap(), b"state\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

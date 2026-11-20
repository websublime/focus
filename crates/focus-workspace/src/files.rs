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
use std::time::SystemTime;

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

/// What tells one file from another, and one state of a file from a later
/// one (`sameArtifactIdentity`): where it is kept, what kind it is, how
/// many names it has, its size and when it and its inode last changed; the
/// kind, the size and the time of the last write where the host says no
/// more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    len: u64,
    modified: Option<SystemTime>,
    kind: (bool, bool),
    /// Unix: device, inode, mode, link count and the inode change time.
    held: Option<(u64, u64, u32, u64, i64, i64)>,
}

/// The [`Identity`] `meta` describes.
#[must_use]
pub fn identity(meta: &Metadata) -> Identity {
    #[cfg(unix)]
    let held = {
        use std::os::unix::fs::MetadataExt;
        Some((
            meta.dev(),
            meta.ino(),
            meta.mode(),
            meta.nlink(),
            meta.ctime(),
            meta.ctime_nsec(),
        ))
    };
    #[cfg(not(unix))]
    let held = None;
    Identity {
        len: meta.len(),
        modified: meta.modified().ok(),
        kind: (meta.is_file(), meta.is_dir()),
        held,
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

/// `sameFileIdentity` of two paths, each followed to the file it names:
/// whether they name one file.
///
/// # Errors
///
/// One of them names nothing that can be asked, in the host's words.
pub fn same_file(left: &Path, right: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let asked = |path: &Path| {
            std::fs::metadata(path)
                .map(|meta| (meta.dev(), meta.ino()))
                .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))
        };
        Ok(asked(left)? == asked(right)?)
    }
    #[cfg(windows)]
    {
        let asked = |path: &Path| {
            same_file::Handle::from_path(path)
                .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))
        };
        Ok(asked(left)? == asked(right)?)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(left == right)
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
    modified: Option<SystemTime>,
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
    read_regular_within(path, what, None)
}

/// [`read_regular`] for a file with a size it may not pass
/// (`readRegularFileNoFollowOrThrow` with `maxBytes`): a larger one is
/// refused before a byte of it is read.
///
/// # Errors
///
/// As [`read_regular`]; and the file is larger than `max` bytes.
pub fn read_regular_within(path: &Path, what: &str, max: Option<u64>) -> Result<Vec<u8>> {
    read_checked(path, what, max).map(|(bytes, _)| bytes)
}

/// [`read_regular`], with when the file was last written
/// (`readRegularFileNoFollowOrThrow`'s `mtimeMs`): the time of the file
/// that was opened and read, which a later look at its name need not find.
///
/// # Errors
///
/// As [`read_regular`]; and the file system keeps no time for the file.
pub fn read_regular_with_time(path: &Path, what: &str) -> Result<(Vec<u8>, SystemTime)> {
    let (bytes, modified) = read_checked(path, what, None)?;
    match modified {
        Some(modified) => Ok((bytes, modified)),
        None => bail!("{what} has no modification time: {}", path.display()),
    }
}

fn read_checked(
    path: &Path,
    what: &str,
    max: Option<u64>,
) -> Result<(Vec<u8>, Option<SystemTime>)> {
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
    if let Some(max) = max
        && before.len > max
    {
        bail!(
            "{what} is {} bytes, above the {max}-byte limit: {shown}. Reduce the file before retrying.",
            before.len
        );
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
    Ok((bytes, before.modified))
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
) -> focus_js::Result<std::path::PathBuf> {
    crate::observer::refuse_write(crate::observer::WRITE_FILE_ATOMIC)?;
    let anchor = std::fs::canonicalize(record_root)
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", record_root.display())))?;
    let rel = Path::new(relative);
    crate::links::refuse_link_in_chain(&anchor, rel)?;
    let target = anchor.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", parent.display())))?;
    }
    // The parents exist now; none of them, nor the leaf, may be a link.
    crate::links::refuse_link_in_chain(&anchor, rel)?;
    crate::atomic::write_atomic(&target, data)
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
    layout: &crate::Layout,
    selection: &crate::records::Selection,
    name: &str,
    data: &str,
) -> focus_js::Result<std::path::PathBuf> {
    let record_root = selection.docs_root(layout);
    std::fs::create_dir_all(&record_root)
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", record_root.display())))?;
    write_record_file_no_follow(
        &record_root,
        &format!("{}/{name}", crate::names::ENGINE_DIR),
        data.as_bytes(),
    )
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

//! The commit a git repository's `HEAD` names, read from its files.
//!
//! A repository embedded in a source tree is listed as one entry: a
//! gitlink to its current commit. The walk asks for that commit without
//! running git (`gitHeadOid`, `aidlc-lib.ts` L23 005): it reads `HEAD`,
//! follows the reference it names through loose and packed references,
//! and accepts the object id only when the repository's object store
//! holds a commit under it, loose or in a pack.
//!
//! Every read is bounded, and every doubt is an answer of "none": a file
//! that is not regular or is larger than it may be, a pointer that leads
//! nowhere, a reference that is not a reference's name, an object that is
//! not a commit. The caller then cannot bind the source, which is the
//! safe reading of a repository it does not understand.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use flate2::{Decompress, FlushDecompress, Status};
use focus_js::order::cmp_utf16;
use focus_js::path::{self as jspath, Flavour};
use focus_js::re;
use focus_js::text::{len_utf16, to_lower_case, trim};

/// What an object is, as far as the walk cares.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ObjectType {
    Commit,
    Other,
}

fn said(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `realpathSync`, spelled as a person reads it.
fn real_path(path: &Path) -> Option<PathBuf> {
    let real = fs::canonicalize(path).ok()?;
    let text = real.to_string_lossy();
    Some(PathBuf::from(match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned(),
    }))
}

fn is_plain_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|stat| stat.is_dir())
}

fn is_plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|stat| stat.is_file())
}

/// The marks a file is known to have stood still by.
fn standing(meta: &fs::Metadata) -> (u64, Option<std::time::SystemTime>, i64, i64) {
    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt;
        (meta.ctime(), meta.ctime_nsec())
    };
    #[cfg(not(unix))]
    let changed = (0, 0);
    (meta.len(), meta.modified().ok(), changed.0, changed.1)
}

/// `boundedGitMetadataText`: the text of a regular file of at most
/// `max_bytes` that stood still while it was read.
fn bounded_text(path: &Path, max_bytes: u64) -> Option<String> {
    let stat = fs::symlink_metadata(path).ok()?;
    if !stat.is_file() || stat.len() > max_bytes {
        return None;
    }
    let mut file = File::open(path).ok()?;
    let before = file.metadata().ok()?;
    if !before.is_file() || before.len() > max_bytes {
        return None;
    }
    let mut bytes = vec![0_u8; before.len() as usize];
    let mut offset = 0;
    while offset < bytes.len() {
        let count = file.read(&mut bytes[offset..]).ok()?;
        if count == 0 {
            return None;
        }
        offset += count;
    }
    let after = file.metadata().ok()?;
    if standing(&before) != standing(&after) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `singleGitMetadataLine`: the one line a pointer file holds.
fn single_line(raw: &str) -> Option<&str> {
    let line = raw
        .strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(raw);
    (!line.is_empty() && !line.contains('\n') && !line.contains('\r')).then_some(line)
}

/// Where a pointer written in a file leads from `base`.
fn pointed(base: &Path, pointer: &str) -> PathBuf {
    let host = Flavour::host();
    let base = said(base);
    if jspath::is_absolute(host, pointer) {
        PathBuf::from(pointer)
    } else {
        PathBuf::from(jspath::resolve(host, &base, &[&base, pointer]))
    }
}

/// `gitMetadataDirectory`: the repository's own directory, whether `.git`
/// is it or a file that points at it.
fn metadata_directory(worktree: &Path) -> Option<PathBuf> {
    let marker = worktree.join(".git");
    let stat = fs::symlink_metadata(&marker).ok()?;
    if stat.is_dir() {
        return real_path(&marker);
    }
    if !stat.is_file() {
        return None;
    }
    let raw = bounded_text(&marker, 8192)?;
    let pointer = single_line(&raw)?.strip_prefix("gitdir: ")?;
    if pointer.is_empty() || pointer.contains('\0') {
        return None;
    }
    let target = pointed(worktree, pointer);
    is_plain_dir(&target).then(|| real_path(&target))?
}

/// `gitCommonDirectory`: where the references and objects are, which a
/// linked worktree's directory names in `commondir`.
fn common_directory(git_dir: &Path) -> Option<PathBuf> {
    let marker = git_dir.join("commondir");
    match fs::symlink_metadata(&marker) {
        Ok(stat) if stat.is_file() => {}
        Ok(_) => return None,
        Err(error) if error.kind() == ErrorKind::NotFound => return Some(git_dir.to_path_buf()),
        Err(_) => return None,
    }
    let raw = bounded_text(&marker, 8192)?;
    let pointer = single_line(&raw)?;
    if pointer.contains('\0') {
        return None;
    }
    let target = pointed(git_dir, pointer);
    is_plain_dir(&target).then(|| real_path(&target))?
}

/// `gitObjectDirectories`: the object store and the stores it borrows
/// from, at most sixty-four.
fn object_directories(common: &Path) -> Option<Vec<PathBuf>> {
    let host = Flavour::host();
    let mut queue = std::collections::VecDeque::from([common.join("objects")]);
    let mut directories: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    while let Some(candidate) = queue.pop_front() {
        if directories.len() >= 64 {
            return None;
        }
        if !is_plain_dir(&candidate) {
            return None;
        }
        let object_dir = real_path(&candidate)?;
        if !seen.insert(object_dir.clone()) {
            continue;
        }
        directories.push(object_dir.clone());
        let alternates_path = object_dir.join("info").join("alternates");
        match fs::symlink_metadata(&alternates_path) {
            Ok(stat) if stat.is_file() => {}
            Ok(_) => return None,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(_) => return None,
        }
        let alternates = bounded_text(&alternates_path, 1024 * 1024)?;
        let lines: Vec<&str> = alternates
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter(|line| !line.is_empty())
            .collect();
        if lines.len() > 64 {
            return None;
        }
        for line in lines {
            if len_utf16(line) > 4096 || line.contains('\0') || line.starts_with('#') {
                return None;
            }
            let base = said(&object_dir);
            queue.push_back(PathBuf::from(if jspath::is_absolute(host, line) {
                jspath::resolve(host, &base, &[line])
            } else {
                jspath::resolve(host, &base, &[&base, line])
            }));
        }
    }
    Some(directories)
}

/// `inflateSync`: the whole zlib stream at the start of `compressed`, or
/// `None` for one that is corrupt, stops short of its end, or inflates
/// past `limit`. What follows the stream is not read.
fn inflate(compressed: &[u8], limit: usize) -> Option<Vec<u8>> {
    let mut stream = Decompress::new(true);
    let mut inflated: Vec<u8> =
        Vec::with_capacity(compressed.len().saturating_mul(4).clamp(64, 1 << 20));
    loop {
        if inflated.len() == inflated.capacity() {
            if inflated.len() > limit {
                return None;
            }
            inflated.reserve(inflated.len().max(64));
        }
        let (read, written) = (stream.total_in(), stream.total_out());
        let rest = &compressed[usize::try_from(read).ok()?..];
        let status = stream
            .decompress_vec(rest, &mut inflated, FlushDecompress::None)
            .ok()?;
        if status == Status::StreamEnd {
            return (inflated.len() <= limit).then_some(inflated);
        }
        // No way forward: the input ran out before the stream ended.
        if stream.total_in() == read && stream.total_out() == written {
            return None;
        }
    }
}

fn loose_path(object_dir: &Path, oid: &str) -> PathBuf {
    object_dir.join(&oid[..2]).join(&oid[2..])
}

/// `looseGitObjectType`: what the loose object under `oid` says it is,
/// when its header and its length agree.
fn loose_object_type(object_dir: &Path, oid: &str) -> Option<ObjectType> {
    let path = loose_path(object_dir, oid);
    let stat = fs::symlink_metadata(&path).ok()?;
    if !stat.is_file() || stat.len() > 16 * 1024 * 1024 {
        return None;
    }
    let compressed = fs::read(&path).ok()?;
    let inflated = inflate(&compressed, 64 * 1024 * 1024)?;
    let separator = inflated.iter().position(|byte| *byte == 0)?;
    if separator == 0 || separator > 128 {
        return None;
    }
    // Read as ASCII, which in Node drops each byte's high bit.
    let header: String = inflated[..separator]
        .iter()
        .map(|byte| char::from(byte & 0x7f))
        .collect();
    let found = re::find("^(commit|tree|blob|tag) ([0-9]+)$", "", &header)?;
    let kind = re::group(&header, &found, 1)?;
    let declared: f64 = re::group(&header, &found, 2)?.parse().ok()?;
    if declared > 9_007_199_254_740_991.0 || declared != (inflated.len() - separator - 1) as f64 {
        return None;
    }
    Some(if kind == "commit" {
        ObjectType::Commit
    } else {
        ObjectType::Other
    })
}

/// Where an object is inside a pack.
struct Packed {
    offset: u64,
    pack_path: PathBuf,
}

fn be32(bytes: &[u8], at: usize) -> Option<u64> {
    let four: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    Some(u64::from(u32::from_be_bytes(four)))
}

/// The index of the first of `count` sorted ids, `width` bytes each and
/// `stride` apart from `first`, that is not below `oid`.
fn lower_bound(index: &[u8], first: usize, stride: usize, count: usize, oid: &[u8]) -> usize {
    let (mut low, mut high) = (0, count);
    while low < high {
        let mid = (low + high) / 2;
        let start = first + mid * stride;
        if index[start..start + oid.len()] < *oid {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// `packedGitObjectLocation`: the pack that holds `oid` and where in it,
/// from the pack indexes of one object store (either version).
///
/// `None` both for "not in these packs" and for a store whose packs cannot
/// be trusted; the caller moves on to the next store either way.
fn packed_object_location(object_dir: &Path, oid: &str) -> Option<Packed> {
    let pack_dir = object_dir.join("pack");
    let mut indexes: Vec<String> = fs::read_dir(&pack_dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| said(Path::new(&entry.file_name()))))
        .filter(|name| re::test(r"^pack-[0-9a-f]+\.idx$", "", name))
        .collect();
    indexes.sort_by(|a, b| cmp_utf16(a, b));
    if indexes.len() > 256 {
        return None;
    }
    let oid_bytes: Vec<u8> = (0..oid.len() / 2)
        .map(|at| u8::from_str_radix(&oid[at * 2..at * 2 + 2], 16))
        .collect::<Result<_, _>>()
        .ok()?;
    let width = oid_bytes.len();
    let mut total_index_bytes: u64 = 0;
    for index_name in indexes {
        let index_path = pack_dir.join(&index_name);
        let stat = fs::symlink_metadata(&index_path).ok()?;
        if !stat.is_file() || stat.len() > 128 * 1024 * 1024 {
            return None;
        }
        total_index_bytes += stat.len();
        if total_index_bytes > 256 * 1024 * 1024 {
            return None;
        }
        let index = fs::read(&index_path).ok()?;
        if index.len() < 1024 {
            return None;
        }
        let pack_path = pack_dir.join(format!(
            "{}.pack",
            index_name.strip_suffix(".idx").unwrap_or(&index_name)
        ));
        let version_two = be32(&index, 0)? == 0xff74_4f63;
        if !version_two {
            let count = be32(&index, 255 * 4)? as usize;
            let table = 256 * 4;
            let entry_size = 4 + width;
            if table + count * entry_size + width * 2 > index.len() {
                return None;
            }
            let low = lower_bound(&index, table + 4, entry_size, count, &oid_bytes);
            if low >= count {
                continue;
            }
            let entry = table + low * entry_size;
            if index[entry + 4..entry + 4 + width] != oid_bytes[..] {
                continue;
            }
            if !is_plain_file(&pack_path) {
                return None;
            }
            return Some(Packed {
                offset: be32(&index, entry)?,
                pack_path,
            });
        }
        if index.len() < 1032 || be32(&index, 4)? != 2 {
            return None;
        }
        let count = be32(&index, 8 + 255 * 4)? as usize;
        let oid_table = 8 + 256 * 4;
        let crc_table = oid_table + count * width;
        let offset_table = crc_table + count * 4;
        if offset_table + count * 4 > index.len() {
            return None;
        }
        let low = lower_bound(&index, oid_table, width, count, &oid_bytes);
        if low >= count {
            continue;
        }
        let found = oid_table + low * width;
        if index[found..found + width] != oid_bytes[..] {
            continue;
        }
        let raw_offset = be32(&index, offset_table + low * 4)?;
        let offset = if raw_offset & 0x8000_0000 == 0 {
            raw_offset
        } else {
            let large = offset_table + count * 4 + (raw_offset & 0x7fff_ffff) as usize * 8;
            let eight: [u8; 8] = index.get(large..large + 8)?.try_into().ok()?;
            let value = u64::from_be_bytes(eight);
            if value > 9_007_199_254_740_991 {
                return None;
            }
            value
        };
        if !is_plain_file(&pack_path) {
            return None;
        }
        return Some(Packed { offset, pack_path });
    }
    None
}

/// `packedGitObjectType`: what the object at a place in a pack is,
/// following a delta to the object it is a change of.
fn packed_object_type(
    location: &Packed,
    object_dirs: &[PathBuf],
    oid_width: usize,
    seen: &mut HashSet<String>,
    depth: usize,
) -> Option<ObjectType> {
    if depth > 64 {
        return None;
    }
    if !seen.insert(format!(
        "{}\0{}",
        said(&location.pack_path),
        location.offset
    )) {
        return None;
    }
    let mut file = File::open(&location.pack_path).ok()?;
    let stat = file.metadata().ok()?;
    if !stat.is_file() || location.offset < 12 || location.offset >= stat.len() {
        return None;
    }
    let mut pack_header = [0_u8; 12];
    file.read_exact(&mut pack_header).ok()?;
    if &pack_header[..4] != b"PACK" || !matches!(be32(&pack_header, 4)?, 2 | 3) {
        return None;
    }
    let available = (stat.len() - location.offset).min(96) as usize;
    let mut header = vec![0_u8; available];
    file.seek(SeekFrom::Start(location.offset)).ok()?;
    file.read_exact(&mut header).ok()?;
    let mut cursor = 0;
    let mut byte = header[cursor];
    cursor += 1;
    let packed_type = (byte >> 4) & 0x07;
    while byte & 0x80 != 0 {
        byte = *header.get(cursor)?;
        cursor += 1;
    }
    match packed_type {
        1 => Some(ObjectType::Commit),
        2..=4 => Some(ObjectType::Other),
        6 => {
            let mut offset_byte = *header.get(cursor)?;
            cursor += 1;
            let mut distance = u64::from(offset_byte & 0x7f);
            while offset_byte & 0x80 != 0 {
                offset_byte = *header.get(cursor)?;
                cursor += 1;
                distance = (distance + 1) * 128 + u64::from(offset_byte & 0x7f);
                if distance > 9_007_199_254_740_991 {
                    return None;
                }
            }
            let base_offset = location.offset.checked_sub(distance)?;
            if base_offset < 12 {
                return None;
            }
            packed_object_type(
                &Packed {
                    offset: base_offset,
                    pack_path: location.pack_path.clone(),
                },
                object_dirs,
                oid_width,
                seen,
                depth + 1,
            )
        }
        7 => {
            let base = header.get(cursor..cursor + oid_width)?;
            let base_oid: String = base.iter().map(|byte| format!("{byte:02x}")).collect();
            object_type_by_oid(object_dirs, &base_oid, seen, depth + 1)
        }
        _ => None,
    }
}

fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64)
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// `normalizeGitObjectId`: an object id in either case, in lower case.
fn normalized_object_id(text: &str) -> Option<String> {
    (matches!(text.len(), 40 | 64) && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

/// `gitObjectTypeByOid`: a loose object answers for its id wherever one
/// is found, before any pack is asked.
fn object_type_by_oid(
    object_dirs: &[PathBuf],
    oid: &str,
    seen: &mut HashSet<String>,
    depth: usize,
) -> Option<ObjectType> {
    if depth > 64 || !is_object_id(oid) {
        return None;
    }
    if !seen.insert(format!("oid\0{oid}")) {
        return None;
    }
    for object_dir in object_dirs {
        if loose_path(object_dir, oid).exists() {
            return loose_object_type(object_dir, oid);
        }
    }
    for object_dir in object_dirs {
        let Some(packed) = packed_object_location(object_dir, oid) else {
            continue;
        };
        return packed_object_type(&packed, object_dirs, oid.len() / 2, seen, depth + 1);
    }
    None
}

/// `validGitRefName`.
fn valid_ref_name(name: &str) -> bool {
    let forbidden = name.chars().any(|character| {
        let code = character.encode_utf16(&mut [0; 2])[0];
        code <= 0x20 || code == 0x7f || "~^:?*[\\]".contains(character)
    });
    if !name.starts_with("refs/")
        || name.ends_with('/')
        || name.contains("//")
        || name.contains("..")
        || name.contains("@{")
        || forbidden
    {
        return false;
    }
    name.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.starts_with('.')
            && !part.ends_with('.')
            && !part.ends_with(".lock")
    })
}

/// `gitRepositoryObjectIdLength`: how long an object id is in this
/// repository, from its `config`; `None` for a repository whose format
/// this reading does not cover.
fn object_id_length(common: &Path) -> Option<usize> {
    const SUPPORTED_EXTENSIONS: [&str; 8] = [
        "compatobjectformat",
        "noop",
        "objectformat",
        "partialclone",
        "preciousobjects",
        "relativeworktrees",
        "refstorage",
        "worktreeconfig",
    ];
    let raw = bounded_text(&common.join("config"), 1024 * 1024)?;
    let mut section = String::new();
    let mut has_subsection = false;
    let mut format_version: u64 = 0;
    let mut format_version_set = false;
    let mut object_format = 40;
    let mut explicitly_set = false;
    let mut saw_extension = false;
    for raw_line in raw.split('\n') {
        let line = trim(raw_line.strip_suffix('\r').unwrap_or(raw_line));
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(found) = re::find(
            r#"^\[\s*([A-Za-z0-9.-]+)(?:\s+"(?:[^"\\]|\\.)*")?\s*\]$"#,
            "",
            line,
        ) {
            section = to_lower_case(re::group(line, &found, 1).unwrap_or_default());
            has_subsection = re::test(r#"\s+""#, "", line);
            continue;
        }
        if has_subsection || (section != "core" && section != "extensions") {
            continue;
        }
        let found = re::find(r"^([A-Za-z][A-Za-z0-9-]*)\s*=\s*(.*?)\s*$", "", line)?;
        let key = to_lower_case(re::group(line, &found, 1)?);
        let value = re::group(line, &found, 2).unwrap_or_default();
        // One pair of quotes around the whole value comes off.
        let value = to_lower_case(
            value
                .strip_prefix('"')
                .and_then(|rest| rest.strip_suffix('"'))
                .unwrap_or(value),
        );
        if section == "core" {
            if key != "repositoryformatversion" {
                continue;
            }
            if !re::test("^[0-9]+$", "", &value) {
                return None;
            }
            let parsed = match trimmed_zeros(&value) {
                "0" => 0,
                "1" => 1,
                _ => return None,
            };
            if format_version_set && parsed != format_version {
                return None;
            }
            format_version = parsed;
            format_version_set = true;
            continue;
        }
        saw_extension = true;
        if !SUPPORTED_EXTENSIONS.contains(&key.as_str()) {
            return None;
        }
        if key == "refstorage" && value != "files" {
            return None;
        }
        if key != "objectformat" {
            continue;
        }
        let parsed = match value.as_str() {
            "sha1" => 40,
            "sha256" => 64,
            _ => return None,
        };
        if explicitly_set && parsed != object_format {
            return None;
        }
        object_format = parsed;
        explicitly_set = true;
    }
    if saw_extension && format_version != 1 {
        return None;
    }
    if object_format == 64 && format_version != 1 {
        return None;
    }
    Some(object_format)
}

/// A run of digits without its leading zeros (`Number` reads `"01"` as 1).
fn trimmed_zeros(digits: &str) -> &str {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() { "0" } else { trimmed }
}

/// `gitOidFromRef`: the object id a reference names, through loose
/// references (which may name another reference) and `packed-refs`.
fn oid_from_ref(
    git_dir: &Path,
    common: &Path,
    name: &str,
    seen: &mut HashSet<String>,
    depth: usize,
) -> Option<String> {
    if depth > 8 || !valid_ref_name(name) || !seen.insert(name.to_owned()) {
        return None;
    }
    // A worktree's own references are in its own directory.
    let base = if ["refs/bisect/", "refs/worktree/", "refs/rewritten/"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        git_dir
    } else {
        common
    };
    let candidate = name
        .split('/')
        .fold(base.to_path_buf(), |path, part| path.join(part));
    if candidate.exists() {
        let raw = bounded_text(&candidate, 8192)?;
        let line = single_line(&raw)?;
        if let Some(oid) = normalized_object_id(line) {
            return Some(oid);
        }
        return oid_from_ref(
            git_dir,
            common,
            line.strip_prefix("ref: ")?,
            seen,
            depth + 1,
        );
    }
    let packed_path = common.join("packed-refs");
    if !packed_path.exists() {
        return None;
    }
    let packed = bounded_text(&packed_path, 16 * 1024 * 1024)?;
    for line in packed.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let Some((oid, named)) = line.split_once(' ') else {
            continue;
        };
        if named != name {
            continue;
        }
        return normalized_object_id(oid);
    }
    None
}

/// `gitHeadOid`: the commit the repository checked out at `worktree` is
/// on, or `None` when that cannot be read with certainty.
#[must_use]
pub fn head_oid(worktree: &Path) -> Option<String> {
    let git_dir = metadata_directory(worktree)?;
    let common = common_directory(&git_dir)?;
    let width = object_id_length(&common)?;
    let raw = bounded_text(&git_dir.join("HEAD"), 8192)?;
    let line = single_line(&raw)?;
    let oid = match normalized_object_id(line) {
        Some(detached) => detached,
        None => oid_from_ref(
            &git_dir,
            &common,
            line.strip_prefix("ref: ")?,
            &mut HashSet::new(),
            0,
        )?,
    };
    if oid.len() != width {
        return None;
    }
    let object_dirs = object_directories(&common)?;
    (object_type_by_oid(&object_dirs, &oid, &mut HashSet::new(), 0)? == ObjectType::Commit)
        .then_some(oid)
}

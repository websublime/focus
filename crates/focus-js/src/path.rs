//! Node's `path` module, as Bun implements it: `resolve`, `normalize`,
//! `isAbsolute`, `join` and `basename`, in both the POSIX and the Windows
//! flavour.
//!
//! The ported hooks resolve the paths a command names with `node:path`, which
//! picks the flavour of the host it runs on, so a target reads differently on
//! Windows (`C:\x`, `\\server\share`, a drive-relative `D:x`). Whatever
//! reaches a decision or a message goes through here, not `std::path`, whose
//! normalisation (`..` kept, `//` kept) is not Node's.
//!
//! Bun's `node:path` is Node's `lib/path.js` as it stood before Node 22's
//! Windows reserved-name handling (CVE-2024-36139): `CON:` and `./C:x` pass
//! through unchanged, and a device root (`\\.\X`, `\\?\X`) is read as a
//! UNC root, server and share. That is what upstream runs on, so that is
//! what is ported, and the oracle corpus (`node_path` cases) holds Bun's
//! answers.
//!
//! Every function takes the working directory it would otherwise read from
//! `process.cwd()`. A drive's own working directory (`process.env["=C:"]`) is
//! not modelled: Node falls back to the process's directory without it.

/// Which host's rules a path follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Posix,
    Win32,
}

impl Flavour {
    /// The flavour of the host this binary runs on, as `node:path` picks it.
    #[must_use]
    pub fn host() -> Self {
        if cfg!(windows) {
            Self::Win32
        } else {
            Self::Posix
        }
    }

    fn is_separator(self, byte: u8) -> bool {
        match self {
            Self::Posix => byte == b'/',
            Self::Win32 => byte == b'/' || byte == b'\\',
        }
    }

    /// The separator the flavour writes.
    #[must_use]
    pub fn separator(self) -> &'static str {
        match self {
            Self::Posix => "/",
            Self::Win32 => "\\",
        }
    }
}

fn is_device_root(byte: u8) -> bool {
    byte.is_ascii_alphabetic()
}

/// `normalizeString`: the segments of `path` with `.` dropped and `..`
/// folded, joined by `separator`; `..` above the root is kept only when
/// `allow_above_root`.
fn normalize_string(path: &str, allow_above_root: bool, flavour: Flavour) -> String {
    let separator = flavour.separator();
    let bytes = path.as_bytes();
    let mut res = String::new();
    let mut last_segment_length = 0usize;
    let mut last_slash: isize = -1;
    let mut dots: isize = 0;
    let mut code = 0u8;
    let mut i = 0usize;
    while i <= bytes.len() {
        if i < bytes.len() {
            code = bytes[i];
        } else if flavour.is_separator(code) {
            break;
        } else {
            code = b'/';
        }
        if flavour.is_separator(code) {
            if last_slash == i as isize - 1 || dots == 1 {
                // Nothing: an empty segment or a `.`.
            } else if dots == 2 {
                if res.len() < 2 || last_segment_length != 2 || !res.ends_with("..") {
                    if res.len() > 2 {
                        match res.rfind(separator) {
                            None => {
                                res.clear();
                                last_segment_length = 0;
                            }
                            Some(last_slash_index) => {
                                res.truncate(last_slash_index);
                                last_segment_length = match res.rfind(separator) {
                                    Some(at) => res.len() - 1 - at,
                                    None => res.len(),
                                };
                            }
                        }
                        last_slash = i as isize;
                        dots = 0;
                        i += 1;
                        continue;
                    } else if !res.is_empty() {
                        res.clear();
                        last_segment_length = 0;
                        last_slash = i as isize;
                        dots = 0;
                        i += 1;
                        continue;
                    }
                }
                if allow_above_root {
                    if res.is_empty() {
                        res.push_str("..");
                    } else {
                        res.push_str(separator);
                        res.push_str("..");
                    }
                    last_segment_length = 2;
                }
            } else {
                let segment = &path[(last_slash + 1) as usize..i];
                if res.is_empty() {
                    res.push_str(segment);
                } else {
                    res.push_str(separator);
                    res.push_str(segment);
                }
                last_segment_length = i - (last_slash + 1) as usize;
            }
            last_slash = i as isize;
            dots = 0;
        } else if code == b'.' && dots != -1 {
            dots += 1;
        } else {
            dots = -1;
        }
        i += 1;
    }
    res
}

/// The root of a Windows path: `(root_end, device, is_absolute)`, as
/// `win32.resolve` reads it.
fn win32_root(path: &str) -> (usize, String, bool) {
    let bytes = path.as_bytes();
    let len = bytes.len();
    let flavour = Flavour::Win32;
    let mut root_end = 0;
    let mut device = String::new();
    let mut is_absolute = false;
    let code = bytes[0];
    if len == 1 {
        if flavour.is_separator(code) {
            root_end = 1;
            is_absolute = true;
        }
    } else if flavour.is_separator(code) {
        is_absolute = true;
        if flavour.is_separator(bytes[1]) {
            let mut j = 2;
            let mut last = j;
            while j < len && !flavour.is_separator(bytes[j]) {
                j += 1;
            }
            if j < len && j != last {
                let first_part = &path[last..j];
                last = j;
                while j < len && flavour.is_separator(bytes[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !flavour.is_separator(bytes[j]) {
                        j += 1;
                    }
                    if j == len || j != last {
                        // Bun reads a device root (`\\.\X`, `\\?\X`) as a
                        // UNC root too, where Node keeps `\\.` apart.
                        device = format!("\\\\{first_part}\\{}", &path[last..j]);
                        root_end = j;
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else if is_device_root(code) && bytes[1] == b':' {
        device = path[..2].to_owned();
        root_end = 2;
        if len > 2 && flavour.is_separator(bytes[2]) {
            is_absolute = true;
            root_end = 3;
        }
    }
    (root_end, device, is_absolute)
}

/// `path.resolve(...parts)` with `cwd` standing in for `process.cwd()`.
#[must_use]
pub fn resolve(flavour: Flavour, cwd: &str, parts: &[&str]) -> String {
    match flavour {
        Flavour::Posix => posix_resolve(cwd, parts),
        Flavour::Win32 => win32_resolve(cwd, parts),
    }
}

fn posix_resolve(cwd: &str, parts: &[&str]) -> String {
    if (parts.is_empty() || (parts.len() == 1 && (parts[0].is_empty() || parts[0] == ".")))
        && cwd.starts_with('/')
    {
        return cwd.to_owned();
    }
    let mut resolved_path = String::new();
    let mut resolved_absolute = false;
    for path in parts.iter().rev() {
        if resolved_absolute {
            break;
        }
        if path.is_empty() {
            continue;
        }
        resolved_path = format!("{path}/{resolved_path}");
        resolved_absolute = path.starts_with('/');
    }
    if !resolved_absolute {
        resolved_path = format!("{cwd}/{resolved_path}");
        resolved_absolute = cwd.starts_with('/');
    }
    resolved_path = normalize_string(&resolved_path, !resolved_absolute, Flavour::Posix);
    if resolved_absolute {
        format!("/{resolved_path}")
    } else if resolved_path.is_empty() {
        ".".to_owned()
    } else {
        resolved_path
    }
}

fn win32_resolve(cwd: &str, parts: &[&str]) -> String {
    let flavour = Flavour::Win32;
    let mut resolved_device = String::new();
    let mut resolved_tail = String::new();
    let mut resolved_absolute = false;
    let mut i = parts.len() as isize - 1;
    while i >= -1 {
        let owned: String;
        let path: &str = if i >= 0 {
            let part = parts[i as usize];
            if part.is_empty() {
                i -= 1;
                continue;
            }
            part
        } else if resolved_device.is_empty() {
            // Fast path for the current directory.
            if parts.is_empty()
                || (parts.len() == 1
                    && (parts[0].is_empty() || parts[0] == ".")
                    && cwd
                        .as_bytes()
                        .first()
                        .is_some_and(|b| flavour.is_separator(*b)))
            {
                // `process.cwd()` as the Windows flavour returns it: on
                // Windows it already has backslashes, elsewhere Bun's answer
                // has the POSIX separators turned, so the one rule serves
                // both hosts (and the corpus, recorded on one).
                return cwd.replace('/', "\\");
            }
            cwd
        } else {
            // The drive's own working directory is not modelled: the
            // process's stands in, or the drive's root when it is elsewhere.
            if cwd.len() >= 2
                && !cwd[..2].eq_ignore_ascii_case(&resolved_device)
                && cwd.as_bytes().get(2) == Some(&b'\\')
            {
                owned = format!("{resolved_device}\\");
                owned.as_str()
            } else {
                cwd
            }
        };
        if path.is_empty() {
            // `process.cwd()` is never empty; a caller's empty `cwd` reads as
            // a path with no root.
            i -= 1;
            continue;
        }
        let (root_end, device, is_absolute) = win32_root(path);
        if !device.is_empty() {
            if !resolved_device.is_empty() {
                if !device.eq_ignore_ascii_case(&resolved_device) {
                    i -= 1;
                    continue;
                }
            } else {
                resolved_device = device;
            }
        }
        if resolved_absolute {
            if !resolved_device.is_empty() {
                break;
            }
        } else {
            resolved_tail = format!("{}\\{resolved_tail}", &path[root_end..]);
            resolved_absolute = is_absolute;
            if is_absolute && !resolved_device.is_empty() {
                break;
            }
        }
        i -= 1;
    }
    resolved_tail = normalize_string(&resolved_tail, !resolved_absolute, flavour);
    if resolved_absolute {
        format!("{resolved_device}\\{resolved_tail}")
    } else {
        let joined = format!("{resolved_device}{resolved_tail}");
        if joined.is_empty() {
            ".".to_owned()
        } else {
            joined
        }
    }
}

/// `path.normalize`.
#[must_use]
pub fn normalize(flavour: Flavour, path: &str) -> String {
    match flavour {
        Flavour::Posix => posix_normalize(path),
        Flavour::Win32 => win32_normalize(path),
    }
}

fn posix_normalize(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let is_absolute = path.starts_with('/');
    let trailing_separator = path.ends_with('/');
    let normalized = normalize_string(path, !is_absolute, Flavour::Posix);
    if normalized.is_empty() {
        if is_absolute {
            return "/".to_owned();
        }
        return if trailing_separator { "./" } else { "." }.to_owned();
    }
    let mut out = String::new();
    if is_absolute {
        out.push('/');
    }
    out.push_str(&normalized);
    if trailing_separator {
        out.push('/');
    }
    out
}

fn win32_normalize(path: &str) -> String {
    let flavour = Flavour::Win32;
    let bytes = path.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return ".".to_owned();
    }
    let code = bytes[0];
    if len == 1 {
        return if code == b'/' {
            "\\".to_owned()
        } else {
            path.to_owned()
        };
    }
    let mut root_end = 0;
    let mut device: Option<String> = None;
    let mut is_absolute = false;
    if flavour.is_separator(code) {
        is_absolute = true;
        if flavour.is_separator(bytes[1]) {
            let mut j = 2;
            let mut last = j;
            while j < len && !flavour.is_separator(bytes[j]) {
                j += 1;
            }
            if j < len && j != last {
                let first_part = &path[last..j];
                last = j;
                while j < len && flavour.is_separator(bytes[j]) {
                    j += 1;
                }
                if j < len && j != last {
                    last = j;
                    while j < len && !flavour.is_separator(bytes[j]) {
                        j += 1;
                    }
                    if j == len {
                        // A UNC root alone.
                        return format!("\\\\{first_part}\\{}\\", &path[last..]);
                    }
                    if j != last {
                        device = Some(format!("\\\\{first_part}\\{}", &path[last..j]));
                        root_end = j;
                    }
                }
            }
        } else {
            root_end = 1;
        }
    } else if is_device_root(code) && bytes[1] == b':' {
        device = Some(path[..2].to_owned());
        root_end = 2;
        if len > 2 && flavour.is_separator(bytes[2]) {
            is_absolute = true;
            root_end = 3;
        }
    }
    let mut tail = if root_end < len {
        normalize_string(&path[root_end..], !is_absolute, flavour)
    } else {
        String::new()
    };
    if tail.is_empty() && !is_absolute {
        tail.push('.');
    }
    if !tail.is_empty() && flavour.is_separator(bytes[len - 1]) {
        tail.push('\\');
    }
    match device {
        None => {
            if is_absolute {
                format!("\\{tail}")
            } else {
                tail
            }
        }
        Some(device) => {
            if is_absolute {
                format!("{device}\\{tail}")
            } else {
                format!("{device}{tail}")
            }
        }
    }
}

/// `path.isAbsolute`.
#[must_use]
pub fn is_absolute(flavour: Flavour, path: &str) -> bool {
    let bytes = path.as_bytes();
    match flavour {
        Flavour::Posix => bytes.first() == Some(&b'/'),
        Flavour::Win32 => {
            let Some(&code) = bytes.first() else {
                return false;
            };
            flavour.is_separator(code)
                || (bytes.len() > 2
                    && is_device_root(code)
                    && bytes[1] == b':'
                    && flavour.is_separator(bytes[2]))
        }
    }
}

/// `path.join(...parts)`.
#[must_use]
pub fn join(flavour: Flavour, parts: &[&str]) -> String {
    let parts: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|part| !part.is_empty())
        .collect();
    if parts.is_empty() {
        return ".".to_owned();
    }
    match flavour {
        Flavour::Posix => posix_normalize(&parts.join("/")),
        Flavour::Win32 => {
            let first_part = parts[0].as_bytes();
            let mut joined = parts.join("\\");
            // Two leading separators would read as a UNC root: they are
            // folded unless the first part plainly names one.
            let mut needs_replace = true;
            let mut slash_count = 0;
            if flavour.is_separator(first_part[0]) {
                slash_count += 1;
                if first_part.len() > 1 && flavour.is_separator(first_part[1]) {
                    slash_count += 1;
                    if first_part.len() > 2 {
                        if flavour.is_separator(first_part[2]) {
                            slash_count += 1;
                        } else {
                            needs_replace = false;
                        }
                    }
                }
            }
            if needs_replace {
                let bytes = joined.as_bytes();
                while slash_count < bytes.len() && flavour.is_separator(bytes[slash_count]) {
                    slash_count += 1;
                }
                if slash_count >= 2 {
                    joined = format!("\\{}", &joined[slash_count..]);
                }
            }
            win32_normalize(&joined)
        }
    }
}

/// `path.basename(path)`: the last segment, trailing separators ignored.
#[must_use]
pub fn basename(flavour: Flavour, path: &str) -> String {
    let bytes = path.as_bytes();
    let mut start = 0usize;
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    if flavour == Flavour::Win32 && bytes.len() >= 2 && is_device_root(bytes[0]) && bytes[1] == b':'
    {
        start = 2;
    }
    let mut i = bytes.len();
    while i > start {
        i -= 1;
        if flavour.is_separator(bytes[i]) {
            if !matched_slash {
                start = i + 1;
                break;
            }
        } else if end.is_none() {
            matched_slash = false;
            end = Some(i + 1);
        }
    }
    match end {
        None => String::new(),
        Some(end) => path[start..end].to_owned(),
    }
}

/// `path.dirname`.
#[must_use]
pub fn dirname(flavour: Flavour, path: &str) -> String {
    let bytes = path.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return ".".to_owned();
    }
    match flavour {
        Flavour::Posix => {
            let has_root = bytes[0] == b'/';
            let mut end: Option<usize> = None;
            let mut matched_slash = true;
            let mut i = len;
            while i > 1 {
                i -= 1;
                if bytes[i] == b'/' {
                    if !matched_slash {
                        end = Some(i);
                        break;
                    }
                } else {
                    matched_slash = false;
                }
            }
            match end {
                None => if has_root { "/" } else { "." }.to_owned(),
                Some(1) if has_root => "//".to_owned(),
                Some(end) => path[..end].to_owned(),
            }
        }
        Flavour::Win32 => {
            let code = bytes[0];
            if len == 1 {
                return if flavour.is_separator(code) {
                    path.to_owned()
                } else {
                    ".".to_owned()
                };
            }
            let mut root_end: Option<usize> = None;
            let mut offset = 0;
            if flavour.is_separator(code) {
                root_end = Some(1);
                offset = 1;
                if flavour.is_separator(bytes[1]) {
                    let mut j = 2;
                    let mut last = j;
                    while j < len && !flavour.is_separator(bytes[j]) {
                        j += 1;
                    }
                    if j < len && j != last {
                        last = j;
                        while j < len && flavour.is_separator(bytes[j]) {
                            j += 1;
                        }
                        if j < len && j != last {
                            last = j;
                            while j < len && !flavour.is_separator(bytes[j]) {
                                j += 1;
                            }
                            if j == len {
                                // A UNC root alone.
                                return path.to_owned();
                            }
                            if j != last {
                                // Offset by 1 to include the separator after
                                // the UNC root, a "normal root" on top of it.
                                root_end = Some(j + 1);
                                offset = j + 1;
                            }
                        }
                    }
                }
            } else if is_device_root(code) && bytes[1] == b':' {
                let at = if len > 2 && flavour.is_separator(bytes[2]) {
                    3
                } else {
                    2
                };
                root_end = Some(at);
                offset = at;
            }
            let mut end: Option<usize> = None;
            let mut matched_slash = true;
            let mut i = len;
            while i > offset {
                i -= 1;
                if flavour.is_separator(bytes[i]) {
                    if !matched_slash {
                        end = Some(i);
                        break;
                    }
                } else {
                    matched_slash = false;
                }
            }
            let end = match (end, root_end) {
                (Some(end), _) => end,
                (None, None) => return ".".to_owned(),
                (None, Some(root_end)) => root_end,
            };
            path[..end].to_owned()
        }
    }
}

/// `path.relative(from, to)` with `cwd` standing in for `process.cwd()`.
#[must_use]
pub fn relative(flavour: Flavour, cwd: &str, from: &str, to: &str) -> String {
    if from == to {
        return String::new();
    }
    match flavour {
        Flavour::Posix => posix_relative(cwd, from, to),
        Flavour::Win32 => win32_relative(cwd, from, to),
    }
}

fn posix_relative(cwd: &str, from: &str, to: &str) -> String {
    let from = posix_resolve(cwd, &[from]);
    let to = posix_resolve(cwd, &[to]);
    if from == to {
        return String::new();
    }
    let (from_bytes, to_bytes) = (from.as_bytes(), to.as_bytes());
    let from_start = 1;
    let from_end = from_bytes.len();
    let from_len = from_end - from_start;
    let to_start = 1;
    let to_len = to_bytes.len() - to_start;
    let length = from_len.min(to_len);
    let mut last_common_sep: Option<usize> = None;
    let mut i = 0;
    while i < length {
        let from_code = from_bytes[from_start + i];
        if from_code != to_bytes[to_start + i] {
            break;
        } else if from_code == b'/' {
            last_common_sep = Some(i);
        }
        i += 1;
    }
    if i == length {
        if to_len > length {
            if to_bytes[to_start + i] == b'/' {
                return to[to_start + i + 1..].to_owned();
            }
            if i == 0 {
                return to[to_start + i..].to_owned();
            }
        } else if from_len > length {
            if from_bytes[from_start + i] == b'/' {
                last_common_sep = Some(i);
            } else if i == 0 {
                last_common_sep = Some(0);
            }
        }
    }
    let mut out = String::new();
    let mut i = from_start + last_common_sep.map_or(0, |at| at + 1);
    while i <= from_end {
        if i == from_end || from_bytes[i] == b'/' {
            out.push_str(if out.is_empty() { ".." } else { "/.." });
        }
        i += 1;
    }
    // `to.slice(toStart + lastCommonSep)`: the whole of `to` when no
    // separator was common.
    let tail_from = match last_common_sep {
        Some(at) => to_start + at,
        None => to_start - 1,
    };
    format!("{out}{}", &to[tail_from..])
}

fn win32_relative(cwd: &str, from: &str, to: &str) -> String {
    let from_orig = win32_resolve(cwd, &[from]);
    let to_orig = win32_resolve(cwd, &[to]);
    if from_orig == to_orig {
        return String::new();
    }
    // Bun compares the two paths with ASCII case folded, where Node folds
    // every character (and re-compares by segment when that moves one).
    let from_lower = from_orig.to_ascii_lowercase();
    let to_lower = to_orig.to_ascii_lowercase();
    if from_lower == to_lower {
        return String::new();
    }
    let to_o: Vec<char> = to_orig.chars().collect();
    let (from, to): (Vec<char>, Vec<char>) =
        (from_lower.chars().collect(), to_lower.chars().collect());
    let mut from_start = 0;
    while from_start < from.len() && from[from_start] == '\\' {
        from_start += 1;
    }
    let mut from_end = from.len();
    while from_end > from_start + 1 && from[from_end - 1] == '\\' {
        from_end -= 1;
    }
    let from_len = from_end - from_start;
    let mut to_start = 0;
    while to_start < to.len() && to[to_start] == '\\' {
        to_start += 1;
    }
    let mut to_end = to.len();
    while to_end > to_start + 1 && to[to_end - 1] == '\\' {
        to_end -= 1;
    }
    let to_len = to_end - to_start;
    let length = from_len.min(to_len);
    let mut last_common_sep: Option<usize> = None;
    let mut i = 0;
    while i < length {
        let from_code = from[from_start + i];
        if from_code != to[to_start + i] {
            break;
        } else if from_code == '\\' {
            last_common_sep = Some(i);
        }
        i += 1;
    }
    let text = |chars: &[char], from: usize, to: usize| -> String {
        chars[from.min(chars.len())..to.min(chars.len()).max(from.min(chars.len()))]
            .iter()
            .collect()
    };
    if i != length {
        if last_common_sep.is_none() {
            return to_orig;
        }
    } else {
        if to_len > length {
            if to[to_start + i] == '\\' {
                return text(&to_o, to_start + i + 1, to_o.len());
            }
            if i == 2 {
                return text(&to_o, to_start + i, to_o.len());
            }
        }
        if from_len > length {
            if from[from_start + i] == '\\' {
                last_common_sep = Some(i);
            } else if i == 2 {
                last_common_sep = Some(3);
            }
        }
        if last_common_sep.is_none() {
            last_common_sep = Some(0);
        }
    }
    let mut out = String::new();
    let mut i = from_start + last_common_sep.map_or(0, |at| at + 1);
    while i <= from_end {
        if i == from_end || from[i] == '\\' {
            out.push_str(if out.is_empty() { ".." } else { "\\.." });
        }
        i += 1;
    }
    let mut to_start = to_start + last_common_sep.unwrap_or(0);
    if !out.is_empty() {
        return format!("{out}{}", text(&to_o, to_start, to_end));
    }
    if to_o.get(to_start) == Some(&'\\') {
        to_start += 1;
    }
    text(&to_o, to_start, to_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_folds_dots_and_keeps_a_trailing_separator() {
        assert_eq!(normalize(Flavour::Posix, "a//b/../c/./"), "a/c/");
        assert_eq!(normalize(Flavour::Posix, "/../../x"), "/x");
        assert_eq!(normalize(Flavour::Posix, "..//./../a"), "../../a");
        assert_eq!(normalize(Flavour::Posix, ""), ".");
        assert_eq!(normalize(Flavour::Posix, "/"), "/");
        assert_eq!(resolve(Flavour::Posix, "/w", &["/a", "../../b", ""]), "/b");
        assert_eq!(resolve(Flavour::Posix, "/w", &["x", "/y", "z"]), "/y/z");
        assert_eq!(resolve(Flavour::Posix, "/w", &["a"]), "/w/a");
        assert_eq!(resolve(Flavour::Posix, "/w", &[]), "/w");
        assert_eq!(join(Flavour::Posix, &["", "a", "", "/b/"]), "a/b/");
        assert_eq!(basename(Flavour::Posix, "/a/b//"), "b");
        assert_eq!(basename(Flavour::Posix, "//"), "");
        assert!(is_absolute(Flavour::Posix, "/x"));
        assert!(!is_absolute(Flavour::Posix, "x"));
        assert_eq!(relative(Flavour::Posix, "/w", "/a/b/c", "/a/d"), "../../d");
        assert_eq!(relative(Flavour::Posix, "/w", "/a/b", "/a/b/c"), "c");
        assert_eq!(relative(Flavour::Posix, "/w", "/a/b/c", "/a/b"), "..");
        assert_eq!(relative(Flavour::Posix, "/w", "/", "/foo"), "foo");
        assert_eq!(relative(Flavour::Posix, "/w", "/foo/bar", "/"), "../..");
        assert_eq!(relative(Flavour::Posix, "/w", "x", "x/y"), "y");
        assert_eq!(relative(Flavour::Posix, "/w", "/a", "/ab"), "../ab");
        assert_eq!(dirname(Flavour::Posix, "/a/b/c"), "/a/b");
        assert_eq!(dirname(Flavour::Posix, "/a"), "/");
        assert_eq!(dirname(Flavour::Posix, "a"), ".");
        assert_eq!(dirname(Flavour::Posix, "//a"), "//");
    }

    #[test]
    fn win32_reads_drives_and_unc_roots_as_bun_does() {
        assert_eq!(normalize(Flavour::Win32, "CON:"), "CON:");
        assert_eq!(normalize(Flavour::Win32, "x:"), "x:.");
        assert_eq!(normalize(Flavour::Win32, "./C:x"), "C:x");
        assert_eq!(normalize(Flavour::Win32, "..\\C:\\x"), "..\\C:\\x");
        assert_eq!(
            join(Flavour::Win32, &["//server", "share"]),
            "\\\\server\\share\\"
        );
        assert_eq!(join(Flavour::Win32, &["\\\\\\a", "b"]), "\\a\\b");
        assert_eq!(join(Flavour::Win32, &["C:", "..", "x"]), "C:\\x");
        assert_eq!(
            resolve(Flavour::Win32, "/tmp", &["C:x", "y"]),
            "C:\\tmp\\x\\y"
        );
        assert_eq!(
            resolve(Flavour::Win32, "/tmp", &["D:\\a", "..\\..\\b"]),
            "D:\\b"
        );
        assert_eq!(
            resolve(Flavour::Win32, "/tmp", &["\\\\s\\sh", "x"]),
            "\\\\s\\sh\\x"
        );
        assert_eq!(resolve(Flavour::Win32, "/tmp", &["a"]), "\\tmp\\a");
        assert_eq!(
            resolve(Flavour::Win32, "/tmp", &["\\\\.\\x\\y"]),
            "\\\\.\\x\\y"
        );
        assert_eq!(
            resolve(Flavour::Win32, "/tmp", &["//s//sh//x"]),
            "\\\\s\\sh\\x"
        );
        assert_eq!(resolve(Flavour::Win32, "C:\\w", &["D:x"]), "D:\\x");
        assert_eq!(basename(Flavour::Win32, "C:\\a\\b\\"), "b");
        assert_eq!(basename(Flavour::Win32, "C:"), "");
        assert_eq!(basename(Flavour::Win32, "a/b\\c//"), "c");
        assert!(is_absolute(Flavour::Win32, "C:\\"));
        assert!(!is_absolute(Flavour::Win32, "C:"));
        assert!(is_absolute(Flavour::Win32, "/x"));
        assert_eq!(
            relative(Flavour::Win32, "C:\\w", "C:\\foo\\bar", "C:\\foo\\bar\\baz"),
            "baz"
        );
        assert_eq!(
            relative(Flavour::Win32, "C:\\w", "C:\\foo\\bar", "C:\\foo"),
            ".."
        );
        assert_eq!(relative(Flavour::Win32, "C:\\w", "C:\\", "C:\\foo"), "foo");
        assert_eq!(
            relative(Flavour::Win32, "C:\\w", "C:\\foo\\bar", "C:\\"),
            "..\\.."
        );
        assert_eq!(
            relative(Flavour::Win32, "C:\\w", "C:\\a\\b", "D:\\a"),
            "D:\\a"
        );
        assert_eq!(
            relative(Flavour::Win32, "C:\\w", "c:\\A\\b", "C:\\a\\C"),
            "..\\C"
        );
        assert_eq!(dirname(Flavour::Win32, "C:\\a\\b"), "C:\\a");
        assert_eq!(dirname(Flavour::Win32, "C:\\a"), "C:\\");
        assert_eq!(dirname(Flavour::Win32, "\\\\s\\sh\\f"), "\\\\s\\sh\\");
        assert_eq!(dirname(Flavour::Win32, "\\\\s\\sh"), "\\\\s\\sh");
    }
}

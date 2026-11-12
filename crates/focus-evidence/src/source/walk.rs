//! The walk that reads a source tree into a listing and a fingerprint.
//!
//! One bounded pass over the files of a project (`filesystemSourceIdentity`,
//! `aidlc-lib.ts` L23 035) says what its source is: every file with the
//! hash of its text, every link with where it points and what is there,
//! and nothing of what a build, a package manager, an editor or the engine
//! itself leaves in the tree. [`workspace_source`] (`walkWorkspaceSource`)
//! is the same over a project that is one repository or a roof over
//! several.
//!
//! What stays outside the boundary is decided by name, never by asking git,
//! so the same tree reads the same with and without a repository around
//! it. A project names its own exceptions in a registry file it commits:
//! paths that are source whatever they look like, and paths that are not.
//!
//! What this port leaves out, each refused by name where it would be read:
//! the context of a delegated worktree, and a git repository embedded in
//! the tree (its `HEAD` is the next slice). The paths the walk collects for
//! a snapshot index (pathspecs, snapshot paths) serve parallel Construction
//! only, which is deferred, and are not collected.
//!
//! # Tests that another test covers
//!
//! Upstream asks some questions twice, and the port keeps each where
//! upstream has it. Taking one out changes no answer, because another
//! answers for it:
//!
//! - A hop out of the root is seen in the link's directory, in where its
//!   text leads, and in the real target; then again by the registered path
//!   after each part. The first two each have a case only they see; the
//!   target's and the registered path's own (including the part that does
//!   not exist) are always preceded by one of them.
//! - A chain of links that loops is ended by the set of links seen, and
//!   would be by the forty hops.
//! - A registry path that is `.`, absolute or starts with `//` also fails
//!   the test of its parts; a registry that is a link is not a regular
//!   file; the workspace's hidden directory is among the known harness
//!   directories; a harness manifest over the size is over it again as
//!   text; a file at the root cannot hold a manifest, whatever its name.
//! - The engine's directory of a record needs a part after `intents`: with
//!   none there is nothing for it to be.
//! - Text has a CRLF whenever a second read of the file was asked for; a
//!   file without a carriage return is not read twice; an empty file reads
//!   as text with or without the question.
//! - Once the earlier walk's value is given up, nothing more is added to
//!   it, counted or not.
//! - A path the registry excludes and everything under it are excluded by
//!   the same prefix, so a directory under one is walked the same by
//!   either branch; a file with a registered path under it cannot exist (the
//!   registered path fails to resolve first), so a file that is only on the
//!   way to one is never reached.
//! - A registered path that resolves outside the root, or to the root
//!   itself, adds a name no entry of the walk has.
//!
//! One group holds only until embedded repositories are read: a directory
//! is visited a second time under the same key only when the first visit
//! was inside a repository of its own, so until then what is recorded on a
//! second visit, and whether there is one, cannot be told apart.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use focus_js::order::cmp_utf16;
use focus_js::path::{self as jspath, Flavour};
use focus_js::text::{len_utf16, trim};
use focus_js::{json, re};
use focus_workspace::names::{
    ENGINE_DIR, KNOWN_HARNESS_DIRS, LOCAL_SETTINGS_FILE, SETTINGS_FILE, WORKSPACE_DIR,
    WORKTREE_META,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::listing::Listing;
use super::raw;

/// `SOURCE_FINGERPRINT_REGISTRY`: the project's own say over the boundary.
pub const REGISTRY: &str = ".focus-source-paths.json";
/// The workspace's hidden directory, where a delegated worktree keeps its
/// record.
const HIDDEN_WORKSPACE_DIR: &str = ".focus";
/// `LEGACY_SENSORS_DIR`.
const SENSORS_DIR: &str = ".focus-sensors";
/// `SOURCE_FINGERPRINT_HARD_EXCLUDED_NAMES`: directories that are never
/// source, wherever they are.
const HARD_EXCLUDED_DIRS: [&str; 17] = [
    ".cache",
    ".codegraph",
    ".git",
    ".gradle",
    ".idea",
    ".mypy_cache",
    ".next",
    ".nuxt",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".venv",
    ".vs",
    "DerivedData",
    "__pycache__",
    "node_modules",
    "venv",
];
const PYCACHE_DIR: &str = "__pycache__";
/// `SOURCE_FINGERPRINT_HARD_EXCLUDED_FILES`.
const HARD_EXCLUDED_FILES: [&str; 4] = [".DS_Store", ".coverage", "Thumbs.db", "desktop.ini"];
/// `SOURCE_FINGERPRINT_CONDITIONAL_NAMES`: names that commonly hold
/// generated output and can hold source; outside unless registered.
const CONDITIONAL_DIRS: [&str; 6] = ["build", "coverage", "dist", "logs", "target", "tmp"];
/// `SOURCE_FINGERPRINT_DOTNET_OUTPUT_NAMES`: conditional beside a .NET
/// project file only.
const DOTNET_OUTPUT_NAMES: [&str; 3] = ["bin", "obj", "out"];
/// `SOURCE_FINGERPRINT_MANIFEST_DEPENDENCY_DIRS`: the directories a package
/// manager fills beside the manifest it reads.
const MANIFEST_DEPENDENCY_DIRS: [(&str, &[&str]); 8] = [
    ("composer.json", &["vendor"]),
    ("go.mod", &["vendor"]),
    ("Podfile", &["Pods"]),
    ("mix.exs", &["deps", "_build"]),
    ("pubspec.yaml", &[".dart_tool"]),
    ("bower.json", &["bower_components"]),
    ("Gemfile", &[".bundle"]),
    ("stack.yaml", &[".stack-work"]),
];
const SOURCE_EXTENSION: &str = r"\.(?:astro|c|cc|cmake|cpp|cs|css|dart|erl|ex|exs|go|gql|graphql|h|hcl|hpp|hrl|html|java|js|jsx|json|kt|kts|lua|m|mm|php|proto|ps1|psd1|psm1|py|r|rb|rs|scala|sh|sol|sql|svelte|swift|tf|tfvars|toml|ts|tsx|vue|xml|ya?ml|zig)$";
const SOURCE_BASENAME: &str = r"^(?:BUILD|CMakeLists\.txt|Dockerfile(?:\..+)?|Gemfile|Justfile|Makefile|Procfile|Tiltfile|WORKSPACE)$";

const MAX_FILES: u64 = 250_000;
const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SOURCE_ONLY_MAX_FILES: u64 = 10_000;
const SOURCE_ONLY_MAX_BYTES: u64 = 64 * 1024 * 1024;
const LEGACY_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// `SOURCE_FILE_READ_ATTEMPTS`: a file that moves under the read is read
/// again this many times before it counts as unreadable.
const READ_ATTEMPTS: usize = 3;

/// Why a walk could not say what the source is (`WorkspaceSourceFailure`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    /// The kind of failure, as upstream names it.
    pub code: &'static str,
    /// What happened, in a sentence fragment.
    pub detail: String,
    /// The path it happened at, relative to the root walked.
    pub path: Option<String>,
    /// The member repository it happened in, in a project of several.
    pub repo: Option<String>,
}

/// Why a walk stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The source cannot be bound, as upstream would also find.
    Failed(Failure),
    /// The tree holds something this port does not read yet: named, so the
    /// command refuses instead of answering for a tree it did not read.
    NotPorted(&'static str),
}

fn failed<T>(code: &'static str, detail: impl Into<String>, path: &str) -> Result<T, Stop> {
    Err(Stop::Failed(Failure {
        code,
        detail: detail.into(),
        path: Some(path.to_owned()),
        repo: None,
    }))
}

/// The limits of one walk that a test can lower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budgets {
    /// Directory entries read.
    pub entries: u64,
    /// Directories entered.
    pub directories: u64,
    /// Links met.
    pub symlinks: u64,
    /// Files the earlier walk's value may be rebuilt from.
    pub legacy_files: u64,
}

impl Budgets {
    /// The budgets from what the four variables hold, each read as
    /// upstream's `sourceIdentityBudget` reads it: a positive decimal
    /// integer without a leading zero, or the default.
    #[must_use]
    pub fn from(read: impl Fn(&str) -> Option<String>) -> Self {
        let budget = |name: &str, fallback: u64| {
            read(name)
                .filter(|raw| re::test("^[1-9][0-9]*$", "", raw))
                .and_then(|raw| raw.parse::<f64>().ok())
                // A count past what a `u64` holds is no limit at all.
                .map_or(fallback, |value| {
                    if value >= 1.8e19 {
                        u64::MAX
                    } else {
                        value as u64
                    }
                })
        };
        Self {
            entries: budget("TEST_SOURCE_MAX_ENTRIES", 250_000),
            directories: budget("TEST_SOURCE_MAX_DIRECTORIES", 100_000),
            symlinks: budget("TEST_SOURCE_MAX_SYMLINKS", 100_000),
            legacy_files: budget("TEST_SOURCE_LEGACY_MAX_FILES", 10_000),
        }
    }

    /// The budgets this process's environment asks for.
    #[must_use]
    pub fn from_env() -> Self {
        Self::from(focus_workspace::env::var)
    }
}

/// What a walk reads or hashes under the engine's own name: the two
/// labels a fingerprint is taken under, and the name of the registry file.
/// The labels are part of what is hashed, so a fingerprint of Focus's is
/// never one of upstream's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    /// The label of one root's files.
    pub filesystem: String,
    /// The label of a project's roots together.
    pub workspace: String,
    /// The registry file, at the root of what is walked.
    pub registry: String,
}

impl Default for Names {
    fn default() -> Self {
        Self {
            filesystem: "focus-filesystem-source-v2".to_owned(),
            workspace: "focus-workspace-source-v2".to_owned(),
            registry: REGISTRY.to_owned(),
        }
    }
}

/// What a walk is run with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The limits.
    pub budgets: Budgets,
    /// `TEST_RAW_LINE_ENDINGS=1`: hash every file over its raw bytes.
    pub raw_line_endings: bool,
    /// The names that carry the engine's.
    pub names: Names,
}

impl Options {
    /// What this process's environment asks for.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            budgets: Budgets::from_env(),
            raw_line_endings: focus_workspace::env::var("TEST_RAW_LINE_ENDINGS").as_deref()
                == Some("1"),
            names: Names::default(),
        }
    }
}

/// What one root's files are (`FilesystemSourceIdentity`, without the paths
/// collected for a snapshot index).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// The hash over every line the walk recorded.
    pub fingerprint: String,
    /// The fingerprint the walk produced before some files were excluded
    /// by name, when it left any out and could reproduce that walk.
    pub legacy_fingerprint: Option<String>,
    /// Every path with what it held, keyed by the path alone.
    pub listing: Listing,
    /// The walk met a .NET output directory beside a project file.
    pub dotnet_output_seen: bool,
}

/// What a project's source is (`WorkspaceSourceWalk`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSource {
    /// The hash over the fingerprints of the project's roots.
    pub fingerprint: String,
    /// Every path of every root, keyed `<repo>\0<path>`.
    pub listing: Listing,
    /// The fingerprint as the earlier walk would have produced it.
    pub legacy: Option<String>,
    /// A root held a .NET output directory beside a project file.
    pub dotnet_output_seen: bool,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn digest_lines(label: &str, lines: &[String]) -> String {
    let mut text = label.to_owned();
    for line in lines {
        text.push('\n');
        text.push_str(line);
    }
    sha256_hex(text.as_bytes())
}

fn said(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `realpathSync`, spelled as a person reads it: Windows reports a
/// verbatim path, Node's `realpath` does not.
fn real_path(path: &Path) -> std::io::Result<String> {
    let real = fs::canonicalize(path)?;
    let text = real.to_string_lossy();
    Ok(match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned(),
    })
}

fn to_posix(native: &str) -> String {
    if Flavour::host().separator() == "/" {
        native.to_owned()
    } else {
        native.replace(Flavour::host().separator(), "/")
    }
}

/// `pathIsWithinRoot`. (The empty answer, the root itself, passes the
/// three tests after it too; upstream names it first.)
fn within_root(root: &str, target: &str) -> bool {
    let host = Flavour::host();
    let rel = jspath::relative(host, root, root, target);
    rel.is_empty()
        || (!jspath::is_absolute(host, &rel)
            && rel != ".."
            && !rel.starts_with(&format!("..{}", host.separator())))
}

#[cfg(unix)]
fn mode_of(meta: &Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode()
}

/// The mode a file reports where the system keeps none: a regular file
/// that anyone may read, as Node reports one there.
#[cfg(not(unix))]
fn mode_of(meta: &Metadata) -> u32 {
    if meta.is_dir() { 0o040_666 } else { 0o100_666 }
}

fn executable(meta: &Metadata) -> bool {
    mode_of(meta) & 0o111 != 0
}

/// Where a link leads, followed a hop at a time (`resolveSourceSymlinkTarget`).
struct Resolution {
    /// A hop left the root.
    external_hop: bool,
    /// What the chain ends at, resolved; `None` when nothing is there.
    target: Option<String>,
}

/// `resolveSourceSymlinkTarget`: `None` for a chain that loops, is longer
/// than forty hops, or cannot be read.
fn resolve_symlink_target(root_real: &str, link_path: &str) -> Option<Resolution> {
    let host = Flavour::host();
    let mut current = link_path.to_owned();
    let mut external_hop = false;
    let mut visited: HashSet<String> = HashSet::new();
    for _ in 0..40 {
        let key = jspath::resolve(host, &current, &[&current]);
        if !visited.insert(key) {
            return None;
        }
        let current_dir = real_path(Path::new(&jspath::dirname(host, &current))).ok()?;
        if !within_root(root_real, &current_dir) {
            external_hop = true;
        }
        let link_text = said(&fs::read_link(&current).ok()?);
        let next = if jspath::is_absolute(host, &link_text) {
            jspath::resolve(host, &current_dir, &[&link_text])
        } else {
            jspath::resolve(host, &current_dir, &[&current_dir, &link_text])
        };
        if !within_root(root_real, &next) {
            external_hop = true;
        }
        let Ok(stat) = fs::symlink_metadata(&next) else {
            return Some(Resolution {
                external_hop,
                target: None,
            });
        };
        if stat.file_type().is_symlink() {
            current = next;
            continue;
        }
        let Ok(target) = real_path(Path::new(&next)) else {
            return Some(Resolution {
                external_hop,
                target: None,
            });
        };
        if !within_root(root_real, &target) {
            external_hop = true;
        }
        return Some(Resolution {
            external_hop,
            target: Some(target),
        });
    }
    None
}

/// `registeredPhysicalSourcePath`: where a registered path physically is,
/// following each link on the way. `None` when a link on it loops or the
/// path cannot be read; a `path` of `None` when a link on it dangles.
fn registered_physical_path(root_real: &str, logical: &str) -> Option<(bool, Option<String>)> {
    let host = Flavour::host();
    let parts: Vec<&str> = logical.split('/').collect();
    let mut current = root_real.to_owned();
    let mut external_hop = false;
    for (index, part) in parts.iter().enumerate() {
        let candidate = jspath::join(host, &[&current, part]);
        let stat = match fs::symlink_metadata(&candidate) {
            Ok(stat) => stat,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let mut rest = vec![current.as_str()];
                rest.extend(&parts[index..]);
                let unresolved = jspath::join(host, &rest);
                return Some((
                    external_hop || !within_root(root_real, &unresolved),
                    Some(unresolved),
                ));
            }
            Err(_) => return None,
        };
        if stat.file_type().is_symlink() {
            let resolution = resolve_symlink_target(root_real, &candidate)?;
            external_hop = external_hop || resolution.external_hop;
            match resolution.target {
                None => return Some((external_hop, None)),
                Some(target) => current = target,
            }
        } else {
            current = candidate;
        }
        if !within_root(root_real, &current) {
            external_hop = true;
        }
    }
    Some((external_hop, Some(current)))
}

/// `isAidlcSensorCachePath`: a path into the engine's own directory of a
/// record, wherever in the tree a workspace is found.
fn is_sensor_cache_path(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    (0..parts.len()).any(|i| {
        i + 4 < parts.len()
            && parts[i] == WORKSPACE_DIR
            && parts[i + 1] == "spaces"
            && parts[i + 3] == "intents"
            && parts[i + 4..]
                .iter()
                .any(|part| *part == ENGINE_DIR || *part == SENSORS_DIR)
    })
}

/// `sourceFingerprintHardExcludedFile`: what an operating system, a
/// coverage run or an editor leaves beside the source.
fn hard_excluded_file(name: &str) -> bool {
    HARD_EXCLUDED_FILES.contains(&name)
        || name.starts_with(".coverage.")
        || re::test(r"(?:\.sw[m-p]|~)$", "", name)
}

/// `sourcePathPhysicallyExcluded`, and the same test over a registered
/// path as written.
fn physically_excluded(path: &str, carries_shell: bool, shell_dirs: &HashSet<String>) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    (carries_shell
        && (parts[0] == WORKSPACE_DIR
            || parts[0] == HIDDEN_WORKSPACE_DIR
            || KNOWN_HARNESS_DIRS.contains(&parts[0])
            || shell_dirs.contains(parts[0])))
        || parts.iter().any(|part| HARD_EXCLUDED_DIRS.contains(part))
        || is_sensor_cache_path(path)
}

/// `isSourceHarnessShellDir`: a dot-directory that announces a harness.
fn is_harness_shell_dir(root: &str, name: &str) -> bool {
    if !re::test(r"^\.[a-z0-9][a-z0-9._-]*$", "i", name) {
        return false;
    }
    let manifest = Path::new(root)
        .join(name)
        .join("tools")
        .join("data")
        .join("harness.json");
    let Ok(stat) = fs::symlink_metadata(&manifest) else {
        return false;
    };
    if !stat.is_file() || stat.len() > 64 * 1024 {
        return false;
    }
    let Ok(bytes) = fs::read(&manifest) else {
        return false;
    };
    let text = String::from_utf8_lossy(&bytes);
    if len_utf16(&text) > 64 * 1024 {
        return false;
    }
    json::parse(&text).is_ok_and(|parsed| {
        parsed
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| !trim(name).is_empty())
    })
}

/// `sourceHarnessShellDirs`: the harness trees at the root of a project.
fn harness_shell_dirs(root: &str, budgets: &Budgets) -> Result<HashSet<String>, Stop> {
    let mut dirs = HashSet::new();
    let listed = |error: std::io::Error| {
        failed(
            "unreadable",
            format!("the project root could not be listed: {error}"),
            ".",
        )
    };
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => return listed(error),
    };
    let mut seen: u64 = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => return listed(error),
        };
        seen += 1;
        if seen > budgets.entries {
            return failed(
                "budget-entries",
                format!(
                    "the project root lists more than {} entries",
                    budgets.entries
                ),
                ".",
            );
        }
        let name = said(Path::new(&entry.file_name()));
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(error) => return listed(error),
        };
        if (kind.is_dir() || kind.is_symlink()) && is_harness_shell_dir(root, &name) {
            dirs.insert(name);
        }
    }
    Ok(dirs)
}

/// The project's registry (`SourceFingerprintRegistry`).
struct Registry {
    /// Paths bound whatever their name or encoding, in the file's order.
    paths: Vec<String>,
    /// Paths outside the boundary with everything under them.
    excludes: Vec<String>,
}

/// A registry path as the walk names paths; `None` for anything that is
/// not a relative path inside the project.
fn normalized_registry_path(raw: &str) -> Option<String> {
    let path = raw.replace('\\', "/");
    let path = re::replace(r"^\./+", "", &path, "");
    let path = path.trim_end_matches('/');
    let outside = path.is_empty()
        || len_utf16(path) > 4096
        || path == "."
        || path.contains('\0')
        || jspath::is_absolute(Flavour::host(), path)
        || re::test("^[A-Za-z]:/", "", path)
        || path.starts_with("//")
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..");
    (!outside).then(|| path.to_owned())
}

/// `sourceFingerprintRegistry`.
fn read_registry(
    root: &str,
    name: &str,
    carries_shell: bool,
    shell_dirs: &HashSet<String>,
) -> Result<Registry, Stop> {
    let invalid = |detail: String| failed("registered-sources-invalid", detail, name);
    let path = Path::new(root).join(name);
    let stat = match fs::symlink_metadata(&path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(Registry {
                paths: Vec::new(),
                excludes: Vec::new(),
            });
        }
        Err(error) => {
            return failed(
                "unreadable",
                format!("{name} could not be read: {error}"),
                name,
            );
        }
    };
    if stat.file_type().is_symlink() || !stat.is_file() || stat.len() > 1024 * 1024 {
        return invalid(format!(
            "{name} must be a regular file of at most 1 MiB, not a symlink"
        ));
    }
    let unparsed = |error: String| invalid(format!("{name} could not be parsed: {error}"));
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => return unparsed(error.to_string()),
    };
    let parsed = match json::parse(&String::from_utf8_lossy(&bytes)) {
        Ok(parsed) => parsed,
        Err(error) => return unparsed(error.to_string()),
    };
    // Reading a property of `null` throws, and the throw is reported as a
    // file that could not be parsed.
    if parsed.is_null() {
        return unparsed("null has no properties".to_owned());
    }
    let list = |key: &str| -> Option<Vec<String>> {
        match parsed.get(key) {
            None | Some(Value::Null) => Some(Vec::new()),
            Some(Value::Array(items)) if items.len() <= 10_000 => items
                .iter()
                .map(|item| item.as_str().map(str::to_owned))
                .collect(),
            Some(_) => None,
        }
    };
    let version_is_one = parsed
        .get("version")
        .and_then(Value::as_f64)
        .is_some_and(|version| version == 1.0);
    let (Some(paths), Some(excludes), true) = (list("paths"), list("exclude"), version_is_one)
    else {
        return invalid(format!(
            "{name} must be {{\"version\":1,\"paths\":[...],\"exclude\":[...]}}, each list optional and at most 10000 strings"
        ));
    };
    let quoted = |raw: &str| json::stringify(&Value::String(raw.to_owned()));
    let mut registry = Registry {
        paths: Vec::new(),
        excludes: Vec::new(),
    };
    for raw in &excludes {
        let Some(path) = normalized_registry_path(raw) else {
            return invalid(format!(
                "excluded path {} must be a relative path inside the project without \".\" or \"..\" segments",
                quoted(raw)
            ));
        };
        if !registry.excludes.contains(&path) {
            registry.excludes.push(path);
        }
    }
    for raw in &paths {
        let Some(path) = normalized_registry_path(raw) else {
            return invalid(format!(
                "registered source path {} must be a relative path inside the project without \".\" or \"..\" segments",
                quoted(raw)
            ));
        };
        if physically_excluded(&path, carries_shell, shell_dirs) {
            return failed(
                "excluded-path",
                "a registered source path lies under the framework shell or a hard-excluded directory",
                &path,
            );
        }
        if !registry.paths.contains(&path) {
            registry.paths.push(path);
        }
    }
    Ok(registry)
}

/// What a file hashes to (`stableFileShas`).
enum Shas {
    /// The file was gone when it was opened.
    Vanished,
    /// It could not be read, or changed under the read.
    Unstable,
    /// Its hash with line endings read as LF, and over the raw bytes when
    /// that differs.
    Read { sha: String, raw: Option<String> },
}

/// The text of a file a chunk at a time, with each CRLF read as LF
/// (`committedTextParts`): whether that reading applies is known at the
/// end, and is true only for text that has a CRLF.
#[derive(Default)]
struct TextParts {
    crlf: bool,
    pending_cr: bool,
    printable: u64,
    nonprintable: i64,
    last: Option<u8>,
    not_text: bool,
}

impl TextParts {
    fn feed(&mut self, chunk: &[u8], hash: &mut Sha256) {
        if self.not_text || chunk.is_empty() {
            return;
        }
        if self.pending_cr {
            if chunk[0] != 10 {
                self.not_text = true;
                return;
            }
            self.pending_cr = false;
        }
        let mut start = 0;
        let mut at = 0;
        while at < chunk.len() {
            let byte = chunk[at];
            if byte == 13 {
                if at + 1 == chunk.len() {
                    self.pending_cr = true;
                } else if chunk[at + 1] != 10 {
                    self.not_text = true;
                    return;
                }
                self.crlf = true;
                hash.update(&chunk[start..at]);
                start = at + 1;
                at += 1;
            } else if byte == 0 {
                self.not_text = true;
                return;
            } else if byte == 127 || (byte < 32 && !matches!(byte, 10 | 8 | 9 | 27 | 12)) {
                self.nonprintable += 1;
            } else if byte != 10 {
                self.printable += 1;
            }
            at += 1;
        }
        // A carriage return that ended the chunk moved `start` past it.
        hash.update(&chunk[start.min(chunk.len())..]);
        self.last = chunk.last().copied();
    }

    fn applies(mut self) -> bool {
        if self.not_text {
            return false;
        }
        // A trailing DOS end-of-file mark is not counted against the text.
        if self.last == Some(26) {
            self.nonprintable -= 1;
        }
        self.crlf && !self.pending_cr && (self.printable >> 7) as i64 >= self.nonprintable
    }
}

/// The marks a file is known to have stood still by.
fn standing(meta: &Metadata) -> (u64, Option<std::time::SystemTime>, i64, i64) {
    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt;
        (meta.ctime(), meta.ctime_nsec())
    };
    #[cfg(not(unix))]
    let changed = (0, 0);
    (meta.len(), meta.modified().ok(), changed.0, changed.1)
}

/// `stableFileShas`.
fn stable_file_shas(path: &Path, raw_line_endings: bool) -> Shas {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Shas::Vanished,
        Err(_) => return Shas::Unstable,
    };
    let Ok(before) = file.metadata() else {
        return Shas::Unstable;
    };
    if !before.is_file() {
        return Shas::Unstable;
    }
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut read_all = |file: &mut File, each: &mut dyn FnMut(&[u8])| -> Option<u64> {
        file.seek(SeekFrom::Start(0)).ok()?;
        let mut position: u64 = 0;
        loop {
            let count = file.read(&mut buffer).ok()?;
            if count == 0 {
                return Some(position);
            }
            position += count as u64;
            each(&buffer[..count]);
        }
    };
    let unchanged = |file: &File, position: u64| {
        file.metadata()
            .is_ok_and(|after| standing(&before) == standing(&after) && position == after.len())
    };
    let mut hash = Sha256::new();
    let mut carriage_return = false;
    let Some(position) = read_all(&mut file, &mut |chunk| {
        hash.update(chunk);
        carriage_return = carriage_return || chunk.contains(&13);
    }) else {
        return Shas::Unstable;
    };
    if !unchanged(&file, position) {
        return Shas::Unstable;
    }
    let raw = sha256_hex_of(hash);
    if !carriage_return || raw_line_endings {
        return Shas::Read {
            sha: raw,
            raw: None,
        };
    }
    let mut text = Sha256::new();
    let mut parts = TextParts::default();
    let Some(position) = read_all(&mut file, &mut |chunk| parts.feed(chunk, &mut text)) else {
        return Shas::Unstable;
    };
    if !parts.applies() {
        return Shas::Read {
            sha: raw,
            raw: None,
        };
    }
    if unchanged(&file, position) {
        Shas::Read {
            sha: sha256_hex_of(text),
            raw: Some(raw),
        }
    } else {
        Shas::Unstable
    }
}

fn sha256_hex_of(hash: Sha256) -> String {
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `textLike`: the first eight kibibytes hold no NUL.
fn text_like(path: &Path, size: u64) -> bool {
    if size == 0 {
        return true;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut sample = vec![0_u8; size.min(8192) as usize];
    let mut filled = 0;
    while filled < sample.len() {
        match file.read(&mut sample[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(_) => return false,
        }
    }
    !sample[..filled].contains(&0)
}

fn entry_of(mode: &str, sha: &str) -> String {
    format!("{mode} {sha}")
}

/// One directory entry, as the listing of its directory gave it.
struct Entry {
    name: String,
    is_dir: bool,
    is_symlink: bool,
}

/// The walk of one root, with everything it counts and collects.
struct Walk<'a> {
    options: &'a Options,
    root_real: String,
    carries_shell: bool,
    excluded_top_level: &'a HashSet<String>,
    dotnet_outputs: bool,
    shell_dirs: HashSet<String>,
    registered: HashSet<String>,
    registry_excludes: Vec<String>,
    lines: Vec<String>,
    raw_lines: HashMap<usize, String>,
    legacy_inserts: Vec<(usize, String)>,
    legacy_unavailable: bool,
    legacy_files: u64,
    legacy_bytes: u64,
    dotnet_output_seen: bool,
    earlier_output_depth: u32,
    earlier_output_files: u64,
    earlier_output_bytes: u64,
    listing: Listing,
    visited: HashMap<String, bool>,
    active: HashSet<String>,
    entries_seen: u64,
    directories_seen: u64,
    symlinks_seen: u64,
    total_files: u64,
    total_bytes: u64,
    source_only_files: u64,
    source_only_bytes: u64,
}

/// How one directory is walked.
#[derive(Clone)]
struct Frame {
    rel: String,
    source_only: bool,
    registered_only: bool,
    snapshot_eligible: bool,
    registry_rel: String,
    listing_rel: String,
}

fn child_of(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

impl Walk<'_> {
    fn registered_includes(&self, rel: &str) -> bool {
        if self.registered.contains(rel) {
            return true;
        }
        rel.match_indices('/')
            .any(|(slash, _)| self.registered.contains(&rel[..slash]))
    }

    fn registered_descendant_exists(&self, rel: &str) -> bool {
        let prefix = format!("{rel}/");
        self.registered.iter().any(|path| path.starts_with(&prefix))
    }

    fn registered_relevant(&self, rel: &str) -> bool {
        self.registered_includes(rel) || self.registered_descendant_exists(rel)
    }

    fn registry_excluded(&self, rel: &str) -> bool {
        self.registry_excludes
            .iter()
            .any(|excluded| rel == excluded || rel.starts_with(&format!("{excluded}/")))
    }

    fn named_source(name: &str) -> bool {
        re::test(SOURCE_EXTENSION, "i", name) || re::test(SOURCE_BASENAME, "i", name)
    }

    /// A file only the earlier walk recorded, kept at the index it held.
    fn legacy_only_file(&mut self, path: &Path, rel: &str, executable: bool, size: u64) {
        if self.legacy_unavailable {
            return;
        }
        self.legacy_files += 1;
        self.legacy_bytes += size;
        if self.legacy_files > self.options.budgets.legacy_files
            || self.legacy_bytes > LEGACY_MAX_BYTES
        {
            self.legacy_unavailable = true;
            return;
        }
        let Shas::Read { sha, .. } = stable_file_shas(path, self.options.raw_line_endings) else {
            self.legacy_unavailable = true;
            return;
        };
        let flag = if executable { "x" } else { "-" };
        self.legacy_inserts
            .push((self.lines.len(), format!("file:{rel}:{flag}={sha}")));
    }

    /// `__pycache__` holds flat compiled files; anything else in it is not
    /// reproduced, and old evidence then compares as it always did.
    fn legacy_pycache(&mut self, dir: &Path, rel: &str) {
        if self.legacy_unavailable {
            return;
        }
        let mut names: Vec<(String, bool)> = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            self.legacy_unavailable = true;
            return;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                self.legacy_unavailable = true;
                return;
            };
            let is_file = entry.file_type().is_ok_and(|kind| kind.is_file());
            names.push((said(Path::new(&entry.file_name())), is_file));
        }
        if names.len() as u64 > self.options.budgets.legacy_files {
            self.legacy_unavailable = true;
            return;
        }
        names.sort_by(|a, b| cmp_utf16(&a.0, &b.0));
        for (name, is_file) in names {
            if !is_file {
                self.legacy_unavailable = true;
                return;
            }
            let path = dir.join(&name);
            let Ok(stat) = fs::symlink_metadata(&path) else {
                self.legacy_unavailable = true;
                return;
            };
            self.legacy_only_file(
                &path,
                &format!("{rel}/{name}"),
                executable(&stat),
                stat.len(),
            );
            if self.legacy_unavailable {
                return;
            }
        }
    }

    /// Record one file. `Ok(false)` when it was gone by the time it was
    /// read: it is left out, as the next walk would leave it.
    fn record_file(
        &mut self,
        path: &Path,
        rel: &str,
        size: u64,
        executable: bool,
        source_only: bool,
        listing_path: &str,
    ) -> Result<bool, Stop> {
        self.total_files += 1;
        self.total_bytes += size;
        if self.total_files > MAX_FILES {
            return failed("budget-files", format!("more than {MAX_FILES} files"), rel);
        }
        if self.total_bytes > MAX_BYTES {
            return failed(
                "budget-bytes",
                format!("more than {MAX_BYTES} bytes of source"),
                rel,
            );
        }
        if self.earlier_output_depth > 0 {
            self.earlier_output_files += 1;
            self.earlier_output_bytes += size;
            let files = self.earlier_output_files > self.options.budgets.legacy_files;
            if files || self.earlier_output_bytes > LEGACY_MAX_BYTES {
                return failed(
                    if files {
                        "budget-files"
                    } else {
                        "budget-bytes"
                    },
                    "the .NET build outputs are too large to compare old evidence",
                    rel,
                );
            }
        }
        if source_only {
            self.source_only_files += 1;
            self.source_only_bytes += size;
            if self.source_only_files > SOURCE_ONLY_MAX_FILES
                || self.source_only_bytes > SOURCE_ONLY_MAX_BYTES
            {
                return failed(
                    "source-only-budget",
                    format!(
                        "a symlinked external tree holds more than {SOURCE_ONLY_MAX_FILES} source files or {SOURCE_ONLY_MAX_BYTES} bytes"
                    ),
                    rel,
                );
            }
        }
        // A file still being written moves under the read; it is read again
        // a few times before it counts as unreadable.
        let mut shas = Shas::Unstable;
        for _ in 0..READ_ATTEMPTS {
            shas = stable_file_shas(path, self.options.raw_line_endings);
            if !matches!(shas, Shas::Unstable) {
                break;
            }
        }
        let (sha, raw) = match shas {
            Shas::Vanished => {
                self.total_files -= 1;
                self.total_bytes -= size;
                return Ok(false);
            }
            Shas::Unstable => return failed("unreadable", "the file could not be hashed", rel),
            Shas::Read { sha, raw } => (sha, raw),
        };
        let flag = if executable { "x" } else { "-" };
        let mode = if executable { "100755" } else { "100644" };
        self.lines.push(format!("file:{rel}:{flag}={sha}"));
        let entry = entry_of(mode, &sha);
        if let Some(raw) = raw {
            // As recorded before line endings were read as LF.
            self.raw_lines
                .insert(self.lines.len() - 1, format!("file:{rel}:{flag}={raw}"));
            raw::remember(&entry_of(mode, &raw), &entry);
        }
        self.listing.set(listing_path, entry);
        Ok(true)
    }

    fn walk(&mut self, dir: &Path, frame: &Frame) -> Result<(), Stop> {
        let here = if frame.rel.is_empty() {
            "."
        } else {
            &frame.rel
        };
        let real = match real_path(dir) {
            Ok(real) => real,
            Err(error) => {
                return failed(
                    "unreadable",
                    format!("the directory could not be resolved: {error}"),
                    here,
                );
            }
        };
        if self.active.contains(&real) {
            return Ok(());
        }
        let identity_mode = if frame.registered_only {
            format!("registered:{}", frame.registry_rel)
        } else if frame.source_only {
            "source-only".to_owned()
        } else {
            "full".to_owned()
        };
        let visit_key = format!("{real}\0{identity_mode}");
        let prior = self.visited.get(&visit_key).copied();
        if prior == Some(true) || (prior == Some(false) && !frame.snapshot_eligible) {
            return Ok(());
        }
        let record_identity = prior.is_none();
        self.visited
            .insert(visit_key, prior.unwrap_or(false) || frame.snapshot_eligible);
        self.active.insert(real.clone());
        let walked = self.walk_entries(dir, frame, here, record_identity);
        self.active.remove(&real);
        walked
    }

    fn list(&mut self, dir: &Path, here: &str) -> Result<Vec<Entry>, Stop> {
        let listed = |error: std::io::Error| {
            failed(
                "unreadable",
                format!("the directory could not be listed: {error}"),
                here,
            )
        };
        let read = match fs::read_dir(dir) {
            Ok(read) => read,
            Err(error) => return listed(error),
        };
        let mut entries = Vec::new();
        for entry in read {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => return listed(error),
            };
            self.entries_seen += 1;
            if self.entries_seen > self.options.budgets.entries {
                return failed(
                    "budget-entries",
                    format!(
                        "more than {} directory entries",
                        self.options.budgets.entries
                    ),
                    here,
                );
            }
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(error) => return listed(error),
            };
            entries.push(Entry {
                name: said(Path::new(&entry.file_name())),
                is_dir: kind.is_dir(),
                is_symlink: kind.is_symlink(),
            });
        }
        entries.sort_by(|a, b| cmp_utf16(&a.name, &b.name));
        Ok(entries)
    }

    #[allow(clippy::too_many_lines)]
    fn walk_entries(
        &mut self,
        dir: &Path,
        frame: &Frame,
        here: &str,
        record_identity: bool,
    ) -> Result<(), Stop> {
        self.directories_seen += 1;
        if self.directories_seen > self.options.budgets.directories {
            return failed(
                "budget-directories",
                format!("more than {} directories", self.options.budgets.directories),
                here,
            );
        }
        let entries = self.list(dir, here)?;
        let dotnet_project = entries
            .iter()
            .any(|entry| re::test(r"\.(?:cs|fs|vb)proj$", "i", &entry.name));
        let dependency_dirs: HashSet<&str> = MANIFEST_DEPENDENCY_DIRS
            .iter()
            .filter(|(manifest, _)| entries.iter().any(|entry| entry.name == *manifest))
            .flat_map(|(_, dirs)| dirs.iter().copied())
            .collect();
        for entry in &entries {
            let name = entry.name.as_str();
            let child_rel = child_of(&frame.rel, name);
            let child_registry_rel = child_of(&frame.registry_rel, name);
            let child_listing_rel = child_of(&frame.listing_rel, name);
            let container = entry.is_dir || entry.is_symlink;
            if entry.is_symlink {
                self.symlinks_seen += 1;
                if self.symlinks_seen > self.options.budgets.symlinks {
                    return failed(
                        "budget-symlinks",
                        format!("more than {} symlinks", self.options.budgets.symlinks),
                        &child_rel,
                    );
                }
            }
            if frame.registered_only && !self.registered_relevant(&child_registry_rel) {
                continue;
            }
            // Excluded by the registry: skipped, or walked only for the
            // registered path beneath it, like a conditional directory.
            let registry_excluded = self.registry_excluded(&child_registry_rel)
                && !self.registered_includes(&child_registry_rel);
            if registry_excluded && !self.registered_descendant_exists(&child_registry_rel) {
                continue;
            }
            if frame.rel.is_empty()
                && self.excluded_top_level.contains(name)
                && !self.registered_relevant(&child_registry_rel)
            {
                continue;
            }
            if name == ".git" {
                continue;
            }
            if container && HARD_EXCLUDED_DIRS.contains(&name) {
                if name == PYCACHE_DIR
                    && record_identity
                    && !frame.source_only
                    && !frame.registered_only
                {
                    if entry.is_symlink {
                        self.legacy_unavailable = true;
                    } else {
                        self.legacy_pycache(&dir.join(name), &child_rel);
                    }
                }
                continue;
            }
            if frame.rel.is_empty()
                && self.carries_shell
                && container
                && (name == WORKSPACE_DIR
                    || name == HIDDEN_WORKSPACE_DIR
                    || self.shell_dirs.contains(name))
            {
                continue;
            }
            if container && is_sensor_cache_path(&child_rel) {
                continue;
            }
            let dotnet_output = dotnet_project && container && DOTNET_OUTPUT_NAMES.contains(&name);
            self.dotnet_output_seen = self.dotnet_output_seen || dotnet_output;
            let earlier_output = dotnet_output && !self.dotnet_outputs;
            let conditional_boundary = container
                && (CONDITIONAL_DIRS.contains(&name)
                    || (dotnet_output && self.dotnet_outputs)
                    || dependency_dirs.contains(name));
            if conditional_boundary && !self.registered_relevant(&child_registry_rel) {
                continue;
            }
            let child_registered_only =
                frame.registered_only || conditional_boundary || registry_excluded;
            let child = dir.join(name);
            let stat = match fs::symlink_metadata(&child) {
                Ok(stat) => stat,
                // Gone since the directory listed it (an editor's temporary
                // file, say): left out, as the next walk would leave it.
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => {
                    return failed(
                        "unreadable",
                        format!("the entry could not be stat'ed: {error}"),
                        &child_rel,
                    );
                }
            };
            if stat.file_type().is_symlink() {
                let link_text = match fs::read_link(&child) {
                    Ok(text) => said(&text),
                    Err(error) => {
                        return failed(
                            "unreadable",
                            format!("the symlink could not be read: {error}"),
                            &child_rel,
                        );
                    }
                };
                if record_identity {
                    self.lines.push(format!("link:{child_rel}={link_text}"));
                    self.listing.set(
                        child_listing_rel.as_str(),
                        entry_of("120000", &sha256_hex(link_text.as_bytes())),
                    );
                }
                let Some(resolution) = resolve_symlink_target(&self.root_real, &said(&child))
                else {
                    return failed(
                        "symlink-loop",
                        "the symlink loops or its chain cannot be read",
                        &child_rel,
                    );
                };
                let Some(target) = resolution.target else {
                    if record_identity {
                        self.lines.push(format!("link-target:{child_rel}=missing"));
                        self.listing.set(
                            format!("{child_listing_rel}@target"),
                            entry_of("000000", &sha256_hex(b"missing")),
                        );
                    }
                    continue;
                };
                let internal = within_root(&self.root_real, &target);
                let target_rel = to_posix(&jspath::relative(
                    Flavour::host(),
                    &self.root_real,
                    &self.root_real,
                    &target,
                ));
                let target_path = PathBuf::from(&target);
                let target_stat = match fs::metadata(&target_path) {
                    Ok(stat) => stat,
                    Err(error) => {
                        return failed(
                            "unreadable",
                            format!("the symlink target could not be stat'ed: {error}"),
                            &format!("{child_rel}@target"),
                        );
                    }
                };
                if target_stat.is_file() {
                    if child_registered_only && !self.registered_includes(&child_registry_rel) {
                        continue;
                    }
                    if frame.source_only
                        && !child_registered_only
                        && !Self::named_source(name)
                        && !text_like(&target_path, target_stat.len())
                    {
                        continue;
                    }
                    let target_listing_rel = if internal {
                        target_rel
                    } else {
                        format!("{child_listing_rel}@target")
                    };
                    if record_identity {
                        self.record_file(
                            &target_path,
                            &format!("{child_rel}@target"),
                            target_stat.len(),
                            executable(&target_stat),
                            frame.source_only,
                            &target_listing_rel,
                        )?;
                    }
                } else if target_stat.is_dir() {
                    let below = Frame {
                        rel: format!("{child_rel}@target"),
                        source_only: frame.source_only || !internal,
                        registered_only: child_registered_only,
                        snapshot_eligible: frame.snapshot_eligible
                            && internal
                            && !target_path.join(".git").exists(),
                        registry_rel: child_registry_rel,
                        listing_rel: if internal {
                            target_rel
                        } else {
                            format!("{child_listing_rel}@target")
                        },
                    };
                    self.earlier_output_depth += u32::from(earlier_output);
                    self.walk(&target_path, &below)?;
                    self.earlier_output_depth -= u32::from(earlier_output);
                } else if record_identity {
                    let special = format!("special:{}", mode_of(&target_stat));
                    self.lines
                        .push(format!("link-target:{child_rel}={special}"));
                    self.listing.set(
                        format!("{child_listing_rel}@target"),
                        entry_of("000000", &sha256_hex(special.as_bytes())),
                    );
                }
                continue;
            }
            if stat.is_dir() {
                if conditional_boundary || registry_excluded {
                    let below = Frame {
                        rel: child_rel,
                        source_only: false,
                        registered_only: true,
                        snapshot_eligible: frame.snapshot_eligible,
                        registry_rel: child_registry_rel,
                        listing_rel: child_listing_rel,
                    };
                    self.walk(&child, &below)?;
                    continue;
                }
                let nested_git_repo = child.join(".git").exists();
                if nested_git_repo && frame.snapshot_eligible {
                    // Listed as a gitlink to the commit its `HEAD` names.
                    return Err(Stop::NotPorted(
                        "a git repository embedded in the source tree",
                    ));
                }
                let below = Frame {
                    rel: child_rel,
                    source_only: frame.source_only,
                    registered_only: frame.registered_only,
                    snapshot_eligible: frame.snapshot_eligible && !nested_git_repo,
                    registry_rel: child_registry_rel,
                    listing_rel: child_listing_rel,
                };
                self.earlier_output_depth += u32::from(earlier_output);
                self.walk(&child, &below)?;
                self.earlier_output_depth -= u32::from(earlier_output);
                continue;
            }
            if stat.is_file() {
                let excluded_by_name = (hard_excluded_file(name)
                    || (self.carries_shell
                        && (child_rel == SETTINGS_FILE || child_rel == LOCAL_SETTINGS_FILE)))
                    && !self.registered_includes(&child_registry_rel);
                if frame.source_only
                    && !child_registered_only
                    && !Self::named_source(name)
                    && !text_like(&child, stat.len())
                {
                    continue;
                }
                if child_registered_only && !self.registered_includes(&child_registry_rel) {
                    continue;
                }
                if excluded_by_name {
                    if record_identity {
                        self.legacy_only_file(&child, &child_rel, executable(&stat), stat.len());
                    }
                    continue;
                }
                if record_identity {
                    self.record_file(
                        &child,
                        &child_rel,
                        stat.len(),
                        executable(&stat),
                        frame.source_only,
                        &child_listing_rel,
                    )?;
                }
                continue;
            }
            if record_identity {
                let special = format!("special:{}", mode_of(&stat));
                self.lines.push(format!("{special}:{child_rel}"));
                self.listing.set(
                    child_listing_rel,
                    entry_of("000000", &sha256_hex(special.as_bytes())),
                );
            }
        }
        Ok(())
    }
}

/// `filesystemSourceIdentity`: what the files under `root` are.
///
/// `carries_shell` says the root is a project's own (the workspace, its
/// hidden directory, the harness trees and the engine's settings files are
/// then the framework's, not source); `excluded_top_level` names the
/// entries of the root that belong to someone else (the member
/// repositories under a roof); `dotnet_outputs` false walks .NET output
/// directories as the walk did before they left the boundary.
///
/// # Errors
///
/// [`Stop::Failed`] with upstream's reason when the source cannot be
/// bound; [`Stop::NotPorted`] for a tree this port does not read yet.
pub fn identity(
    root: &Path,
    carries_shell: bool,
    excluded_top_level: &HashSet<String>,
    dotnet_outputs: bool,
    options: &Options,
) -> Result<Identity, Stop> {
    let root_real = match real_path(root) {
        Ok(real) => real,
        Err(error) => {
            return failed(
                "unreadable",
                format!("the workspace root could not be resolved: {error}"),
                ".",
            );
        }
    };
    if Path::new(&root_real).join(WORKTREE_META).exists() {
        return Err(Stop::NotPorted("a delegated worktree's source boundary"));
    }
    let shell_dirs = if carries_shell {
        harness_shell_dirs(&root_real, &options.budgets)?
    } else {
        HashSet::new()
    };
    let registry = read_registry(
        &root_real,
        &options.names.registry,
        carries_shell,
        &shell_dirs,
    )?;
    let mut registered: HashSet<String> = registry.paths.iter().cloned().collect();
    for logical in &registry.paths {
        let Some((external_hop, physical)) = registered_physical_path(&root_real, logical) else {
            return failed(
                "symlink-loop",
                "a registered source path could not be resolved: a symlink in it loops or cannot be read",
                logical,
            );
        };
        let Some(physical) = physical else {
            return failed(
                "dangling-symlink",
                "a registered source path resolves through a symlink whose target is missing",
                logical,
            );
        };
        let inside = within_root(&root_real, &physical);
        if external_hop && inside {
            return failed(
                "external-symlink",
                "a registered source path leaves the project through a symlink and comes back inside it",
                logical,
            );
        }
        if !inside {
            continue;
        }
        let physical_rel = to_posix(&jspath::relative(
            Flavour::host(),
            &root_real,
            &root_real,
            &physical,
        ));
        if !physical_rel.is_empty() {
            if physically_excluded(&physical_rel, carries_shell, &shell_dirs) {
                return failed(
                    "excluded-path",
                    "a registered source path resolves into the framework shell or a hard-excluded directory",
                    &physical_rel,
                );
            }
            registered.insert(physical_rel);
        }
    }
    let mut walk = Walk {
        options,
        root_real: root_real.clone(),
        carries_shell,
        excluded_top_level,
        dotnet_outputs,
        shell_dirs,
        registered,
        registry_excludes: registry.excludes,
        lines: Vec::new(),
        raw_lines: HashMap::new(),
        legacy_inserts: Vec::new(),
        legacy_unavailable: false,
        legacy_files: 0,
        legacy_bytes: 0,
        dotnet_output_seen: false,
        earlier_output_depth: 0,
        earlier_output_files: 0,
        earlier_output_bytes: 0,
        listing: Listing::new(),
        visited: HashMap::new(),
        active: HashSet::new(),
        entries_seen: 0,
        directories_seen: 0,
        symlinks_seen: 0,
        total_files: 0,
        total_bytes: 0,
        source_only_files: 0,
        source_only_bytes: 0,
    };
    let top = Frame {
        rel: String::new(),
        source_only: false,
        registered_only: false,
        snapshot_eligible: true,
        registry_rel: String::new(),
        listing_rel: String::new(),
    };
    walk.walk(Path::new(&root_real), &top)?;
    let label = &options.names.filesystem;
    let fingerprint = digest_lines(label, &walk.lines);
    if !walk.raw_lines.is_empty() {
        let raw: Vec<String> = walk
            .lines
            .iter()
            .enumerate()
            .map(|(at, line)| walk.raw_lines.get(&at).unwrap_or(line).clone())
            .collect();
        raw::remember(&digest_lines(label, &raw), &fingerprint);
    }
    // Today's lines with each legacy-only line put back at the index it held.
    let legacy_fingerprint =
        (!walk.legacy_inserts.is_empty() && !walk.legacy_unavailable).then(|| {
            let mut legacy: Vec<String> = Vec::new();
            let mut next = 0;
            for index in 0..=walk.lines.len() {
                while next < walk.legacy_inserts.len() && walk.legacy_inserts[next].0 == index {
                    legacy.push(walk.legacy_inserts[next].1.clone());
                    next += 1;
                }
                if index < walk.lines.len() {
                    legacy.push(walk.lines[index].clone());
                }
            }
            digest_lines(label, &legacy)
        });
    Ok(Identity {
        fingerprint,
        legacy_fingerprint,
        listing: walk.listing,
        dotnet_output_seen: walk.dotnet_output_seen,
    })
}

fn prefixed(listing: &Listing, repo: &str) -> impl Iterator<Item = (String, String)> {
    listing
        .iter()
        .map(move |(path, entry)| (format!("{repo}\0{path}"), entry.to_owned()))
}

/// `multiRepoRoofExcludedTopLevel`: under a roof, every directory at the
/// top is someone else's: a member repository, or not source of the roof.
fn roof_excluded(
    project: &Path,
    repos: &[String],
    budgets: &Budgets,
) -> Result<HashSet<String>, Stop> {
    let mut excluded: HashSet<String> = repos.iter().cloned().collect();
    let listed = |error: std::io::Error| {
        failed(
            "unreadable",
            format!("the multi-repo roof could not be listed: {error}"),
            ".",
        )
    };
    let entries = match fs::read_dir(project) {
        Ok(entries) => entries,
        Err(error) => return listed(error),
    };
    let mut seen: u64 = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => return listed(error),
        };
        seen += 1;
        if seen > budgets.entries {
            return failed(
                "budget-entries",
                format!(
                    "the multi-repo roof lists more than {} entries",
                    budgets.entries
                ),
                ".",
            );
        }
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(error) => return listed(error),
        };
        let name = said(Path::new(&entry.file_name()));
        // A broken link at the roof is fingerprinted as a file of the roof.
        if kind.is_dir()
            || (kind.is_symlink() && fs::metadata(entry.path()).is_ok_and(|m| m.is_dir()))
        {
            excluded.insert(name);
        }
    }
    Ok(excluded)
}

/// `walkWorkspaceSource`: what a project's source is. `repos` names the
/// member repositories of a project that is a roof over several; empty for
/// a project that is one.
///
/// # Errors
///
/// As [`identity`]; a failure inside a member repository names it.
pub fn workspace_source(
    project: &Path,
    repos: &[String],
    dotnet_outputs: bool,
    options: &Options,
) -> Result<WorkspaceSource, Stop> {
    let label = &options.names.workspace;
    let nobody = HashSet::new();
    if repos.is_empty() {
        if project.join(WORKTREE_META).exists() {
            return Err(Stop::NotPorted("a delegated worktree's source boundary"));
        }
        let source = identity(project, true, &nobody, dotnet_outputs, options)?;
        let digest = |filesystem: &str| digest_lines(label, &[format!("filesystem={filesystem}")]);
        let fingerprint = digest(&source.fingerprint);
        if let Some(raw_source) = raw::raw_form(&source.fingerprint) {
            raw::remember(&digest(&raw_source), &fingerprint);
        }
        return Ok(WorkspaceSource {
            fingerprint,
            listing: prefixed(&source.listing, "").collect(),
            legacy: source.legacy_fingerprint.as_deref().map(digest),
            dotnet_output_seen: source.dotnet_output_seen,
        });
    }
    let mut lines: Vec<String> = Vec::new();
    let mut legacy_lines: Vec<String> = Vec::new();
    let mut legacy_differs = false;
    let mut listing = Listing::new();
    let excluded = roof_excluded(project, repos, &options.budgets)?;
    let roof = identity(project, true, &excluded, dotnet_outputs, options)?;
    let mut dotnet_output_seen = roof.dotnet_output_seen;
    lines.push(format!("roof=filesystem:{}", roof.fingerprint));
    legacy_lines.push(format!(
        "roof=filesystem:{}",
        roof.legacy_fingerprint
            .as_ref()
            .unwrap_or(&roof.fingerprint)
    ));
    legacy_differs = legacy_differs || roof.legacy_fingerprint.is_some();
    for (key, entry) in prefixed(&roof.listing, "") {
        listing.set(key, entry);
    }
    let mut sorted: Vec<&String> = repos.iter().collect();
    sorted.sort_by(|a, b| cmp_utf16(a, b));
    for name in sorted {
        // A sibling repo is a child of the roof; the shell is its sibling,
        // never nested inside it, so nothing there belongs to the framework.
        let dir = project.join(name);
        if !dir.exists() {
            lines.push(format!("{name}=missing"));
            legacy_lines.push(format!("{name}=missing"));
            continue;
        }
        let source =
            identity(&dir, false, &nobody, dotnet_outputs, options).map_err(|stop| match stop {
                Stop::Failed(failure) => Stop::Failed(Failure {
                    repo: Some(name.clone()),
                    ..failure
                }),
                other => other,
            })?;
        dotnet_output_seen = dotnet_output_seen || source.dotnet_output_seen;
        lines.push(format!("{name}=filesystem:{}", source.fingerprint));
        legacy_lines.push(format!(
            "{name}=filesystem:{}",
            source
                .legacy_fingerprint
                .as_ref()
                .unwrap_or(&source.fingerprint)
        ));
        legacy_differs = legacy_differs || source.legacy_fingerprint.is_some();
        for (key, entry) in prefixed(&source.listing, name) {
            listing.set(key, entry);
        }
    }
    let fingerprint = digest_lines(label, &lines);
    // As recorded before line endings were read as LF.
    let raw_parts: Vec<String> = lines
        .iter()
        .map(|line| {
            line.rsplit_once("=filesystem:")
                .and_then(|(head, hex)| {
                    raw::raw_form(hex).map(|raw| format!("{head}=filesystem:{raw}"))
                })
                .unwrap_or_else(|| line.clone())
        })
        .collect();
    if raw_parts != lines {
        raw::remember(&digest_lines(label, &raw_parts), &fingerprint);
    }
    Ok(WorkspaceSource {
        fingerprint,
        listing,
        legacy: legacy_differs.then(|| digest_lines(label, &legacy_lines)),
        dotnet_output_seen,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("source-walk-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(&dir).unwrap()
    }

    fn options() -> Options {
        Options {
            budgets: Budgets::from(|_| None),
            raw_line_endings: false,
            names: Names::default(),
        }
    }

    /// Deliberate deviation: the labels a fingerprint is taken under and
    /// the registry's file name carry this engine's name, so a fingerprint
    /// of Focus's is never one of upstream's for the same tree. The corpus
    /// takes every fingerprint under the names the case gives it; this is
    /// the one taken under Focus's own, worked out by hand.
    #[test]
    fn a_fingerprint_is_taken_under_this_engines_names() {
        let root = scratch("names");
        fs::write(root.join("a.ts"), "x\n").unwrap();
        fs::write(
            root.join(REGISTRY),
            "{\"version\":1,\"exclude\":[\"a.ts\"]}",
        )
        .unwrap();
        let source = workspace_source(&root, &[], true, &options()).unwrap();
        let registry = sha256_hex(b"{\"version\":1,\"exclude\":[\"a.ts\"]}");
        let filesystem = sha256_hex(
            format!("focus-filesystem-source-v2\nfile:.focus-source-paths.json:-={registry}")
                .as_bytes(),
        );
        assert_eq!(
            source.fingerprint,
            sha256_hex(format!("focus-workspace-source-v2\nfilesystem={filesystem}").as_bytes())
        );
        let rows: Vec<_> = source.listing.iter().collect();
        assert_eq!(
            rows,
            [(
                "\0.focus-source-paths.json",
                format!("100644 {registry}").as_str()
            )]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_budget_is_a_positive_integer_without_a_leading_zero() {
        let asked = |value: &'static str| Budgets::from(move |_| Some(value.to_owned())).entries;
        assert_eq!(asked("7"), 7);
        for not_one in ["0", "07", "7.0", " 7", "-7", "seven", ""] {
            assert_eq!(asked(not_one), 250_000, "{not_one:?}");
        }
        assert_eq!(asked("99999999999999999999999"), u64::MAX);
        let defaults = Budgets::from(|_| None);
        assert_eq!(
            (
                defaults.entries,
                defaults.directories,
                defaults.symlinks,
                defaults.legacy_files
            ),
            (250_000, 100_000, 100_000, 10_000)
        );
    }

    /// What is not read yet is refused by name, never walked as if it were
    /// something else.
    #[test]
    fn what_is_not_ported_is_refused_by_name() {
        let root = scratch("embedded");
        fs::create_dir_all(root.join("vendor/lib/.git")).unwrap();
        fs::write(root.join("vendor/lib/x.c"), "x\n").unwrap();
        assert_eq!(
            workspace_source(&root, &[], true, &options()),
            Err(Stop::NotPorted(
                "a git repository embedded in the source tree"
            ))
        );
        let _ = fs::remove_dir_all(&root);
        let root = scratch("worktree");
        fs::create_dir_all(root.join(".focus")).unwrap();
        fs::write(root.join(WORKTREE_META), "{}").unwrap();
        assert_eq!(
            workspace_source(&root, &[], true, &options()),
            Err(Stop::NotPorted("a delegated worktree's source boundary"))
        );
        assert_eq!(
            workspace_source(&root, &["api".to_owned()], true, &options()),
            Err(Stop::NotPorted("a delegated worktree's source boundary"))
        );
        let _ = fs::remove_dir_all(root);
    }
}

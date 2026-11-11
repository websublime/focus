//! Which project, which harness, and where each file is.

use std::path::{Path, PathBuf};

use focus_harness::Harness;
use focus_js::text::trim;
use focus_js::{EngineError, Result, bail, re};

use crate::env;
use crate::manifest::{MANIFEST_REL, announced_name};
use crate::names::{
    ACTIVE_SPACE_POINTER, CLI, DEFAULT_SPACE, ENGINE_DIR, LOCK_FILE, LOCK_OWNER_FILE, WORKSPACE_DIR,
};

/// The project a command acts on.
///
/// In order: an explicit `--project-dir`; `FOCUS_PROJECT_DIR`; the directory
/// the host names (`CLAUDE_PROJECT_DIR`); the nearest directory at or above
/// the current one that has a harness installed; the current directory.
///
/// Upstream's fourth step reads the project off the running script's own
/// path, which works because its tools are copied into each project. Focus is
/// installed once, outside any project, so the equivalent is to look upwards
/// from where the command was run, as `git` and `cargo` do.
///
/// # Errors
///
/// The current directory cannot be read.
pub fn resolve_project_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    let cwd = std::env::current_dir()
        .map_err(|err| EngineError::new(format!("cannot read the current directory: {err}")))?;
    let named = explicit
        .map(Path::to_path_buf)
        .or_else(|| env::path("PROJECT_DIR").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("CLAUDE_PROJECT_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        });
    if let Some(dir) = named {
        // Settled by reading the path, as every path upstream joins onto its
        // project is: `a/link/..` is `a`, wherever the link leads.
        return Ok(settled(&cwd.join(dir)));
    }
    Ok(cwd
        .ancestors()
        .find(|dir| !installed_harnesses(dir).is_empty())
        .unwrap_or(&cwd)
        .to_path_buf())
}

/// `path.resolve`: `path` made absolute, with `.` and `..` settled by reading
/// the path, not the disk.
#[must_use]
pub fn settled(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut settled = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::ParentDir => {
                settled.pop();
            }
            std::path::Component::CurDir => {}
            other => settled.push(other),
        }
    }
    settled
}

/// `normalizeDriveLetter`: a lower-case drive letter upper-cased, so two
/// spellings of one Windows path compare equal.
#[must_use]
pub fn normalize_drive_letter(path: &str) -> String {
    let mut chars = path.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(drive), Some(':'), None | Some('\\' | '/')) if drive.is_ascii_lowercase() => {
            format!("{}{}", drive.to_ascii_uppercase(), &path[1..])
        }
        _ => path.to_owned(),
    }
}

/// `path.join(base, ...parts)`: the parts put after `base` one by one and
/// the whole settled by reading it. Unlike [`Path::join`], a part that looks
/// absolute does not start over: it goes under what came before, as in
/// upstream's `join`.
#[must_use]
pub fn joined(base: &Path, parts: &[&str]) -> PathBuf {
    let mut path = base.to_path_buf();
    for part in parts {
        for component in Path::new(part).components() {
            match component {
                std::path::Component::Normal(name) => path.push(name),
                std::path::Component::ParentDir => path.push(".."),
                _ => {}
            }
        }
    }
    settled(&path)
}

/// The harnesses installed in `project`: each supported harness whose
/// directory holds a manifest announcing that harness.
#[must_use]
pub fn installed_harnesses(project: &Path) -> Vec<Harness> {
    Harness::ALL
        .into_iter()
        .filter(|harness| {
            let manifest = MANIFEST_REL
                .iter()
                .fold(project.join(harness.dir()), |path, part| path.join(part));
            announced_name(&manifest).as_deref() == Some(harness.name())
        })
        .collect()
}

/// The harness a command acts for.
///
/// In order: an explicit `--harness`; `FOCUS_HARNESS`; the one harness
/// installed in the project. The commands Focus installs into a harness pass
/// `--harness` themselves, so discovery only serves a person at a terminal.
///
/// # Errors
///
/// The name is not a supported harness, no harness is installed, or several
/// are and nothing says which one is meant. A project can hold several
/// harnesses, each with its own compiled data, so guessing would read or
/// write the wrong tree.
pub fn resolve_harness(project: &Path, explicit: Option<&str>) -> Result<Harness> {
    let named = explicit.map(str::to_owned).or_else(|| env::var("HARNESS"));
    if let Some(name) = named.filter(|name| !name.is_empty()) {
        return Harness::from_name(&name).ok_or_else(|| {
            let known: Vec<&str> = Harness::ALL.iter().map(|h| h.name()).collect();
            EngineError::new(format!(
                "Unknown harness: {name}. Valid: {}",
                known.join(", ")
            ))
        });
    }
    match installed_harnesses(project).as_slice() {
        [only] => Ok(*only),
        [] => bail!(
            "No harness is installed in {}. Run {CLI} from inside a project, or pass --project-dir <dir>.",
            project.display()
        ),
        several => {
            let names: Vec<&str> = several.iter().map(|h| h.name()).collect();
            bail!(
                "{} has several harnesses installed ({}). Pass --harness <name>.",
                project.display(),
                names.join(", ")
            )
        }
    }
}

/// Every path the engine reads or writes, for one harness in one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub project: PathBuf,
    pub harness: Harness,
}

impl Layout {
    #[must_use]
    pub fn new(project: impl Into<PathBuf>, harness: Harness) -> Self {
        Self {
            project: project.into(),
            harness,
        }
    }

    /// The harness tree: `<project>/.claude` and the like.
    #[must_use]
    pub fn harness_root(&self) -> PathBuf {
        self.project.join(self.harness.dir())
    }

    /// The compiled and shipped data the engine reads at runtime.
    #[must_use]
    pub fn data_dir(&self) -> PathBuf {
        self.harness_root().join("tools").join("data")
    }

    #[must_use]
    pub fn manifest(&self) -> PathBuf {
        self.data_dir().join("harness.json")
    }

    #[must_use]
    pub fn stage_graph(&self) -> PathBuf {
        self.data_dir().join("stage-graph.json")
    }

    #[must_use]
    pub fn scope_grid(&self) -> PathBuf {
        self.data_dir().join("scope-grid.json")
    }

    #[must_use]
    pub fn ars_priors(&self) -> PathBuf {
        self.data_dir().join("ars-priors.json")
    }

    /// One directory per phase, one `<slug>.md` per stage.
    #[must_use]
    pub fn stages_dir(&self) -> PathBuf {
        self.harness_root().join("focus-common").join("stages")
    }

    #[must_use]
    pub fn agents_dir(&self) -> PathBuf {
        self.harness_root().join("agents")
    }

    #[must_use]
    pub fn sensors_dir(&self) -> PathBuf {
        self.harness_root().join("sensors")
    }

    /// Scope identity files, one per scope.
    #[must_use]
    pub fn scopes_dir(&self) -> PathBuf {
        self.harness_root().join("scopes")
    }

    /// The team-owned workspace: `<project>/focus`.
    #[must_use]
    pub fn workspace_root(&self) -> PathBuf {
        self.project.join(WORKSPACE_DIR)
    }

    /// The durable, harness-neutral records of composed scopes.
    #[must_use]
    pub fn composed_scopes_dir(&self) -> PathBuf {
        self.workspace_root().join("scopes")
    }

    #[must_use]
    pub fn space_dir(&self, space: &str) -> PathBuf {
        self.workspace_root().join("spaces").join(space)
    }

    /// A space's method tree: `org.md`, `team.md`, `project.md`, `phases/`.
    #[must_use]
    pub fn memory_dir(&self, space: &str) -> PathBuf {
        self.space_dir(space).join("memory")
    }

    /// A space's intent registry and record root.
    #[must_use]
    pub fn intents_dir(&self, space: &str) -> PathBuf {
        self.space_dir(space).join("intents")
    }

    /// Where the composer agent writes its grid proposal and `validate-grid`
    /// reads it when no path is passed. It sits in the space's engine
    /// directory because a composition can run before any intent exists.
    #[must_use]
    pub fn composer_proposal(&self, space: &str) -> PathBuf {
        self.intents_dir(space)
            .join(ENGINE_DIR)
            .join("composer-proposal.json")
    }

    /// The space the `focus/active-space` cursor names: its text, when that is
    /// a space name whose directory exists; otherwise the default space. Never
    /// fails, because the default space is valid before anything is on disk.
    #[must_use]
    pub fn active_space(&self) -> String {
        let cursor = self.workspace_root().join(ACTIVE_SPACE_POINTER);
        let text = std::fs::read(cursor)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        let name = trim(&text);
        if is_space_name(name) && self.space_dir(name).is_dir() {
            name.to_owned()
        } else {
            DEFAULT_SPACE.to_owned()
        }
    }

    /// The workspace lock file. One per project, shared by every harness
    /// installed in it, because they share the workspace.
    #[must_use]
    pub fn lock_file(&self) -> PathBuf {
        self.workspace_root().join(LOCK_FILE)
    }

    #[must_use]
    pub fn lock_owner_file(&self) -> PathBuf {
        self.workspace_root().join(LOCK_OWNER_FILE)
    }
}

/// A space name: one plain path segment, the shape `space create` produces.
#[must_use]
pub fn is_space_name(name: &str) -> bool {
    re::test(r"^[a-z][a-z0-9-]*$", "", name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lower_case_drive_letter_is_upper_cased() {
        for (written, normal) in [
            (r"c:\x", r"C:\x"),
            ("c:/x", "C:/x"),
            ("c:", "C:"),
            // Not a drive: no separator after the colon, already upper
            // case, not at the start, not a letter.
            ("c:x", "c:x"),
            (r"C:\x", r"C:\x"),
            ("/c:/x", "/c:/x"),
            (r"é:\x", r"é:\x"),
            ("", ""),
        ] {
            assert_eq!(normalize_drive_letter(written), normal, "{written}");
        }
    }

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("focus-ws-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // The paths are a Unix machine's.
    #[cfg(unix)]
    #[test]
    fn a_path_is_settled_by_reading_it() {
        let settled = |path: &str| settled(Path::new(path));
        assert_eq!(settled("/a/b/../c/./d/"), Path::new("/a/c/d"));
        assert_eq!(settled("/a/../../b"), Path::new("/b"));
        assert_eq!(settled("/"), Path::new("/"));
        // A relative path is under the current directory.
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(settled("x/../y"), cwd.join("y"));
        assert_eq!(settled("."), cwd);
        // The disk is not asked: a link on the way is a name like any other.
        let tree = Tree::new("settled");
        std::fs::create_dir_all(tree.0.join("real/inner")).unwrap();
        std::os::unix::fs::symlink(tree.0.join("real/inner"), tree.0.join("link")).unwrap();
        assert_eq!(super::settled(&tree.0.join("link/..")), tree.0);

        let joined = |parts: &[&str]| joined(Path::new("/base/dir"), parts);
        assert_eq!(joined(&["a", "b"]), Path::new("/base/dir/a/b"));
        assert_eq!(joined(&["/abs", "b"]), Path::new("/base/dir/abs/b"));
        assert_eq!(joined(&["a", "..", "b"]), Path::new("/base/dir/b"));
        assert_eq!(joined(&["..", "..", "..", "x"]), Path::new("/x"));
        assert_eq!(joined(&["a/../b", "./c"]), Path::new("/base/dir/b/c"));
        assert_eq!(joined(&[]), Path::new("/base/dir"));
    }

    #[test]
    fn a_harness_is_installed_only_where_its_manifest_says_so() {
        let tree = Tree::new("installed");
        // A .claude directory without a manifest is somebody's settings.
        tree.write(".claude/settings.json", "{}");
        assert_eq!(installed_harnesses(&tree.0), []);
        tree.write(".claude/tools/data/harness.json", r#"{"name":"claude"}"#);
        // Copilot's tree is .focus; a manifest naming another harness there
        // does not count.
        tree.write(".focus/tools/data/harness.json", r#"{"name":"claude"}"#);
        assert_eq!(installed_harnesses(&tree.0), [Harness::Claude]);
        tree.write(".focus/tools/data/harness.json", r#"{"name":"copilot"}"#);
        assert_eq!(
            installed_harnesses(&tree.0),
            [Harness::Claude, Harness::Copilot]
        );
    }

    #[test]
    fn harness_resolution_refuses_to_guess() {
        let tree = Tree::new("resolve");
        assert!(
            resolve_harness(&tree.0, None)
                .unwrap_err()
                .message()
                .starts_with("No harness is installed in ")
        );
        tree.write(".cursor/tools/data/harness.json", r#"{"name":"cursor"}"#);
        assert_eq!(resolve_harness(&tree.0, None).unwrap(), Harness::Cursor);
        tree.write(".claude/tools/data/harness.json", r#"{"name":"claude"}"#);
        assert!(
            resolve_harness(&tree.0, None)
                .unwrap_err()
                .message()
                .ends_with(
                    "has several harnesses installed (claude, cursor). Pass --harness <name>."
                )
        );
        assert_eq!(
            resolve_harness(&tree.0, Some("claude")).unwrap(),
            Harness::Claude
        );
        assert_eq!(
            resolve_harness(&tree.0, Some("kiro"))
                .unwrap_err()
                .message(),
            "Unknown harness: kiro. Valid: claude, cursor, copilot"
        );
    }

    #[test]
    fn the_active_space_falls_back_to_default() {
        let tree = Tree::new("space");
        let layout = Layout::new(&tree.0, Harness::Claude);
        assert_eq!(layout.active_space(), "default");
        tree.write("focus/active-space", "payments\n");
        // The cursor names a space this project does not have.
        assert_eq!(layout.active_space(), "default");
        tree.write("focus/spaces/payments/memory/org.md", "");
        assert_eq!(layout.active_space(), "payments");
        tree.write("focus/active-space", "../escape");
        assert_eq!(layout.active_space(), "default");
        assert_eq!(
            layout.composer_proposal("payments"),
            tree.0
                .join("focus/spaces/payments/intents/.focus-engine/composer-proposal.json")
        );
    }
}

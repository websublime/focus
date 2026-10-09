//! Shared test support: the vendored upstream fixtures and throwaway trees
//! built from them.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use focus_graph::Sources;
use focus_harness::Harness;

/// `fixtures/` at the workspace root.
pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// The authored Markdown: upstream's at the pinned commit, under Focus's names.
pub fn core() -> PathBuf {
    fixtures().join("upstream/core")
}

/// A compiled file exactly as upstream's packager produced it for `harness`.
pub fn golden(harness: Harness, file: &str) -> String {
    let path = fixtures()
        .join("upstream/golden")
        .join(harness.name())
        .join(file);
    fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()))
}

/// A Claude Code project as upstream's installer leaves it: the vendored
/// subset of upstream's own distribution (stages, agents, scopes, sensors,
/// compiled data, harness manifest, method tree). Removed when dropped.
pub struct Project {
    root: PathBuf,
}

impl Project {
    pub fn installed(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("focus-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        copy_dir(&fixtures().join("upstream/installed/claude"), &root);
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn layout(&self) -> focus_workspace::Layout {
        focus_workspace::Layout::new(&self.root, Harness::Claude)
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }

    pub fn harness_dir(&self) -> PathBuf {
        self.root.join(".claude")
    }

    pub fn sources(&self) -> Sources {
        Sources::from_installed(&self.harness_dir(), Harness::Claude)
    }

    /// Write a project-relative file, creating its directories.
    pub fn write(&self, rel: &str, content: &str) {
        let path = self.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.root.join(rel)).unwrap()
    }

    /// Replace `from` with `to` in a project-relative file; the text must occur.
    pub fn edit(&self, rel: &str, from: &str, to: &str) {
        let before = self.read(rel);
        assert!(before.contains(from), "{rel} does not contain {from:?}");
        self.write(rel, &before.replacen(from, to, 1));
    }

    /// Add a minimal valid stage; `extra` is spliced into its frontmatter and
    /// may override the empty `produces` and `requires_stage` defaults.
    pub fn add_stage(&self, phase: &str, slug: &str, extra: &str) {
        let default = |key: &str| {
            if extra.contains(&format!("{key}:")) {
                String::new()
            } else {
                format!("{key}: []\n")
            }
        };
        let content = format!(
            "---\nslug: {slug}\nphase: {phase}\nexecution: ALWAYS\ncondition: c\nlead_agent: orchestrator\nsupport_agents: []\nmode: inline\n{extra}{}{}consumes: []\ninputs: i\noutputs: o\n---\n\n# {slug}\n",
            default("produces"),
            default("requires_stage"),
        );
        self.write(
            &format!(".claude/focus-common/stages/{phase}/{slug}.md"),
            &content,
        );
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

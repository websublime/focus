//! Which paths runtime integrity protects: the runtime records, the audit
//! trail, and the installed enforcement files of every harness in the
//! project.
//!
//! Mirrors `protectedRuntimePath` through `harnessInstallRoots` in
//! `runtime-integrity.ts`.

use std::path::Path;

use focus_js::path::{Flavour, basename, dirname, is_absolute, join, relative, resolve};
use focus_js::re;
use focus_workspace::project::installed_harnesses;

use super::Integrity;

/// Session records, Plan Approval records, and the words the human-turn
/// hook keeps for a stage gate. Only the hooks and the engine write them;
/// the rest of `.focus-engine/` stays writable. Case-insensitive, like
/// `AUDIT_TRAIL_PATH`, because Windows resolves any casing to the same
/// record.
const RUNTIME_RECORD_PATH: &str =
    r"(?:^|[\\/])\.(?:focus-sessions|focus-plan-approval|focus-engine[\\/]+gate-words)(?:[\\/]|$)";
pub(super) const RUNTIME_RECORD_MENTION: &str = r#"(?:^|[\\/'"`\s])\.(?:focus-sessions|focus-plan-approval|focus-engine[\\/]+gate-words)(?=[\\/'"`\s]|$)"#;
/// The audit trail: per-intent shards and the bare space shard directory,
/// anchored on the workspace layout, never on a bare `audit` segment.
const AUDIT_TRAIL_PATH: &str =
    r"(?:^|[\\/])focus[\\/]spaces[\\/][^\\/]+[\\/]intents[\\/](?:[^\\/]+[\\/])?audit(?:[\\/]|$)";
pub(super) const HOOK_FILE: &str = r"(?:^|[\\/])hooks[\\/]focus-[a-z-]+\.ts$|(?:^|[\\/])focus-(?:kiro|codex|copilot|cursor)-adapter\.ts$";
pub(super) const HOOK_MODULE: &str =
    r"(?:^|[\\/])(?:hooks[\\/]focus-[a-z-]+|focus-(?:record-human-turn|guard-switch))(?:\.ts)?$";

/// These shipped entrypoints legitimately load hook helpers or perform
/// runtime maintenance. Their paths are also protected from direct tool-call
/// mutation. Other scripts do not gain trust from being placed in a harness
/// directory.
const TRUSTED_RUNTIME_ENTRYPOINTS: [&str; 8] = [
    "focus.ts",
    "focus-lib.ts",
    "focus-utility.ts",
    "focus-orchestrate.ts",
    "focus-plugin-validate.ts",
    "focus-init.ts",
    "focus-runtime.ts",
    "focus-update.ts",
];

impl Integrity {
    pub(super) fn flavour(&self) -> Flavour {
        self.host.flavour()
    }

    /// `path.resolve(...parts)` on this host.
    pub(super) fn resolve(&self, parts: &[&str]) -> String {
        resolve(self.flavour(), &self.host.cwd, parts)
    }

    fn relative(&self, from: &str, to: &str) -> String {
        relative(self.flavour(), &self.host.cwd, from, to)
    }

    /// `pathWithin`.
    pub(super) fn path_within(&self, path: &str, root: &str) -> bool {
        let child = self.relative(root, path);
        let sep = self.flavour().separator();
        child != ".."
            && !child.starts_with(&format!("..{sep}"))
            && !is_absolute(self.flavour(), &child)
    }

    /// `canonicalExistingPath`: the real path of the longest existing
    /// prefix, with the rest joined on, so creating or replacing a file
    /// through a directory symlink has the same target classification as
    /// its real path.
    pub(super) fn canonical_existing_path(&self, path: &str) -> String {
        let absolute = self.resolve(&[path]);
        let mut existing = absolute.clone();
        let mut suffix: Vec<String> = Vec::new();
        loop {
            if let Ok(real) = std::fs::canonicalize(&existing) {
                let real = real.to_string_lossy().into_owned();
                // Windows reports a verbatim path; Node's realpath does not.
                let real = real
                    .strip_prefix(r"\\?\")
                    .map_or(real.clone(), str::to_owned);
                let mut parts: Vec<&str> = vec![real.as_str()];
                parts.extend(suffix.iter().map(String::as_str));
                return self.resolve(&parts);
            }
            let parent = dirname(self.flavour(), &existing);
            if parent == existing {
                return absolute;
            }
            suffix.insert(0, basename(self.flavour(), &existing));
            existing = parent;
        }
    }

    /// `protectedRuntimePath`.
    pub(super) fn protected_runtime_path(&self, path: &str, cwd: &str) -> bool {
        !path.is_empty()
            && (re::test(RUNTIME_RECORD_PATH, "i", path)
                || re::test(RUNTIME_RECORD_PATH, "i", &self.resolve(&[cwd, path])))
    }

    /// `protectedAuditTrailPath`.
    pub(super) fn protected_audit_trail_path(&self, path: &str, cwd: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        if re::test(AUDIT_TRAIL_PATH, "i", path) {
            return true;
        }
        let absolute = self.resolve(&[cwd, path]);
        re::test(AUDIT_TRAIL_PATH, "i", &absolute)
            || re::test(
                AUDIT_TRAIL_PATH,
                "i",
                &self.canonical_existing_path(&absolute),
            )
    }

    /// `authoredRuntimeRoot`: upstream's own `core/`, which is not an
    /// installed tree.
    fn authored_runtime_root(&self, root: &str) -> bool {
        basename(self.flavour(), root) == "core"
            && Path::new(&self.resolve(&[root, "../scripts/package.ts"])).exists()
    }

    /// `integrityProjectRoots`: where the tool call runs, and the hook's own
    /// project.
    fn integrity_project_roots(&self, cwd: &str) -> Vec<String> {
        let mut roots = vec![self.resolve(&[cwd])];
        let project = self.resolve(&[&self.project_dir]);
        if !roots.contains(&project) {
            roots.push(project);
        }
        roots
    }

    /// `runtimeHarnessDir(projectDir)`: the explicit directory, else the
    /// first harness installed, else `.claude`.
    fn runtime_harness_dir(&self, project_dir: &str) -> String {
        if let Some(explicit) = self
            .harness_dir
            .as_deref()
            .map(str::trim)
            .filter(|dir| !dir.is_empty())
        {
            return explicit.to_owned();
        }
        installed_harnesses(Path::new(project_dir))
            .first()
            .map_or_else(|| ".claude".to_owned(), |harness| harness.dir().to_owned())
    }

    /// `harnessInstallRoots`: the project's own harness root (unless a link
    /// is on the way to it), and the conventional harness directories.
    fn harness_install_roots(&self, cwd: &str) -> Vec<String> {
        let mut roots: Vec<String> = Vec::new();
        let mut add = |root: String| {
            if !roots.contains(&root) {
                roots.push(root);
            }
        };
        let flavour = self.flavour();
        for project_dir in self.integrity_project_roots(cwd) {
            let harness_dir = self.runtime_harness_dir(&project_dir);
            let project_root = if basename(flavour, &project_dir) == harness_dir
                && Path::new(&join(flavour, &[&project_dir, "tools"])).exists()
            {
                project_dir.clone()
            } else {
                join(flavour, &[&project_dir, &harness_dir])
            };
            if focus_workspace::links::link_on_the_way(
                Path::new(&project_dir),
                Path::new(&project_root),
            )
            .is_none()
            {
                add(project_root);
            }
            for dir in [".claude", ".codex", ".kiro", ".cursor", ".focus"] {
                add(self.resolve(&[&project_dir, dir]));
            }
        }
        roots
    }

    fn installed_roots(&self, cwd: &str) -> Vec<String> {
        self.harness_install_roots(cwd)
            .into_iter()
            .filter(|root| !self.authored_runtime_root(root))
            .collect()
    }

    /// `installedRelativeProtected`.
    fn installed_relative_protected(&self, path: &str, root: &str, harness_name: &str) -> bool {
        let rel = self.relative(root, path).replace('\\', "/");
        if !self.path_within(path, root) {
            return false;
        }
        if rel.is_empty() || rel == "hooks" || rel.starts_with("hooks/") || rel == "tools" {
            return true;
        }
        if re::test(r"^tools/focus(?:-[a-z0-9-]+)?(?:\.ts|\.exe)?$", "", &rel) {
            return true;
        }
        if re::test(r"^bin/focus(?:\.exe)?$", "", &rel) || rel == "bin" {
            return true;
        }
        if rel == "hooks.json" {
            return true;
        }
        if harness_name == ".claude" && rel == "settings.json" {
            return true;
        }
        // Kiro's agents carry their own tool grants: JSON in the agent-v1
        // layout, Markdown frontmatter in the KAS layout.
        harness_name == ".kiro"
            && (rel == "agents" || re::test(r"^agents/focus(?:-[a-z-]+)?\.(?:json|md)$", "", &rel))
    }

    /// `protectedInstalledPath`: a path in an installed harness tree that the
    /// enforcement depends on; with `ancestors`, a directory above one too.
    pub(super) fn protected_installed_path(&self, path: &str, cwd: &str, ancestors: bool) -> bool {
        if path.is_empty() {
            return false;
        }
        let flavour = self.flavour();
        let absolute = self.resolve(&[cwd, path]);
        let canonical = self.canonical_existing_path(&absolute);
        for root in self.installed_roots(cwd) {
            let real_root = self.canonical_existing_path(&root);
            let harness_name = basename(flavour, &root);
            if (ancestors
                && (self.path_within(&root, &absolute) || self.path_within(&real_root, &canonical)))
                || self.installed_relative_protected(&absolute, &root, &harness_name)
                || self.installed_relative_protected(&canonical, &real_root, &harness_name)
            {
                return true;
            }
        }
        // These native hook entrypoints live beside the shared engine.
        let mut entrypoints: Vec<String> = self
            .integrity_project_roots(cwd)
            .iter()
            .flat_map(|root| {
                [
                    self.resolve(&[root, ".opencode/plugin/focus-opencode-adapter.ts"]),
                    self.resolve(&[root, ".github/hooks/focus.json"]),
                ]
            })
            .collect();
        if let Some(executable) = &self.executable {
            entrypoints.push(executable.clone());
        }
        entrypoints.iter().any(|entry| {
            absolute == *entry
                || canonical == self.canonical_existing_path(entry)
                || (ancestors
                    && (self.path_within(entry, &absolute)
                        || self.path_within(&self.canonical_existing_path(entry), &canonical)))
        })
    }

    /// `authoredDevelopmentRoot`: upstream's own repository, which must be
    /// able to develop and document its hooks.
    fn authored_development_root(&self, path: &str, cwd: &str) -> Option<String> {
        let absolute = self.resolve(&[cwd, path]);
        self.integrity_project_roots(cwd).into_iter().find(|root| {
            Path::new(&self.resolve(&[root, "scripts/package.ts"])).exists()
                && ["core", "harness", "tests", "docs"]
                    .iter()
                    .any(|tree| self.path_within(&absolute, &self.resolve(&[root, tree])))
        })
    }

    pub(super) fn is_authored_development_path(&self, path: &str, cwd: &str) -> bool {
        self.authored_development_root(path, cwd).is_some()
    }

    /// `trustedInstalledScript`.
    pub(super) fn trusted_installed_script(&self, path: &str, cwd: &str) -> bool {
        let canonical = self.canonical_existing_path(path);
        if let Some(authored_root) = self.authored_development_root(path, cwd) {
            let rel = self
                .relative(
                    &self.resolve(&[&authored_root, "core/tools"]),
                    &self.resolve(&[cwd, path]),
                )
                .replace('\\', "/");
            if TRUSTED_RUNTIME_ENTRYPOINTS.contains(&rel.as_str()) {
                return true;
            }
        }
        for root in self.installed_roots(cwd) {
            let rel = self
                .relative(&self.canonical_existing_path(&root), &canonical)
                .replace('\\', "/");
            if let Some(name) = rel.strip_prefix("tools/")
                && TRUSTED_RUNTIME_ENTRYPOINTS.contains(&name)
            {
                return true;
            }
        }
        false
    }
}

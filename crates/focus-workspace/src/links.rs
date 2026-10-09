//! Refusing to write through a link.
//!
//! The engine writes inside the project: compiled data, composed scopes,
//! later state and audit. If a directory on the way to a write target is a
//! symbolic link, the write lands wherever the link points, which can be
//! outside the project and outside what the person reviewed. Every write
//! target is checked first. Mirrors `linkOnTheWay` / `LinkedFolderError` in
//! `aidlc-runtime-paths.ts` (L598).

use std::path::{Component, Path};

use focus_js::{Result, bail};

/// The directory and file names the engine itself gives its tree. The refusal
/// names a path only as far as it is made of these, so no name a repository
/// chose is printed.
const ENGINE_NAMES: [&str; 25] = [
    ".agents",
    ".claude",
    ".codex",
    ".cursor",
    ".focus",
    ".github",
    ".kiro",
    ".opencode",
    "agents",
    "command",
    "data",
    "focus",
    "focus-common",
    "hooks",
    "knowledge",
    "plugin",
    "rules",
    "scopes",
    "sensors",
    "settings",
    "skills",
    "spaces",
    "stages",
    "steering",
    "tools",
];

/// The first directory or file on the way from `project` to `target`,
/// `target` included, that is a symbolic link, as its path parts relative to
/// the project. `None` when there is none, when `target` is not inside
/// `project`, or from the first part that does not exist yet.
#[must_use]
pub fn link_on_the_way(project: &Path, target: &Path) -> Option<Vec<String>> {
    let rel = target.strip_prefix(project).ok()?;
    let mut path = project.to_path_buf();
    let mut parts: Vec<String> = Vec::new();
    for component in rel.components() {
        let Component::Normal(part) = component else {
            return None;
        };
        path.push(part);
        parts.push(part.to_string_lossy().into_owned());
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => return Some(parts),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Fail when writing `target` would go through a link inside `project`.
///
/// # Errors
///
/// A link is on the way. The message says where, naming only engine-owned
/// directories.
pub fn refuse_link_on_the_way(project: &Path, target: &Path) -> Result<()> {
    let Some(parts) = link_on_the_way(project, target) else {
        return Ok(());
    };
    let separator = std::path::MAIN_SEPARATOR.to_string();
    let remedy = "Replace the link with a real folder or file, then run this again.";
    match parts
        .iter()
        .position(|part| !ENGINE_NAMES.contains(&part.as_str()))
    {
        None => bail!(
            "{} is a link, so Focus changed nothing there. {remedy}",
            parts.join(&separator)
        ),
        Some(0) => bail!("This project holds a link, so Focus changed nothing there. {remedy}"),
        Some(known) => bail!(
            "{} holds a link, so Focus changed nothing there. {remedy}",
            parts[..known].join(&separator)
        ),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn a_link_on_the_way_is_refused_and_named_by_engine_directory() {
        let dir = std::env::temp_dir().join(format!("focus-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let project = dir.join("project");
        let elsewhere = dir.join("elsewhere");
        std::fs::create_dir_all(project.join(".claude/tools")).unwrap();
        std::fs::create_dir_all(project.join("focus")).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();

        let data = project.join(".claude/tools/data/stage-graph.json");
        // Nothing on the way is a link; the missing tail is fine.
        assert!(refuse_link_on_the_way(&project, &data).is_ok());
        // Outside the project is not this check's to judge.
        assert!(refuse_link_on_the_way(&project, &elsewhere.join("x")).is_ok());

        std::os::unix::fs::symlink(&elsewhere, project.join(".claude/tools/data")).unwrap();
        assert_eq!(
            refuse_link_on_the_way(&project, &data)
                .unwrap_err()
                .message(),
            ".claude/tools/data is a link, so Focus changed nothing there. Replace the link with a real folder or file, then run this again."
        );

        std::os::unix::fs::symlink(&elsewhere, project.join("focus/team-notes")).unwrap();
        assert_eq!(
            refuse_link_on_the_way(&project, &project.join("focus/team-notes/x.md"))
                .unwrap_err()
                .message(),
            "focus holds a link, so Focus changed nothing there. Replace the link with a real folder or file, then run this again."
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

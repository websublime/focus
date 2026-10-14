//! The copy of a space's memory files that the harness includes read.
//!
//! The harnesses' include files point at `focus/active-memory/`, a copy of
//! the active space's memory files kept in step with them: each copy carries
//! a header line saying which file it mirrors. Creating a piece of work
//! brings the copy up to date (so does a space switch, later). The copy is
//! never written through a link, and a file that cannot be copied is left
//! as it was.
//!
//! Mirrors `memoryLayerFiles`, `activeMemoryCopyDrift` and
//! `refreshActiveMemoryCopy` in `aidlc-includes.ts`.

use std::path::{Path, PathBuf};

use focus_js::order::cmp_utf16;

use crate::atomic::write_atomic;
use crate::names::WORKSPACE_DIR;
use crate::{Layout, links};

/// Where the copy lives, relative to the project.
pub const ACTIVE_MEMORY_DIR: &str = "focus/active-memory";

/// The memory layers, in the order the copy lists them.
const MEMORY_LAYERS: [&str; 7] = [
    "org.md",
    "team.md",
    "project.md",
    "phases/ideation.md",
    "phases/inception.md",
    "phases/construction.md",
    "phases/operation.md",
];

/// `spaceMemoryRel`: the space's memory directory, relative to the project.
#[must_use]
pub fn space_memory_rel(space: &str) -> String {
    format!("{WORKSPACE_DIR}/spaces/{space}/memory")
}

/// The header line a copy starts with.
#[must_use]
pub fn copy_header(space: &str, rel: &str) -> String {
    format!(
        "<!-- Focus keeps this copy in step with {}/{rel}. Edit that file; this copy is replaced. -->\n",
        space_memory_rel(space)
    )
}

/// `memoryLayerFiles`: the layers, then any other `.md` file at the top and
/// under `phases/`, sorted; only those that are regular files (a link is
/// not one).
fn memory_layer_files(memory_dir: &Path) -> Vec<String> {
    let extra = |sub: &str| -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(if sub.is_empty() {
            memory_dir.to_path_buf()
        } else {
            memory_dir.join(sub)
        }) else {
            return Vec::new();
        };
        let mut found: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".md"))
            .map(|name| {
                if sub.is_empty() {
                    name
                } else {
                    format!("{sub}/{name}")
                }
            })
            .filter(|rel| !MEMORY_LAYERS.contains(&rel.as_str()))
            .collect();
        found.sort_by(|a, b| cmp_utf16(a, b));
        found
    };
    MEMORY_LAYERS
        .iter()
        .map(|layer| (*layer).to_owned())
        .chain(extra(""))
        .chain(extra("phases"))
        .filter(|rel| {
            std::fs::symlink_metadata(memory_dir.join(rel)).is_ok_and(|meta| meta.is_file())
        })
        .collect()
}

/// A file's text, or nothing when it is not UTF-8 (then it is not copied).
fn utf8_text(path: &Path) -> Option<String> {
    String::from_utf8(std::fs::read(path).ok()?).ok()
}

fn copied_text(layout: &Layout, space: &str, rel: &str) -> Option<String> {
    let text = utf8_text(&layout.memory_dir(space).join(rel))?;
    Some(format!("{}{text}", copy_header(space, rel)))
}

fn copy_dir(layout: &Layout) -> PathBuf {
    layout.project.join(WORKSPACE_DIR).join("active-memory")
}

/// `activeMemoryCopyDrift`: the copy's files that are not the memory files
/// of `space` now: missing or different ones (the header included), and
/// ones the space no longer has. Empty when the copy is current.
#[must_use]
pub fn drift(layout: &Layout, space: &str) -> Vec<String> {
    let source = layout.memory_dir(space);
    let copy = copy_dir(layout);
    let wanted: Vec<String> = memory_layer_files(&source)
        .into_iter()
        .filter(|rel| utf8_text(&source.join(rel)).is_some())
        .collect();
    let mut out: Vec<String> = wanted
        .iter()
        .filter(|rel| utf8_text(&copy.join(rel)) != copied_text(layout, space, rel))
        .cloned()
        .collect();
    out.extend(
        memory_layer_files(&copy)
            .into_iter()
            .filter(|rel| !wanted.contains(rel)),
    );
    out
}

/// `refreshActiveMemoryCopy`: write the memory files of `space` into the
/// copy, file by file, and remove copied files the space no longer has.
/// Whether anything changed. Best effort: a file that cannot be copied is
/// left as it was, and nothing is written through a link.
pub fn refresh(layout: &Layout, space: &str) -> bool {
    let drifted = drift(layout, space);
    if drifted.is_empty() {
        return false;
    }
    let copy = copy_dir(layout);
    let mut changed = false;
    for rel in drifted {
        let target = copy.join(&rel);
        let relative = Path::new(ACTIVE_MEMORY_DIR).join(&rel);
        if links::refuse_link_in_chain(&layout.project, &relative).is_err() {
            continue;
        }
        let done = match copied_text(layout, space, &rel) {
            None => match std::fs::remove_file(&target) {
                Ok(()) => true,
                Err(err) => err.kind() == std::io::ErrorKind::NotFound,
            },
            Some(text) => {
                target
                    .parent()
                    .is_some_and(|dir| std::fs::create_dir_all(dir).is_ok())
                    && write_atomic(&target, text.as_bytes()).is_ok()
            }
        };
        changed |= done;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    fn layout(name: &str) -> Layout {
        let root =
            std::env::temp_dir().join(format!("focus-active-memory-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Layout::new(root, Harness::Claude)
    }

    fn write(path: &Path, content: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn the_copy_follows_the_space_memory() {
        let layout = layout("copy");
        let memory = layout.memory_dir("default");
        write(&memory.join("org.md"), b"# Org\n");
        write(&memory.join("phases/ideation.md"), b"# Ideation\n");
        write(&memory.join("extra.md"), b"# Extra\n");
        write(&memory.join("phases/zz.md"), b"# ZZ\n");
        write(&memory.join("phases/aa.md"), b"# AA\n");
        write(&memory.join("notes.txt"), b"not md\n");
        write(&memory.join("binary.md"), b"\xff\xfe");
        assert_eq!(
            memory_layer_files(&memory),
            [
                "org.md",
                "phases/ideation.md",
                "binary.md",
                "extra.md",
                "phases/aa.md",
                "phases/zz.md"
            ]
        );
        assert_eq!(
            drift(&layout, "default"),
            [
                "org.md",
                "phases/ideation.md",
                "extra.md",
                "phases/aa.md",
                "phases/zz.md"
            ]
        );
        assert!(refresh(&layout, "default"));
        let copy = copy_dir(&layout);
        assert_eq!(
            std::fs::read_to_string(copy.join("org.md")).unwrap(),
            "<!-- Focus keeps this copy in step with focus/spaces/default/memory/org.md. Edit that file; this copy is replaced. -->\n# Org\n"
        );
        assert!(copy.join("phases/zz.md").is_file());
        assert!(!copy.join("binary.md").exists());
        assert!(!copy.join("notes.txt").exists());
        // Current: nothing to do. A file the space loses leaves the copy.
        assert!(!refresh(&layout, "default"));
        std::fs::remove_file(memory.join("extra.md")).unwrap();
        write(&memory.join("org.md"), b"# Org 2\n");
        assert_eq!(drift(&layout, "default"), ["org.md", "extra.md"]);
        assert!(refresh(&layout, "default"));
        assert!(!copy.join("extra.md").exists());
        assert!(
            std::fs::read_to_string(copy.join("org.md"))
                .unwrap()
                .ends_with("# Org 2\n")
        );
        let _ = std::fs::remove_dir_all(&layout.project);
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_link() {
        let layout = layout("link");
        let memory = layout.memory_dir("default");
        write(&memory.join("org.md"), b"# Org\n");
        let elsewhere = layout.project.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(layout.workspace_root()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, copy_dir(&layout)).unwrap();
        assert!(!refresh(&layout, "default"));
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&layout.project);
    }
}

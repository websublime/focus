//! The hooks-health directory of a record: the heartbeat each hook leaves,
//! the drops it had to swallow, and the debug log. Mirrors `hooksHealthDir`,
//! `hookStatusTarget`, `writeHookStatusFile`, `recordHookDrop` and
//! `hookDebug` in `aidlc-lib.ts`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_workspace::Layout;
use focus_workspace::links::refuse_link_in_chain;
use focus_workspace::records::Selection;

/// `hooksHealthDir`.
#[must_use]
pub fn hooks_health_dir(layout: &Layout, selection: &Selection) -> PathBuf {
    selection.engine_dir(layout).join("hooks-health")
}

/// `hookStatusTarget`: the file, once nothing between the record and it is
/// a link; with `create`, the directories are made first.
fn status_target(health_dir: &Path, file_name: &str, create: bool) -> Option<PathBuf> {
    let record = health_dir.parent()?.parent()?;
    if create {
        std::fs::create_dir_all(record).ok()?;
    }
    let anchor = std::fs::canonicalize(record).ok()?;
    let rel = health_dir.join(file_name);
    let rel = rel.strip_prefix(record).ok()?;
    refuse_link_in_chain(&anchor, rel).ok()?;
    let target = anchor.join(rel);
    if create {
        std::fs::create_dir_all(target.parent()?).ok()?;
    }
    // The directories exist now; none of them, nor the file, may be a link.
    refuse_link_in_chain(&anchor, rel).ok()?;
    Some(target)
}

/// `writeHookStatusFile`: never fails; answers whether it wrote.
pub fn write_status_file(health_dir: &Path, file_name: &str, data: &str, append: bool) -> bool {
    let Some(target) = status_target(health_dir, file_name, true) else {
        return false;
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644).custom_flags(libc_o_nofollow());
    }
    match options.open(&target) {
        Ok(mut file) => file.write_all(data.as_bytes()).is_ok(),
        Err(_) => false,
    }
}

#[cfg(unix)]
const fn libc_o_nofollow() -> i32 {
    // O_NOFOLLOW on Linux and the BSDs (macOS included) differ; both are
    // what `std::fs` would pass through, so each is spelled for its target.
    if cfg!(target_os = "linux") {
        0o400_000
    } else {
        0x0100
    }
}

/// The record upstream's path helpers name when a hook passes none: what
/// the cursor says (`hooksHealthDir(projectDir)`); `None` when the
/// resolution refuses, in which case upstream's `recordHookDrop` swallows
/// the throw and nothing is written.
#[must_use]
pub fn cursor_selection(layout: &Layout) -> Option<Selection> {
    focus_workspace::records::resolve_selection(
        layout,
        &focus_workspace::records::Selector::default(),
        &focus_workspace::records::Invoker::default(),
    )
    .ok()
}

/// `recordHookDrop`: one dropped emission, as an ISO timestamp, a tab and
/// its one-line reason, appended to `<hook>.drops`.
pub fn record_drop(layout: &Layout, selection: &Selection, hook: &str, reason: &str) {
    let line = format!(
        "{}\t{}\n",
        iso_timestamp(SystemTime::now()),
        reason.replace("\r\n", " ").replace('\n', " ")
    );
    write_status_file(
        &hooks_health_dir(layout, selection),
        &format!("{hook}.drops"),
        &line,
        true,
    );
}

/// `hookDebugEnabled`: `FOCUS_HOOK_DEBUG`, or the workspace's
/// `.focus-hook-debug` marker.
#[must_use]
pub fn debug_enabled(layout: &Layout) -> bool {
    focus_workspace::env::is_on("HOOK_DEBUG")
        || layout.workspace_root().join(".focus-hook-debug").exists()
}

/// `hookDebug`: a line of `hook-debug.log`, when debugging is on.
pub fn debug(
    layout: &Layout,
    selection: &Selection,
    hook: &str,
    message: &str,
    fields: &[(&str, serde_json::Value)],
) {
    if !debug_enabled(layout) {
        return;
    }
    let mut parts = vec![
        iso_timestamp(SystemTime::now()),
        hook.to_owned(),
        message.to_owned(),
    ];
    if !fields.is_empty() {
        let flat: Vec<String> = fields
            .iter()
            .map(|(key, value)| format!("{key}={}", focus_js::json::stringify(value)))
            .collect();
        parts.push(flat.join(" "));
    }
    let line = format!(
        "{}\n",
        parts.join("\t").replace("\r\n", " ").replace('\n', " ")
    );
    write_status_file(
        &hooks_health_dir(layout, selection),
        "hook-debug.log",
        &line,
        true,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    const RECORD: &str = "focus/spaces/default/intents/260105-add-login";

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("focus-hooks-health-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join(RECORD)).unwrap();
            Self(Layout::new(root, Harness::Claude))
        }

        fn selection(&self) -> Selection {
            Selection {
                space: "default".to_owned(),
                intent: Some("260105-add-login".to_owned()),
                session_id: None,
                binding: None,
            }
        }

        fn read(&self, rel: &str) -> Option<String> {
            std::fs::read_to_string(self.0.project.join(rel)).ok()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    #[test]
    fn the_health_directory_is_the_records_engine_directory_or_the_spaces() {
        let tree = Tree::new("dir");
        assert_eq!(
            hooks_health_dir(&tree.0, &tree.selection()),
            tree.0
                .project
                .join(RECORD)
                .join(".focus-engine/hooks-health")
        );
        let bare = Selection {
            intent: None,
            ..tree.selection()
        };
        assert_eq!(
            hooks_health_dir(&tree.0, &bare),
            tree.0
                .project
                .join("focus/spaces/default/intents/.focus-engine/hooks-health")
        );
    }

    #[test]
    fn a_status_file_is_replaced_and_a_drop_is_appended() {
        let tree = Tree::new("write");
        let dir = hooks_health_dir(&tree.0, &tree.selection());
        assert!(write_status_file(&dir, "x.last", "one", false));
        assert!(write_status_file(&dir, "x.last", "two", false));
        assert_eq!(
            tree.read(&format!("{RECORD}/.focus-engine/hooks-health/x.last"))
                .as_deref(),
            Some("two")
        );
        record_drop(&tree.0, &tree.selection(), "x", "first\r\nline");
        record_drop(&tree.0, &tree.selection(), "x", "second\nline");
        let drops = tree
            .read(&format!("{RECORD}/.focus-engine/hooks-health/x.drops"))
            .unwrap();
        let lines: Vec<&str> = drops.lines().collect();
        assert_eq!(lines.len(), 2, "{drops}");
        assert!(lines[0].ends_with("Z\tfirst line"), "{drops}");
        assert!(lines[1].ends_with("Z\tsecond line"), "{drops}");
        // The record directory is created on the way when missing.
        let _ = std::fs::remove_dir_all(tree.0.project.join(RECORD));
        assert!(write_status_file(&dir, "y.last", "back", false));
        assert_eq!(
            tree.read(&format!("{RECORD}/.focus-engine/hooks-health/y.last"))
                .as_deref(),
            Some("back")
        );
    }

    #[test]
    fn the_debug_log_is_written_only_when_asked() {
        let tree = Tree::new("debug");
        debug(&tree.0, &tree.selection(), "x", "quiet", &[]);
        assert!(
            tree.read(&format!(
                "{RECORD}/.focus-engine/hooks-health/hook-debug.log"
            ))
            .is_none()
        );
        std::fs::write(tree.0.project.join("focus/.focus-hook-debug"), "").unwrap();
        debug(
            &tree.0,
            &tree.selection(),
            "x",
            "said",
            &[("k", serde_json::json!("v")), ("n", serde_json::json!(1))],
        );
        let log = tree
            .read(&format!(
                "{RECORD}/.focus-engine/hooks-health/hook-debug.log"
            ))
            .unwrap();
        assert!(log.ends_with("Z\tx\tsaid\tk=\"v\" n=1\n"), "{log}");
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_link() {
        let tree = Tree::new("link");
        let elsewhere = tree.0.project.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        // The engine directory as a link to somewhere else: refused, and
        // nothing lands at the other end.
        std::os::unix::fs::symlink(
            &elsewhere,
            tree.0.project.join(RECORD).join(".focus-engine"),
        )
        .unwrap();
        let dir = hooks_health_dir(&tree.0, &tree.selection());
        assert!(!write_status_file(&dir, "x.last", "no", false));
        assert!(std::fs::read_dir(&elsewhere).unwrap().next().is_none());
        std::fs::remove_file(tree.0.project.join(RECORD).join(".focus-engine")).unwrap();
        // The file itself as a link: refused too.
        std::fs::create_dir_all(&dir).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("target"), dir.join("x.last")).unwrap();
        assert!(!write_status_file(&dir, "x.last", "no", false));
        assert!(!elsewhere.join("target").exists());
        assert!(write_status_file(&dir, "y.last", "yes", false));
    }
}

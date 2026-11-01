//! What the host loaded into a chat, so a later step can tell whether the
//! chat already holds a stage's exact rule text (`aidlc-rules-held.ts`).
//!
//! The session-start hook records what the host read when the chat
//! started, resumed, cleared, compacted or forked
//! (`.focus-sessions/<id>.rules-held.json`); every start, and a compaction,
//! also forgets the full text the thread was handed
//! (`<id>.rules-delivered.json`), since the chat may no longer hold it. The
//! steps that read these records are the orchestrator's (M5); this module
//! writes them as upstream does, so those steps find them.
//!
//! What counts as proof was measured on each host: Claude Code loads its
//! `@`-imports at startup, resume, clear and compact, so the record holds the
//! hash of each imported file; Cursor and Copilot have no proven copy, so
//! they get no record and always the full text.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_harness::Harness;
use focus_js::json;
use focus_workspace::Layout;
use focus_workspace::active_memory::{ACTIVE_MEMORY_DIR, space_memory_rel};
use focus_workspace::atomic::write_atomic;
use focus_workspace::names::SESSIONS_DIR;
use focus_workspace::observer;
use focus_workspace::records::valid_session_id;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

fn sessions_dir(layout: &Layout) -> PathBuf {
    layout.workspace_root().join(SESSIONS_DIR)
}

fn load_record_path(layout: &Layout, session_id: &str) -> PathBuf {
    sessions_dir(layout).join(format!("{session_id}.rules-held.json"))
}

fn delivery_record_path(layout: &Layout, session_id: &str) -> PathBuf {
    sessions_dir(layout).join(format!("{session_id}.rules-delivered.json"))
}

fn sha256_file(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => format!("{:x}", Sha256::digest(&bytes)),
        Err(_) => String::new(),
    }
}

fn read_json(path: &Path) -> Option<Value> {
    let raw = std::fs::read_to_string(path).ok()?;
    json::parse(raw.strip_prefix('\u{feff}').unwrap_or(&raw)).ok()
}

/// `readDelivery`: a record of schema 2 whose `last` is text.
fn read_delivery(layout: &Layout, session_id: &str) -> Option<Map<String, Value>> {
    let Value::Object(record) = read_json(&delivery_record_path(layout, session_id))? else {
        return None;
    };
    (record.get("v") == Some(&json!(2)) && record.get("last").is_some_and(Value::is_string))
        .then_some(record)
}

fn write_delivery(layout: &Layout, session_id: &str, record: &Value) {
    let path = delivery_record_path(layout, session_id);
    // `writeFileAtomic` starts at the observer barrier; a refused write
    // leaves no record, as upstream's catch does.
    if observer::refuse_write("writeFileAtomic").is_err()
        || std::fs::create_dir_all(sessions_dir(layout)).is_err()
        || write_atomic(&path, format!("{}\n", json::stringify(record)).as_bytes()).is_err()
    {
        let _ = std::fs::remove_file(&path);
    }
}

/// `forgetFullDelivery`: the chat may no longer hold what it was handed in
/// full; what its last step named is kept.
fn forget_full_delivery(layout: &Layout, session_id: &str) {
    match read_delivery(layout, session_id) {
        None => {
            let _ = std::fs::remove_file(delivery_record_path(layout, session_id));
        }
        Some(record) => {
            if record.contains_key("full") || record.contains_key("persona") {
                write_delivery(
                    layout,
                    session_id,
                    &json!({ "v": 2, "last": record["last"] }),
                );
            }
        }
    }
}

/// `clearRulesDelivered`: a compaction forgets the full text the thread was
/// handed.
pub fn clear_rules_delivered(layout: &Layout, session_id: &str) {
    if let Some(valid) = valid_session_id(Some(session_id)) {
        forget_full_delivery(layout, valid);
    }
}

/// The memory file of `space` a path the host reads stands for: the same
/// file under the copy, or the file itself (an include from before the
/// copy).
fn memory_file_for(rel: &str, space: &str) -> String {
    match rel.strip_prefix(&format!("{ACTIVE_MEMORY_DIR}/")) {
        Some(under_copy) => format!("{}/{under_copy}", space_memory_rel(space)),
        None => rel.to_owned(),
    }
}

/// `claudeImportedFiles`: the memory files the Claude `@`-import stub
/// (`<harness>/rules/focus.md`) names, as project-relative paths. Each
/// `@` line is a path relative to the stub; one that leaves the project is
/// not a file of it.
fn claude_imported_files(layout: &Layout, space: &str) -> Vec<String> {
    let stub = layout.harness_root().join("rules").join("focus.md");
    let Ok(text) = std::fs::read_to_string(&stub) else {
        return Vec::new();
    };
    let flavour = focus_js::path::Flavour::host();
    let project = layout.project.to_string_lossy().into_owned();
    let stub_dir = focus_js::path::dirname(flavour, &stub.to_string_lossy());
    let mut files = Vec::new();
    for line in text.split(['\n', '\r']) {
        let line = focus_js::text::trim(line);
        let Some(found) = focus_js::re::find(r"^@(\S+\.md)\s*$", "", line) else {
            continue;
        };
        let imported = focus_js::re::group(line, &found, 1).unwrap_or("");
        let absolute = focus_js::path::resolve(flavour, &project, &[&stub_dir, imported]);
        let rel =
            focus_js::path::relative(flavour, &project, &project, &absolute).replace('\\', "/");
        if !rel.starts_with("..") {
            files.push(memory_file_for(&rel, space));
        }
    }
    files
}

/// `recordRulesLoad`: record what the host loaded when this chat started,
/// resumed, cleared, compacted or forked. Every session start also forgets
/// the full text the thread was handed. `include_changed` says the start
/// just re-pointed the include, so the host may have loaded the old one.
pub fn record_rules_load(
    layout: &Layout,
    session_id: &str,
    source: &str,
    space: &str,
    include_changed: bool,
    now: SystemTime,
) {
    let Some(sid) = valid_session_id(Some(session_id)) else {
        return;
    };
    forget_full_delivery(layout, sid);
    let path = load_record_path(layout, sid);
    let record = match layout.harness {
        Harness::Claude => {
            let mut files = Map::new();
            for rel in claude_imported_files(layout, space) {
                let hash = sha256_file(&layout.project.join(&rel));
                files.insert(rel, json!(hash));
            }
            let previous = read_json(&path);
            let same_as_before = previous.as_ref().is_some_and(|previous| {
                previous.get("stale") != Some(&json!(true))
                    && previous.get("files").is_some_and(|before| {
                        json::stringify(before) == json::stringify(&Value::Object(files.clone()))
                    })
            });
            let stale = include_changed
                || source == "fork"
                || (source == "resume" && !same_as_before)
                || !["startup", "clear", "compact", "resume"].contains(&source);
            let mut record = json!({
                "v": 1,
                "harness": layout.harness.name(),
                "source": source,
                "at": iso_timestamp(now),
                "space": space,
                "refresh": "at-load",
                "files": Value::Object(files),
            });
            if stale {
                record["stale"] = json!(true);
            }
            Some(record)
        }
        // Cursor and Copilot have no proven copy: no record, the full text.
        Harness::Cursor | Harness::Copilot => None,
    };
    match record {
        None => {
            let _ = std::fs::remove_file(&path);
        }
        Some(record) => {
            if observer::refuse_write("writeFileAtomic").is_err()
                || write_atomic(&path, format!("{}\n", json::stringify(&record)).as_bytes())
                    .is_err()
            {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str, harness: Harness) -> Self {
            let root = std::env::temp_dir()
                .join(format!("focus-hooks-rules-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("focus/.focus-sessions")).unwrap();
            Self(Layout::new(root, harness))
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.project.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn read(&self, rel: &str) -> Option<String> {
            std::fs::read_to_string(self.0.project.join("focus/.focus-sessions").join(rel)).ok()
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    const AT: SystemTime = SystemTime::UNIX_EPOCH;

    #[test]
    fn the_claude_record_hashes_the_imported_files_and_marks_what_may_be_stale() {
        let tree = Tree::new("claude", Harness::Claude);
        tree.write(".claude/rules/focus.md", "<!-- stub -->\n@../../focus/active-memory/org.md\n@../../focus/active-memory/phases/ideation.md\n@../../../outside.md\nnot an import\n");
        tree.write("focus/spaces/default/memory/org.md", "# Org\n");
        let org = format!("{:x}", Sha256::digest(b"# Org\n"));
        record_rules_load(&tree.0, "s1", "startup", "default", false, AT);
        let record = tree.read("s1.rules-held.json").unwrap();
        assert_eq!(
            record,
            format!(
                "{{\"v\":1,\"harness\":\"claude\",\"source\":\"startup\",\"at\":\"1970-01-01T00:00:00Z\",\"space\":\"default\",\"refresh\":\"at-load\",\"files\":{{\"focus/spaces/default/memory/org.md\":\"{org}\",\"focus/spaces/default/memory/phases/ideation.md\":\"\"}}}}\n"
            )
        );
        // A resume with the same files is not stale; one after an edit is, as
        // are a fork, a re-pointed include and a source not known.
        record_rules_load(&tree.0, "s1", "resume", "default", false, AT);
        assert!(!tree.read("s1.rules-held.json").unwrap().contains("stale"));
        tree.write("focus/spaces/default/memory/org.md", "# Org 2\n");
        record_rules_load(&tree.0, "s1", "resume", "default", false, AT);
        assert!(
            tree.read("s1.rules-held.json")
                .unwrap()
                .ends_with("\"stale\":true}\n")
        );
        // A stale record stays stale through resumes; a fresh start clears it.
        record_rules_load(&tree.0, "s1", "resume", "default", false, AT);
        assert!(tree.read("s1.rules-held.json").unwrap().contains("stale"));
        record_rules_load(&tree.0, "s1", "startup", "default", false, AT);
        record_rules_load(&tree.0, "s1", "resume", "default", false, AT);
        assert!(!tree.read("s1.rules-held.json").unwrap().contains("stale"));
        for (source, changed) in [("fork", false), ("startup", true), ("other", false)] {
            record_rules_load(&tree.0, "s1", source, "default", changed, AT);
            assert!(
                tree.read("s1.rules-held.json")
                    .unwrap()
                    .contains("\"stale\":true"),
                "{source} {changed}"
            );
        }
        // No stub: a record with no files.
        std::fs::remove_file(tree.0.project.join(".claude/rules/focus.md")).unwrap();
        record_rules_load(&tree.0, "s1", "startup", "default", false, AT);
        assert!(
            tree.read("s1.rules-held.json")
                .unwrap()
                .contains("\"files\":{}}")
        );
        // A session that is not a session name records nothing.
        record_rules_load(&tree.0, "../s2", "startup", "default", false, AT);
        assert!(!tree.0.project.join("focus/s2.rules-held.json").exists());
    }

    #[test]
    fn a_host_with_no_proven_copy_gets_no_record() {
        for harness in [Harness::Cursor, Harness::Copilot] {
            let tree = Tree::new(harness.name(), harness);
            tree.write("focus/.focus-sessions/s1.rules-held.json", "{\"v\":1}\n");
            record_rules_load(&tree.0, "s1", "startup", "default", false, AT);
            assert!(
                tree.read("s1.rules-held.json").is_none(),
                "{}",
                harness.name()
            );
        }
    }

    #[test]
    fn a_start_or_a_compaction_forgets_the_full_text_handed_to_the_thread() {
        let tree = Tree::new("delivery", Harness::Claude);
        // No record: nothing to forget, and a malformed one is removed.
        clear_rules_delivered(&tree.0, "s1");
        tree.write(
            "focus/.focus-sessions/s1.rules-delivered.json",
            "{\"v\":1,\"last\":\"a\"}\n",
        );
        clear_rules_delivered(&tree.0, "s1");
        assert!(tree.read("s1.rules-delivered.json").is_none());
        // A record with the full text: kept to its last bundle.
        tree.write(
            "focus/.focus-sessions/s1.rules-delivered.json",
            "{\"v\":2,\"last\":\"b\",\"full\":{\"bundle\":\"b\",\"at\":\"x\"},\"persona\":{\"sha256\":\"p\",\"at\":\"x\"}}\n",
        );
        clear_rules_delivered(&tree.0, "s1");
        assert_eq!(
            tree.read("s1.rules-delivered.json").as_deref(),
            Some("{\"v\":2,\"last\":\"b\"}\n")
        );
        // One already without: left as it is.
        tree.write(
            "focus/.focus-sessions/s1.rules-delivered.json",
            "{\"v\":2,\"last\":\"c\",\"x\":1}\n",
        );
        record_rules_load(&tree.0, "s1", "startup", "default", false, AT);
        assert_eq!(
            tree.read("s1.rules-delivered.json").as_deref(),
            Some("{\"v\":2,\"last\":\"c\",\"x\":1}\n")
        );
        clear_rules_delivered(&tree.0, "../s1");
    }
}

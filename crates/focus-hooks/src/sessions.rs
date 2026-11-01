//! What a hook writes about a conversation beyond its binding: the intent
//! stamp (`.focus-sessions/<id>`), and the one-shot handoff receipt a
//! session leaves when it moves from one record to another. Per-user
//! runtime state, written best-effort, as upstream's `writeSessionIntentUuid`,
//! `clearSessionIntentUuid` and `writeSessionIntentHandoff` are.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use focus_js::json;
use focus_workspace::Layout;
use focus_workspace::names::SESSIONS_DIR;
use focus_workspace::records::valid_session_id;
use serde_json::json;

fn sessions_dir(layout: &Layout) -> PathBuf {
    layout.workspace_root().join(SESSIONS_DIR)
}

/// `sessionRecordPath`: where a session's stamp lives; `None` for an id
/// that is not a session name.
fn record_path(layout: &Layout, session_id: &str) -> Option<PathBuf> {
    valid_session_id(Some(session_id)).map(|valid| sessions_dir(layout).join(valid))
}

/// `writeSessionIntentUuid`: stamp the session to a record's uuid.
pub fn write_session_intent_uuid(layout: &Layout, session_id: &str, uuid: &str) {
    let Some(path) = record_path(layout, session_id) else {
        return;
    };
    if uuid.is_empty() {
        return;
    }
    if std::fs::create_dir_all(sessions_dir(layout)).is_err() {
        return;
    }
    let _ = std::fs::write(path, format!("{uuid}\n"));
}

/// `clearSessionIntentUuid`: take the stamp away when the conversation
/// deliberately goes on with a record that has no uuid.
pub fn clear_session_intent_uuid(layout: &Layout, session_id: &str) {
    if let Some(path) = record_path(layout, session_id) {
        let _ = std::fs::remove_file(path);
    }
}

/// `writeSessionIntentHandoff`: the exact boundary a session crossed by
/// creating a second record or switching to another. Transient and
/// one-shot: the Stop hook reads it and clears it.
pub fn write_session_intent_handoff(
    layout: &Layout,
    session_id: &str,
    from_uuid: &str,
    to_uuid: &str,
    via_switch: bool,
    now: SystemTime,
) {
    let Some(path) = record_path(layout, session_id) else {
        return;
    };
    if from_uuid.is_empty() || to_uuid.is_empty() || from_uuid == to_uuid {
        return;
    }
    if std::fs::create_dir_all(sessions_dir(layout)).is_err() {
        return;
    }
    let issued = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    let mut receipt = json!({
        "fromIntentUuid": from_uuid,
        "toIntentUuid": to_uuid,
        "issuedAtMs": issued,
    });
    if via_switch {
        receipt["via"] = json!("switch");
    }
    let path = PathBuf::from(format!("{}.handoff.json", path.display()));
    let _ = std::fs::write(path, format!("{}\n", json::stringify(&receipt)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "focus-hooks-sessions-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(Layout::new(root, Harness::Claude))
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

    #[test]
    fn the_stamp_is_written_and_cleared_for_a_session_name() {
        let tree = Tree::new("stamp");
        write_session_intent_uuid(&tree.0, "s1", "01a138fa-d37d-787d-af4a-966efbb0b6fd");
        assert_eq!(
            tree.read("s1").as_deref(),
            Some("01a138fa-d37d-787d-af4a-966efbb0b6fd\n")
        );
        write_session_intent_uuid(&tree.0, "s1", "");
        assert_eq!(
            tree.read("s1").as_deref(),
            Some("01a138fa-d37d-787d-af4a-966efbb0b6fd\n")
        );
        write_session_intent_uuid(&tree.0, "../s2", "x");
        assert!(!tree.0.project.join("focus/s2").exists());
        clear_session_intent_uuid(&tree.0, "s1");
        assert!(tree.read("s1").is_none());
        clear_session_intent_uuid(&tree.0, "s1");
    }

    #[test]
    fn a_handoff_names_two_different_records() {
        let tree = Tree::new("handoff");
        let now = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_791_562_962_123);
        write_session_intent_handoff(&tree.0, "s1", "a", "a", false, now);
        write_session_intent_handoff(&tree.0, "s1", "", "b", false, now);
        assert!(tree.read("s1.handoff.json").is_none());
        write_session_intent_handoff(&tree.0, "s1", "a", "b", false, now);
        assert_eq!(
            tree.read("s1.handoff.json").as_deref(),
            Some(
                "{\"fromIntentUuid\":\"a\",\"toIntentUuid\":\"b\",\"issuedAtMs\":1791562962123}\n"
            )
        );
        write_session_intent_handoff(&tree.0, "s1", "b", "c", true, now);
        assert_eq!(
            tree.read("s1.handoff.json").as_deref(),
            Some(
                "{\"fromIntentUuid\":\"b\",\"toIntentUuid\":\"c\",\"issuedAtMs\":1791562962123,\"via\":\"switch\"}\n"
            )
        );
    }
}

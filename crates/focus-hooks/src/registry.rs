//! What a hook reads of the intent registries beyond the selection: the
//! spaces, an intent by its uuid, the active intent's uuid, and the uuid a
//! session was stamped to. Mirrors `listSpaces`, `findIntentByUuid`,
//! `activeIntentUuid` and `readSessionIntentUuid` in `aidlc-lib.ts`.

use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_workspace::Layout;
use focus_workspace::names::{DEFAULT_SPACE, SESSIONS_DIR};
use focus_workspace::records::{
    active_intent, list_intent_dirs, read_intent_registry, row_names_record, valid_session_id,
};
use serde_json::Value;

/// `listSpaces`: the space directories and the default, sorted.
#[must_use]
pub fn list_spaces(layout: &Layout) -> Vec<String> {
    let mut names: Vec<String> = vec![DEFAULT_SPACE.to_owned()];
    if let Ok(entries) = std::fs::read_dir(layout.workspace_root().join("spaces")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names.sort_by(|a, b| cmp_utf16(a, b));
    names
}

/// An intent found by its uuid: its space and record directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub space: String,
    pub slug: String,
    pub dir_name: String,
}

/// `findIntentByUuid`: the first registry row with that uuid that has a
/// record directory, space by space.
#[must_use]
pub fn find_intent_by_uuid(layout: &Layout, uuid: &str) -> Option<Found> {
    if uuid.is_empty() {
        return None;
    }
    for space in list_spaces(layout) {
        let dirs = list_intent_dirs(layout, &space);
        for row in read_intent_registry(layout, &space) {
            if row.get("uuid").and_then(Value::as_str) != Some(uuid) {
                continue;
            }
            if let Some(dir_name) = dirs.iter().find(|dir| row_names_record(&row, dir)) {
                return Some(Found {
                    space,
                    slug: row
                        .get("slug")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    dir_name: dir_name.clone(),
                });
            }
        }
    }
    None
}

/// `listIntents(space).find(dirName)?.uuid`: the registry uuid of a record
/// that exists in the space.
#[must_use]
pub fn intent_uuid_of(layout: &Layout, space: &str, dir_name: &str) -> Option<String> {
    if !list_intent_dirs(layout, space)
        .iter()
        .any(|dir| dir == dir_name)
    {
        return None;
    }
    read_intent_registry(layout, space)
        .into_iter()
        .find(|row| row_names_record(row, dir_name))
        .and_then(|row| row.get("uuid").and_then(Value::as_str).map(str::to_owned))
}

/// `activeIntentUuid`: the uuid of the active intent of `space` (the active
/// space when none), from its registry row.
#[must_use]
pub fn active_intent_uuid(layout: &Layout, space: Option<&str>) -> Option<String> {
    let space = space.map_or_else(|| layout.active_space(), str::to_owned);
    let active = active_intent(layout, &space).ok().flatten()?;
    read_intent_registry(layout, &space)
        .into_iter()
        .find(|row| row_names_record(row, &active))
        .and_then(|row| row.get("uuid").and_then(Value::as_str).map(str::to_owned))
        .filter(|uuid| !uuid.is_empty())
}

/// `readSessionIntentUuid`: the uuid stamped in `.focus-sessions/<id>`.
#[must_use]
pub fn session_intent_uuid(layout: &Layout, session_id: &str) -> Option<String> {
    let valid = valid_session_id(Some(session_id))?;
    let path = layout.workspace_root().join(SESSIONS_DIR).join(valid);
    let raw = std::fs::read_to_string(path).ok()?;
    let raw = trim(&raw);
    (!raw.is_empty()).then(|| raw.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    const ROW: &str = "{\"uuid\":\"01a138fa-d37d-787d-af4a-966efbb0b6fd\",\"slug\":\"add-login\",\"dirName\":\"260105-add-login\",\"scope\":\"feature\",\"status\":\"in-flight\"}";

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "focus-hooks-registry-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(Layout::new(root, Harness::Claude))
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.project.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    #[test]
    fn the_spaces_are_the_default_and_the_directories_sorted() {
        let tree = Tree::new("spaces");
        assert_eq!(list_spaces(&tree.0), ["default"]);
        tree.write("focus/spaces/payments/intents/intents.json", "[]");
        tree.write("focus/spaces/Billing/intents/intents.json", "[]");
        tree.write("focus/spaces/default/intents/intents.json", "[]");
        tree.write("focus/spaces/not-a-dir", "");
        assert_eq!(list_spaces(&tree.0), ["Billing", "default", "payments"]);
    }

    #[test]
    fn an_intent_is_found_by_uuid_when_its_record_exists() {
        let tree = Tree::new("uuid");
        tree.write(
            "focus/spaces/default/intents/intents.json",
            &format!("[{ROW}]"),
        );
        assert_eq!(
            find_intent_by_uuid(&tree.0, "01a138fa-d37d-787d-af4a-966efbb0b6fd"),
            None
        );
        tree.write(
            "focus/spaces/default/intents/260105-add-login/focus-state.md",
            "x",
        );
        assert_eq!(
            find_intent_by_uuid(&tree.0, "01a138fa-d37d-787d-af4a-966efbb0b6fd"),
            Some(Found {
                space: "default".to_owned(),
                slug: "add-login".to_owned(),
                dir_name: "260105-add-login".to_owned()
            })
        );
        assert_eq!(
            find_intent_by_uuid(&tree.0, "01a138fa-0000-787d-af4a-966efbb0b6fd"),
            None
        );
        assert_eq!(find_intent_by_uuid(&tree.0, ""), None);
        // Another space's row is found in its own space.
        tree.write(
            "focus/spaces/payments/intents/intents.json",
            &format!(
                "[{}]",
                ROW.replace("d37d", "ffff").replace("add-login", "pay")
            ),
        );
        tree.write(
            "focus/spaces/payments/intents/260105-pay/focus-state.md",
            "x",
        );
        assert_eq!(
            find_intent_by_uuid(&tree.0, "01a138fa-ffff-787d-af4a-966efbb0b6fd")
                .map(|found| found.space)
                .as_deref(),
            Some("payments")
        );
    }

    #[test]
    fn the_active_intents_uuid_and_a_sessions_stamp_are_read() {
        let tree = Tree::new("active");
        tree.write(
            "focus/spaces/default/intents/intents.json",
            &format!("[{ROW}]"),
        );
        tree.write(
            "focus/spaces/default/intents/260105-add-login/focus-state.md",
            "x",
        );
        // A lone record is the active one without a cursor; a second makes
        // the cursor decide.
        assert_eq!(
            active_intent_uuid(&tree.0, None).as_deref(),
            Some("01a138fa-d37d-787d-af4a-966efbb0b6fd")
        );
        tree.write(
            "focus/spaces/default/intents/260106-other/focus-state.md",
            "x",
        );
        assert_eq!(active_intent_uuid(&tree.0, None), None);
        tree.write(
            "focus/spaces/default/intents/active-intent",
            "260105-add-login\n",
        );
        assert_eq!(
            active_intent_uuid(&tree.0, None).as_deref(),
            Some("01a138fa-d37d-787d-af4a-966efbb0b6fd")
        );
        assert_eq!(
            active_intent_uuid(&tree.0, Some("default")).as_deref(),
            Some("01a138fa-d37d-787d-af4a-966efbb0b6fd")
        );
        assert_eq!(active_intent_uuid(&tree.0, Some("payments")), None);
        // A flat registry row has no uuid.
        tree.write(
            "focus/spaces/default/intents/intents.json",
            &format!(
                "[{}]",
                ROW.replace("\"uuid\":\"01a138fa-d37d-787d-af4a-966efbb0b6fd\",", "")
            ),
        );
        assert_eq!(active_intent_uuid(&tree.0, None), None);

        assert_eq!(session_intent_uuid(&tree.0, "s1"), None);
        tree.write(
            "focus/.focus-sessions/s1",
            "  01a138fa-d37d-787d-af4a-966efbb0b6fd \n",
        );
        assert_eq!(
            session_intent_uuid(&tree.0, "s1").as_deref(),
            Some("01a138fa-d37d-787d-af4a-966efbb0b6fd")
        );
        tree.write("focus/.focus-sessions/s2", " \n");
        assert_eq!(session_intent_uuid(&tree.0, "s2"), None);
        assert_eq!(session_intent_uuid(&tree.0, "../s1"), None);
    }
}

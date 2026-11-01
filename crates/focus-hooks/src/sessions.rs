//! What a hook writes about a conversation beyond its binding: the intent
//! stamp (`.focus-sessions/<id>`), and the one-shot handoff receipt a
//! session leaves when it moves from one record to another. Per-user
//! runtime state, written best-effort, as upstream's `writeSessionIntentUuid`,
//! `clearSessionIntentUuid` and `writeSessionIntentHandoff` are.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use focus_audit::time::iso_timestamp;
use focus_js::json;
use focus_workspace::Layout;
use focus_workspace::atomic::write_atomic;
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

/// `isBindableIntentRecordName`: a record name a binding may carry.
#[must_use]
pub fn is_bindable_record_name(name: &str) -> bool {
    !name.is_empty()
        && focus_js::text::trim(name) == name
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !(cfg!(windows) && name.contains('\\'))
        && name.chars().all(|ch| {
            let code = u32::from(ch);
            code >= 0x20 && code != 0x7f && !(0x80..=0x9f).contains(&code)
        })
}

/// `isSafeIntentRecordName`: a record name a command may carry.
#[must_use]
pub fn is_safe_record_name(name: &str) -> bool {
    focus_js::re::test(r"^[A-Za-z0-9][A-Za-z0-9._-]*$", "", name) && name != "." && name != ".."
}

/// `writeSessionBinding`: pin a session to one space and an optional
/// record, with how the record was chosen. Best effort; nothing for a
/// space or a record name a binding cannot carry.
pub fn write_session_binding(
    layout: &Layout,
    session_id: &str,
    space: &str,
    intent: Option<&str>,
    source: Option<&str>,
    at: SystemTime,
) {
    let Some(id) = valid_session_id(Some(session_id)) else {
        return;
    };
    if !focus_workspace::project::is_space_name(space)
        || intent.is_some_and(|name| !is_bindable_record_name(name))
    {
        return;
    }
    if std::fs::create_dir_all(sessions_dir(layout)).is_err() {
        return;
    }
    let mut binding = json!({ "space": space, "intent": intent, "boundAt": iso_timestamp(at) });
    if let Some(source) = source {
        binding["source"] = json!(source);
    }
    let _ = std::fs::write(
        sessions_dir(layout).join(format!("{id}.binding.json")),
        format!("{}\n", json::stringify(&binding)),
    );
}

/// `writeCurrentSessionId`: the live conversation, for a creation that
/// runs before any binding exists.
pub fn write_current_session_id(layout: &Layout, session_id: &str) {
    if valid_session_id(Some(session_id)).is_none()
        || std::fs::create_dir_all(sessions_dir(layout)).is_err()
    {
        return;
    }
    let _ = std::fs::write(
        sessions_dir(layout).join(".current-session"),
        format!("{session_id}\n"),
    );
}

/// `readSessionRebindOffer`: the signature of the offer this session was
/// last made, so it is made once.
#[must_use]
pub fn read_rebind_offer(layout: &Layout, session_id: &str) -> Option<String> {
    let path = record_path(layout, session_id)?;
    let raw = std::fs::read_to_string(format!("{}.rebind-offer", path.display())).ok()?;
    let trimmed = focus_js::text::trim(&raw);
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// `writeSessionRebindOffer`.
pub fn write_rebind_offer(layout: &Layout, session_id: &str, signature: &str) {
    let Some(path) = record_path(layout, session_id) else {
        return;
    };
    if signature.is_empty() || std::fs::create_dir_all(sessions_dir(layout)).is_err() {
        return;
    }
    let _ = std::fs::write(
        format!("{}.rebind-offer", path.display()),
        format!("{signature}\n"),
    );
}

/// `clearSessionRebindOffer`.
pub fn clear_rebind_offer(layout: &Layout, session_id: &str) {
    if let Some(path) = record_path(layout, session_id) {
        let _ = std::fs::remove_file(format!("{}.rebind-offer", path.display()));
    }
}

/// `writeSessionSelectionNotice`: the line a host with no context channel
/// shows on the conversation's next directive, true while the chat stays
/// on the work it is on now.
pub fn write_selection_notice(layout: &Layout, session_id: &str, line: &str) {
    let Some(path) = record_path(layout, session_id) else {
        return;
    };
    if line.is_empty() {
        return;
    }
    let binding = focus_workspace::records::read_session_binding(layout, session_id);
    if std::fs::create_dir_all(sessions_dir(layout)).is_err() {
        return;
    }
    let notice = json!({
        "line": line,
        "space": binding.as_ref().map(|b| b.space.clone()),
        "intent": binding.as_ref().and_then(|b| b.intent.clone()),
    });
    let _ = std::fs::write(
        format!("{}.selection-notice", path.display()),
        format!("{}\n", json::stringify(&notice)),
    );
}

/// `writeCurrentTranscriptPath`: the live transcript, for the tools that
/// cannot see it; a session-scoped pointer and a convenience one. Nothing
/// with the usage tracking switched off, or no path.
pub fn write_current_transcript_path(layout: &Layout, session_id: &str, transcript_path: &str) {
    if transcript_path.is_empty() || focus_workspace::env::is_on("DISABLE_USAGE_TRACKING") {
        return;
    }
    // `usageTrackingDisabled` reads the settings files; a file it refuses
    // throws out of the write, which the session start swallows.
    let places = focus_workspace::settings::Places::of(&layout.project);
    match places.flag(
        &focus_workspace::env::full_name("DISABLE_USAGE_TRACKING"),
        &|name| std::env::var(name).ok(),
    ) {
        Ok(Some(on)) if on == "1" => return,
        Ok(_) => {}
        Err(_) => return,
    }
    let dir = sessions_dir(layout);
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let segment = session_id_segment(session_id);
    if !segment.is_empty() {
        let _ = write_atomic(
            &dir.join(format!("{segment}.transcript")),
            transcript_path.as_bytes(),
        );
    }
    let _ = write_atomic(&dir.join("current.transcript"), transcript_path.as_bytes());
}

/// `safeSessionSegment` (the usage ledger's): the session id with every
/// run of other characters a dash, and no dash at either end; unlike the
/// runtime segment it is not cut at 96.
fn session_id_segment(session_id: &str) -> String {
    let segment = focus_js::re::replace(r"[^A-Za-z0-9._-]+", "g", session_id, "-");
    focus_js::re::replace(r"^-+|-+$", "g", &segment, "")
}

/// The plan-approval runtime records of a session: `presence-bypass-<id>`
/// and `plan-approval-bypass-<id>` under `.focus-sessions/plan-approval/`,
/// written when the harness launched with the switch.
fn plan_approval_dir(layout: &Layout) -> PathBuf {
    sessions_dir(layout).join("plan-approval")
}

/// `recordSessionPresenceBypass` / `recordSessionPlanApprovalBypass`.
///
/// # Errors
///
/// A link is on the way to the directory, or the record cannot be written.
pub fn record_bypass(
    layout: &Layout,
    kind: &str,
    session_id: &str,
    at: SystemTime,
) -> focus_js::Result<()> {
    let segment = focus_intent::create::runtime_session_segment(session_id);
    if segment.is_empty() {
        focus_js::bail!("Session {kind} bypass requires a nonblank session");
    }
    focus_workspace::observer::refuse_write("writeFileAtomic")?;
    let dir = plan_approval_dir(layout);
    focus_workspace::links::refuse_link_in_chain(
        &layout.project,
        dir.strip_prefix(&layout.project).unwrap_or(&dir),
    )?;
    std::fs::create_dir_all(&dir)
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", dir.display())))?;
    let path = dir.join(format!("{kind}-bypass-{segment}"));
    write_atomic(&path, format!("{}\n", iso_timestamp(at)).as_bytes())
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))
}

/// `clearSessionPlanApprovalBypass`.
pub fn clear_plan_approval_bypass(layout: &Layout, session_id: &str) {
    let segment = focus_intent::create::runtime_session_segment(session_id);
    if segment.is_empty() {
        return;
    }
    let _ = std::fs::remove_file(
        plan_approval_dir(layout).join(format!("plan-approval-bypass-{segment}")),
    );
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

    #[test]
    fn record_names_a_binding_and_a_command_may_carry() {
        assert!(is_bindable_record_name("260105-add-login"));
        assert!(is_bindable_record_name("with space"));
        assert!(!is_bindable_record_name(""));
        assert!(!is_bindable_record_name(" padded"));
        assert!(!is_bindable_record_name(".."));
        assert!(!is_bindable_record_name("a/b"));
        assert!(!is_bindable_record_name("tab\there"));
        assert!(is_safe_record_name("260105-add-login"));
        assert!(!is_safe_record_name("with space"));
        assert!(!is_safe_record_name("-leading"));
        assert!(!is_safe_record_name(".."));
    }

    #[test]
    fn a_binding_names_the_space_the_record_and_how_it_was_chosen() {
        let tree = Tree::new("binding");
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_767_607_200_000);
        write_session_binding(
            &tree.0,
            "s1",
            "default",
            Some("260105-add-login"),
            Some("create"),
            at,
        );
        assert_eq!(
            tree.read("s1.binding.json").as_deref(),
            Some(
                "{\"space\":\"default\",\"intent\":\"260105-add-login\",\"boundAt\":\"2026-01-05T10:00:00Z\",\"source\":\"create\"}\n"
            )
        );
        write_session_binding(&tree.0, "s2", "default", None, None, at);
        assert_eq!(
            tree.read("s2.binding.json").as_deref(),
            Some("{\"space\":\"default\",\"intent\":null,\"boundAt\":\"2026-01-05T10:00:00Z\"}\n")
        );
        // A space or a record name a binding cannot carry, or a session name
        // that is not one: nothing.
        write_session_binding(&tree.0, "s3", "Bad Space", None, None, at);
        write_session_binding(&tree.0, "s4", "default", Some("../x"), None, at);
        write_session_binding(&tree.0, "../s5", "default", None, None, at);
        assert!(tree.read("s3.binding.json").is_none());
        assert!(tree.read("s4.binding.json").is_none());
        assert!(!tree.0.project.join("focus/s5.binding.json").exists());
    }

    #[test]
    fn the_live_session_and_its_rebind_offer_are_recorded_by_name() {
        let tree = Tree::new("live");
        write_current_session_id(&tree.0, "s1");
        assert_eq!(tree.read(".current-session").as_deref(), Some("s1\n"));
        write_current_session_id(&tree.0, "../s1");
        assert_eq!(tree.read(".current-session").as_deref(), Some("s1\n"));
        assert_eq!(read_rebind_offer(&tree.0, "s1"), None);
        write_rebind_offer(&tree.0, "s1", "");
        assert!(tree.read("s1.rebind-offer").is_none());
        write_rebind_offer(&tree.0, "s1", "rebind:default:a:b");
        assert_eq!(
            read_rebind_offer(&tree.0, "s1").as_deref(),
            Some("rebind:default:a:b")
        );
        clear_rebind_offer(&tree.0, "s1");
        assert_eq!(read_rebind_offer(&tree.0, "s1"), None);
        write_selection_notice(&tree.0, "s1", "");
        assert!(tree.read("s1.selection-notice").is_none());
        write_selection_notice(&tree.0, "s1", "Now on x.");
        assert_eq!(
            tree.read("s1.selection-notice").as_deref(),
            Some("{\"line\":\"Now on x.\",\"space\":null,\"intent\":null}\n")
        );
    }

    #[test]
    fn the_transcript_pointers_follow_the_usage_kill_switch() {
        let tree = Tree::new("transcript");
        write_current_transcript_path(&tree.0, "s1", "");
        assert!(tree.read("current.transcript").is_none());
        write_current_transcript_path(&tree.0, "s 1/x", "/tmp/t.jsonl");
        assert_eq!(
            tree.read("s-1-x.transcript").as_deref(),
            Some("/tmp/t.jsonl")
        );
        assert_eq!(
            tree.read("current.transcript").as_deref(),
            Some("/tmp/t.jsonl")
        );
        // A settings file the resolution refuses throws out of upstream's
        // write; one that records the switch turns it off.
        let local = tree.0.project.join("focus.settings.local.json");
        std::fs::write(&local, "{\"flags\": {}}\n").unwrap();
        write_current_transcript_path(&tree.0, "s2", "/tmp/u.jsonl");
        assert!(tree.read("s2.transcript").is_none());
        assert_eq!(
            tree.read("current.transcript").as_deref(),
            Some("/tmp/t.jsonl")
        );
        std::fs::write(
            &local,
            "{\"schemaVersion\": 1, \"flags\": {\"schemaVersion\": 1, \"bypasses\": [\"FOCUS_DISABLE_USAGE_TRACKING\"]}}\n",
        )
        .unwrap();
        write_current_transcript_path(&tree.0, "s3", "/tmp/v.jsonl");
        assert!(tree.read("s3.transcript").is_none());
    }

    #[test]
    fn a_bypass_is_dated_by_kind_and_cleared_by_name() {
        let tree = Tree::new("bypass");
        let at = SystemTime::UNIX_EPOCH + std::time::Duration::from_millis(1_767_607_200_000);
        assert!(record_bypass(&tree.0, "presence", "  ", at).is_err());
        record_bypass(&tree.0, "presence", "s1", at).unwrap();
        record_bypass(&tree.0, "plan-approval", "s1", at).unwrap();
        assert_eq!(
            tree.read("plan-approval/presence-bypass-s1").as_deref(),
            Some("2026-01-05T10:00:00Z\n")
        );
        assert_eq!(
            tree.read("plan-approval/plan-approval-bypass-s1")
                .as_deref(),
            Some("2026-01-05T10:00:00Z\n")
        );
        clear_plan_approval_bypass(&tree.0, "s1");
        assert!(tree.read("plan-approval/plan-approval-bypass-s1").is_none());
        assert!(tree.read("plan-approval/presence-bypass-s1").is_some());
        clear_plan_approval_bypass(&tree.0, "s1");
    }
}

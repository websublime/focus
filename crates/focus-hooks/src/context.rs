//! Which workflow a hook fires in, and whether this conversation has joined
//! it: `enterHookWorkflow`, `workflowParticipation`, `hookStandsOutside`
//! and `hookOutsideGate` in `aidlc-lib.ts`.
//!
//! Resolution (which record the path helpers name) keeps the lone-record
//! fallback, because worktrees inherit no cursor; participation asks for
//! local evidence that this conversation chose the record. A lone committed
//! record is not such evidence: in a fresh clone it is a teammate's.

use focus_js::{Result, bail};
use focus_workspace::Layout;
use focus_workspace::names::UNIT_SCOPE_FILE;
use focus_workspace::records::{
    Invoker, Selection, Selector, read_active_intent_cursor, read_session_binding,
    resolve_selection, valid_session_id,
};

/// `TRUSTED_BINDING_SOURCES`: the sources that record the conversation's own
/// choice of a record.
const TRUSTED_BINDING_SOURCES: [&str; 6] = [
    "create",
    "migration",
    "switch",
    "space-switch-cursor",
    "cursor",
    "stamp",
];

/// `isTrustedBindingSource`.
#[must_use]
pub fn is_trusted_binding_source(source: &str) -> bool {
    TRUSTED_BINDING_SOURCES.contains(&source)
}

/// `WorkflowParticipation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Participation {
    Participant,
    Outsider,
    Indeterminate,
}

/// What `enterHookWorkflow` answers: the selection, how far this
/// conversation is in it, and the invoker every later path helper resolves
/// with (a payload session that has a binding is pinned as the session
/// override, so the helpers resolve the record the hook classified).
#[derive(Debug, Clone)]
pub struct HookWorkflow {
    pub selection: Option<Selection>,
    pub participation: Participation,
    pub invoker: Invoker,
}

/// `workflowParticipation`: whether this conversation's hooks may write
/// into, or enforce, the record a selection names.
///
/// # Errors
///
/// The checkout carries a unit scope stamp, which claims a unit of work for
/// it (M4's units): Focus does not read it yet, and refuses rather than
/// decide without it.
pub fn participation(layout: &Layout, selection: &Selection) -> Result<Participation> {
    let Some(intent) = &selection.intent else {
        return Ok(Participation::Outsider);
    };
    let cursor_names_it =
        read_active_intent_cursor(layout, &selection.space).as_deref() == Some(intent.as_str());
    if let Some(binding) = &selection.binding
        && binding.space == selection.space
        && binding.intent.as_deref() == Some(intent.as_str())
        && binding
            .source
            .as_deref()
            .is_some_and(|source| TRUSTED_BINDING_SOURCES.contains(&source))
    {
        return Ok(Participation::Participant);
    }
    // A Unit claimed on this machine names its space and intent.
    let stamp = layout.workspace_root().join(UNIT_SCOPE_FILE);
    if stamp.exists() {
        bail!(
            "{} claims a unit of work for this checkout, which would make this conversation a participant in that unit's record: Focus does not read unit claims yet.",
            stamp.display()
        );
    }
    // The per-user cursor naming the record: covers bindings written before
    // sources were recorded, creations observed after the fact, and sessions
    // that reach hooks without a SessionStart binding.
    Ok(if cursor_names_it {
        Participation::Participant
    } else {
        Participation::Outsider
    })
}

/// The invoker every later path helper resolves with once the hook's own
/// payload named the session: what `enterHookWorkflow` exports for the
/// helpers to read.
#[must_use]
pub fn pinned(session_id: &str) -> Invoker {
    Invoker {
        session_override: Some(session_id.to_owned()),
        from_payload: true,
    }
}

/// `enterHookWorkflow`: the workflow a hook acts on, with a bound payload
/// session pinned as the session override. A session that cannot be
/// resolved leaves the hook outside, as a resolution failure does upstream.
///
/// # Errors
///
/// A unit scope stamp is present (see [`participation`]); the resolution
/// refuses by name (a delegated worktree, sessions recorded by process:
/// neither is ported).
pub fn enter(layout: &Layout, payload_session: Option<&str>) -> Result<HookWorkflow> {
    let session_id = payload_session.and_then(|id| valid_session_id(Some(id)).map(str::to_owned));
    let mut invoker = Invoker::from_env();
    if let Some(id) = &session_id
        && read_session_binding(layout, id).is_some()
    {
        invoker = pinned(id);
    }
    match resolve_selection(layout, &Selector::default(), &invoker) {
        Ok(selection) => {
            let participation = participation(layout, &selection)?;
            Ok(HookWorkflow {
                selection: Some(selection),
                participation,
                invoker,
            })
        }
        // Upstream stands the hook outside whatever the resolution threw;
        // a refusal by name (a delegated worktree, sessions recorded by
        // process) is said instead, as the rule for input not ported.
        Err(err) if crate::hooks::is_refusal(&err) => Err(err),
        Err(_) => Ok(HookWorkflow {
            selection: None,
            participation: Participation::Indeterminate,
            invoker,
        }),
    }
}

impl HookWorkflow {
    /// `hookStandsOutside`: the selection names a record this conversation
    /// has not joined, or participation cannot be decided. A selection with
    /// no record (a cold workspace) is not outside: there is nothing to
    /// protect.
    #[must_use]
    pub fn stands_outside(&self) -> bool {
        let Some(selection) = &self.selection else {
            return true;
        };
        if selection.intent.is_some() {
            return self.participation != Participation::Participant;
        }
        // Bound to no record because the record it found was not joined:
        // still outside that record.
        selection
            .binding
            .as_ref()
            .and_then(|binding| binding.source.as_deref())
            == Some("unjoined")
    }

    /// `hookOutsideGate`: whether a gate may stand aside for this
    /// conversation: only when the selection resolved and the conversation
    /// is known not to have joined it.
    #[must_use]
    pub fn outside_gate(&self) -> bool {
        if self.selection.is_none() || self.participation == Participation::Indeterminate {
            return false;
        }
        self.stands_outside()
    }
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
                .join(format!("focus-hooks-context-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            let tree = Self(Layout::new(root, Harness::Claude));
            tree.write(&format!("{RECORD}/focus-state.md"), "x");
            tree.write("focus/spaces/default/intents/intents.json", "[]");
            tree
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.project.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn bind(&self, source: &str) {
            self.write(
                "focus/.focus-sessions/s1.binding.json",
                &format!("{{\"space\":\"default\",\"intent\":\"260105-add-login\",\"boundAt\":\"2026-01-05T10:00:00Z\",\"source\":\"{source}\"}}"),
            );
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    #[test]
    fn participation_is_the_cursor_or_a_trusted_binding() {
        let tree = Tree::new("participation");
        // No cursor, no binding: the lone record resolves (a worktree
        // inherits no cursor), but it is a teammate's for all this
        // conversation shows.
        let outside = enter(&tree.0, Some("s1")).unwrap();
        assert_eq!(outside.participation, Participation::Outsider);
        assert!(outside.stands_outside());
        assert!(outside.outside_gate());
        assert_eq!(
            outside.selection.as_ref().and_then(|s| s.intent.as_deref()),
            Some("260105-add-login")
        );
        // The cursor names it.
        tree.write(
            "focus/spaces/default/intents/active-intent",
            "260105-add-login\n",
        );
        let joined = enter(&tree.0, None).unwrap();
        assert_eq!(joined.participation, Participation::Participant);
        assert!(!joined.stands_outside());
        assert_eq!(joined.invoker, Invoker::default());
        // A binding pins the session for the helpers; its source decides.
        std::fs::remove_file(
            tree.0
                .project
                .join("focus/spaces/default/intents/active-intent"),
        )
        .unwrap();
        for source in [
            "create",
            "migration",
            "switch",
            "space-switch-cursor",
            "cursor",
            "stamp",
        ] {
            tree.bind(source);
            let entered = enter(&tree.0, Some("s1")).unwrap();
            assert_eq!(
                entered.participation,
                Participation::Participant,
                "{source}"
            );
            assert_eq!(entered.invoker, pinned("s1"));
        }
        // A binding that names the record by a source that is not the
        // conversation's own choice, with the cursor elsewhere: outside.
        tree.write("focus/spaces/default/intents/other/focus-state.md", "x");
        tree.write("focus/spaces/default/intents/active-intent", "other\n");
        tree.bind("worktree");
        let entered = enter(&tree.0, Some("s1")).unwrap();
        assert_eq!(
            entered.selection.as_ref().and_then(|s| s.intent.as_deref()),
            Some("260105-add-login")
        );
        assert_eq!(entered.participation, Participation::Outsider);
        std::fs::remove_file(
            tree.0
                .project
                .join("focus/spaces/default/intents/active-intent"),
        )
        .unwrap();
        // Participation is asked of the selection as named: a binding to
        // another record does not make this one joined.
        tree.bind("create");
        let elsewhere = Selection {
            space: "default".to_owned(),
            intent: Some("other".to_owned()),
            session_id: Some("s1".to_owned()),
            binding: read_session_binding(&tree.0, "s1"),
        };
        assert!(elsewhere.binding.is_some());
        assert_eq!(
            participation(&tree.0, &elsewhere).unwrap(),
            Participation::Outsider
        );
        for source in ["worktree", "none"] {
            tree.bind(source);
            assert_eq!(
                enter(&tree.0, Some("s1")).unwrap().participation,
                Participation::Outsider,
                "{source}"
            );
        }
        // A session that is not a session name is no session.
        tree.bind("create");
        assert_eq!(
            enter(&tree.0, Some("../s1")).unwrap().participation,
            Participation::Outsider
        );
    }

    #[test]
    fn a_unit_claim_is_refused_by_name_unless_the_binding_settles_it() {
        let tree = Tree::new("claim");
        tree.write(
            "focus/spaces/default/intents/active-intent",
            "260105-add-login\n",
        );
        tree.write("focus/.focus-unit-scope.json", "{}");
        let err = enter(&tree.0, None).unwrap_err().to_string();
        assert!(
            err.contains("focus-unit-scope.json claims a unit of work for this checkout"),
            "{err}"
        );
        // With the cursor elsewhere, or gone: the claim could still name
        // this record, so it is refused rather than read as outside.
        tree.write("focus/spaces/default/intents/active-intent", "other\n");
        assert!(enter(&tree.0, None).is_err());
        std::fs::remove_file(
            tree.0
                .project
                .join("focus/spaces/default/intents/active-intent"),
        )
        .unwrap();
        assert!(enter(&tree.0, None).is_err());
        tree.bind("create");
        assert_eq!(
            enter(&tree.0, Some("s1")).unwrap().participation,
            Participation::Participant
        );
    }

    #[test]
    fn a_resolution_not_ported_is_refused_and_any_other_failure_stands_outside() {
        let tree = Tree::new("resolution");
        tree.write("focus/.focus-sessions/pids/1.json", "{}");
        let err = enter(&tree.0, None).unwrap_err().to_string();
        assert!(
            err.contains("records which process belongs to which conversation"),
            "{err}"
        );
        // The payload's bound session settles it.
        tree.bind("create");
        assert_eq!(
            enter(&tree.0, Some("s1")).unwrap().participation,
            Participation::Participant
        );
        // A registry row that cannot be read: the resolution fails, and
        // the hook stands outside, indeterminate.
        std::fs::remove_dir_all(tree.0.project.join("focus/.focus-sessions")).unwrap();
        tree.write("focus/spaces/default/intents/intents.json", "[null]");
        let entered = enter(&tree.0, None).unwrap();
        assert_eq!(entered.participation, Participation::Indeterminate);
        assert!(entered.selection.is_none());
        assert!(entered.stands_outside());
    }
}

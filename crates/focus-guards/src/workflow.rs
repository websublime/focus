//! The guards of one workflow, read from its project.
//!
//! [`Guards::read`] gathers what a decision needs: the state text of the
//! record a command acts on, the scope's Guard Policy default, the memory
//! declarations of the space, and the settings the kill switches may be
//! recorded in. From there [`Guards::policy`], [`Guards::fences`] and
//! [`Guards::decide`] answer without touching the disk again.
//!
//! The authority behind the action is read off markers the hooks and the
//! orchestrator write (`active-directive.json`, the turn markers). Neither
//! exists yet in Focus, so a record that carries one is refused rather than
//! read with an authority that could be wrong; without them, the authority
//! is what upstream resolves with none: nothing covers the action.
//!
//! Mirrors `resolveGuardPolicy`'s reads, `scopeGuardPolicyDefault`,
//! `changeControlMemoryDir`, `authorityStateText` and `decideFence` in
//! `aidlc-lib.ts`.

use std::fs;
use std::path::{Path, PathBuf};

use focus_graph::guard_memory;
use focus_graph::scopes::{self, ScopeMetadata};
use focus_js::text::trim;
use focus_js::{Result, bail};
use focus_workspace::manifest::HarnessManifest;
use focus_workspace::names::{DEFAULT_SPACE, STATE_FILE};
use focus_workspace::records::{self, Invoker, Selector};
use focus_workspace::settings::Places;
use focus_workspace::{Layout, env};

use crate::authority::{Authority, Cover, FenceDecision, GrantSource, decide_fence};
use crate::fences::{Fence, FenceResolution, resolve_fences};
use crate::policy::{self, Declaration, GuardPolicy, Resolution};

/// The marker in a record's engine directory that carries the engine's
/// instruction and the turn sequence, written by the orchestrator (M5).
const DIRECTIVE_MARKER: &str = "active-directive.json";

/// What the guards of a workflow are decided from.
pub struct Guards {
    /// The state file's text, or empty when the record has none.
    pub state: String,
    pub state_path: PathBuf,
    pub scope_default: GuardPolicy,
    /// The space's memory directory, read for its declarations when the
    /// policy is resolved.
    pub memory_dir: PathBuf,
    places: Places,
    unattended: bool,
    /// The turn markers' times, when the record carries both as regular
    /// files: the person's last turn and the engine's last touch.
    turn_markers: Option<(std::time::SystemTime, std::time::SystemTime)>,
}

impl Guards {
    /// Read the guards of the record `selector` names in `layout`.
    ///
    /// # Errors
    ///
    /// The selection is one Focus cannot resolve yet, a scope file cannot be
    /// read, or the record carries an authority marker.
    pub fn read(layout: &Layout, selector: &Selector, invoker: &Invoker) -> Result<Self> {
        // Upstream resolves the selection for the state, and again for the
        // memory tree, taking the default space when it cannot; a selection
        // Focus does not resolve yet is refused rather than defaulted.
        if let Some(refusal) = records::not_ported(layout, selector, invoker) {
            return Err(refusal);
        }
        let selection = records::resolve_selection(layout, selector, invoker).ok();
        let space = selection
            .as_ref()
            .map_or(DEFAULT_SPACE, |selection| selection.space.as_str());
        let state_path = selection.as_ref().map_or_else(
            || layout.intents_dir(DEFAULT_SPACE).join(STATE_FILE),
            |selection| selection.docs_root(layout).join(STATE_FILE),
        );
        let state = read_text(&state_path).unwrap_or_default();
        let mut turn_markers = None;
        if let Some(selection) = &selection {
            let engine_dir = selection.engine_dir(layout);
            refuse_directive_marker(&engine_dir)?;
            turn_markers = read_turn_markers(&selection.docs_root(layout), &engine_dir);
        }
        Ok(Self {
            scope_default: scope_default(layout, &state)?,
            state,
            state_path,
            memory_dir: layout.memory_dir(space),
            places: Places::of(&layout.project),
            unattended: env::var("UNATTENDED").as_deref() == Some("1"),
            turn_markers,
        })
    }

    /// The guards decided from a state text handed in, as `decideFence` is
    /// given one by a caller that already read the file.
    #[must_use]
    pub fn with_state(mut self, state: &str) -> Self {
        self.state = state.to_owned();
        self
    }

    /// The workflow's Guard Policy.
    ///
    /// # Errors
    ///
    /// The state's policy line is malformed and not tolerated, or a memory
    /// file cannot be read or declares a mode that is not one of the three.
    pub fn policy(&self, tolerate_invalid_state: bool) -> Result<Resolution> {
        policy::resolve(
            &self.state,
            &self.state_path,
            self.scope_default,
            || self.declarations(),
            tolerate_invalid_state,
        )
    }

    /// `memoryGuardPolicyDeclarations`: every memory layer of the space that
    /// declares a mode.
    ///
    /// # Errors
    ///
    /// A memory file cannot be read or declares a mode that is not one of
    /// the three.
    pub fn declarations(&self) -> Result<Vec<Declaration>> {
        guard_memory::declarations(&self.memory_dir)
    }

    /// `guardPolicyAcceptsChanges`: whether this run records and announces a
    /// changed input instead of stopping on it. A policy that cannot be read
    /// counts as strict.
    #[must_use]
    pub fn accepts_changes(&self) -> bool {
        self.policy(false)
            .is_ok_and(|policy| policy.value != GuardPolicy::Strict)
    }

    /// The effective setting of every fence under `policy`.
    #[must_use]
    pub fn fences(&self, policy: &Resolution) -> Vec<FenceResolution> {
        // A kill switch is named in full (`FOCUS_…`), so the environment is
        // read as it is.
        let read = |name: &str| std::env::var(name).ok();
        let flag = |name: &str| self.places.flag(name, &read).ok().flatten();
        let source = |name: &str| self.places.kill_switch_source(name, &read);
        resolve_fences(policy, &self.state, &flag, &source)
    }

    /// The authority behind the action, read off the turn markers: a
    /// person's turn recorded after the engine's last touch is a grant; an
    /// unattended driver's prompt is not a person's message, even when an
    /// attended turn left fresh markers; otherwise nothing covers the action
    /// (see the module docs).
    #[must_use]
    pub fn authority(&self) -> Authority {
        let mut authority = Authority::without_markers(self.unattended);
        if !self.unattended
            && let Some((human, engine)) = self.turn_markers
            && human > engine
        {
            authority.covered = Cover::Grant;
            authority.grant = Some((GrantSource::TurnMarker, None));
        }
        authority
    }

    /// `decideFence`: the whole decision for one fence. An unreadable policy
    /// is the strictest policy, so the fence stays up.
    #[must_use]
    pub fn decide(&self, fence: Fence) -> FenceDecision {
        let policy = self.policy(true).unwrap_or_else(|_| Resolution {
            value: GuardPolicy::Strict,
            source: "not set".to_owned(),
            scope_default: GuardPolicy::Strict,
            intent: None,
            state_value: GuardPolicy::Strict,
            raw_state_value: None,
            state_field: None,
            conflict: None,
            memory_strict: None,
        });
        let fences = self.fences(&policy);
        let resolved = fences
            .iter()
            .find(|resolved| resolved.fence == fence)
            .expect("every fence is resolved");
        decide_fence(&policy, resolved, self.authority())
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn refuse_directive_marker(engine_dir: &Path) -> Result<()> {
    let path = engine_dir.join(DIRECTIVE_MARKER);
    if path.symlink_metadata().is_ok() {
        bail!(
            "{} records the authority behind what the agent is doing (a person's turn or the engine's own instruction), and Focus does not read that yet, so a guard cannot say whether this action is covered. Remove the marker or run this with the engine's own tools.",
            path.display()
        );
    }
    Ok(())
}

/// `turnMarkersShowConversational`'s reading of the markers: both present
/// as regular files, through no link, with their times; anything else
/// proves nothing. An absent engine marker is not "the engine was never
/// touched": it is also the shape of a fresh install.
fn read_turn_markers(
    record_root: &Path,
    engine_dir: &Path,
) -> Option<(std::time::SystemTime, std::time::SystemTime)> {
    let time_of = |name: &str| {
        let relative = Path::new(focus_workspace::names::ENGINE_DIR).join(name);
        focus_workspace::links::refuse_link_in_chain(record_root, &relative).ok()?;
        let meta = fs::symlink_metadata(engine_dir.join(name)).ok()?;
        if !meta.is_file() {
            return None;
        }
        meta.modified().ok()
    };
    Some((time_of("human-turn")?, time_of("engine-touch")?))
}

/// `scopeGuardPolicyDefault`: the default of the state's scope from the
/// enabled scope files, `off` for a scope that declares none and `strict`
/// for one that is not defined or when there is no scope. A scope is looked
/// up by its name lowercased, as upstream looks it up.
///
/// # Errors
///
/// A scope file cannot be read.
pub fn scope_default(layout: &Layout, state: &str) -> Result<GuardPolicy> {
    let Some(scope) = focus_state::get_field(state, "Scope") else {
        return Ok(GuardPolicy::Strict);
    };
    let Ok(enabled) = enabled_scopes(layout) else {
        // Upstream takes a mapping it cannot load as "not defined".
        return Ok(GuardPolicy::Strict);
    };
    let wanted = trim(scope).to_lowercase();
    let found = enabled.iter().find(|meta| meta.name == wanted);
    Ok(policy::scope_default(found.map(|meta| meta.guard_policy)))
}

fn enabled_scopes(layout: &Layout) -> Result<Vec<ScopeMetadata>> {
    let all = scopes::load_all(&layout.scopes_dir())?;
    let manifest = HarnessManifest::read(&layout.manifest())?;
    Ok(scopes::enabled(&all, &manifest))
}

/// What the guards decided from, for a reader.
impl std::fmt::Debug for Guards {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Guards")
            .field("state_path", &self.state_path)
            .field("scope_default", &self.scope_default)
            .field("memory_dir", &self.memory_dir)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use focus_harness::Harness;

    use super::*;
    use crate::authority::Decision;

    /// An installed Claude Code project, copied from the fixture, with one
    /// intent record.
    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("focus-guards-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/upstream");
            copy_dir(&fixtures.join("installed/claude"), &root);
            let tree = Self(Layout::new(root, Harness::Claude));
            tree.write(
                "focus/spaces/default/intents/active-intent",
                "add-login-1a2b3c4d\n",
            );
            tree.write(
                "focus/spaces/default/intents/add-login-1a2b3c4d/focus-state.md",
                "# State\n\n- **Scope**: feature\n",
            );
            tree
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.project.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        fn guards(&self) -> Result<Guards> {
            Guards::read(&self.0, &Selector::default(), &Invoker::default())
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.project);
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

    #[test]
    fn the_scope_default_comes_from_the_scope_files() {
        let tree = Tree::new("scopes");
        let guards = tree.guards().unwrap();
        assert_eq!(guards.scope_default, GuardPolicy::Off);
        assert!(
            guards
                .state_path
                .ends_with("add-login-1a2b3c4d/focus-state.md")
        );
        assert_eq!(guards.state, "# State\n\n- **Scope**: feature\n");
        let policy = guards.policy(false).unwrap();
        assert_eq!(
            (policy.value, policy.source.as_str()),
            (GuardPolicy::Strict, "not set")
        );
        // Enterprise declares strict; a scope with no file is strict too,
        // and the name is looked up lowercased.
        for (scope, wanted) in [
            ("enterprise", GuardPolicy::Strict),
            ("Enterprise", GuardPolicy::Strict),
            ("nope", GuardPolicy::Strict),
            ("Bugfix", GuardPolicy::Off),
            ("", GuardPolicy::Strict),
        ] {
            assert_eq!(
                scope_default(&tree.0, &format!("- **Scope**: {scope}\n")).unwrap(),
                wanted,
                "{scope}"
            );
        }
        // A space with no record: the state is empty and the policy strict.
        let bare = Tree::new("bare");
        fs::remove_dir_all(bare.0.project.join("focus/spaces/default/intents")).unwrap();
        let guards = bare.guards().unwrap();
        assert_eq!(guards.state, "");
        assert_eq!(
            guards.decide(Fence::StateTransition).decision,
            Decision::Hold
        );
    }

    #[test]
    fn a_fence_is_decided_from_the_state_the_memory_and_the_environment() {
        let tree = Tree::new("decide");
        tree.write(
            "focus/spaces/default/intents/add-login-1a2b3c4d/focus-state.md",
            "# State\n\n- **Scope**: feature\n- **Guard Policy**: off (from scope feature)\n- **Guards On**: state-transition (set by you)\n",
        );
        let guards = tree.guards().unwrap();
        let plan = guards.decide(Fence::PlanApproval);
        assert_eq!(plan.decision, Decision::StandAside);
        assert_eq!(plan.source, "guard policy off (from scope feature)");
        assert!(!plan.speaks());
        assert_eq!(plan.authority.covered.as_str(), "none");
        let state = guards.decide(Fence::StateTransition);
        assert_eq!(
            (state.decision, state.source.as_str()),
            (Decision::Hold, "set by you")
        );
        assert!(guards.accepts_changes());

        // The team's memory pins strict: every fence is up and the policy
        // names the file.
        tree.write(
            "focus/spaces/default/memory/team.md",
            "# Team\n\n## Guard Policy\n\nMode: strict\n",
        );
        let guards = tree.guards().unwrap();
        let policy = guards.policy(false).unwrap();
        assert_eq!(policy.source, "team.md");
        assert!(guards.decide(Fence::PlanApproval).decision == Decision::Hold);
        assert!(!guards.accepts_changes());
        // A memory file with a mode that is not one of the three refuses the
        // policy, and the decision falls to the strictest.
        tree.write(
            "focus/spaces/default/memory/team.md",
            "## Guard Policy\n\nMode: maybe\n",
        );
        let guards = tree.guards().unwrap();
        assert!(
            guards
                .policy(true)
                .unwrap_err()
                .message()
                .contains("Invalid Guard Policy Mode \"maybe\"")
        );
        assert_eq!(guards.decide(Fence::PlanApproval).decision, Decision::Hold);
        assert_eq!(
            guards.decide(Fence::PlanApproval).policy,
            GuardPolicy::Strict
        );

        // A state handed in replaces the one read.
        let handed = tree
            .guards()
            .unwrap()
            .with_state("- **Scope**: feature\n- **Guard Policy**: relaxed\n");
        fs::remove_file(tree.0.project.join("focus/spaces/default/memory/team.md")).unwrap();
        assert_eq!(
            handed.decide(Fence::ReviewFreeze).decision,
            Decision::StandAside
        );
        assert_eq!(
            handed.decide(Fence::StateTransition).decision,
            Decision::Hold
        );
    }

    /// Deliberate deviation. Upstream reads the engine's instruction and the
    /// turn sequence off the active-directive marker its orchestrator
    /// writes; Focus has no orchestrator yet, and refuses a record that
    /// carries the marker rather than decide with an authority it cannot
    /// read. The turn markers are read.
    #[test]
    fn a_record_with_an_authority_marker_is_refused() {
        let tree = Tree::new("markers");
        let engine = "focus/spaces/default/intents/add-login-1a2b3c4d/.focus-engine";
        tree.write(&format!("{engine}/active-directive.json"), "");
        let refused = tree.guards().unwrap_err();
        assert!(
            refused.message().contains("active-directive.json")
                && refused.message().contains("Focus does not read that yet"),
            "{refused}"
        );
        fs::remove_file(
            tree.0
                .project
                .join(format!("{engine}/active-directive.json")),
        )
        .unwrap();
        // The engine's touch alone covers nothing; a person's turn after it
        // is a grant; an unattended driver's is not.
        tree.write(&format!("{engine}/engine-touch"), "2026-01-01T00:00:00Z\n");
        let guards = tree.guards().unwrap();
        assert_eq!(guards.authority().covered, Cover::None);
        tree.write(&format!("{engine}/human-turn"), "2026-01-01T00:00:01Z\n");
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        let touch = fs::File::options()
            .write(true)
            .open(tree.0.project.join(format!("{engine}/human-turn")))
            .unwrap();
        touch.set_modified(later).unwrap();
        let guards = tree.guards().unwrap();
        assert_eq!(guards.authority().covered, Cover::Grant);
        assert_eq!(guards.authority().grant_source(), "turn-marker");
        let mut unattended = tree.guards().unwrap();
        unattended.unattended = true;
        assert_eq!(unattended.authority().covered, Cover::None);
        let engine_touch = fs::File::options()
            .write(true)
            .open(tree.0.project.join(format!("{engine}/engine-touch")))
            .unwrap();
        engine_touch
            .set_modified(later + std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(tree.guards().unwrap().authority().covered, Cover::None);
        // A marker that is not a regular file proves nothing, however new.
        fs::remove_file(tree.0.project.join(format!("{engine}/human-turn"))).unwrap();
        tree.write(&format!("{engine}/human-turn/x"), "");
        fs::File::open(tree.0.project.join(format!("{engine}/human-turn")))
            .unwrap()
            .set_modified(later + std::time::Duration::from_secs(50))
            .unwrap();
        assert_eq!(tree.guards().unwrap().authority().covered, Cover::None);
        fs::remove_dir_all(tree.0.project.join(format!("{engine}/human-turn"))).unwrap();
        fs::remove_file(tree.0.project.join(format!("{engine}/engine-touch"))).unwrap();
        assert!(tree.guards().is_ok());
    }
}

//! `intent-create`: refuse everything that can be refused, then mint, write
//! the ledger, build the state, list and select.
//!
//! What upstream reads from its hooks is not here: an engine question's
//! answer (`--request`), the creation grants the human-turn hook keeps for
//! a chat, a recorded presence bypass, and a flat `focus-docs/` layout to
//! migrate are each refused by name rather than acted on half-way. The
//! `.gitignore` and `AGENTS.md` blocks a copy that config never ran in gets
//! are the installer's (M6), and the source baseline of the first row is
//! the attestation's (M5).

use std::path::Path;
use std::time::SystemTime;

use focus_audit::ledger::{Ledger, Machine};
use focus_audit::time::iso_timestamp;
use focus_audit::{Entry, Fields};
use focus_graph::guard_memory::{self, Declaration};
use focus_graph::runtime::{self, ScopeDefinition, Stage};
use focus_graph::scopes::{self, GuardPolicy, ScopeMetadata};
use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{EngineError, Result, bail, json, re};
use focus_state::file::write_state_file;
use focus_workspace::atomic::write_atomic;
use focus_workspace::manifest::HarnessManifest;
use focus_workspace::names::{DEFAULT_SPACE, ENGINE_DIR, SESSIONS_DIR, STATE_FILE};
use focus_workspace::records::{self, Invoker, Selector};
use focus_workspace::scan::{self, SUBMODULE_INIT_REMEDY, enumerate_submodule_paths};
use focus_workspace::{Layout, WorkspaceLock, active_memory, env, links, observer, terminal};
use serde_json::Value;

use crate::names::{
    RESERVED_RECORD_NAMES, date_stamp, reserved_name_refusal, slugify, unique_intent_dir, uuid_v7,
};
use crate::plan::{
    self, Changes, PHASES, PLAN_FIELD, Plan, composed_plan_label, phases_with_execute_stages,
    split_slug_list,
};
use crate::record::{self, Minted};
use crate::template::{
    self, CEREMONIES, PROJECT_DESCRIPTION_FILE, PROJECT_TYPE_SOURCE_FIELD, declared_project_type,
    level_word,
};

/// What was asked for, flag by flag, each already trimmed and known to be
/// non-blank.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Request {
    pub scope: Option<String>,
    pub arguments: Option<String>,
    pub label: Option<String>,
    pub depth: Option<String>,
    pub test_strategy: Option<String>,
    pub project_type: Option<String>,
    pub review: Option<String>,
    pub guard_policy: Option<String>,
    pub change_control: Option<String>,
    /// `(key, raw value)` for each ceremony flag given.
    pub ceremonies: Vec<(&'static str, String)>,
    pub skip: Option<String>,
    pub add: Option<String>,
    pub plan_name: Option<String>,
    pub repos: Option<String>,
    pub space: Option<String>,
}

/// What a creation says, and where the record is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub dir_name: String,
    pub space: String,
    /// The summary printed on standard output.
    pub summary: String,
    /// Lines for standard error, in order.
    pub notes: Vec<String>,
}

/// The entry skill, as a refusal names it.
const ENTRY_SKILL: &str = "/focus";
const GUARD_POLICY_RENAME_NOTICE: &str = "Change Control is now Guard Policy (--guard-policy, config key guard-policy, scope key guard_policy, memory heading ## Guard Policy). The old names still work in this release and are removed in the next minor.";
const NO_CODE_FOUND_YET: &str = "The scan found no code in this folder yet; Reverse Engineering documents what is here when it runs.";

/// The install's scopes and graph, read once. Creation reads it to plan a
/// piece of work; a transition, to find a stage and the scope's next one.
pub struct Install {
    /// The enabled stages, in graph order.
    pub graph: Vec<Stage>,
    pub enabled_scopes: Vec<ScopeMetadata>,
    pub definitions: Vec<ScopeDefinition>,
    pub grid: serde_json::Map<String, Value>,
    /// The scope names a command accepts, sorted.
    pub valid_scopes: Vec<String>,
}

impl Install {
    /// Read the install of `layout`'s harness.
    ///
    /// # Errors
    ///
    /// The stage graph, a scope file or the manifest cannot be read.
    pub fn read(layout: &Layout) -> Result<Self> {
        let all = runtime::load_stages_all(&layout.stage_graph())?;
        let graph = runtime::enabled(&all);
        let scopes_all = scopes::load_all(&layout.scopes_dir())?;
        let manifest = HarnessManifest::read(&layout.manifest())?;
        let enabled_scopes = scopes::enabled(&scopes_all, &manifest);
        let grid = runtime::load_scope_grid_for_mapping(&layout.scope_grid(), &graph);
        let definitions = runtime::scope_mapping(&enabled_scopes, &grid);
        let valid_scopes = runtime::valid_scopes(&enabled_scopes);
        Ok(Self {
            graph,
            enabled_scopes,
            definitions,
            grid,
            valid_scopes,
        })
    }

    /// The scope's definition, when the install has it.
    #[must_use]
    pub fn definition(&self, scope: &str) -> Option<&ScopeDefinition> {
        self.definitions.iter().find(|def| def.meta.name == scope)
    }

    /// `findStageBySlug`: the enabled stage with this slug.
    #[must_use]
    pub fn stage(&self, slug: &str) -> Option<&Stage> {
        self.graph.iter().find(|stage| stage.slug == slug)
    }

    /// The refusal of a scope the install does not have.
    #[must_use]
    pub fn unknown_scope(&self, scope: &str) -> EngineError {
        EngineError::new(format!(
            "Unknown scope: \"{scope}\". Valid scopes: {}.",
            self.valid_scopes.join(", ")
        ))
    }

    /// `defaultScope`: the configured default, else `classic`, else the one
    /// scope an install with a single plugin and no core offers.
    fn default_scope(&self, layout: &Layout) -> Result<String> {
        let configured = focus_workspace::settings::Places::of(&layout.project)
            .flag("FOCUS_DEFAULT_SCOPE", &|name| std::env::var(name).ok())
            .ok()
            .flatten()
            .map(|raw| trim(&raw).to_owned())
            .unwrap_or_default();
        if !configured.is_empty() {
            if self.valid_scopes.contains(&configured) {
                return Ok(configured);
            }
            // A name no scope file carries is the person's error, said with
            // the scope list; an installed but disabled one is rescued.
            if !self.enabled_scopes_all_names().contains(&configured) {
                return Ok(configured);
            }
            return self.selection_aware_default(&configured);
        }
        self.selection_aware_default("classic")
    }

    fn enabled_scopes_all_names(&self) -> Vec<String> {
        // The disabled scopes are not kept here: a configured default that
        // names one is rescued the same way as an unknown one is refused,
        // by the scope list, so the distinction only changes the note.
        self.enabled_scopes
            .iter()
            .map(|meta| meta.name.clone())
            .collect()
    }

    fn selection_aware_default(&self, preferred: &str) -> Result<String> {
        if self.valid_scopes.iter().any(|name| name == preferred) {
            return Ok(preferred.to_owned());
        }
        if let Some(nominated) = self
            .valid_scopes
            .iter()
            .find(|name| self.meta(name).is_some_and(|meta| meta.freeform_default))
        {
            return Ok(nominated.clone());
        }
        let owner = |name: &str| {
            self.meta(name)
                .map_or("focus".to_owned(), |meta| meta.owner().to_owned())
        };
        let core: Vec<&String> = self
            .valid_scopes
            .iter()
            .filter(|name| owner(name) == "focus")
            .collect();
        let mut owners: Vec<String> = self
            .valid_scopes
            .iter()
            .map(|name| owner(name))
            .filter(|owner| owner != "focus")
            .collect();
        owners.sort_by(|a, b| cmp_utf16(a, b));
        owners.dedup();
        if core.is_empty() && owners.len() == 1 {
            let mut only: Vec<&String> = self
                .valid_scopes
                .iter()
                .filter(|name| owner(name) == owners[0])
                .collect();
            only.sort_by(|a, b| cmp_utf16(a, b));
            if let Some(first) = only.first() {
                return Ok((*first).clone());
            }
        }
        if self.valid_scopes.is_empty() {
            bail!(
                "No default scope is available: core scope \"{preferred}\" is disabled or absent and no plugin scopes are enabled. Pass --scope explicitly."
            );
        }
        if !core.is_empty() {
            bail!(
                "No default scope is available: scope \"{preferred}\" is disabled or absent while core scopes are enabled. Pass --scope explicitly."
            );
        }
        bail!(
            "No default scope is available: core scope \"{preferred}\" is disabled or absent and multiple plugin scope owners are enabled ({}). Pass --scope explicitly.",
            owners.join(", ")
        )
    }

    fn meta(&self, name: &str) -> Option<&ScopeMetadata> {
        self.enabled_scopes.iter().find(|meta| meta.name == name)
    }
}

/// `listSpaces`: `default` and every directory under the spaces root,
/// sorted.
fn list_spaces(layout: &Layout) -> Vec<String> {
    let mut names = vec![DEFAULT_SPACE.to_owned()];
    if let Ok(entries) = std::fs::read_dir(layout.workspace_root().join("spaces")) {
        for entry in entries.filter_map(|entry| entry.ok()) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.path().is_dir() && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names.sort_by(|a, b| cmp_utf16(a, b));
    names
}

/// `resolveIntentRepoSet`: an explicit `--repos a,b`, each a single path
/// segment, sorted without repeats; else the sibling repositories: every
/// child directory of the project holding a `.git`, but the workspace, the
/// harness trees and dependencies.
fn resolve_repo_set(layout: &Layout, explicit: Option<&str>) -> Result<Vec<String>> {
    if let Some(csv) = explicit.filter(|csv| !trim(csv).is_empty()) {
        let mut names: Vec<String> = csv
            .split(',')
            .map(trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        for name in &names {
            if !is_valid_repo_name(name) {
                bail!(
                    "Invalid --repos entry \"{name}\": a repo name must be a single path segment matching /^[A-Za-z0-9][A-Za-z0-9._-]*$/ (no separators or \"..\")."
                );
            }
        }
        let mut unique: Vec<String> = Vec::new();
        for name in names.drain(..) {
            if !unique.contains(&name) {
                unique.push(name);
            }
        }
        unique.sort_by(|a, b| cmp_utf16(a, b));
        return Ok(unique);
    }
    Ok(discover_sibling_repos(&layout.project))
}

/// `isValidRepoName`: one path segment, neither `.` nor `..`.
#[must_use]
pub fn is_valid_repo_name(name: &str) -> bool {
    re::test(r"^[A-Za-z0-9][A-Za-z0-9._-]*$", "", name) && name != "." && name != ".."
}

/// `discoverSiblingRepos`.
fn discover_sibling_repos(project: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(project) else {
        return Vec::new();
    };
    let mut found: Vec<String> = Vec::new();
    for entry in entries.filter_map(|entry| entry.ok()) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if ["focus", ".git", ".focus", "node_modules"].contains(&name.as_str())
            || re::test(r"^\.[a-z0-9][a-z0-9._-]*$", "i", &name)
        {
            continue;
        }
        let dir = project.join(&name);
        if dir.is_dir() && dir.join(".git").exists() && !found.contains(&name) {
            found.push(name);
        }
    }
    found.sort_by(|a, b| cmp_utf16(a, b));
    found
}

/// The strict declaration of the space's memory, if any layer holds one.
fn memory_strict(layout: &Layout, space: &str) -> Result<Option<Declaration>> {
    Ok(guard_memory::declarations(&layout.memory_dir(space))?
        .into_iter()
        .find(|declaration| declaration.value == GuardPolicy::Strict))
}

/// `guardSwitchRefusal` for a creation: in a tool's chat, the person is
/// told to ask there; otherwise the agent is told to create the work and
/// run the setter when the person asks.
fn guard_switch_refusal(layout: &Layout, key: &str, value: &str) -> String {
    if let Some(host) = terminal::agent_terminal_host() {
        let what = if key == "guard-policy" {
            format!("set Guard Policy {value}")
        } else {
            "turn plan approval off".to_owned()
        };
        return format!("To {what}, ask for it in your {host} chat.");
    }
    let unattended = env::var("UNATTENDED").as_deref() == Some("1");
    let own_terminal = !unattended && terminal::person_at_own_terminal(layout);
    let hint = if unattended {
        " FOCUS_UNATTENDED=1 is set, so automated prompt submissions cannot count as a human reply. Unset FOCUS_UNATTENDED before returning to interactive mode, then submit a new human response.".to_owned()
    } else if own_terminal {
        format!(" {}", terminal::OWN_TERMINAL_PRESENCE_STEP)
    } else {
        String::new()
    };
    if key == "plan-approval" {
        return format!(
            "Turning plan approval off lets code generation start without the person approving the plan, so it is their call. Create the piece of work without it; when they ask for it in their own words, run `focus engine config set guard.plan-approval off` yourself and say in one line that it is off for this piece of work.{hint}"
        );
    }
    format!(
        "Creating this intent with Guard Policy {value} would lower fences, which is the person's call. Create it, then, when they ask for it in their own words, run `focus engine config set guard-policy {value}` yourself and say in one line what changed. A scope default applies without asking.{hint}"
    )
}

/// `fenceKeyBypassed`: the human-presence kill switch is on and no chat is
/// recorded. A session's recorded bypass is the human-turn hook's (M4).
fn fence_key_bypassed(layout: &Layout, session_id: Option<&str>) -> Result<bool> {
    if !terminal::human_presence_guard_disabled(layout) {
        return Ok(false);
    }
    match session_id {
        None => Ok(terminal::current_session_id(layout).is_none()),
        Some(session) => {
            let recorded = plan_approval_runtime_dir(layout).join(format!(
                "presence-bypass-{}",
                runtime_session_segment(session)
            ));
            if recorded.exists() {
                bail!(
                    "{} records a presence bypass for this session, which the human-turn hook wrote; Focus does not read the hooks' records yet.",
                    recorded.display()
                );
            }
            Ok(false)
        }
    }
}

fn plan_approval_runtime_dir(layout: &Layout) -> std::path::PathBuf {
    layout
        .workspace_root()
        .join(SESSIONS_DIR)
        .join("plan-approval")
}

/// `runtimeSessionSegment`: a session id as a file-name segment.
#[must_use]
pub fn runtime_session_segment(session: &str) -> String {
    let segment = re::replace(r"[^A-Za-z0-9._-]+", "g", session, "-");
    let segment = re::replace(r"^-+|-+$", "g", &segment, "");
    segment.chars().take(96).collect()
}

/// The grants the human-turn hook keeps for a chat: the words the person
/// typed before this work existed. Focus has no hooks yet, so a grant on
/// disk for this session is refused rather than spent or ignored.
fn refuse_creation_grants(layout: &Layout, session_id: Option<&str>) -> Result<()> {
    let Some(session) = session_id else {
        return Ok(());
    };
    let segment = runtime_session_segment(session);
    for kind in [
        "plan-approval-off-at-creation",
        "guard-policy-at-creation",
        "ceremonies-at-creation",
        "fences-off-at-creation",
    ] {
        let path = plan_approval_runtime_dir(layout).join(format!("{kind}-{segment}.json"));
        if path.exists() {
            bail!(
                "{} holds what the person said before this work existed, which the human-turn hook recorded; Focus does not read the hooks' records yet.",
                path.display()
            );
        }
    }
    Ok(())
}

/// `needsFlatMigration`: a project still at the flat `focus-docs/` layout.
fn needs_flat_migration(layout: &Layout) -> bool {
    let flat = layout.project.join("focus-docs").join(STATE_FILE);
    if layout.workspace_root().join(".migrated").exists() || !flat.exists() {
        return false;
    }
    !any_intent_record_exists(layout)
}

fn any_intent_record_exists(layout: &Layout) -> bool {
    let Ok(spaces) = std::fs::read_dir(layout.workspace_root().join("spaces")) else {
        return false;
    };
    spaces.filter_map(|entry| entry.ok()).any(|space| {
        std::fs::read_dir(space.path().join("intents")).is_ok_and(|records| {
            records
                .filter_map(|entry| entry.ok())
                .any(|record| record.path().join(STATE_FILE).exists())
        })
    })
}

/// A creation's ledger: every row goes to the created record, named
/// explicitly, since it is listed last.
struct Rows<'a> {
    ledger: Ledger<'a>,
    held: &'a WorkspaceLock,
    selector: Selector,
    invoker: Invoker,
}

impl Rows<'_> {
    fn append(&self, event: &str, fields: &[(&str, String)]) -> Result<()> {
        let mut set = Fields::new();
        for (name, value) in fields {
            set.set(*name, value);
        }
        self.ledger.append(
            self.held,
            &[Entry::new(event, set)],
            &self.selector,
            &self.invoker,
        )?;
        Ok(())
    }
}

/// Create a piece of work in `layout`.
///
/// # Errors
///
/// Anything upstream refuses, in upstream's words, plus what Focus refuses
/// by name (see the module docs). A refusal creates nothing.
pub fn create(layout: &Layout, request: &Request) -> Result<Created> {
    let install = Install::read(layout)?;
    // Creation mutates the registry and the cursor: a bare invocation would
    // mint a garbage default-scope record.
    if request.scope.is_none() && request.arguments.is_none() && request.label.is_none() {
        bail!(
            "intent-create refused: no --scope, --arguments, or --label given. Creation is a mutation and a bare invocation mints a garbage default-scope intent. Start work via `{ENTRY_SKILL} \"<what to build>\"` (the engine names the create move for you); to invoke this tool directly, pass at least `--scope <name>` (and ideally `--arguments \"<description>\" --label \"<2-3 word essence>\"`)."
        );
    }
    let scope = match &request.scope {
        Some(scope) => scope.clone(),
        None => install.default_scope(layout)?,
    };
    if !install.valid_scopes.contains(&scope) {
        return Err(install.unknown_scope(&scope));
    }
    if let Some(depth) = &request.depth
        && level_word(depth).is_none()
    {
        bail!("Unknown depth: \"{depth}\". Valid depths: minimal, standard, comprehensive.");
    }
    if let Some(strategy) = &request.test_strategy
        && level_word(strategy).is_none()
    {
        bail!("Unknown test strategy: \"{strategy}\". Valid: minimal, standard, comprehensive.");
    }
    if let Some(kind) = &request.project_type
        && declared_project_type(Some(kind)).is_none()
    {
        bail!(
            "Unknown project type: \"{kind}\". Valid: greenfield (a new project), brownfield (existing code)."
        );
    }
    let review_override = match &request.review {
        None => None,
        Some(raw) => {
            let value = raw.to_lowercase();
            if !["adversarial", "advisory", "none"].contains(&value.as_str()) {
                bail!("Unknown review class: \"{raw}\". Valid: adversarial, advisory, none.");
            }
            Some(value)
        }
    };
    let mut notes = Vec::new();
    let mut guard_policy_flag = request.guard_policy.clone();
    if let Some(retired) = &request.change_control {
        if guard_policy_flag
            .as_ref()
            .is_some_and(|wanted| wanted != retired)
        {
            bail!(
                "--change-control is the retired name of --guard-policy; pass one of them, not both."
            );
        }
        notes.push(GUARD_POLICY_RENAME_NOTICE.to_owned());
        guard_policy_flag.get_or_insert_with(|| retired.clone());
    }
    let flagged_policy = match &guard_policy_flag {
        None => None,
        Some(raw) => Some(GuardPolicy::parse(raw).ok_or_else(|| {
            EngineError::new(format!(
                "Unknown Guard Policy value: \"{raw}\". Valid: strict, relaxed, off."
            ))
        })?),
    };
    // The ceremonies asked for with the creation: `(key, value, set by the person)`.
    let mut ceremonies: Vec<(&'static str, &'static str, bool)> = Vec::new();
    for (key, raw) in &request.ceremonies {
        let word = trim(&re::replace(r"[`*_]", "g", &raw.to_lowercase(), "")).to_owned();
        let value = match word.as_str() {
            "on" => "on",
            "off" => "off",
            _ => {
                let flag = CEREMONIES
                    .iter()
                    .find(|(k, _, _)| k == key)
                    .map_or("", |(_, _, flag)| flag);
                bail!("{flag} requires <on|off>; received \"{raw}\".");
            }
        };
        ceremonies.push((key, value, false));
    }
    // A plan composed for this piece of work, checked before any mutation.
    let changes = Changes {
        skip: split_slug_list(request.skip.as_deref()),
        add: split_slug_list(request.add.as_deref()),
    };
    let composed = !changes.is_empty();
    if let Some(name) = &request.plan_name
        && !plan::is_plan_name(name)
    {
        bail!("--plan-name takes lowercase letters, digits and hyphens; received \"{name}\".");
    }
    let definition = install
        .definition(&scope)
        .ok_or_else(|| install.unknown_scope(&scope))?;
    let (planned, errors) = plan::plan_with_changes(definition, &install.graph, &changes);
    if !errors.is_empty() {
        bail!("intent-create refused: {}", errors.join(" "));
    }
    // An explicit --space is the one selector creation takes; a space that
    // does not exist is refused: creating one is a separate move.
    if let Some(space) = &request.space {
        let spaces = list_spaces(layout);
        if !spaces.contains(space) {
            bail!(
                "Unknown space \"{space}\". Existing: {}. intent-create only creates in an existing space; create the space first ({ENTRY_SKILL} space create <name>, or legacy {ENTRY_SKILL} space-create <name>).",
                spaces.join(", ")
            );
        }
    }
    let invoker = Invoker::from_env();
    let initial = records::resolve_selection(
        layout,
        &Selector {
            space: request.space.clone(),
            intent: None,
            session_id: None,
        },
        &invoker,
    )?;
    let session_id = initial.session_id.clone();

    // Preflight the target space's memory before any mutation.
    let preflight_strict = memory_strict(layout, &initial.space)?;
    if let (Some(declaration), Some(flagged)) = (&preflight_strict, flagged_policy)
        && flagged != GuardPolicy::Strict
    {
        return Err(EngineError::new(declaration.strict_refusal()));
    }
    let scope_default_policy = definition.guard_policy();
    refuse_creation_grants(layout, session_id.as_deref())?;
    // Naming the scope's own default is not a lowering.
    let requested_policy = match flagged_policy {
        Some(wanted) if wanted != GuardPolicy::Strict && wanted == scope_default_policy => None,
        other => other,
    };
    let unattended = env::var("UNATTENDED").as_deref() == Some("1");
    if ceremonies
        .iter()
        .any(|(key, value, _)| *key == "plan_approval" && *value == "off")
    {
        if let Some(declaration) = &preflight_strict {
            bail!(
                "Your team set Guard Policy to strict in {}, so plan approval stays on for everyone on this repo. Changing that line there changes it.",
                declaration.path.display()
            );
        }
        if template::scope_ceremony_default(definition, "plan_approval") != "off" {
            if unattended {
                return Err(EngineError::new(guard_switch_refusal(
                    layout,
                    "plan-approval",
                    "off",
                )));
            }
            if !fence_key_bypassed(layout, session_id.as_deref())? {
                return Err(EngineError::new(guard_switch_refusal(
                    layout,
                    "plan-approval",
                    "off",
                )));
            }
        }
    }
    // Only a value below the scope default lowers fences.
    if let Some(wanted) = requested_policy
        && wanted != GuardPolicy::Strict
        && !wanted.at_least(scope_default_policy)
        && (unattended || !fence_key_bypassed(layout, session_id.as_deref())?)
    {
        return Err(EngineError::new(guard_switch_refusal(
            layout,
            "guard-policy",
            wanted.as_str(),
        )));
    }
    if needs_flat_migration(layout) {
        bail!(
            "intent-create refused: this project still has the flat focus-docs/ layout, which the first creation moves into its own intent. Focus does not migrate a flat layout yet."
        );
    }
    let repos = resolve_repo_set(layout, request.repos.as_deref())?;

    // The whole mutation under the workspace lock.
    let held = WorkspaceLock::acquire(layout, "creating an intent")?;
    let description = request.arguments.as_deref().map(trim);
    let label = request.label.as_deref().map(trim);
    let slug_source = label
        .filter(|s| !s.is_empty())
        .or(description.filter(|s| !s.is_empty()))
        .unwrap_or(&scope);
    let slug = slugify(slug_source, 24);
    if RESERVED_RECORD_NAMES.contains(&slug.as_str()) {
        return Err(reserved_name_refusal(&slug));
    }
    let space = initial.space.clone();
    // Re-read under the lock: a memory edit could land in between.
    let locked_strict = memory_strict(layout, &space)?;
    if let (Some(declaration), Some(wanted)) = (&locked_strict, requested_policy)
        && wanted != GuardPolicy::Strict
    {
        return Err(EngineError::new(declaration.strict_refusal()));
    }
    let effective_policy = match (&locked_strict, requested_policy) {
        (Some(declaration), _) => focus_guards::policy::format_guard_policy(
            GuardPolicy::Strict,
            &format!("{}.md", declaration.layer),
        ),
        (None, Some(wanted)) => focus_guards::policy::format_guard_policy(wanted, "you"),
        (None, None) => focus_guards::policy::format_guard_policy(
            scope_default_policy,
            &format!("scope {scope}"),
        ),
    };

    // Mint. The slug is slugified again, as upstream slugifies the label it
    // is handed; a slug is its own slug.
    let now = SystemTime::now();
    let intents_root = layout.intents_dir(&space);
    let dir_name = unique_intent_dir(
        &intents_root,
        &format!("{}-{}", date_stamp(now), slugify(&slug, 24)),
    )?;
    let record_dir = intents_root.join(&dir_name);
    links::refuse_link_on_the_way(&layout.project, &record_dir)?;
    std::fs::create_dir_all(&record_dir)
        .map_err(|err| EngineError::new(format!("{}: {err}", record_dir.display())))?;
    let minted = Minted {
        uuid: uuid_v7(now),
        slug: slug.clone(),
        dir_name: dir_name.clone(),
        record_dir: record_dir.clone(),
        space: space.clone(),
    };
    let ts = iso_timestamp(now);

    let rows = Rows {
        ledger: Ledger::new(layout, Machine::this()),
        held: &held,
        selector: Selector {
            space: Some(space.clone()),
            intent: Some(dir_name.clone()),
            session_id: None,
        },
        invoker: Invoker::from_env(),
    };
    // The shard, header only, when absent; the first row follows.
    let shard = rows.ledger.audit_file(&rows.selector, &rows.invoker)?;
    if !shard.exists() {
        if let Some(dir) = shard.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| EngineError::new(format!("{}: {err}", dir.display())))?;
        }
        std::fs::write(&shard, focus_audit::entry::SHARD_TITLE)
            .map_err(|err| EngineError::new(format!("{}: {err}", shard.display())))?;
    }
    let request_line = format!(
        "{ENTRY_SKILL} {}",
        description.filter(|s| !s.is_empty()).unwrap_or(&scope)
    );
    let stored_review = review_override
        .as_deref()
        .map(|value| stored_review_override(value, definition));
    let mut started = vec![("Scope", scope.clone()), ("Request", request_line.clone())];
    if let Some(stored) = &stored_review {
        started.push((
            "Review Override",
            if stored.is_empty() {
                "scope default".to_owned()
            } else {
                stored.clone()
            },
        ));
    }
    if !repos.is_empty() {
        started.push(("Repos", repos.join(", ")));
    }
    if composed {
        started.push((
            PLAN_FIELD,
            composed_plan_label(request.plan_name.as_deref()),
        ));
        started.push(("Stages skipped", list_or_none(&changes.skip)));
        started.push(("Stages added", list_or_none(&changes.add)));
    }
    rows.append("WORKFLOW_STARTED", &started)?;
    let in_scope = plan::stages_in_scope(&install.grid, &scope, &install.graph);
    let init_count = install
        .graph
        .iter()
        .filter(|stage| {
            stage.phase == "initialization" && plan::action_of(&in_scope, &stage.slug) == "EXECUTE"
        })
        .count();
    rows.append(
        "PHASE_STARTED",
        &[
            ("Phase", "initialization".to_owned()),
            ("Stage count", init_count.to_string()),
            ("Scope", scope.clone()),
        ],
    )?;
    let running = phases_with_execute_stages(&planned, &install.graph);
    for phase in PHASES.into_iter().skip(1) {
        let in_phase = install.graph.iter().any(|stage| stage.phase == phase);
        if !running.contains(&phase) && in_phase {
            rows.append(
                "PHASE_SKIPPED",
                &[
                    ("Phase", phase.to_owned()),
                    ("Scope", scope.clone()),
                    (
                        "Reason",
                        if composed {
                            format!("this plan excludes {phase}")
                        } else {
                            format!("scope {scope} excludes {phase}")
                        },
                    ),
                ],
            )?;
        }
    }
    rows.append(
        "STAGE_STARTED",
        &[
            ("Stage", "workspace-scaffold".to_owned()),
            ("Agent", "orchestrator".to_owned()),
        ],
    )?;
    ensure_workspace_dirs(layout, &planned, &install.graph, &record_dir, &space)?;
    let phase_dir_detail = format!(
        "{} in-scope phase dirs + verification/ + space-level knowledge/ ensured",
        running.len()
    );
    rows.append(
        "WORKSPACE_SCAFFOLDED",
        &[
            ("Request", request_line.clone()),
            (
                "Details",
                format!("{phase_dir_detail} (shell shipped by SEED)"),
            ),
        ],
    )?;
    rows.append(
        "STAGE_COMPLETED",
        &[
            ("Stage", "workspace-scaffold".to_owned()),
            ("Details", phase_dir_detail),
        ],
    )?;

    // Workspace detection.
    rows.append(
        "STAGE_STARTED",
        &[
            ("Stage", "workspace-detection".to_owned()),
            ("Agent", "orchestrator".to_owned()),
        ],
    )?;
    let scanned = scan::detect(&layout.project);
    let declared = declared_project_type(request.project_type.as_deref());
    let uninitialized: Vec<_> = scanned
        .submodules
        .iter()
        .filter(|s| !s.initialized)
        .cloned()
        .collect();
    let submodule_remedy = if uninitialized.is_empty() {
        String::new()
    } else {
        format!(
            "{} uninitialized submodule path(s) ({}) - run '{SUBMODULE_INIT_REMEDY}' to fetch them",
            uninitialized.len(),
            enumerate_submodule_paths(&uninitialized)
        )
    };
    let mut scanned_fields = vec![
        ("Project Type", scanned.project_type.to_owned()),
        ("Languages", scanned.languages.clone()),
        ("Frameworks", scanned.frameworks.clone()),
        ("Build System", scanned.build_system.clone()),
    ];
    if let Some(nested) = &scanned.nested_root {
        scanned_fields.push(("Nested Root", nested.clone()));
    }
    if !scanned.submodules.is_empty() {
        scanned_fields.push((
            "Submodules",
            format!(
                "{} declared, {} uninitialized",
                scanned.submodules.len(),
                uninitialized.len()
            ),
        ));
    }
    scanned_fields.push((
        "Details",
        if uninitialized.is_empty() {
            "Deterministic rule-based scan".to_owned()
        } else {
            format!("Deterministic rule-based scan; {submodule_remedy}")
        },
    ));
    rows.append("WORKSPACE_SCANNED", &scanned_fields)?;
    rows.append(
        "STAGE_COMPLETED",
        &[
            ("Stage", "workspace-detection".to_owned()),
            (
                "Details",
                format!(
                    "Classified {}; languages={}; frameworks={}",
                    scanned.project_type, scanned.languages, scanned.frameworks
                ),
            ),
        ],
    )?;

    // State init.
    rows.append(
        "STAGE_STARTED",
        &[
            ("Stage", "state-init".to_owned()),
            ("Agent", "orchestrator".to_owned()),
        ],
    )?;
    let raw_description = description
        .filter(|s| !s.is_empty())
        .unwrap_or("[Project description]");
    let built = template::build(&template::Inputs {
        definition,
        graph: &install.graph,
        plan: &planned,
        composed: composed.then_some(request.plan_name.as_deref()),
        declared_type: declared,
        scan: &scanned,
        depth: request.depth.as_deref(),
        test_strategy: request.test_strategy.as_deref(),
        review_override: stored_review.clone(),
        guard_policy: &effective_policy,
        ceremonies: &ceremonies,
        description: raw_description,
        request: None,
        timestamp: &ts,
    });
    if built.greenfield_note {
        notes.push(format!(
            "Note: scope \"{scope}\" usually targets existing code, but the workspace scanned as Greenfield so Reverse Engineering will be skipped. If this project has a codebase the scanner missed, say so (or run {ENTRY_SKILL} --project-type brownfield) and I'll scan again and reverse-engineer it."
        ));
    }
    observer::refuse_write(observer::WRITE_FILE_ATOMIC)?;
    let description_path = record_dir.join(PROJECT_DESCRIPTION_FILE);
    links::refuse_link_on_the_way(&layout.project, &description_path)?;
    write_atomic(
        &description_path,
        format!(
            "{}\n",
            json::stringify(&Value::String(raw_description.to_owned()))
        )
        .as_bytes(),
    )
    .map_err(|err| EngineError::new(format!("{}: {err}", description_path.display())))?;
    rows.append(
        "WORKSPACE_INITIALISED",
        &[
            ("Request", request_line),
            ("Project Type", built.project_type.to_owned()),
            (
                PROJECT_TYPE_SOURCE_FIELD,
                built.project_type_source.to_owned(),
            ),
            ("Scope", scope.clone()),
            ("Languages", scanned.languages.clone()),
            ("Frameworks", scanned.frameworks.clone()),
            ("Build System", scanned.build_system.clone()),
            (
                "Details",
                format!(
                    "{} stages in scope, routing to {}",
                    built.total_in_scope, built.first_post_init
                ),
            ),
        ],
    )?;
    rows.append(
        "STAGE_COMPLETED",
        &[
            ("Stage", "state-init".to_owned()),
            (
                "Details",
                format!(
                    "State initialized: {scope} scope, {} stages, routing to {}",
                    built.total_in_scope, built.first_post_init
                ),
            ),
        ],
    )?;
    let first_entry = install
        .graph
        .iter()
        .find(|stage| stage.slug == built.first_post_init);
    if let Some(first) = first_entry
        && first.phase != "initialization"
    {
        rows.append(
            "PHASE_COMPLETED",
            &[
                ("From phase", "initialization".to_owned()),
                ("To phase", first.phase.clone()),
                ("Stages completed", built.completed_init.to_string()),
            ],
        )?;
        rows.append(
            "PHASE_VERIFIED",
            &[(
                "Phase boundary",
                format!("initialization \u{2192} {}", first.phase),
            )],
        )?;
        rows.append(
            "PHASE_STARTED",
            &[("Phase", first.phase.clone()), ("Scope", scope.clone())],
        )?;
        rows.append(
            "STAGE_STARTED",
            &[
                ("Stage", built.first_post_init.clone()),
                ("Agent", built.first_agent.clone()),
            ],
        )?;
    }
    write_state_file(&record_dir.join(STATE_FILE), &built.content)?;
    mark_engine_touch(&record_dir, &ts);

    let submodule_warning = if uninitialized.is_empty() {
        String::new()
    } else {
        format!(
            "Warning: {} uninitialized git submodule path(s) ({}) - run '{SUBMODULE_INIT_REMEDY}' before proceeding so reverse-engineering can read the code.\n",
            uninitialized.len(),
            enumerate_submodule_paths(&uninitialized)
        )
    };
    let summary = format!(
        "Intent created: {dir_name} (space: {space})\nState initialized: {scope} scope, {} stages, {} depth\n{}Project type: {}{}\n{}Languages: {}\nFrameworks: {}\nBuild System: {}\n{submodule_warning}First post-init stage: {} ({})\n",
        built.total_in_scope,
        built.depth,
        if composed {
            format!(
                "Plan: {}, for this piece of work only (no scope file written)\n",
                composed_plan_label(request.plan_name.as_deref())
            )
        } else {
            String::new()
        },
        built.project_type,
        if declared.is_some() {
            " (you said so)"
        } else {
            ""
        },
        if declared == Some("Brownfield") && scanned.project_type != "Brownfield" {
            format!("{NO_CODE_FOUND_YET}\n")
        } else {
            String::new()
        },
        scanned.languages,
        scanned.frameworks,
        scanned.build_system,
        built.first_post_init,
        built.first_phase,
    );

    // The commit point: list the finished record, then select it.
    record::register(
        layout,
        &minted,
        &scope,
        &repos,
        session_id.as_deref(),
        None,
        now,
    )?;
    drop(held);
    Ok(Created {
        dir_name,
        space,
        summary,
        notes,
    })
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join(", ")
    }
}

/// `storedReviewOverride`: a level equal to the scope's own clears the
/// override (`""`); any other is stored.
fn stored_review_override(value: &str, definition: &ScopeDefinition) -> String {
    let scope_level = definition
        .meta
        .review_cap
        .as_deref()
        .unwrap_or("adversarial");
    if value == scope_level {
        String::new()
    } else {
        value.to_owned()
    }
}

/// `ensureWorkspaceDirs`: the record's directories for the phases the plan
/// runs, `verification/`, the space's `codekb/` parent and `knowledge/`;
/// the default space's memory tree, seeded from the install when absent;
/// and the active-memory copy brought up to date. Idempotent: nothing is
/// ever removed.
fn ensure_workspace_dirs(
    layout: &Layout,
    planned: &Plan,
    graph: &[Stage],
    record_dir: &Path,
    space: &str,
) -> Result<()> {
    let mkdir = |dir: &Path| -> Result<()> {
        links::refuse_link_on_the_way(&layout.project, dir)?;
        std::fs::create_dir_all(dir)
            .map_err(|err| EngineError::new(format!("{}: {err}", dir.display())))
    };
    mkdir(record_dir)?;
    for phase in phases_with_execute_stages(planned, graph) {
        mkdir(&record_dir.join(phase))?;
    }
    mkdir(&record_dir.join("verification"))?;
    mkdir(&layout.space_dir(space).join("codekb"))?;
    mkdir(&layout.space_dir(space).join("knowledge"))?;
    // An engine-only copy, without the workspace shell: the default space's
    // memory comes from the seed the install carries; a copy that left the
    // team's files out gets each, only if missing.
    let default_memory = layout.memory_dir(DEFAULT_SPACE);
    let seed = env::path("MEMORY_SEED_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| layout.data_dir().join("memory-seed"));
    if !default_memory.exists() {
        if seed.exists() {
            copy_tree(&seed, &default_memory)?;
        }
    } else {
        for name in ["team.md", "project.md"] {
            let source = seed.join(name);
            if !source.exists() {
                continue;
            }
            let target = default_memory.join(name);
            if !target.exists() {
                std::fs::copy(&source, &target)
                    .map_err(|err| EngineError::new(format!("{}: {err}", target.display())))?;
            }
        }
    }
    active_memory::refresh(layout, &layout.active_space());
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)
        .map_err(|err| EngineError::new(format!("{}: {err}", to.display())))?;
    for entry in std::fs::read_dir(from)
        .map_err(|err| EngineError::new(format!("{}: {err}", from.display())))?
    {
        let entry = entry.map_err(|err| EngineError::new(format!("{}: {err}", from.display())))?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)
                .map_err(|err| EngineError::new(format!("{}: {err}", target.display())))?;
        }
    }
    Ok(())
}

/// `markEngineTouch`: creating the work started its first stage, recorded
/// on the record's engine-touch marker, dated. An observer leaves none;
/// a marker that cannot be written is cleared rather than left stale.
fn mark_engine_touch(record_dir: &Path, timestamp: &str) {
    if observer::observing().is_some() {
        return;
    }
    let relative = Path::new(ENGINE_DIR).join("engine-touch");
    let target = record_dir.join(&relative);
    let written = links::refuse_link_in_chain(record_dir, &relative).is_ok()
        && std::fs::create_dir_all(record_dir.join(ENGINE_DIR)).is_ok()
        && links::refuse_link_in_chain(record_dir, &relative).is_ok()
        && std::fs::write(&target, format!("{timestamp}\n")).is_ok();
    if !written {
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&target);
    }
}

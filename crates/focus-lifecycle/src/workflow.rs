//! The piece of work a transition acts on: its record, its ledger and the
//! install it runs under.

use std::path::PathBuf;

use focus_audit::ledger::{Ledger, Machine};
use focus_audit::shards::ShardEvent;
use focus_audit::{Entry, Fields};
use focus_graph::runtime::{ScopeDefinition, Stage};
use focus_intent::create::Install;
use focus_js::{Result, bail};
use focus_state::fields::get_field;
use focus_state::file::{read_state_file, write_state_file};
use focus_workspace::names::STATE_FILE;
use focus_workspace::records::{Invoker, Selection, Selector};
use focus_workspace::{Layout, WorkspaceLock};

/// `GATE_WORDS_SPENT_BY`: the rows that spend the words the person typed at
/// a gate (the human-turn hook keeps them; this crate only clears them).
const GATE_WORDS_SPENT_BY: [&str; 4] = [
    "STAGE_AWAITING_APPROVAL",
    "GATE_APPROVED",
    "GATE_REJECTED",
    "WORKFLOW_COMPLETED",
];

/// The record a transition reads and writes.
pub struct Workflow {
    pub layout: Layout,
    pub selection: Selection,
    pub invoker: Invoker,
    pub install: Install,
}

impl Workflow {
    /// Open the selected record of `layout` and read its install.
    ///
    /// # Errors
    ///
    /// The install cannot be read.
    pub fn open(layout: Layout, selection: Selection, invoker: Invoker) -> Result<Self> {
        let install = Install::read(&layout)?;
        Ok(Self {
            layout,
            selection,
            invoker,
            install,
        })
    }

    /// The selector that names this record explicitly, for the ledger.
    #[must_use]
    pub fn selector(&self) -> Selector {
        Selector {
            space: Some(self.selection.space.clone()),
            intent: self.selection.intent.clone(),
            session_id: None,
        }
    }

    /// The ledger, named as this clone.
    #[must_use]
    pub fn ledger(&self) -> Ledger<'_> {
        Ledger::new(&self.layout, Machine::this())
    }

    /// The state file's path.
    #[must_use]
    pub fn state_path(&self) -> PathBuf {
        self.selection.docs_root(&self.layout).join(STATE_FILE)
    }

    /// The record directory, when an intent resolved.
    #[must_use]
    pub fn record_dir(&self) -> Option<PathBuf> {
        self.selection.record_dir(&self.layout)
    }

    /// `readStateFile`.
    ///
    /// # Errors
    ///
    /// The state file is missing or cannot be read.
    pub fn read_state(&self) -> Result<String> {
        read_state_file(&self.state_path())
    }

    /// `writeStateFile`.
    ///
    /// # Errors
    ///
    /// The state file cannot be written.
    pub fn write_state(&self, content: &str) -> Result<()> {
        write_state_file(&self.state_path(), content)
    }

    /// `withAuditLock`: the lock every transition runs under.
    ///
    /// # Errors
    ///
    /// The lock is held elsewhere for too long.
    pub fn lock(&self, purpose: &str) -> Result<WorkspaceLock> {
        WorkspaceLock::acquire(&self.layout, purpose)
    }

    /// `findStageBySlug`, as a refusal when the slug names no stage.
    ///
    /// # Errors
    ///
    /// `Unknown stage: <slug>`.
    pub fn stage(&self, slug: &str) -> Result<&Stage> {
        match self.install.stage(slug) {
            Some(stage) => Ok(stage),
            None => bail!("Unknown stage: {slug}"),
        }
    }

    /// The state's `Scope`, checked as upstream checks it before deriving a
    /// next stage: present, and one the install has. `refusing` is the
    /// sentence the missing field is refused with, which each transition
    /// words its own way.
    ///
    /// # Errors
    ///
    /// The field is missing or names an unknown scope.
    pub fn scope_definition<'a>(
        &'a self,
        content: &str,
        refusing: &str,
    ) -> Result<(String, &'a ScopeDefinition)> {
        let Some(scope) = get_field(content, "Scope").filter(|scope| !scope.is_empty()) else {
            bail!("State file has no Scope field. {refusing}");
        };
        if !self.install.valid_scopes.iter().any(|name| name == scope) {
            bail!(
                "State file has invalid Scope \"{scope}\". Valid scopes: {}.",
                self.install.valid_scopes.join(", ")
            );
        }
        match self.install.definition(scope) {
            Some(definition) => Ok((scope.to_owned(), definition)),
            None => bail!(
                "State file has invalid Scope \"{scope}\". Valid scopes: {}.",
                self.install.valid_scopes.join(", ")
            ),
        }
    }

    /// `emitAudit`: one row in this clone's shard, under the held lock. A
    /// row that answers a gate also clears the gate words kept for it.
    ///
    /// # Errors
    ///
    /// The row cannot be written, in which case nothing else is.
    pub fn emit(&self, held: &WorkspaceLock, event: &str, fields: &[(&str, String)]) -> Result<()> {
        self.emit_plain(held, event, fields)
            .map_err(|err| err.prefixed("Audit emission failed"))
    }

    /// [`Self::emit`] with the refusal as the ledger words it: for the
    /// verbs upstream does not wrap in its own sentence.
    ///
    /// # Errors
    ///
    /// The row cannot be written.
    pub fn emit_plain(
        &self,
        held: &WorkspaceLock,
        event: &str,
        fields: &[(&str, String)],
    ) -> Result<()> {
        let mut set = Fields::new();
        for (name, value) in fields {
            set.set(*name, value);
        }
        self.ledger().append(
            held,
            &[Entry::new(event, set)],
            &self.selector(),
            &self.invoker,
        )?;
        if GATE_WORDS_SPENT_BY.contains(&event) {
            self.clear_gate_words();
        }
        Ok(())
    }

    /// `clearGateWords`: best effort.
    fn clear_gate_words(&self) {
        let dir = self.selection.engine_dir(&self.layout).join("gate-words");
        let _ = std::fs::remove_dir_all(dir);
    }

    /// `readAllAuditShards`: every shard of the record as one text; empty
    /// when there is none or none can be read.
    #[must_use]
    pub fn audit_text(&self) -> String {
        self.ledger()
            .read_all(&self.selector(), &self.invoker)
            .unwrap_or_default()
    }

    /// `readAuditShardEvents`: every event of the record's shards, with
    /// where each stands.
    #[must_use]
    pub fn events(&self) -> Vec<ShardEvent> {
        self.ledger()
            .read_events(&self.selector(), &self.invoker, None)
            .unwrap_or_default()
    }
}

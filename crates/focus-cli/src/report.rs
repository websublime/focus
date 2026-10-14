//! How an engine tool refuses, and what the refusal leaves behind.
//!
//! Upstream's state and utility tools fail the same way (`exitWithError` in
//! `aidlc-state.ts`, `die` in `aidlc-utility.ts`, both through `emitError`
//! in `aidlc-lib.ts`): `{"error": "<why>"}` on standard error, exit 1, and,
//! when the record the command targeted has a state file, an `ERROR_LOGGED`
//! row in its ledger naming the tool, the command and the error, with the
//! project's path redacted from both (as the row's renderer redacts every
//! field, the row reads the same either way). The refusal of a selection
//! Focus cannot resolve finds no record to write beside, and is printed
//! alone, as upstream prints it.

use std::path::PathBuf;

use focus_audit::entry::redact_project_dir_prefix;
use focus_audit::ledger::{Ledger, Machine};
use focus_audit::{Entry, Fields};
use focus_harness::Harness;
use focus_js::json;
use focus_workspace::names::STATE_FILE;
use focus_workspace::records::{self, Invoker, Selection, Selector};
use focus_workspace::{Layout, WorkspaceLock, env, project};
use serde_json::json;

use crate::Failure;

/// Why a verb refused.
pub struct Refusal {
    pub message: String,
}

impl Refusal {
    pub fn of(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl From<String> for Refusal {
    fn from(message: String) -> Self {
        Self::of(message)
    }
}

impl From<focus_js::EngineError> for Refusal {
    fn from(err: focus_js::EngineError) -> Self {
        Self::of(err.to_string())
    }
}

/// What a tool was asked, for the row a refusal leaves.
pub struct Context {
    pub project_dir: Option<String>,
    pub harness: Option<String>,
    /// The ledger's name for the tool: `focus-state`, `focus-utility`.
    pub tool: &'static str,
    /// The command as the ledger names it: the tool and its own arguments.
    pub command: String,
    /// The record a refusal is written beside: an explicit `--intent` or
    /// `--space`, else the active one.
    pub selector: Selector,
}

impl Context {
    pub fn project(&self) -> Result<PathBuf, String> {
        project::resolve_project_dir(self.project_dir.as_deref().map(std::path::Path::new))
            .map_err(|err| err.to_string())
    }

    /// The workspace of the project. No path the tools' verbs read depends
    /// on the harness, so a project with several installed, or none, is not
    /// asked which; what reads the scope files resolves it when reached.
    pub fn workspace(&self) -> Result<Layout, String> {
        let project = self.project()?;
        let harness = if self.harness.is_some()
            || env::var("HARNESS").is_some_and(|harness| !harness.is_empty())
        {
            project::resolve_harness(&project, self.harness.as_deref())
                .map_err(|err| err.to_string())?
        } else {
            project::installed_harnesses(&project)
                .first()
                .copied()
                .unwrap_or(Harness::Claude)
        };
        Ok(Layout::new(project, harness))
    }

    /// Print the refusal as upstream prints it, after offering it to the
    /// ledger.
    pub fn refuse(&self, refusal: &Refusal) -> Failure {
        self.record_error(&refusal.message);
        eprintln!("{}", json::stringify(&json!({ "error": refusal.message })));
        Failure::Reported
    }

    /// `emitError`'s row: an `ERROR_LOGGED` in the ledger of the record the
    /// command targeted, when that record has a state file. Best effort,
    /// on an already failing command.
    fn record_error(&self, message: &str) {
        let Ok(layout) = self.workspace() else {
            return;
        };
        let invoker = Invoker::from_env();
        let Ok(selection) = records::resolve_selection(&layout, &self.selector, &invoker) else {
            return;
        };
        if selection.intent.is_none() || !state_path(&layout, &selection).exists() {
            return;
        }
        let Ok(held) = WorkspaceLock::acquire(&layout, "recording an error") else {
            return;
        };
        if !state_path(&layout, &selection).exists() {
            return;
        }
        let mut fields = Fields::new();
        fields.set("Tool", self.tool);
        // Redacted here as upstream redacts them, before the row's renderer
        // redacts every field again.
        fields.set(
            "Command",
            redact_project_dir_prefix(&self.command, &layout.project),
        );
        fields.set("Error", redact_project_dir_prefix(message, &layout.project));
        let ledger = Ledger::new(&layout, Machine::this());
        let _ = ledger.append(
            &held,
            &[Entry::new("ERROR_LOGGED", fields)],
            &self.selector,
            &invoker,
        );
    }
}

/// The state file of a selected record.
pub fn state_path(layout: &Layout, selection: &Selection) -> PathBuf {
    selection.docs_root(layout).join(STATE_FILE)
}

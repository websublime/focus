//! Every name Focus owns, in one place.
//!
//! All of them are `focus`: the command, the environment variables
//! (`FOCUS_*`), the lock file, the workspace directory (`focus/`), core scope,
//! sensor and agent files (`focus-<name>.md`), the engine's per-record
//! directory (`.focus-engine`), the core plugin.
//!
//! Upstream says `aidlc` in each of those places, in its engine and in the
//! authored Markdown alike. Focus is a fork frozen at the pinned commit, and
//! its content was renamed once, by the same textual map the parity tooling
//! applies when it compares the two engines (`tools/oracle/names.py`): where
//! upstream reads or writes `aidlc`, Focus reads or writes `focus`, and
//! nothing else differs. None of upstream's names are read: a project laid
//! out for upstream is not a Focus project.

/// The command people and hooks run.
pub const CLI: &str = "focus";

/// Prefix of every environment variable Focus reads for itself.
pub const ENV_PREFIX: &str = "FOCUS_";

/// The team-owned workspace directory at the project root.
pub const WORKSPACE_DIR: &str = "focus";

/// The cursor file naming the active space, inside [`WORKSPACE_DIR`].
pub const ACTIVE_SPACE_POINTER: &str = "active-space";

/// The space every project has.
pub const DEFAULT_SPACE: &str = "default";

/// The engine's working directory inside an intent record or a space's
/// intents directory. Never committed.
pub const ENGINE_DIR: &str = ".focus-engine";

/// The plugin name that owns core stages and scopes, and the file-name prefix
/// of core scope, sensor and agent files.
pub const CORE_PLUGIN: &str = "focus";

/// The workspace lock file, inside [`WORKSPACE_DIR`].
pub const LOCK_FILE: &str = ".focus-lock";

/// Who holds the lock, for the message a waiting process prints.
pub const LOCK_OWNER_FILE: &str = ".focus-lock.owner";

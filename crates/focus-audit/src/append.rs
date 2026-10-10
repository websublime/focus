//! Appending to a shard.
//!
//! Every write to the ledger ends in [`append_block_at`], so one set of
//! checks covers them all. The ledger is evidence, and where it is written
//! matters as much as what is written: an entry that lands outside the
//! project, or through a link someone planted, is an entry no reader will
//! find, or one written into a file the engine does not own. So the shard is
//! opened once without following a link, the open file is checked to be a
//! regular file inside the project that the path still names, and the same
//! is checked again after the write.
//!
//! An append is made holding the workspace lock, which the caller passes in:
//! two processes never interleave their blocks, and a batch is written with
//! one write so no reader sees half of it.
//!
//! Mirrors `appendAuditBlockAtPath`, `appendAuditEntries` and the checks of
//! `handleAppendRaw` in `aidlc-audit.ts` (L878, L1042, L1149).
//!
//! Not ported: upstream's metrics tap (refused when configured, see
//! [`Ledger::append`]) and the fork and merge of a bolt's ledger, which come
//! with bolts and worktrees.

use std::io::Write;
use std::path::Path;

use focus_js::text::trim;
use focus_js::{Result, bail};
use focus_workspace::files::{names_the_same_file, open_to_append};
use focus_workspace::links::refuse_link_in_chain;
use focus_workspace::records::{Invoker, Selector};
use focus_workspace::{WorkspaceLock, env};

use crate::entry::{
    Entry, SHARD_TITLE, lexical_absolute_path, redact_project_dir_prefix, render_block, validate,
};
use crate::events::heading;
use crate::ledger::Ledger;
use crate::read::has_unsafe_single_line_character;
use crate::time::iso_timestamp;

/// A free-form note, checked and ready to write: a heading and a body, with
/// no line a reader would take for an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    heading: String,
    body: String,
}

impl Note {
    /// Check a note. The two characters `\n` in `body` stand for a line
    /// break, and the project's own location is redacted from both parts.
    ///
    /// A note's body is written as it is, and every reader finds an event by
    /// its `**Event**:` line wherever in a block it stands. So a body that
    /// carries such a line naming an event of the vocabulary would be that
    /// event to every reader, timestamp and all: it is refused. A line naming
    /// something outside the vocabulary stays allowed, since no reader gives
    /// it authority.
    ///
    /// # Errors
    ///
    /// The heading does not fit on one line, or the body holds an event line
    /// that is empty or names an event of the vocabulary.
    pub fn check(heading_text: &str, body: &str, project: &Path) -> Result<Self> {
        if has_unsafe_single_line_character(heading_text) {
            bail!("append-raw heading must be printable text on one physical line");
        }
        let body = redact_project_dir_prefix(&body.replace("\\n", "\n"), project);
        let heading_text = redact_project_dir_prefix(heading_text, project);
        for raw in body.split(['\r', '\n', '\u{2028}', '\u{2029}']) {
            // The same optional bullet the field readers accept.
            let line = match raw.strip_prefix('-') {
                Some(rest) => rest.trim_start_matches([' ', '\t']),
                None => raw,
            };
            let Some(value) = line.strip_prefix("**Event**:") else {
                continue;
            };
            let value = trim(value);
            if value.is_empty() {
                bail!(
                    "append-raw refuses a body with an empty **Event**: line; put the value on the same line."
                );
            }
            if heading(value).is_some() {
                bail!(
                    "append-raw refuses a body carrying **Event**: {value} — that line would register as a canonical audit event to every reader. Emit taxonomy events through their owning tool (or `append` for diagnostic types); append-raw is for free-form notes only."
                );
            }
        }
        Ok(Self {
            heading: heading_text,
            body,
        })
    }
}

impl Ledger<'_> {
    /// Append `entries` to this clone's shard of the selected record, as one
    /// write, and answer with the timestamp each one was given. Every entry
    /// is validated before anything is written, so a malformed later entry
    /// cannot leave an earlier one behind.
    ///
    /// # Errors
    ///
    /// There is no entry; an entry is refused (see [`validate`]); the record
    /// cannot be resolved; `FOCUS_METRICS_ENDPOINT` is set (upstream sends a
    /// metric for each entry there, which Focus does not do yet, and an
    /// endpoint that silently received nothing would be worse than a
    /// refusal); or the shard cannot be written safely.
    pub fn append(
        &self,
        _held: &WorkspaceLock,
        entries: &[Entry],
        selector: &Selector,
        invoker: &Invoker,
    ) -> Result<Vec<String>> {
        if entries.is_empty() {
            bail!("An append to the audit ledger needs at least one entry");
        }
        for entry in entries {
            validate(entry)?;
        }
        if env::var("METRICS_ENDPOINT").is_some_and(|endpoint| !endpoint.is_empty()) {
            bail!(
                "{} is set, and Focus does not send metrics yet. Unset it to write to the audit ledger.",
                env::full_name("METRICS_ENDPOINT")
            );
        }
        let timestamps: Vec<String> = entries
            .iter()
            .map(|_| iso_timestamp((self.machine.now)()))
            .collect();
        let payload: String = entries
            .iter()
            .zip(&timestamps)
            .map(|(entry, timestamp)| render_block(entry, timestamp, &self.layout.project))
            .collect();
        let shard = self.audit_file(selector, invoker)?;
        append_block_at(&self.layout.project, &shard, &payload)?;
        Ok(timestamps)
    }

    /// Append a free-form note to this clone's shard of the active record,
    /// and answer with its timestamp.
    ///
    /// # Errors
    ///
    /// The record cannot be resolved, or the shard cannot be written safely.
    pub fn append_note(
        &self,
        _held: &WorkspaceLock,
        note: &Note,
        invoker: &Invoker,
    ) -> Result<String> {
        let timestamp = iso_timestamp((self.machine.now)());
        let block = format!(
            "\n## {}\n**Timestamp**: {timestamp}\n{}\n\n---\n",
            note.heading, note.body
        );
        let shard = self.audit_file(&Selector::default(), invoker)?;
        append_block_at(&self.layout.project, &shard, &block)?;
        Ok(timestamp)
    }
}

/// Append `block` to the shard at `shard`, creating the shard, with its
/// title, when it is not there.
///
/// # Errors
///
/// The shard is outside the project; a directory on the way to it, or the
/// shard itself, is a link; it is not a regular file; or the path stopped
/// naming the file that was opened, before or during the write.
pub(crate) fn append_block_at(project: &Path, shard: &Path, block: &str) -> Result<()> {
    let shown = shard.display();
    let outside = || format!("Refusing audit shard outside project: {shown}");
    let project_abs = lexical_absolute_path(project);
    let Ok(project_real) = std::fs::canonicalize(&project_abs) else {
        bail!(
            "The project directory cannot be read: {}",
            project.display()
        );
    };
    let shard_abs = lexical_absolute_path(shard);
    let rel = match shard_abs.strip_prefix(&project_abs) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_path_buf(),
        _ => bail!("{}", outside()),
    };
    refuse_link_in_chain(&project_real, &rel)?;
    if let Some(dir) = shard.parent().filter(|dir| !dir.exists()) {
        std::fs::create_dir_all(dir).map_err(|error| {
            focus_js::EngineError::new(format!(
                "Audit directory could not be created: {} ({error})",
                dir.display()
            ))
        })?;
    }
    refuse_link_in_chain(&project_real, &rel)?;
    let mut file = match open_to_append(shard) {
        Ok(file) => file,
        Err(error) => bail!("Audit shard could not be opened: {shown} ({error})"),
    };
    let opened = match file.metadata() {
        Ok(opened) if opened.is_file() => opened,
        _ => bail!("Refusing non-regular audit shard: {shown}"),
    };

    // The guard on the open covers the last component only, and not on every
    // platform. Resolve again, require the result to be inside the project,
    // and prove the path still names the open file, before and after writing
    // through it.
    let still_the_shard = |file: &std::fs::File, when: &str| -> Result<()> {
        refuse_link_in_chain(&project_real, &rel)?;
        if std::fs::symlink_metadata(shard).is_ok_and(|meta| meta.file_type().is_symlink()) {
            bail!("Refusing symlinked audit shard: {shown}");
        }
        let Ok(real) = std::fs::canonicalize(shard) else {
            bail!("Audit shard changed while {when}: {shown}");
        };
        if !real.starts_with(&project_real) {
            bail!("{}", outside());
        }
        if !names_the_same_file(file, &real) {
            bail!("Audit shard changed while {when}: {shown}");
        }
        Ok(())
    };
    still_the_shard(&file, "opening")?;
    let mut bytes = Vec::with_capacity(SHARD_TITLE.len() + block.len());
    if opened.len() == 0 {
        bytes.extend_from_slice(SHARD_TITLE.as_bytes());
    }
    bytes.extend_from_slice(block.as_bytes());
    if let Err(error) = file.write_all(&bytes) {
        bail!("Audit shard could not be written: {shown} ({error})");
    }
    // A shard renamed away during the write holds a row nobody will find at
    // the path readers use: fail, so the caller does not report it written.
    still_the_shard(&file, "writing")
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use focus_harness::Harness;
    use focus_workspace::Layout;

    use super::*;
    use crate::entry::Fields;
    use crate::ledger::Machine;

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("focus-append-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join(RECORD)).unwrap();
            std::fs::write(root.join(RECORD).join("focus-state.md"), "state").unwrap();
            std::fs::create_dir_all(root.join("focus")).unwrap();
            std::fs::write(root.join("focus/.focus-clone-id"), "abc123\nlaptop\n").unwrap();
            Self(Layout::new(root, Harness::Claude))
        }

        fn ledger(&self) -> Ledger<'_> {
            // 2026-10-09T16:22:42Z
            let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_791_562_962);
            Ledger::new(&self.0, Machine::fixed("laptop", "abc123", at))
        }

        fn shard(&self) -> std::path::PathBuf {
            self.0.project.join(RECORD).join("audit/laptop-abc123.md")
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    const RECORD: &str = "focus/spaces/default/intents/add-login-1a2b3c4d";

    fn entry(event_type: &str, pairs: &[(&str, &str)]) -> Entry {
        Entry::new(event_type, pairs.iter().copied().collect::<Fields>())
    }

    #[test]
    fn a_batch_is_one_write_and_the_first_write_brings_the_title() {
        let tree = Tree::new("batch");
        let lock = WorkspaceLock::acquire(&tree.0, "test").unwrap();
        let ledger = tree.ledger();
        let (selector, invoker) = (Selector::default(), Invoker::default());
        let stamps = ledger
            .append(
                &lock,
                &[
                    entry("ERROR_LOGGED", &[("Tool", "x")]),
                    entry("STAGE_STARTED", &[]),
                ],
                &selector,
                &invoker,
            )
            .unwrap();
        assert_eq!(stamps, ["2026-10-09T16:22:42Z", "2026-10-09T16:22:42Z"]);
        ledger
            .append(&lock, &[entry("STAGE_SKIPPED", &[])], &selector, &invoker)
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(tree.shard()).unwrap(),
            "# Focus Audit Log\n\
             \n## Error Logged\n**Timestamp**: 2026-10-09T16:22:42Z\n**Event**: ERROR_LOGGED\n**Tool**: x\n\n---\n\
             \n## Stage Start\n**Timestamp**: 2026-10-09T16:22:42Z\n**Event**: STAGE_STARTED\n\n---\n\
             \n## Stage Skip\n**Timestamp**: 2026-10-09T16:22:42Z\n**Event**: STAGE_SKIPPED\n\n---\n"
        );
        let events = ledger.read_events(&selector, &invoker, None).unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn one_refused_entry_leaves_nothing_written() {
        let tree = Tree::new("refused");
        let lock = WorkspaceLock::acquire(&tree.0, "test").unwrap();
        let refused = tree
            .ledger()
            .append(
                &lock,
                &[entry("ERROR_LOGGED", &[]), entry("NOPE", &[])],
                &Selector::default(),
                &Invoker::default(),
            )
            .unwrap_err();
        assert!(refused.message().starts_with("Invalid event type: NOPE."));
        assert!(!tree.shard().exists());
    }

    #[test]
    fn a_note_is_written_under_its_heading_and_cannot_pass_for_an_event() {
        let tree = Tree::new("note");
        let project = &tree.0.project;
        let lock = WorkspaceLock::acquire(&tree.0, "test").unwrap();
        let note = Note::check("A decision", "first\\nsecond", project).unwrap();
        tree.ledger()
            .append_note(&lock, &note, &Invoker::default())
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(tree.shard()).unwrap(),
            "# Focus Audit Log\n\n## A decision\n**Timestamp**: 2026-10-09T16:22:42Z\nfirst\nsecond\n\n---\n"
        );

        for body in [
            "**Event**: GATE_APPROVED",
            "fine\\n- **Event**:\tGATE_APPROVED  ",
            "x\r**Event**: HUMAN_TURN",
        ] {
            let refused = Note::check("h", body, project).unwrap_err();
            assert!(
                refused
                    .message()
                    .starts_with("append-raw refuses a body carrying **Event**: "),
                "{body:?}: {refused}"
            );
        }
        assert!(
            Note::check("h", "**Event**:", project)
                .unwrap_err()
                .message()
                .contains("empty **Event**: line")
        );
        assert!(Note::check("h", "**Event**: MY_OWN_DIAGNOSTIC", project).is_ok());
        assert!(Note::check("two\nlines", "body", project).is_err());
    }

    #[test]
    fn a_shard_outside_the_project_is_refused() {
        let tree = Tree::new("outside");
        let elsewhere = tree.0.project.with_extension("elsewhere.md");
        let refused = append_block_at(&tree.0.project, &elsewhere, "x").unwrap_err();
        assert!(
            refused
                .message()
                .starts_with("Refusing audit shard outside project: "),
            "{refused}"
        );
        let climbing = tree.0.project.join("focus/../../climbed.md");
        assert!(append_block_at(&tree.0.project, &climbing, "x").is_err());
        assert!(append_block_at(&tree.0.project, &tree.0.project, "x").is_err());
        assert!(!elsewhere.exists());
    }

    /// Deliberate deviation. Upstream sends a metric for every entry when an
    /// endpoint is configured; metrics are not ported, so the append is
    /// refused instead of leaving the endpoint without what it expects.
    #[test]
    fn a_configured_metrics_endpoint_is_refused() {
        // The variable is read at the moment of the append, so the check runs
        // in a child process that has it set.
        if std::env::var("FOCUS_METRICS_ENDPOINT").is_err() {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "append::tests::a_configured_metrics_endpoint_is_refused",
                ])
                .env("FOCUS_METRICS_ENDPOINT", "http://localhost:4318")
                .output()
                .unwrap();
            assert!(
                child.status.success(),
                "{}",
                String::from_utf8_lossy(&child.stdout)
            );
            return;
        }
        let tree = Tree::new("metrics");
        let lock = WorkspaceLock::acquire(&tree.0, "test").unwrap();
        let refused = tree
            .ledger()
            .append(
                &lock,
                &[entry("ERROR_LOGGED", &[])],
                &Selector::default(),
                &Invoker::default(),
            )
            .unwrap_err();
        assert_eq!(
            refused.message(),
            "FOCUS_METRICS_ENDPOINT is set, and Focus does not send metrics yet. Unset it to write to the audit ledger."
        );
        assert!(!tree.shard().exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_where_the_shard_or_its_directory_goes_is_refused() {
        use std::os::unix::fs::symlink;
        let tree = Tree::new("links");
        let project = &tree.0.project;
        let audit = project.join(RECORD).join("audit");
        std::fs::create_dir_all(project.join("elsewhere")).unwrap();
        symlink(project.join("elsewhere"), &audit).unwrap();
        let refused = append_block_at(project, &tree.shard(), "x").unwrap_err();
        assert!(
            refused
                .message()
                .contains("audit is a symlink, and no path component may be a symlink here."),
            "{refused}"
        );
        assert!(
            std::fs::read_dir(project.join("elsewhere"))
                .unwrap()
                .next()
                .is_none()
        );

        std::fs::remove_file(&audit).unwrap();
        std::fs::create_dir_all(&audit).unwrap();
        std::fs::write(project.join("target.md"), "").unwrap();
        symlink(project.join("target.md"), tree.shard()).unwrap();
        assert!(append_block_at(project, &tree.shard(), "x").is_err());
        assert_eq!(
            std::fs::read_to_string(project.join("target.md")).unwrap(),
            ""
        );

        // A pipe where the shard goes: refused without waiting for a reader.
        std::fs::remove_file(tree.shard()).unwrap();
        let made = std::process::Command::new("mkfifo")
            .arg(tree.shard())
            .status()
            .unwrap();
        assert!(made.success());
        let refused = append_block_at(project, &tree.shard(), "x").unwrap_err();
        assert!(
            refused
                .message()
                .starts_with("Refusing non-regular audit shard: "),
            "{refused}"
        );
    }
}

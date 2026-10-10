//! A record's ledger on disk: which shard this clone writes, and every shard
//! there is to read.
//!
//! The audit trail is committed, and each clone appends to a shard of its own
//! (`<record>/audit/<host>-<token>.md`) so two clones never produce a merge
//! conflict. The token and the host are the clone's identity, kept in
//! `focus/.focus-clone-id`, which is never committed: a fresh checkout has no
//! such file and mints its own. The host is recorded once with the token,
//! because a machine's name changes (another network, a VPN) and a shard must
//! not split when it does.
//!
//! Mirrors `cloneIdentity`, `existingShardHost`, `auditShardName`,
//! `knownAuditShardName`, `auditFilePath`, `auditShards`,
//! `readAuditShardTexts`, `readAuditShardEvents` and `readAllAuditShards` in
//! `aidlc-lib.ts`, and `readAppendOnlyFileNoFollowOrThrow` (L33474).
//!
//! Not ported, and refused: a team member's claim on a unit of work can name
//! the shard to write (`focus/.focus-unit-scope.json`). Units come with the
//! orchestrator.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{EngineError, Result, bail, re};
use focus_workspace::atomic::write_atomic;
use focus_workspace::files::read_append_only;
use focus_workspace::links::link_on_the_way;
use focus_workspace::names::{CLONE_ID_FILE, UNIT_SCOPE_FILE};
use focus_workspace::records::{Invoker, Selector, resolve_selection};
use focus_workspace::{Layout, observer};

use crate::shards::{ShardEvent, ShardText, copied_blocks, parse_shard_events, shard_blocks};

const CLONE_TOKEN: &str = "^[a-z0-9]{1,32}$";
const CLONE_HOST: &str = "^[a-z0-9][a-z0-9-]{0,47}$";

/// What a ledger takes from the machine it runs on: its name, a fresh token
/// when a clone's identity has to be minted, and the time. Passed in, so a
/// test or the oracle can fix all three.
pub struct Machine {
    /// The host segment of a shard name (see [`host_segment`]).
    pub host: String,
    /// Twelve lowercase hex characters nobody else has.
    pub mint_token: Box<dyn Fn() -> String>,
    /// The instant an entry is written.
    pub now: Box<dyn Fn() -> SystemTime>,
}

impl Machine {
    /// This machine: its host name, tokens from the system's randomness, and
    /// the system clock.
    #[must_use]
    pub fn this() -> Self {
        Self {
            host: host_segment(&gethostname::gethostname().to_string_lossy()),
            mint_token: Box::new(random_token),
            now: Box::new(SystemTime::now),
        }
    }

    /// A machine with a given name, one token and a clock stopped at `at`.
    #[must_use]
    pub fn fixed(host: &str, token: &str, at: SystemTime) -> Self {
        let token = token.to_owned();
        Self {
            host: host.to_owned(),
            mint_token: Box::new(move || token.clone()),
            now: Box::new(move || at),
        }
    }
}

fn random_token() -> String {
    let mut bytes = [0_u8; 6];
    if getrandom::fill(&mut bytes).is_err() {
        // No randomness to be had: fall back on what differs between two
        // processes on one machine. A clone keeps the token it first writes.
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let mixed = nanos ^ (u128::from(std::process::id()) << 32);
        bytes.copy_from_slice(&mixed.to_le_bytes()[..6]);
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A machine name as the host segment of a shard name: lowercase, anything
/// but letters, digits and `-` folded to `-`, no `-` at either end, at most
/// 48 characters; `host` when nothing is left. A hint for people, never a
/// path.
#[must_use]
pub fn host_segment(hostname: &str) -> String {
    let mut segment = String::new();
    let mut gap = false;
    for c in hostname.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            if gap {
                segment.push('-');
                gap = false;
            }
            segment.push(c);
        } else {
            gap = true;
        }
    }
    if gap {
        segment.push('-');
    }
    let segment: String = segment.trim_matches('-').chars().take(48).collect();
    if segment.is_empty() {
        "host".to_owned()
    } else {
        segment
    }
}

/// A clone's identity: the two parts of its shard name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloneIdentity {
    pub token: String,
    pub host: String,
}

/// What a clone-id file says: the token on line one and the host on line two,
/// each kept only when well formed.
fn parse_clone_id(raw: &str) -> (Option<String>, Option<String>) {
    let mut lines = raw
        .split('\n')
        .map(|line| trim(line.strip_suffix('\r').unwrap_or(line)));
    let token = lines.next().unwrap_or_default();
    let host = lines.next().unwrap_or_default();
    (
        re::test(CLONE_TOKEN, "", token).then(|| token.to_owned()),
        re::test(CLONE_HOST, "", host).then(|| host.to_owned()),
    )
}

fn read_lossy(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

fn names_in(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// One project's ledgers, read and named as this clone.
pub struct Ledger<'a> {
    pub(crate) layout: &'a Layout,
    pub(crate) machine: Machine,
    /// Settled once per process: a workspace that cannot be written still
    /// gets one identity for as long as the process lives.
    identity: OnceCell<CloneIdentity>,
}

impl<'a> Ledger<'a> {
    #[must_use]
    pub fn new(layout: &'a Layout, machine: Machine) -> Self {
        Self {
            layout,
            machine,
            identity: OnceCell::new(),
        }
    }

    fn clone_id_path(&self) -> PathBuf {
        self.layout.workspace_root().join(CLONE_ID_FILE)
    }

    /// The host an earlier version already named this clone's shards with,
    /// read off the shard files: a clone file that holds only a token is
    /// completed with the host its shard already carries, so the shard
    /// continues instead of a second one starting. A shard under this
    /// machine's current name wins; otherwise the most recently written one,
    /// and the lesser name when two were written at the same instant.
    fn existing_shard_host(&self, token: &str) -> Option<String> {
        let suffix = format!("-{token}.md");
        let spaces = self.layout.workspace_root().join("spaces");
        let mut newest: Option<(String, SystemTime)> = None;
        for space in names_in(&spaces) {
            let intents = spaces.join(space).join("intents");
            let mut audit_dirs = vec![intents.join("audit")];
            audit_dirs.extend(
                names_in(&intents)
                    .into_iter()
                    .map(|entry| intents.join(entry).join("audit")),
            );
            for audit in audit_dirs {
                for file in names_in(&audit) {
                    let Some(host) = file.strip_suffix(suffix.as_str()) else {
                        continue;
                    };
                    if !re::test(CLONE_HOST, "", host) {
                        continue;
                    }
                    if host == self.machine.host {
                        return Some(host.to_owned());
                    }
                    let Ok(written) = std::fs::symlink_metadata(audit.join(&file))
                        .and_then(|meta| meta.modified())
                    else {
                        continue;
                    };
                    let better = match &newest {
                        None => true,
                        Some((held, at)) => {
                            written > *at || (written == *at && cmp_utf16(host, held).is_lt())
                        }
                    };
                    if better {
                        newest = Some((host.to_owned(), written));
                    }
                }
            }
        }
        newest.map(|(host, _)| host)
    }

    /// This clone's identity: what `focus/.focus-clone-id` records, completed
    /// or minted and written back when it is missing or holds only a token.
    /// Two first runs racing settle on whichever write landed last, because
    /// both read the file back.
    pub fn clone_identity(&self) -> &CloneIdentity {
        self.identity.get_or_init(|| {
            let path = self.clone_id_path();
            let (token, host) = read_lossy(&path).map_or((None, None), |raw| parse_clone_id(&raw));
            if let (Some(token), Some(host)) = (&token, &host) {
                return CloneIdentity {
                    token: token.clone(),
                    host: host.clone(),
                };
            }
            let upgrading = token.is_some();
            let host = token
                .as_deref()
                .and_then(|token| self.existing_shard_host(token))
                .unwrap_or_else(|| self.machine.host.clone());
            let mut identity = CloneIdentity {
                token: token.unwrap_or_else(|| (self.machine.mint_token)()),
                host,
            };
            let content = format!("{}\n{}\n", identity.token, identity.host);
            let written = std::fs::create_dir_all(self.layout.workspace_root()).and_then(|()| {
                // An upgrade replaces a file a concurrent reader may be
                // reading; a first write has nothing to tear.
                if upgrading {
                    // The atomic write is behind the observer barrier, and
                    // a refusal there is one more way the write can fail.
                    observer::refuse_write(observer::WRITE_FILE_ATOMIC)
                        .map_err(|refusal| std::io::Error::other(refusal.to_string()))
                        .and_then(|()| write_atomic(&path, content.as_bytes()))
                } else {
                    std::fs::write(&path, &content)
                }
            });
            if written.is_ok()
                && let Some(settled) = read_lossy(&path)
                && let (Some(token), host) = parse_clone_id(&settled)
            {
                identity = CloneIdentity {
                    token,
                    host: host.unwrap_or(identity.host),
                };
            }
            identity
        })
    }

    fn refuse_unit_scope(&self) -> Result<()> {
        let claim = self.layout.workspace_root().join(UNIT_SCOPE_FILE);
        if claim.exists() {
            bail!(
                "{} records a claim on a unit of work, which can name the audit shard to write. Focus does not work with unit claims yet.",
                claim.display()
            );
        }
        Ok(())
    }

    /// The name of the shard this clone writes: `<host>-<token>.md`.
    ///
    /// # Errors
    ///
    /// The workspace holds a unit claim (not ported).
    pub fn shard_name(&self) -> Result<String> {
        self.refuse_unit_scope()?;
        let identity = self.clone_identity();
        Ok(format!("{}-{}.md", identity.host, identity.token))
    }

    /// This clone's shard name when it is already known, without minting a
    /// token or touching the clone file. Readers use it to prefer their own
    /// shard, and a reader must not write.
    ///
    /// # Errors
    ///
    /// The workspace holds a unit claim (not ported).
    pub fn known_shard_name(&self) -> Result<Option<String>> {
        self.refuse_unit_scope()?;
        if let Some(identity) = self.identity.get() {
            return Ok(Some(format!("{}-{}.md", identity.host, identity.token)));
        }
        let Some((Some(token), host)) =
            read_lossy(&self.clone_id_path()).map(|raw| parse_clone_id(&raw))
        else {
            return Ok(None);
        };
        let host = host
            .or_else(|| self.existing_shard_host(&token))
            .unwrap_or_else(|| self.machine.host.clone());
        Ok(Some(format!("{host}-{token}.md")))
    }

    /// The shard this clone appends to for the selected record. With no
    /// intent resolved, the shard sits under the space's record root.
    ///
    /// # Errors
    ///
    /// The selection cannot be resolved, or the workspace holds a unit claim.
    pub fn audit_file(&self, selector: &Selector, invoker: &Invoker) -> Result<PathBuf> {
        let selection = resolve_selection(self.layout, selector, invoker)?;
        let record = selection
            .record_dir(self.layout)
            .unwrap_or_else(|| self.layout.intents_dir(&selection.space));
        Ok(record.join("audit").join(self.shard_name()?))
    }

    /// Every shard the selector names, sorted within each directory. A plain
    /// selection reads the intent's shards only. Naming a space and no intent
    /// adds the space's own shards ahead of the resolved intent's, and when
    /// no intent resolves at all the space's shards are the ledger, which is
    /// where an append would have gone. A directory that cannot be listed, or
    /// that sits behind a link, is reported in `unreadable` and left out.
    ///
    /// # Errors
    ///
    /// The selection cannot be resolved.
    pub fn shards(
        &self,
        selector: &Selector,
        invoker: &Invoker,
        mut unreadable: Option<&mut Vec<PathBuf>>,
    ) -> Result<Vec<PathBuf>> {
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let (None, Some(space)) = (&selector.intent, &selector.space) {
            dirs.push(self.layout.intents_dir(space).join("audit"));
        }
        let selection = resolve_selection(self.layout, selector, invoker)?;
        match selection
            .record_dir(self.layout)
            .map(|dir| dir.join("audit"))
        {
            Some(intent_dir) => {
                if !dirs.contains(&intent_dir) {
                    dirs.push(intent_dir);
                }
            }
            None if dirs.is_empty() => {
                dirs.push(self.layout.intents_dir(&selection.space).join("audit"))
            }
            None => {}
        }
        let mut paths = Vec::new();
        for dir in dirs {
            if link_on_the_way(&self.layout.project, &dir).is_some() {
                if let Some(list) = unreadable.as_deref_mut() {
                    list.push(dir);
                }
                continue;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else {
                if dir.exists()
                    && let Some(list) = unreadable.as_deref_mut()
                {
                    list.push(dir);
                }
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".md"))
                .collect();
            names.sort_by(|a, b| cmp_utf16(a, b));
            paths.extend(names.into_iter().map(|name| dir.join(name)));
        }
        Ok(paths)
    }

    /// The text of each shard, with its index in `shards`. A shard that has
    /// vanished, is not a regular file, or sits behind a link is left out and
    /// reported in `unreadable`.
    pub(crate) fn shard_texts(
        &self,
        shards: &[PathBuf],
        mut unreadable: Option<&mut Vec<PathBuf>>,
    ) -> Vec<(usize, ShardText)> {
        let project = std::fs::canonicalize(&self.layout.project)
            .unwrap_or_else(|_| self.layout.project.clone());
        let mut texts = Vec::new();
        for (index, shard) in shards.iter().enumerate() {
            let inside = shard
                .strip_prefix(&self.layout.project)
                .map(|rel| project.join(rel))
                .unwrap_or_else(|_| shard.clone());
            let read =
                read_append_only(shard).filter(|_| link_on_the_way(&project, &inside).is_none());
            match read {
                Some(bytes) => texts.push((
                    index,
                    ShardText {
                        shard: shard.to_string_lossy().into_owned(),
                        content: String::from_utf8_lossy(&bytes).into_owned(),
                    },
                )),
                None => {
                    if let Some(list) = unreadable.as_deref_mut() {
                        list.push(shard.clone());
                    }
                }
            }
        }
        texts
    }

    /// Every event of the selected record, shard by shard, each keeping its
    /// shard and its position. Rows a copied shard repeats are read once.
    ///
    /// # Errors
    ///
    /// The selection cannot be resolved, or the workspace holds a unit claim.
    pub fn read_events(
        &self,
        selector: &Selector,
        invoker: &Invoker,
        mut unreadable: Option<&mut Vec<PathBuf>>,
    ) -> Result<Vec<ShardEvent>> {
        let shards = self.shards(selector, invoker, unreadable.as_deref_mut())?;
        let texts = self.shard_texts(&shards, unreadable);
        let own = self.known_shard_name()?;
        let only_texts: Vec<ShardText> = texts.iter().map(|(_, text)| text.clone()).collect();
        let copied = copied_blocks(&only_texts, || own);
        Ok(texts
            .iter()
            .zip(&copied)
            .flat_map(|((index, text), skip)| {
                parse_shard_events(&text.content, &text.shard, *index, skip)
            })
            .collect())
    }

    /// Every shard of the selected record as one text the block readers can
    /// walk as if it were one file; empty when there is no shard. Shards are
    /// joined in file-name order, which is not time order: ordering by
    /// timestamp is the reader's job.
    ///
    /// # Errors
    ///
    /// The selection cannot be resolved, or the workspace holds a unit claim.
    pub fn read_all(&self, selector: &Selector, invoker: &Invoker) -> Result<String> {
        let shards = self.shards(selector, invoker, None)?;
        if shards.is_empty() {
            return Ok(String::new());
        }
        let texts: Vec<ShardText> = self
            .shard_texts(&shards, None)
            .into_iter()
            .map(|(_, text)| text)
            .collect();
        let own = self.known_shard_name()?;
        let copied = copied_blocks(&texts, || own);
        Ok(texts
            .iter()
            .zip(&copied)
            .map(|(text, skip)| {
                if skip.is_empty() {
                    text.content.clone()
                } else {
                    shard_blocks(&text.content)
                        .into_iter()
                        .enumerate()
                        .filter(|(pos, _)| !skip.contains(pos))
                        .map(|(_, block)| block)
                        .collect::<Vec<_>>()
                        .join("\n---\n")
                }
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

/// A refusal for callers that must not go on with part of a record.
#[must_use]
pub fn unreadable_history() -> EngineError {
    EngineError::new(
        "Cannot read the active intent's audit history: an audit shard or directory is unreadable.",
    )
}

#[cfg(test)]
mod tests {
    use focus_harness::Harness;

    use super::*;

    struct Tree(Layout);

    impl Tree {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("focus-ledger-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(Layout::new(root, Harness::Claude))
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.project.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn read(&self, rel: &str) -> String {
            std::fs::read_to_string(self.0.project.join(rel)).unwrap()
        }

        fn ledger(&self) -> Ledger<'_> {
            let machine = Machine::fixed("laptop", "0123456789ab", SystemTime::UNIX_EPOCH);
            Ledger::new(&self.0, machine)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.project);
        }
    }

    const RECORD: &str = "focus/spaces/default/intents/add-login-1a2b3c4d";

    fn block(event: &str, at: &str) -> String {
        format!("\n## {event}\n**Timestamp**: {at}\n**Event**: {event}\n\n---\n")
    }

    #[test]
    fn a_fresh_clone_mints_its_identity_once_and_keeps_it() {
        let tree = Tree::new("mint");
        tree.write(&format!("{RECORD}/focus-state.md"), "state");
        let ledger = tree.ledger();
        assert_eq!(ledger.known_shard_name().unwrap(), None);
        let file = ledger
            .audit_file(&Selector::default(), &Invoker::default())
            .unwrap();
        assert!(
            file.ends_with("add-login-1a2b3c4d/audit/laptop-0123456789ab.md"),
            "{}",
            file.display()
        );
        assert_eq!(tree.read("focus/.focus-clone-id"), "0123456789ab\nlaptop\n");

        // Another process on a machine that has since been renamed.
        let renamed = Machine::fixed("elsewhere", "ffffffffffff", SystemTime::UNIX_EPOCH);
        let later = Ledger::new(&tree.0, renamed);
        assert_eq!(later.shard_name().unwrap(), "laptop-0123456789ab.md");
    }

    #[test]
    fn a_token_only_clone_file_is_completed_with_the_host_its_shard_carries() {
        let tree = Tree::new("upgrade");
        tree.write("focus/.focus-clone-id", "abc123\n");
        tree.write(
            &format!("{RECORD}/audit/old-name-abc123.md"),
            "# Focus Audit Log\n",
        );
        assert_eq!(
            tree.ledger().known_shard_name().unwrap().as_deref(),
            Some("old-name-abc123.md")
        );
        assert_eq!(tree.ledger().shard_name().unwrap(), "old-name-abc123.md");
        assert_eq!(tree.read("focus/.focus-clone-id"), "abc123\nold-name\n");
    }

    #[test]
    fn with_no_intent_the_space_shard_is_the_ledger_on_both_sides() {
        let tree = Tree::new("space");
        let ledger = tree.ledger();
        let file = ledger
            .audit_file(&Selector::default(), &Invoker::default())
            .unwrap();
        assert!(file.ends_with("focus/spaces/default/intents/audit/laptop-0123456789ab.md"));
        tree.write(
            "focus/spaces/default/intents/audit/laptop-0123456789ab.md",
            &format!(
                "# Focus Audit Log\n{}",
                block("WORKFLOW_STARTED", "2026-10-09T10:00:00Z")
            ),
        );
        let events = ledger
            .read_events(&Selector::default(), &Invoker::default(), None)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event, "WORKFLOW_STARTED");
    }

    #[test]
    fn every_shard_of_the_record_is_read_and_a_copy_is_read_once() {
        let tree = Tree::new("copies");
        tree.write(&format!("{RECORD}/focus-state.md"), "state");
        tree.write("focus/.focus-clone-id", "0123456789ab\nlaptop\n");
        let mine = format!(
            "# Focus Audit Log\n{}{}",
            block("WORKFLOW_STARTED", "2026-10-09T10:00:00Z"),
            block("STAGE_STARTED", "2026-10-09T10:00:05Z")
        );
        tree.write(&format!("{RECORD}/audit/laptop-0123456789ab.md"), &mine);
        tree.write(&format!("{RECORD}/audit/laptop-0123456789ab 2.md"), &mine);
        tree.write(
            &format!("{RECORD}/audit/desktop-ffffffffffff.md"),
            &format!(
                "# Focus Audit Log\n{}",
                block("STAGE_COMPLETED", "2026-10-09T10:01:00Z")
            ),
        );
        tree.write(&format!("{RECORD}/audit/notes.txt"), "not a shard");
        let ledger = tree.ledger();
        let mut unreadable = Vec::new();
        let events = ledger
            .read_events(
                &Selector::default(),
                &Invoker::default(),
                Some(&mut unreadable),
            )
            .unwrap();
        let seen: Vec<(&str, usize)> = events
            .iter()
            .map(|e| (e.event.as_str(), e.shard_index))
            .collect();
        // Sorted by file name: desktop, then the copy ("…ab 2.md"), then mine.
        assert_eq!(
            seen,
            [
                ("STAGE_COMPLETED", 0),
                ("WORKFLOW_STARTED", 2),
                ("STAGE_STARTED", 2)
            ]
        );
        assert!(unreadable.is_empty());
    }

    /// What upstream answered for each of these trees, asked once on Linux
    /// (a link cannot be part of the corpus, which Windows replays too): a
    /// shard that cannot be trusted is listed, reported and not read; a
    /// directory that cannot be trusted is reported and not listed.
    #[cfg(unix)]
    mod links {
        use std::os::unix::fs::symlink;

        use super::*;

        const SHARD: &str = "laptop-0123456789ab.md";

        /// A record with one honest shard, then whatever `arrange` does to it.
        /// Answers what a reader sees: shards listed, places reported, events.
        fn read(
            name: &str,
            arrange: impl FnOnce(&Path, &Path),
        ) -> (Vec<String>, Vec<String>, Vec<String>) {
            let tree = Tree::new(name);
            tree.write(&format!("{RECORD}/focus-state.md"), "state");
            tree.write("focus/.focus-clone-id", "0123456789ab\nlaptop\n");
            let audit = tree.0.project.join(RECORD).join("audit");
            arrange(&tree.0.project, &audit);
            read_as(&tree.0)
        }

        fn read_as(layout: &Layout) -> (Vec<String>, Vec<String>, Vec<String>) {
            let machine = Machine::fixed("laptop", "0123456789ab", SystemTime::UNIX_EPOCH);
            let ledger = Ledger::new(layout, machine);
            let (selector, invoker) = (Selector::default(), Invoker::default());
            let name = |path: &Path| path.file_name().unwrap().to_string_lossy().into_owned();
            let mut unreadable = Vec::new();
            let events = ledger
                .read_events(&selector, &invoker, Some(&mut unreadable))
                .unwrap();
            let shards = ledger.shards(&selector, &invoker, None).unwrap();
            (
                shards.iter().map(|shard| name(shard)).collect(),
                unreadable.iter().map(|path| name(path)).collect(),
                events.into_iter().map(|row| row.event).collect(),
            )
        }

        fn honest(audit: &Path) {
            std::fs::create_dir_all(audit).unwrap();
            std::fs::write(
                audit.join(SHARD),
                format!(
                    "# Focus Audit Log\n{}",
                    block("STAGE_STARTED", "2026-10-09T10:00:01Z")
                ),
            )
            .unwrap();
        }

        #[test]
        fn a_linked_shard_is_listed_reported_and_not_read() {
            let seen = read("linked-shard", |project, audit| {
                honest(audit);
                let forged = format!(
                    "# Focus Audit Log\n{}",
                    block("GATE_APPROVED", "2026-10-09T10:00:00Z")
                );
                std::fs::write(project.join("elsewhere.md"), forged).unwrap();
                symlink(
                    project.join("elsewhere.md"),
                    audit.join("forged-000000000000.md"),
                )
                .unwrap();
            });
            assert_eq!(seen.0, ["forged-000000000000.md", SHARD]);
            assert_eq!(seen.1, ["forged-000000000000.md"]);
            assert_eq!(seen.2, ["STAGE_STARTED"]);
        }

        #[test]
        fn a_linked_audit_directory_is_reported_and_not_listed() {
            let seen = read("linked-dir", |project, audit| {
                honest(&project.join("real-audit"));
                symlink(project.join("real-audit"), audit).unwrap();
            });
            assert_eq!(seen, (vec![], vec!["audit".to_owned()], vec![]));
            // A link to nowhere is still a link.
            let seen = read("dangling-dir", |project, audit| {
                symlink(project.join("nowhere"), audit).unwrap()
            });
            assert_eq!(seen, (vec![], vec!["audit".to_owned()], vec![]));
        }

        #[test]
        fn a_link_anywhere_above_the_shards_is_reported() {
            let seen = read("linked-spaces", |project, audit| {
                honest(audit);
                std::fs::rename(project.join("focus/spaces"), project.join("moved")).unwrap();
                symlink(project.join("moved"), project.join("focus/spaces")).unwrap();
            });
            assert_eq!(seen, (vec![], vec!["audit".to_owned()], vec![]));
        }

        #[test]
        fn a_project_reached_through_a_link_is_read() {
            let tree = Tree::new("behind-a-link");
            tree.write(&format!("{RECORD}/focus-state.md"), "state");
            honest(&tree.0.project.join(RECORD).join("audit"));
            let link = tree.0.project.with_extension("link");
            let _ = std::fs::remove_file(&link);
            symlink(&tree.0.project, &link).unwrap();
            let seen = read_as(&Layout::new(&link, Harness::Claude));
            std::fs::remove_file(&link).unwrap();
            assert_eq!(
                seen,
                (
                    vec![SHARD.to_owned()],
                    vec![],
                    vec!["STAGE_STARTED".to_owned()]
                )
            );
        }

        #[test]
        fn a_pipe_named_like_a_shard_is_reported_without_waiting_for_a_writer() {
            let seen = read("pipe", |_, audit| {
                honest(audit);
                let made = std::process::Command::new("mkfifo")
                    .arg(audit.join("pipe-000000000000.md"))
                    .status()
                    .unwrap();
                assert!(made.success());
            });
            assert_eq!(seen.0, [SHARD, "pipe-000000000000.md"]);
            assert_eq!(seen.1, ["pipe-000000000000.md"]);
            assert_eq!(seen.2, ["STAGE_STARTED"]);
        }

        #[test]
        fn a_shard_with_a_second_hard_link_is_read() {
            // A backup that hard-links the project must not stop the ledger being read.
            let seen = read("hard-link", |project, audit| {
                honest(audit);
                std::fs::hard_link(audit.join(SHARD), project.join("backup.md")).unwrap();
            });
            assert_eq!(
                seen,
                (
                    vec![SHARD.to_owned()],
                    vec![],
                    vec!["STAGE_STARTED".to_owned()]
                )
            );
        }
    }

    #[test]
    fn a_shard_under_this_machines_name_wins_over_a_newer_one() {
        let tree = Tree::new("current-name");
        tree.write("focus/.focus-clone-id", "abc123\n");
        tree.write(
            &format!("{RECORD}/audit/laptop-abc123.md"),
            "# Focus Audit Log\n",
        );
        tree.write(
            &format!("{RECORD}/audit/zz-newer-abc123.md"),
            "# Focus Audit Log\n",
        );
        let old = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(tree.0.project.join(RECORD).join("audit/laptop-abc123.md"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(tree.ledger().shard_name().unwrap(), "laptop-abc123.md");
    }

    /// Deliberate deviation. Upstream lets a team member's unit claim name
    /// the shard to write. Units are not ported, so the claim is refused.
    #[test]
    fn a_unit_claim_is_refused() {
        let tree = Tree::new("claim");
        tree.write("focus/.focus-unit-scope.json", "{}");
        let refused = tree.ledger().shard_name().unwrap_err();
        assert!(
            refused
                .message()
                .contains("does not work with unit claims yet"),
            "{refused}"
        );
    }

    #[test]
    fn a_machine_name_becomes_a_safe_segment() {
        assert_eq!(
            host_segment("Miguels-MacBook-Pro.local"),
            "miguels-macbook-pro-local"
        );
        assert_eq!(host_segment("--weird__name!!"), "weird-name");
        assert_eq!(host_segment("..."), "host");
        assert_eq!(host_segment(&"a".repeat(60)).len(), 48);
    }
}

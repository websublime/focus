//! Reading several shards as one ledger.
//!
//! Every clone appends to a shard of its own, so that git never has to merge
//! two appends, and a record's history is all of its shards read together.
//! That brings two problems this module settles the way upstream does:
//!
//! - **Order.** Append order is known within a shard and nowhere else, so an
//!   event keeps the shard it came from and its position in it. Timestamps are
//!   to the second; two events with the same timestamp in different shards
//!   are unordered, and a reader that needs an order has to say what it does
//!   about that.
//! - **Copies.** A sync tool's conflict copy, or a hand copy, repeats rows
//!   another shard already holds. Read twice, a row ties with itself across
//!   two files and stops counting. A copy starts as its source starts, so
//!   only files that start alike are compared, and each repeated row is read
//!   once, from the best file that holds it.
//!
//! Mirrors `auditShardBlocks`, `copiedAuditBlocks`, `parseAuditShardEvents`
//! and `parseAuditShardNotes` in `aidlc-lib.ts` (L14508 to L14660).

use std::collections::BTreeSet;

use focus_js::re;
use focus_js::text::trim;

use crate::read::block_field;

/// The name of a shard a clone writes: `<host>-<token>.md`.
const SHARD_FILE_NAME: &str = r"^[a-z0-9][a-z0-9-]*-[a-z0-9]{1,32}\.md$";

/// One shard's text, with the path it was read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardText {
    pub shard: String,
    pub content: String,
}

/// One event, with where it stands: which shard, and which block of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardEvent {
    pub block: String,
    pub event: String,
    pub pos: usize,
    pub shard: String,
    pub shard_index: usize,
    pub timestamp: String,
}

/// A free-form note: a block with a timestamp and no event. History only,
/// never evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardNote {
    pub block: String,
    pub pos: usize,
    pub shard: String,
    pub shard_index: usize,
    pub timestamp: String,
    pub heading: String,
    pub text: String,
}

/// The blocks of a shard, in the order they were appended. Every position
/// this module reports indexes into this sequence.
#[must_use]
pub fn shard_blocks(content: &str) -> Vec<String> {
    content
        .replace("\r\n", "\n")
        .split("\n---\n")
        .map(str::to_owned)
        .collect()
}

fn file_name(shard: &str) -> &str {
    let after_slash = shard.rsplit('/').next().unwrap_or(shard);
    if cfg!(windows) {
        after_slash.rsplit('\\').next().unwrap_or(after_slash)
    } else {
        after_slash
    }
}

/// For each shard, the positions of the blocks to skip because a better file
/// holds the same row.
///
/// Files are compared only when they start with the same block (a title and a
/// first row with a timestamp), so two clones that happen to write one row
/// alike are never taken for copies of each other. Among files that start
/// alike, a timestamped block found in more than one is read from the best of
/// them: this clone's own shard, then a file named like a shard, then the file
/// with more timestamped blocks, then the earlier one. A row repeated inside
/// one file is not a copy and stays. `own_shard` is asked only when a copy
/// exists.
pub fn copied_blocks(
    shards: &[ShardText],
    own_shard: impl FnOnce() -> Option<String>,
) -> Vec<BTreeSet<usize>> {
    let mut skip: Vec<BTreeSet<usize>> = shards.iter().map(|_| BTreeSet::new()).collect();
    if shards.len() < 2 {
        return skip;
    }
    // Groups in the order their first member appears.
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, shard) in shards.iter().enumerate() {
        let head = match re::find(r"\r?\n---\r?\n", "", &shard.content) {
            Some(rule) => &shard.content[..rule.start()],
            None => shard.content.as_str(),
        }
        .replace("\r\n", "\n");
        if block_field(&head, "Timestamp").is_none() {
            continue;
        }
        match groups.iter_mut().find(|(start, _)| *start == head) {
            Some((_, members)) => members.push(index),
            None => groups.push((head, vec![index])),
        }
    }

    let mut own_shard = Some(own_shard);
    let mut own: Option<Option<String>> = None;
    for (_, members) in groups.iter().filter(|(_, members)| members.len() >= 2) {
        let own = own.get_or_insert_with(|| own_shard.take().and_then(|ask| ask()));
        // Per member, its blocks; `None` where a block carries no timestamp.
        let blocks: Vec<Vec<Option<String>>> = members
            .iter()
            .map(|&index| {
                shard_blocks(&shards[index].content)
                    .into_iter()
                    .map(|block| block_field(&block, "Timestamp").is_some().then_some(block))
                    .collect()
            })
            .collect();
        // Which members hold each block, in the order they were found.
        let mut holders: Vec<(&str, Vec<usize>)> = Vec::new();
        for (member, member_blocks) in blocks.iter().enumerate() {
            for block in member_blocks.iter().flatten() {
                match holders.iter_mut().find(|(held, _)| *held == block.as_str()) {
                    Some((_, found)) => {
                        if !found.contains(&member) {
                            found.push(member);
                        }
                    }
                    None => holders.push((block.as_str(), vec![member])),
                }
            }
        }
        let counts: Vec<usize> = blocks.iter().map(|b| b.iter().flatten().count()).collect();
        let is_own =
            |member: usize| own.as_deref() == Some(file_name(&shards[members[member]].shard));
        let is_shard_name = |member: usize| {
            re::test(
                SHARD_FILE_NAME,
                "",
                file_name(&shards[members[member]].shard),
            )
        };
        let mut order: Vec<usize> = (0..members.len()).collect();
        order.sort_by(|&a, &b| {
            is_own(b)
                .cmp(&is_own(a))
                .then(is_shard_name(b).cmp(&is_shard_name(a)))
                .then(counts[b].cmp(&counts[a]))
                .then(a.cmp(&b))
        });
        let rank = |member: usize| {
            order
                .iter()
                .position(|&ranked| ranked == member)
                .unwrap_or(usize::MAX)
        };
        for (member, member_blocks) in blocks.iter().enumerate() {
            for (pos, block) in member_blocks.iter().enumerate() {
                let Some(block) = block else { continue };
                let Some((_, found)) = holders.iter().find(|(held, _)| *held == block.as_str())
                else {
                    continue;
                };
                if found.len() >= 2 && found.iter().any(|&holder| rank(holder) < rank(member)) {
                    skip[members[member]].insert(pos);
                }
            }
        }
    }
    skip
}

/// The events of one shard, in append order. A block is an event when it has
/// both an event and a timestamp, each with a value.
#[must_use]
pub fn parse_shard_events(
    content: &str,
    shard: &str,
    shard_index: usize,
    copied: &BTreeSet<usize>,
) -> Vec<ShardEvent> {
    shard_blocks(content)
        .into_iter()
        .enumerate()
        .filter(|(pos, _)| !copied.contains(pos))
        .filter_map(|(pos, block)| {
            let event = block_field(&block, "Event")
                .filter(|event| !event.is_empty())?
                .to_owned();
            let timestamp = block_field(&block, "Timestamp")
                .filter(|at| !at.is_empty())?
                .to_owned();
            Some(ShardEvent {
                block,
                event,
                pos,
                shard: shard.to_owned(),
                shard_index,
                timestamp,
            })
        })
        .collect()
}

/// The notes of one shard: blocks with a timestamp, no event line at all, and
/// a `## ` heading above the timestamp. The text is everything under the
/// heading except the timestamp line.
#[must_use]
pub fn parse_shard_notes(
    content: &str,
    shard: &str,
    shard_index: usize,
    copied: &BTreeSet<usize>,
) -> Vec<ShardNote> {
    let mut notes = Vec::new();
    for (pos, block) in shard_blocks(content).into_iter().enumerate() {
        if copied.contains(&pos) {
            continue;
        }
        let Some(timestamp) = block_field(&block, "Timestamp").filter(|at| !at.is_empty()) else {
            continue;
        };
        if block_field(&block, "Event").is_some() {
            continue;
        }
        let lines: Vec<&str> = block.split('\n').collect();
        let heading_at = lines.iter().position(|line| re::test(r"^## \S", "", line));
        let timestamp_at = lines
            .iter()
            .position(|line| re::test(r"^(?:- )?\*\*Timestamp\*\*:", "", line));
        let (Some(heading_at), Some(timestamp_at)) = (heading_at, timestamp_at) else {
            continue;
        };
        if timestamp_at <= heading_at {
            continue;
        }
        let under: Vec<&str> = lines
            .iter()
            .enumerate()
            .skip(heading_at + 1)
            .filter(|(index, _)| *index != timestamp_at)
            .map(|(_, line)| *line)
            .collect();
        notes.push(ShardNote {
            timestamp: timestamp.to_owned(),
            heading: trim(&lines[heading_at][3..]).to_owned(),
            text: trim(&under.join("\n")).to_owned(),
            pos,
            shard: shard.to_owned(),
            shard_index,
            block: block.clone(),
        });
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = "# Focus Audit Log\n\n## Workflow Start\n**Timestamp**: 2026-10-09T10:00:00Z\n**Event**: WORKFLOW_STARTED\n";
    const SECOND: &str =
        "\n## Stage Start\n**Timestamp**: 2026-10-09T10:00:05Z\n**Event**: STAGE_STARTED\n";
    const THIRD: &str =
        "\n## Stage Completion\n**Timestamp**: 2026-10-09T10:01:00Z\n**Event**: STAGE_COMPLETED\n";

    fn shard(name: &str, blocks: &[&str]) -> ShardText {
        ShardText {
            shard: name.to_owned(),
            content: blocks
                .iter()
                .map(|block| format!("{block}\n---\n"))
                .collect(),
        }
    }

    #[test]
    fn a_conflict_copy_is_read_once_from_the_shard_with_a_shard_name() {
        let shards = [
            shard("audit/laptop-0123456789ab 2.md", &[FIRST, SECOND, THIRD]),
            shard("audit/laptop-0123456789ab.md", &[FIRST, SECOND]),
        ];
        let skip = copied_blocks(&shards, || None);
        // The copy holds more rows but is not named like a shard: its two
        // repeated rows are skipped, and the row only it holds stays.
        assert_eq!(skip[0].iter().copied().collect::<Vec<_>>(), [0, 1]);
        assert!(skip[1].is_empty());
        let events = parse_shard_events(&shards[0].content, &shards[0].shard, 0, &skip[0]);
        assert_eq!(events.len(), 1);
        assert_eq!(
            (events[0].event.as_str(), events[0].pos),
            ("STAGE_COMPLETED", 2)
        );
    }

    #[test]
    fn shards_that_start_differently_are_never_copies_of_each_other() {
        let other_start = FIRST.replace("10:00:00", "09:00:00");
        let shards = [
            shard("a-1.md", &[FIRST, SECOND]),
            shard("b-2.md", &[&other_start, SECOND]),
        ];
        assert!(
            copied_blocks(&shards, || None)
                .iter()
                .all(BTreeSet::is_empty)
        );
    }

    #[test]
    fn this_clones_own_shard_wins_over_a_longer_one() {
        let shards = [
            shard("a-1.md", &[FIRST, SECOND, THIRD]),
            shard("b-2.md", &[FIRST, SECOND]),
        ];
        let skip = copied_blocks(&shards, || Some("b-2.md".to_owned()));
        assert_eq!(skip[0].iter().copied().collect::<Vec<_>>(), [0, 1]);
        assert!(skip[1].is_empty());
    }

    #[test]
    fn a_note_is_a_timestamped_block_with_a_heading_and_no_event() {
        let content = "# Focus Audit Log\n\n## Decision\n**Timestamp**: 2026-10-09T10:00:00Z\nWe keep the old schema.\nFor now.\n\n---\n\n## No heading above\n\n---\n**Timestamp**: 1\n## Heading below\n\n---\n";
        let notes = parse_shard_notes(content, "a-1.md", 0, &BTreeSet::new());
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].heading, "Decision");
        assert_eq!(notes[0].text, "We keep the old schema.\nFor now.");
        assert!(parse_shard_events(content, "a-1.md", 0, &BTreeSet::new()).is_empty());
    }
}

//! The units of work and the order they can be built in.
//!
//! Units Generation writes `unit-of-work-dependency.md`, and in it one
//! fenced `yaml` block that lists each unit with the units it depends on:
//!
//! ```yaml
//! units:
//!   - name: auth
//!     kind: service
//!     depends_on: []
//!   - name: api
//!     depends_on: [auth]
//! ```
//!
//! From that block comes the order of Construction: the units whose
//! dependencies are all built form a batch and can be worked on side by
//! side, then the next batch, and so on. A block that cannot be read gives
//! no order at all, with the reason, never a guess.
//!
//! The block is read by a scanner of its own, as upstream's is: the engine
//! carries no YAML parser, and what the scanner accepts is the contract.
//!
//! Mirrors `parseBoltDag`, `extractYamlUnitsBlock`, `parseUnitsBlock`,
//! `parseInlineDepsList`, `unquoteScalar`, `computeBatches` and
//! `validateUnitName` in `aidlc-lib.ts` (L41766, L41563, L41649, L41593,
//! L41583, L41723, L29055).

use std::collections::{HashMap, HashSet};

use focus_js::order::cmp_utf16;
use focus_js::re;
use focus_js::text::trim;

/// The kinds a unit may declare.
pub const UNIT_KINDS: [&str; 5] = ["service", "spec", "ui", "packaging", "library"];

/// The longest unit name, in UTF-16 code units as upstream counts them.
pub const UNIT_NAME_MAX_LENGTH: usize = 64;

/// What a unit name must match, as upstream prints it in a refusal.
const UNIT_NAME_PATTERN: &str = r"^[A-Za-z0-9][A-Za-z0-9._-]*$";

/// One unit and what it waits for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub name: String,
    pub depends_on: Vec<String>,
    /// One of [`UNIT_KINDS`], when the block says.
    pub kind: Option<String>,
}

/// The units and their batches: each batch holds the units whose
/// dependencies are all in earlier batches, in name order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltDag {
    pub units: Vec<Unit>,
    pub batches: Vec<Vec<String>>,
}

/// Why a document gives no order: `absent` (no units block), `malformed`
/// or `cyclic`, with the detail upstream gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub reason: &'static str,
    pub detail: String,
}

fn malformed(detail: impl Into<String>) -> Unreadable {
    Unreadable {
        reason: "malformed",
        detail: detail.into(),
    }
}

/// The first `count` UTF-16 code units of `text`. Half a pair cut off reads
/// as U+FFFD, which is what a runtime writes for it.
fn utf16_prefix(text: &str, count: usize) -> String {
    let units: Vec<u16> = text.encode_utf16().take(count).collect();
    String::from_utf16_lossy(&units)
}

/// Why `name` cannot be a unit's name; `None` when it can. A name becomes a
/// directory under `construction/` and a value in one-line state fields.
#[must_use]
pub fn validate_unit_name(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("Unit name is empty".to_owned());
    }
    let length = name.encode_utf16().count();
    if length > UNIT_NAME_MAX_LENGTH {
        return Some(format!(
            "Unit name \"{}...\" is {length} chars; max is {UNIT_NAME_MAX_LENGTH}",
            utf16_prefix(name, 32)
        ));
    }
    if !re::test(UNIT_NAME_PATTERN, "", name) {
        return Some(format!(
            "Invalid Unit name \"{name}\" - must match /{UNIT_NAME_PATTERN}/ (ASCII letter/digit, then ASCII letters/digits/dot/underscore/hyphen)"
        ));
    }
    None
}

/// The lines of `text` as `split(/\r?\n/)` gives them.
fn lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    // A `\r` goes with the `\n` after it; the last line has none.
    let last = lines.len() - 1;
    for line in &mut lines[..last] {
        *line = line.strip_suffix('\r').unwrap_or(line);
    }
    lines
}

/// The inside of the first fenced `yaml` (or `yml`) block that has a
/// `units:` key; `None` when no block has one. Other fenced blocks are
/// stepped over.
fn units_block(body: &str) -> Option<String> {
    let lines = lines(body);
    let mut at = 0;
    while at < lines.len() {
        if re::test(r"^```ya?ml\s*$", "", trim(lines[at])) {
            let close = (at + 1..lines.len())
                .find(|line| re::test(r"^```\s*$", "", trim(lines[*line])))
                .unwrap_or(lines.len());
            let block = lines[at + 1..close].join("\n");
            if re::test(r"^\s*units\s*:", "m", &block) {
                return Some(block);
            }
            at = close;
        }
        at += 1;
    }
    None
}

/// A scalar without the quotes around it, if it has a pair.
fn unquote(scalar: &str) -> &str {
    let scalar = trim(scalar);
    let quoted = (scalar.starts_with('"') && scalar.ends_with('"'))
        || (scalar.starts_with('\'') && scalar.ends_with('\''));
    // One quote character alone is both ends of itself: nothing is left.
    if quoted {
        let mut inner = scalar.chars();
        inner.next();
        inner.next_back();
        inner.as_str()
    } else {
        scalar
    }
}

/// The items of `depends_on:` written on its own line: `[a, "b"]`, nothing,
/// or one bare name. A list that does not close, or with anything but a
/// comment after it, reads as no dependencies.
fn inline_deps(raw: &str) -> Vec<String> {
    let text = trim(raw);
    if text.is_empty() || text == "[]" {
        return Vec::new();
    }
    if !text.starts_with('[') {
        return vec![unquote(text).to_owned()];
    }
    // Walk `chars` from `from`, calling `at_top` for each character outside
    // quotes until it answers true; the index it stopped at, and whether a
    // quote was left open.
    let chars: Vec<char> = text.chars().collect();
    let scan = |from: usize,
                upto: usize,
                at_top: &mut dyn FnMut(usize, char) -> bool|
     -> (Option<usize>, bool) {
        let mut quote: Option<char> = None;
        let mut index = from;
        while index < upto {
            let c = chars[index];
            match quote {
                Some('"') if c == '\\' => index += 1,
                Some(open) if c == open => quote = None,
                Some(_) => {}
                None if c == '"' || c == '\'' => quote = Some(c),
                None => {
                    if at_top(index, c) {
                        return (Some(index), false);
                    }
                }
            }
            index += 1;
        }
        (None, quote.is_some())
    };
    let (close, _) = scan(1, chars.len(), &mut |_, c| c == ']');
    let Some(close) = close else {
        return Vec::new();
    };
    let after: String = chars[close + 1..].iter().collect();
    if !re::test(r"^[ \t]*(?:#[^\r\n]*)?$", "", &after) {
        return Vec::new();
    }
    let mut items = Vec::new();
    let mut start = 1;
    let (_, open_quote) = scan(1, close, &mut |index, c| {
        if c == ',' {
            items.push((start, index));
            start = index + 1;
        }
        false
    });
    if open_quote {
        return Vec::new();
    }
    items.push((start, close));
    items
        .into_iter()
        .map(|(from, to)| unquote(&chars[from..to].iter().collect::<String>()).to_owned())
        .filter(|item| !item.is_empty())
        .collect()
}

/// The units a block lists.
fn parse_units_block(block: &str) -> Result<Vec<Unit>, String> {
    let lines = lines(block);
    let Some(key) = lines
        .iter()
        .position(|line| re::test(r"^\s*units\s*:", "", line))
    else {
        return Err("missing units: key".to_owned());
    };
    let key_line = re::find(r"^\s*units\s*:", "", lines[key])
        .map_or(lines[key], |found| &lines[key][found.end()..]);
    if !trim(key_line).is_empty() {
        return Err("units: must be a block list, not an inline value".to_owned());
    }
    let mut units: Vec<Unit> = Vec::new();
    let mut open = false;
    for line in &lines[key + 1..] {
        if trim(line).is_empty() {
            continue;
        }
        if let Some(found) = re::find(r"^\s*-\s+name\s*:\s*(.+?)\s*$", "", line) {
            units.push(Unit {
                name: unquote(re::group(line, &found, 1).unwrap_or_default()).to_owned(),
                depends_on: Vec::new(),
                kind: None,
            });
            open = true;
            continue;
        }
        let current = if open { units.last_mut() } else { None };
        if let Some(found) = re::find(r"^\s*depends_on\s*:\s*(.*)$", "", line) {
            let Some(current) = current else {
                return Err("depends_on: before any - name: entry".to_owned());
            };
            current.depends_on = inline_deps(re::group(line, &found, 1).unwrap_or_default());
            continue;
        }
        // The value must be one of the kinds: a typo fails here instead of
        // falling back to the full set of design artifacts in silence.
        if let Some(found) = re::find(r"^\s*kind\s*:\s*(.+?)\s*$", "", line) {
            let Some(current) = current else {
                return Err("kind: before any - name: entry".to_owned());
            };
            let value = unquote(re::group(line, &found, 1).unwrap_or_default());
            if !UNIT_KINDS.contains(&value) {
                return Err(format!(
                    "unit \"{}\" has invalid kind \"{value}\" (expected {})",
                    current.name,
                    UNIT_KINDS.join("|")
                ));
            }
            current.kind = Some(value.to_owned());
            continue;
        }
        // A dependency on a line of its own, under `depends_on:`.
        if let Some(found) = re::find(r"^\s*-\s+(.+?)\s*$", "", line)
            && let Some(current) = current
        {
            current
                .depends_on
                .push(unquote(re::group(line, &found, 1).unwrap_or_default()).to_owned());
            continue;
        }
        return Err(format!("unrecognised line in units block: {}", trim(line)));
    }
    for unit in &units {
        if let Some(refusal) = validate_unit_name(&unit.name) {
            return Err(refusal);
        }
    }
    Ok(units)
}

/// The batches, by levels: each round takes every unit none of whose
/// dependencies is still waiting. `None` when a round takes nothing, which
/// is a cycle.
fn batches(units: &[Unit]) -> Option<Vec<Vec<String>>> {
    let mut remaining: Vec<&Unit> = units.iter().collect();
    let mut batches = Vec::new();
    while !remaining.is_empty() {
        let waiting: HashSet<&str> = remaining.iter().map(|unit| unit.name.as_str()).collect();
        let mut level: Vec<String> = remaining
            .iter()
            .filter(|unit| {
                !unit
                    .depends_on
                    .iter()
                    .any(|dep| waiting.contains(dep.as_str()))
            })
            .map(|unit| unit.name.clone())
            .collect();
        if level.is_empty() {
            return None;
        }
        level.sort_by(|a, b| cmp_utf16(a, b));
        remaining.retain(|unit| !level.contains(&unit.name));
        batches.push(level);
    }
    Some(batches)
}

/// The units of a `unit-of-work-dependency.md` and the order they can be
/// built in.
///
/// # Errors
///
/// The document has no units block, the block cannot be read (with what was
/// wrong: a line it does not know, a name that cannot be a unit's, a name
/// twice or twice but for letter case, a unit that depends on itself or on
/// one the block does not list, no unit at all), or the dependencies go
/// round in a circle.
pub fn parse_bolt_dag(body: &str) -> Result<BoltDag, Unreadable> {
    let Some(block) = units_block(body) else {
        return Err(Unreadable {
            reason: "absent",
            detail: "no fenced ```yaml units: block found".to_owned(),
        });
    };
    let units = parse_units_block(&block).map_err(malformed)?;
    if units.is_empty() {
        return Err(malformed("units: block has no entries"));
    }
    let mut names: HashSet<&str> = HashSet::new();
    let mut folded: HashMap<String, &str> = HashMap::new();
    for unit in &units {
        if !names.insert(&unit.name) {
            return Err(malformed(format!("duplicate unit name: {}", unit.name)));
        }
        // Names are ASCII by now, so lower case is ASCII's.
        if let Some(existing) = folded.insert(unit.name.to_ascii_lowercase(), &unit.name) {
            return Err(malformed(format!(
                "case-folding unit name collision: \"{existing}\" and \"{}\"",
                unit.name
            )));
        }
    }
    for unit in &units {
        for dep in &unit.depends_on {
            if *dep == unit.name {
                return Err(malformed(format!(
                    "unit \"{}\" depends on itself",
                    unit.name
                )));
            }
            if !names.contains(dep.as_str()) {
                return Err(malformed(format!(
                    "unit \"{}\" depends on unknown unit \"{dep}\"",
                    unit.name
                )));
            }
        }
    }
    let Some(batches) = batches(&units) else {
        return Err(Unreadable {
            reason: "cyclic",
            detail: "dependency cycle detected".to_owned(),
        });
    };
    Ok(BoltDag { units, batches })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = "# Dependencies\n\n```mermaid\ngraph TD\n```\n\n```yaml\nunits:\n  - name: api\n    depends_on: [auth, \"store\"]\n  - name: auth\n    kind: service\n    depends_on: []\n  - name: store\n  - name: web\n    depends_on:\n      - api\n      - 'auth'\n```\n";

    #[test]
    fn the_block_gives_units_and_batches() {
        let dag = parse_bolt_dag(DOCUMENT).unwrap();
        let names: Vec<&str> = dag.units.iter().map(|unit| unit.name.as_str()).collect();
        assert_eq!(names, ["api", "auth", "store", "web"]);
        assert_eq!(dag.units[0].depends_on, ["auth", "store"]);
        assert_eq!(dag.units[1].kind.as_deref(), Some("service"));
        assert_eq!(dag.units[3].depends_on, ["api", "auth"]);
        assert_eq!(
            dag.batches,
            [vec!["auth", "store"], vec!["api"], vec!["web"]]
        );
    }

    #[test]
    fn a_block_that_cannot_be_read_says_why() {
        let reason = |body: &str| {
            let refused = parse_bolt_dag(body).unwrap_err();
            format!("{}: {}", refused.reason, refused.detail)
        };
        let block = |inner: &str| format!("```yaml\nunits:\n{inner}\n```\n");
        assert_eq!(
            reason("```yaml\nother: 1\n```\n"),
            "absent: no fenced ```yaml units: block found"
        );
        assert_eq!(
            reason("```yaml\nunits: [a]\n```"),
            "malformed: units: must be a block list, not an inline value"
        );
        assert_eq!(reason(&block("")), "malformed: units: block has no entries");
        assert_eq!(
            reason(&block("  depends_on: [a]")),
            "malformed: depends_on: before any - name: entry"
        );
        assert_eq!(
            reason(&block("  kind: ui")),
            "malformed: kind: before any - name: entry"
        );
        assert_eq!(
            reason(&block("  - name: a\n    kind: big")),
            "malformed: unit \"a\" has invalid kind \"big\" (expected service|spec|ui|packaging|library)"
        );
        assert_eq!(
            reason(&block("  - name: a\n    note: x")),
            "malformed: unrecognised line in units block: note: x"
        );
        assert_eq!(
            reason(&block("  - name: a\n  - name: a")),
            "malformed: duplicate unit name: a"
        );
        assert_eq!(
            reason(&block("  - name: Api\n  - name: api")),
            "malformed: case-folding unit name collision: \"Api\" and \"api\""
        );
        assert_eq!(
            reason(&block("  - name: a\n    depends_on: [a]")),
            "malformed: unit \"a\" depends on itself"
        );
        assert_eq!(
            reason(&block("  - name: a\n    depends_on: [b]")),
            "malformed: unit \"a\" depends on unknown unit \"b\""
        );
        assert_eq!(
            reason(&block(
                "  - name: a\n    depends_on: [b]\n  - name: b\n    depends_on: [a]"
            )),
            "cyclic: dependency cycle detected"
        );
        assert_eq!(
            reason(&block("  - name: a b")),
            "malformed: Invalid Unit name \"a b\" - must match /^[A-Za-z0-9][A-Za-z0-9._-]*$/ (ASCII letter/digit, then ASCII letters/digits/dot/underscore/hyphen)"
        );
        let long = "x".repeat(65);
        assert_eq!(
            reason(&block(&format!("  - name: {long}"))),
            format!(
                "malformed: Unit name \"{}...\" is 65 chars; max is 64",
                "x".repeat(32)
            )
        );
        assert_eq!(
            reason(&block("  - name: ''")),
            "malformed: Unit name is empty"
        );
    }

    #[test]
    fn a_list_on_one_line_is_read_with_its_quotes() {
        assert_eq!(
            inline_deps(" [a, 'b, c', \"d\\\"]\", e]  # why"),
            ["a", "b, c", "d\\\"]", "e"]
        );
        assert_eq!(inline_deps("[a, , b,]"), ["a", "b"]);
        assert_eq!(inline_deps("solo"), ["solo"]);
        // Not closed, a quote left open, or something after the list.
        for unreadable in ["[a, b", "[a, 'b]", "[a] b", "[a]]"] {
            assert!(inline_deps(unreadable).is_empty(), "{unreadable}");
        }
        assert!(inline_deps("").is_empty() && inline_deps(" [] ").is_empty());
    }
}

//! Frontmatter readers.
//!
//! Upstream ships no YAML dependency: stage, scope, agent, sensor and rule
//! files are read by a handful of regex-driven helpers in `aidlc-lib.ts`. The
//! accepted grammar is whatever those helpers accept, quirks included, and the
//! authored Markdown under `core/` is written against it. A general YAML parser
//! would read some of those files differently, so this crate ports the helpers
//! themselves: same patterns (run through `focus_js::re`), same order of
//! checks, same error text.
//!
//! Each function names the upstream function it mirrors. Line references are
//! to `core/tools/aidlc-lib.ts` at upstream commit `2a555f7`.

use focus_js::text::{is_js_whitespace, split_lines, trim, trim_end};
use focus_js::{EngineError, Result, bail, re};
use serde_json::{Map, Number, Value};

/// `frontmatterBlock` (L36654): the text between the leading `---` fences, or
/// `None` when the body does not open with a frontmatter block.
#[must_use]
pub fn frontmatter_block(body: &str) -> Option<&str> {
    let m = re::find(r"^---\r?\n([\s\S]*?)\r?\n---", "", body)?;
    re::group(body, &m, 1)
}

/// `body.replace(/^﻿/, "")`.
#[must_use]
pub fn strip_bom(body: &str) -> &str {
    body.strip_prefix('\u{FEFF}').unwrap_or(body)
}

/// `scalarField` (L36677): the value of a top-level `key: value` line.
///
/// Returns `""` for an absent key, for a YAML block-scalar marker (`>`, `|`,
/// `>-`, `|-`), and for an empty quoted value; surrounding quotes are removed.
/// `\s*` after the colon spans newlines, so a key with nothing on its own line
/// reads the next non-blank line as its value. That is upstream's behaviour,
/// kept as is: list-valued keys are read by the list helpers, never by this
/// one.
#[must_use]
pub fn scalar_field(fm: &str, key: &str) -> String {
    let Some(m) = re::find(&format!(r"^{key}:\s*(.+?)\s*$"), "m", fm) else {
        return String::new();
    };
    let raw = trim(re::group(fm, &m, 1).unwrap_or(""));
    if matches!(raw, ">" | "|" | ">-" | "|-") {
        return String::new();
    }
    strip_matching_quotes(raw).to_owned()
}

/// Whether a top-level `key:` line exists (`hasTopLevelField`).
#[must_use]
pub fn has_top_level_field(fm: &str, key: &str) -> bool {
    re::test(&format!("^{key}:"), "m", fm)
}

/// `listField` (L36701): a list of strings, written as a block sequence or as
/// a single-line flow sequence. Absent key and empty list both read as `[]`.
#[must_use]
pub fn list_field(fm: &str, key: &str) -> Vec<String> {
    let block = format!(r"^{key}:\s*\n((?:[ \t]+-[ \t]+[^\r\n]+\r?\n?)+)");
    if let Some(m) = re::find(&block, "m", fm) {
        let body = re::group(fm, &m, 1).unwrap_or("");
        return split_lines(body)
            .into_iter()
            .filter_map(|line| {
                let item = re::find(r"^\s*-[ \t]+(.+?)\s*$", "", line)?;
                let value = re::group(line, &item, 1)?;
                // `.replace(/^["']|["']$/g, "")`: one quote off each end,
                // independently, so a lone leading quote is still removed.
                let value = value.strip_prefix(['"', '\'']).unwrap_or(value);
                let value = value.strip_suffix(['"', '\'']).unwrap_or(value);
                (!value.is_empty()).then(|| value.to_owned())
            })
            .collect();
    }
    let flow = format!(r"^{key}:[ \t]*(\[[^\r\n]*)$");
    re::find(&flow, "m", fm)
        .and_then(|m| re::group(fm, &m, 1))
        .map(parse_inline_list)
        .unwrap_or_default()
}

/// `parseInlineDepsList` (L41594): a flow sequence `[a, "b", 'c']`, with an
/// optional trailing `# comment`. An unbalanced bracket or quote, or anything
/// but a comment after the closing bracket, reads as an empty list. A bare
/// scalar reads as a one-item list.
#[must_use]
pub fn parse_inline_list(raw: &str) -> Vec<String> {
    let t = trim(raw);
    if t.is_empty() || t == "[]" {
        return Vec::new();
    }
    if !t.starts_with('[') {
        return vec![unquote_scalar(t).to_owned()];
    }
    // Every byte the scan compares against is ASCII, and ASCII bytes never
    // occur inside a multi-byte UTF-8 sequence, so walking bytes makes the same
    // decisions as upstream walking UTF-16 code units.
    let bytes = t.as_bytes();
    let mut close = None;
    let mut quote: Option<u8> = None;
    let mut i = 1;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(b'"') if b == b'\\' => i += 1,
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b']' => {
                close = Some(i);
                break;
            }
            None => {}
        }
        i += 1;
    }
    let Some(close) = close else {
        return Vec::new();
    };
    if quote.is_some() || !re::test(r"^[ \t]*(?:#[^\r\n]*)?$", "", &t[close + 1..]) {
        return Vec::new();
    }

    let body = &t[1..close];
    let bytes = body.as_bytes();
    let mut items = Vec::new();
    let mut start = 0;
    let mut quote: Option<u8> = None;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match quote {
            Some(b'"') if b == b'\\' => i += 1,
            Some(q) if b == q => quote = None,
            Some(_) => {}
            None if b == b'"' || b == b'\'' => quote = Some(b),
            None if b == b',' => {
                items.push(&body[start..i]);
                start = i + 1;
            }
            None => {}
        }
        i += 1;
    }
    if quote.is_some() {
        return Vec::new();
    }
    items.push(&body[start.min(body.len())..]);
    items
        .into_iter()
        .map(unquote_scalar)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `unquoteScalar` (L41583): trim, then drop one pair of matching quotes.
#[must_use]
pub fn unquote_scalar(s: &str) -> &str {
    strip_matching_quotes(trim(s))
}

/// The quote rule `scalarField`, `coerceScalar` and `unquoteScalar` share:
/// drop the first and last character when the text starts and ends with the
/// same kind of quote. A lone quote character satisfies both tests and reads
/// as the empty string, as `raw.slice(1, -1)` does.
fn strip_matching_quotes(raw: &str) -> &str {
    let quoted = (raw.starts_with('"') && raw.ends_with('"'))
        || (raw.starts_with('\'') && raw.ends_with('\''));
    if !quoted {
        return raw;
    }
    if raw.len() < 2 {
        ""
    } else {
        &raw[1..raw.len() - 1]
    }
}

/// `coerceScalar` (L37454): bare `true`/`false` become booleans, anything
/// else a string with one pair of matching quotes removed.
fn coerce_scalar(raw: &str) -> Value {
    match raw {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => Value::String(strip_matching_quotes(raw).to_owned()),
    }
}

/// `objectListField` (L37387): a block list of flat objects, the shape of a
/// stage's `consumes:`.
///
/// ```yaml
/// consumes:
///   - artifact: intent-statement
///     required: true
/// ```
///
/// # Errors
///
/// A blank line between items, or a line inside the block that is neither an
/// item start nor a `key: value` continuation.
pub fn object_list_field(fm: &str, key: &str) -> Result<Vec<Value>> {
    let block = format!(
        r"^{key}:\s*\n((?:[ \t]+-[ \t]+[^\n]+(?:\r?\n|$)(?:[ \t]+[^- \t\n][^\n]*(?:\r?\n|$))*)+)"
    );
    let Some(m) = re::find(&block, "m", fm) else {
        return Ok(Vec::new());
    };

    // The block pattern stops at the first blank line. If list items continue
    // after it, the author separated items with a blank line and the tail
    // would be dropped silently; refuse instead.
    for line in split_lines(&fm[m.range().end..]) {
        if line.is_empty() || re::test(r"^[ \t]+$", "", line) {
            continue;
        }
        if re::test(r"^[ \t]+-[ \t]", "", line) {
            bail!("Blank line not allowed inside {key}[] block — list items must be consecutive");
        }
        break;
    }

    let body = re::group(fm, &m, 1).unwrap_or("");
    let mut items: Vec<Value> = Vec::new();
    let mut current: Option<Map<String, Value>> = None;
    for line in split_lines(body) {
        if trim(line).is_empty() {
            continue;
        }
        let item = re::find(r"^\s*-\s+([a-z_]+):\s*(.+?)\s*$", "", line);
        let sub = re::find(r"^\s+([a-z_]+):\s*(.+?)\s*$", "", line);
        if let Some(item) = item {
            if let Some(done) = current.take() {
                items.push(Value::Object(done));
            }
            let mut object = Map::new();
            insert_pair(&mut object, line, &item);
            current = Some(object);
        } else if let (Some(sub), Some(object)) = (sub, current.as_mut()) {
            insert_pair(object, line, &sub);
        } else {
            bail!("Malformed {key}[] entry in frontmatter: {}", trim(line));
        }
    }
    if let Some(done) = current {
        items.push(Value::Object(done));
    }
    Ok(items)
}

fn insert_pair(object: &mut Map<String, Value>, line: &str, m: &re::Match) {
    let key = re::group(line, m, 1).unwrap_or("");
    let value = re::group(line, m, 2).unwrap_or("");
    object.insert(key.to_owned(), coerce_scalar(value));
}

/// `mapOfListsField` (L37342): a block of `name: [a, b]` lines, the shape of a
/// stage's `produces_kinds:`. Keys keep authored order.
///
/// # Errors
///
/// A line that is not `name: value`, or a value that is not an inline list.
pub fn map_of_lists_field(fm: &str, key: &str) -> Result<Map<String, Value>> {
    let block = format!(r"^{key}:\s*\n((?:[ \t]+[a-z][a-z0-9-]*\s*:\s*[^\n]*(?:\r?\n|$))+)");
    let mut out = Map::new();
    let Some(m) = re::find(&block, "m", fm) else {
        return Ok(out);
    };
    for line in split_lines(re::group(fm, &m, 1).unwrap_or("")) {
        if trim(line).is_empty() {
            continue;
        }
        let Some(entry) = re::find(r"^\s+([a-z][a-z0-9-]*)\s*:\s*(.+?)\s*$", "", line) else {
            bail!("Malformed {key} entry in frontmatter: {}", trim(line));
        };
        let name = re::group(line, &entry, 1).unwrap_or("");
        let value = trim(re::group(line, &entry, 2).unwrap_or(""));
        if !(value.starts_with('[') && value.ends_with(']')) {
            bail!("{key}.{name} must be an inline list (e.g. [service, ui]), got: {value}");
        }
        out.insert(name.to_owned(), string_list(parse_inline_list(value)));
    }
    Ok(out)
}

/// `stripYamlComment` (L37233): cut a `# comment` that starts the value or
/// follows whitespace, unless it sits inside quotes.
#[must_use]
pub fn strip_yaml_comment(value: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut previous: Option<char> = None;
    for (idx, c) in value.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && previous.is_none_or(is_js_whitespace) => {
                return trim_end(&value[..idx]);
            }
            None => {}
        }
        previous = Some(c);
    }
    value
}

/// `arsField` (L37270): the nested `ars:` map a plugin stage may carry
/// (`targets`, `cost`, `role`, `project_types`).
///
/// Known children are assembled in that canonical order whatever order they
/// were authored in; a child that does not fit its shape is kept as the raw
/// string, and an unknown child under its own name, so the schema validator
/// can reject each by name. A bare `ars: <scalar>` returns that scalar.
///
/// # Errors
///
/// A nested line under a child that already has an inline value.
pub fn ars_field(fm: &str) -> Result<Value> {
    let lines = split_lines(fm);
    let Some(start) = lines
        .iter()
        .position(|line| re::test(r"^ars:[ \t]*$", "", line))
    else {
        return Ok(Value::String(scalar_field(fm, "ars")));
    };

    let mut raw: Vec<(String, String)> = Vec::new();
    let mut last_key: Option<usize> = None;
    for line in &lines[start + 1..] {
        if trim(line).is_empty() || re::test(r"^\s*#", "", line) {
            continue;
        }
        if !re::test(r"^[ \t]", "", line) {
            break;
        }
        if let Some(entry) = re::find(r"^\s+([a-z_][a-z0-9_]*)\s*:\s*(.*?)\s*$", "", line) {
            let key = re::group(line, &entry, 1).unwrap_or("");
            let value = strip_yaml_comment(re::group(line, &entry, 2).unwrap_or("")).to_owned();
            // Map.set semantics: a repeated key keeps its first position.
            let slot = match raw.iter().position(|(k, _)| k == key) {
                Some(slot) => {
                    raw[slot].1 = value;
                    slot
                }
                None => {
                    raw.push((key.to_owned(), value));
                    raw.len() - 1
                }
            };
            last_key = Some(slot);
        } else if last_key.is_none_or(|slot| !raw[slot].1.is_empty()) {
            // Only a deeper line under a child with no inline value (a block
            // list) is tolerated; that child stays empty for the validator.
            bail!("Malformed ars entry in frontmatter: {}", trim(line));
        }
    }
    if raw.is_empty() {
        return Ok(Value::String(scalar_field(fm, "ars")));
    }

    let get = |name: &str| raw.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
    let mut out = Map::new();
    if let Some(targets) = get("targets") {
        out.insert("targets".to_owned(), ars_inline_list(targets));
    }
    if let Some(cost) = get("cost") {
        let value = if cost == "null" || cost == "~" {
            Value::Null
        } else if re::test(r"^-?\d+(\.\d+)?$", "", cost) {
            js_number(cost)
        } else {
            Value::String(cost.to_owned())
        };
        out.insert("cost".to_owned(), value);
    }
    if let Some(role) = get("role") {
        let unquoted = re::find(r#"^"(.*)"$"#, "", role)
            .or_else(|| re::find(r"^'(.*)'$", "", role))
            .and_then(|m| re::group(role, &m, 1))
            .unwrap_or(role);
        out.insert("role".to_owned(), Value::String(unquoted.to_owned()));
    }
    if let Some(project_types) = get("project_types") {
        out.insert("project_types".to_owned(), ars_inline_list(project_types));
    }
    for (key, value) in &raw {
        if !out.contains_key(key) {
            out.insert(key.clone(), Value::String(value.clone()));
        }
    }
    Ok(Value::Object(out))
}

/// The `inlineList` closure inside `arsField`: a well-formed flow sequence
/// becomes a list; an empty item, a nested or unbalanced bracket keeps the raw
/// text so it cannot read as a shorter list.
fn ars_inline_list(v: &str) -> Value {
    if !v.starts_with('[') || !v.ends_with(']') {
        return Value::String(v.to_owned());
    }
    let inner = if v.len() < 2 { "" } else { &v[1..v.len() - 1] };
    let mut segments: Vec<&str> = inner.split(',').map(trim).collect();
    if segments.last() == Some(&"") {
        segments.pop();
    }
    let items = parse_inline_list(v);
    if segments.iter().all(|s| !s.is_empty()) && items.len() == segments.len() {
        string_list(items)
    } else {
        Value::String(v.to_owned())
    }
}

/// `Number(text)` for a decimal literal the caller has already shape-checked.
///
/// Integral values up to 2^53 become JSON integers, which print as JS prints
/// them. Larger or fractional values fall back to an `f64`, whose text form
/// can differ from JS; no validated frontmatter value reaches that branch.
#[must_use]
pub fn js_number(text: &str) -> Value {
    const MAX_SAFE: f64 = 9_007_199_254_740_992.0;
    let parsed: f64 = text.parse().unwrap_or(f64::NAN);
    if parsed.fract() == 0.0 && parsed.abs() <= MAX_SAFE {
        #[allow(clippy::cast_possible_truncation)]
        return Value::Number(Number::from(parsed as i64));
    }
    Number::from_f64(parsed).map_or(Value::Null, Value::Number)
}

fn string_list(items: Vec<String>) -> Value {
    Value::Array(items.into_iter().map(Value::String).collect())
}

/// Build the error upstream raises when a file has no frontmatter block.
#[must_use]
pub fn missing_frontmatter(what: &str) -> EngineError {
    EngineError::new(format!("{what} missing YAML frontmatter (---...---)"))
}

#[cfg(test)]
mod tests;

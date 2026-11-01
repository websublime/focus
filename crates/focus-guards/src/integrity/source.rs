//! The lexer runtime integrity reads a script with, and the walk over its
//! tokens that finds the execution sites: a hook module imported or
//! required, `eval`/`Function`/`vm`, `child_process`, `Bun.spawn`,
//! `subprocess`, an interpreter named in a template.
//!
//! This is a lexer for the recognized execution sites, not a JS/Python
//! runtime. Comments, regex bodies and strings are opaque; template
//! interpolations are code; literal values are opened only by
//! import/require or an execution sink. Only literal, statically bound API
//! references resolve: a familiar property name on an arbitrary receiver is
//! not proof that it executes code.
//!
//! Mirrors `sourceTokens` through `protectedContent` in
//! `runtime-integrity.ts`. Positions are character indices into the source.

use std::collections::BTreeMap;

use focus_js::re;
use focus_js::text::is_js_whitespace;

use super::MAX_EXECUTION_DEPTH;
use super::shell::Walk;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Word,
    Str,
    Group,
    Symbol,
    Regexp,
    Api,
}

/// `SourceToken`.
#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub kind: Kind,
    pub text: String,
    pub start: usize,
    pub end: usize,
    /// A string literal's decoded value; `None` for a dynamic template.
    pub value: Option<String>,
    pub children: Vec<Token>,
    /// Whether `children` is present at all (an empty group still is).
    has_children: bool,
}

impl Token {
    fn api(name: &str) -> Self {
        Self {
            kind: Kind::Api,
            text: name.to_owned(),
            start: 0,
            end: 0,
            value: None,
            children: Vec::new(),
            has_children: false,
        }
    }

    fn symbol(text: String, start: usize, end: usize) -> Self {
        Self {
            kind: Kind::Symbol,
            text,
            start,
            end,
            value: None,
            children: Vec::new(),
            has_children: false,
        }
    }

    /// `token.value ?? token.text`.
    fn value_or_text(&self) -> &str {
        self.value.as_deref().unwrap_or(&self.text)
    }
}

pub(crate) type Bindings = BTreeMap<String, Vec<Token>>;

fn line_terminator(ch: char) -> bool {
    matches!(ch, '\r' | '\n' | '\u{2028}' | '\u{2029}')
}

fn text_of(chars: &[char], from: usize, to: usize) -> String {
    let (from, to) = (from.min(chars.len()), to.min(chars.len()));
    if from >= to {
        return String::new();
    }
    chars[from..to].iter().collect()
}

/// `decodeLiteral`: the escapes of a string literal.
fn decode_literal(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '\\' || i + 1 >= chars.len() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let next = chars[i + 1];
        // A line continuation: `\` before a newline is nothing.
        if next == '\n' {
            i += 2;
            continue;
        }
        if next == '\r' {
            i += if chars.get(i + 2) == Some(&'\n') {
                3
            } else {
                2
            };
            continue;
        }
        if next == 'u' && chars.get(i + 2) == Some(&'{') {
            let mut j = i + 3;
            while j < chars.len() && chars[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j > i + 3 && chars.get(j) == Some(&'}') {
                let hex: String = chars[i + 3..j].iter().collect();
                let point = u32::from_str_radix(&hex, 16).unwrap_or(u32::MAX);
                match char::from_u32(point) {
                    Some(decoded) if point <= 0x10ffff => out.push(decoded),
                    // A code point the string cannot hold stays as written.
                    _ => out.extend(&chars[i..=j]),
                }
                i = j + 1;
                continue;
            }
        }
        if next == 'u'
            && i + 6 <= chars.len()
            && chars[i + 2..i + 6].iter().all(char::is_ascii_hexdigit)
        {
            let hex: String = chars[i + 2..i + 6].iter().collect();
            let point = u32::from_str_radix(&hex, 16).unwrap_or(0);
            // A lone surrogate has no character: it stays as written.
            match char::from_u32(point) {
                Some(decoded) => out.push(decoded),
                None => out.extend(&chars[i..i + 6]),
            }
            i += 6;
            continue;
        }
        if next == 'x'
            && i + 4 <= chars.len()
            && chars[i + 2..i + 4].iter().all(char::is_ascii_hexdigit)
        {
            let hex: String = chars[i + 2..i + 4].iter().collect();
            out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap_or(0)).unwrap_or('\0'));
            i += 4;
            continue;
        }
        out.push(match next {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'v' => '\u{b}',
            '0' => '\0',
            other => other,
        });
        i += 2;
    }
    out
}

/// `sourceTokens`.
pub(crate) fn source_tokens(source: &str, python: bool) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut index = 0;
    read_tokens(&chars, &mut index, python, None)
}

fn starts_with_at(chars: &[char], index: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, wanted)| chars.get(index + offset) == Some(&wanted))
}

/// The `read` closure of `sourceTokens`: tokens up to `stop` (the closer of
/// a template interpolation) or the end.
fn read_tokens(chars: &[char], index: &mut usize, python: bool, stop: Option<char>) -> Vec<Token> {
    // Groups use an explicit stack so large nested data fixtures need no
    // recursion: `frames` holds the token list of each open group.
    let mut frames: Vec<Vec<Token>> = vec![Vec::new()];
    // The open groups, as (opener, start, the frame index its children are
    // collected in).
    let mut groups: Vec<(char, usize)> = Vec::new();
    while *index < chars.len() {
        let start = *index;
        let ch = chars[*index];
        if is_js_whitespace(ch) {
            *index += 1;
            continue;
        }
        let line_comment = (!python && starts_with_at(chars, *index, "//"))
            || (ch == '#'
                && (python
                    || (groups.is_empty() && {
                        let line_start = chars[..*index]
                            .iter()
                            .rposition(|c| *c == '\n')
                            .map_or(0, |at| at + 1);
                        chars[line_start..*index]
                            .iter()
                            .all(|c| is_js_whitespace(*c))
                    })));
        if line_comment {
            while *index < chars.len() && !line_terminator(chars[*index]) {
                *index += 1;
            }
            continue;
        }
        if !python && starts_with_at(chars, *index, "/*") {
            let mut end = *index + 2;
            loop {
                if end >= chars.len() {
                    *index = chars.len();
                    break;
                }
                if starts_with_at(chars, end, "*/") {
                    *index = end + 2;
                    break;
                }
                end += 1;
            }
            continue;
        }
        if ch == '\'' || ch == '"' || ch == '`' {
            let triple = python && starts_with_at(chars, *index, &ch.to_string().repeat(3));
            let delimiter: String = if triple {
                ch.to_string().repeat(3)
            } else {
                ch.to_string()
            };
            *index += delimiter.chars().count();
            let mut raw = String::new();
            let mut dynamic = false;
            let mut children: Vec<Token> = Vec::new();
            while *index < chars.len() && !starts_with_at(chars, *index, &delimiter) {
                if chars[*index] == '\\' {
                    raw.push_str(&text_of(chars, *index, *index + 2));
                    *index += 2;
                } else if ch == '`' && starts_with_at(chars, *index, "${") {
                    dynamic = true;
                    *index += 2;
                    children.extend(read_tokens(chars, index, python, Some('}')));
                } else {
                    raw.push(chars[*index]);
                    *index += 1;
                }
            }
            *index = chars.len().min(*index + delimiter.chars().count());
            let has_children = !children.is_empty();
            frames.last_mut().expect("a frame").push(Token {
                kind: Kind::Str,
                text: text_of(chars, start, *index),
                start,
                end: *index,
                value: if dynamic {
                    None
                } else {
                    Some(decode_literal(&raw))
                },
                children,
                has_children,
            });
            continue;
        }
        if "([{".contains(ch) {
            groups.push((ch, start));
            frames.push(Vec::new());
            *index += 1;
            continue;
        }
        if ")]}".contains(ch) {
            *index += 1;
            if frames.len() == 1 && Some(ch) == stop {
                return frames.pop().expect("the root frame");
            }
            let closes = groups.last().is_some_and(|(opener, _)| {
                matches!((opener, ch), ('(', ')') | ('[', ']') | ('{', '}'))
            });
            if closes {
                let (opener, group_start) = groups.pop().expect("an open group");
                let children = frames.pop().expect("the group's frame");
                frames.last_mut().expect("a frame").push(Token {
                    kind: Kind::Group,
                    text: opener.to_string(),
                    start: group_start,
                    end: *index,
                    value: None,
                    children,
                    has_children: true,
                });
            } else {
                frames.last_mut().expect("a frame").push(Token::symbol(
                    ch.to_string(),
                    start,
                    *index,
                ));
            }
            continue;
        }
        let tokens = frames.last_mut().expect("a frame");
        let previous = tokens.last();
        let regexp_may_start = previous.is_none_or(|previous| {
            (previous.kind == Kind::Symbol && "=,:;!?&|+-*".contains(previous.text.as_str()))
                || (previous.kind == Kind::Word
                    && matches!(
                        previous.text.as_str(),
                        "return" | "throw" | "case" | "yield" | "await"
                    ))
        });
        if !python && ch == '/' && regexp_may_start {
            *index += 1;
            let mut bracket = false;
            while *index < chars.len() {
                let current = chars[*index];
                *index += 1;
                if current == '\\' {
                    *index += 1;
                    continue;
                }
                if current == '[' {
                    bracket = true;
                }
                if current == ']' {
                    bracket = false;
                }
                if current == '/' && !bracket {
                    break;
                }
                if line_terminator(current) {
                    break;
                }
            }
            while *index < chars.len() && chars[*index].is_ascii_alphabetic() {
                *index += 1;
            }
            tokens.push(Token {
                kind: Kind::Regexp,
                text: text_of(chars, start, *index),
                start,
                end: *index,
                value: None,
                children: Vec::new(),
                has_children: false,
            });
            continue;
        }
        let word_length = word_at(chars, *index);
        *index += word_length.unwrap_or(1);
        tokens.push(Token {
            kind: if word_length.is_some() {
                Kind::Word
            } else {
                Kind::Symbol
            },
            text: text_of(chars, start, *index),
            start,
            end: *index,
            value: None,
            children: Vec::new(),
            has_children: false,
        });
    }
    // Groups left open close at the end of the source, nested as they were.
    while let Some((opener, group_start)) = groups.pop() {
        let children = frames.pop().expect("the group's frame");
        frames.last_mut().expect("a frame").push(Token {
            kind: Kind::Group,
            text: opener.to_string(),
            start: group_start,
            end: chars.len(),
            value: None,
            children,
            has_children: true,
        });
    }
    frames.pop().expect("the root frame")
}

/// `/^[A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)*/` at `index`: its length.
fn word_at(chars: &[char], index: usize) -> Option<usize> {
    let starts = |c: char| c.is_ascii_alphabetic() || c == '_' || c == '$';
    let continues = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    let mut at = index;
    if !chars.get(at).is_some_and(|c| starts(*c)) {
        return None;
    }
    at += 1;
    while chars.get(at).is_some_and(|c| continues(*c)) {
        at += 1;
    }
    while chars.get(at) == Some(&'.') && chars.get(at + 1).is_some_and(|c| starts(*c)) {
        at += 2;
        while chars.get(at).is_some_and(|c| continues(*c)) {
            at += 1;
        }
    }
    Some(at - index)
}

/// `argumentParts`: the tokens split at top-level commas.
fn argument_parts(tokens: &[Token]) -> Vec<Vec<Token>> {
    let mut parts: Vec<Vec<Token>> = vec![Vec::new()];
    for token in tokens {
        if token.text == "," && token.kind == Kind::Symbol {
            parts.push(Vec::new());
        } else {
            parts.last_mut().expect("a part").push(token.clone());
        }
    }
    parts
}

fn global_bindings(python: bool) -> Bindings {
    let globals: &[(&str, &str)] = if python {
        &[
            ("eval", "py:eval"),
            ("exec", "py:exec"),
            ("__import__", "py:__import__"),
        ]
    } else {
        &[
            ("Bun", "bun"),
            ("Deno", "deno"),
            ("eval", "js:eval"),
            ("Function", "js:Function"),
            ("require", "js:require"),
            ("process", "node:process"),
            ("globalThis", "js:global"),
            ("global", "js:global"),
        ]
    };
    globals
        .iter()
        .map(|(name, api)| ((*name).to_owned(), vec![Token::api(api)]))
        .collect()
}

fn module_api(name: Option<&str>, python: bool) -> Option<String> {
    let name = name.filter(|name| !name.is_empty())?;
    if python {
        return ["subprocess", "os", "builtins"]
            .contains(&name)
            .then(|| format!("py:{name}"));
    }
    let module = name.strip_prefix("node:").unwrap_or(name);
    if ["child_process", "vm", "process", "module"].contains(&module) {
        Some(format!("node:{module}"))
    } else if name == "bun" {
        Some("bun".to_owned())
    } else {
        None
    }
}

const API_MEMBERS: [(&str, &[&str]); 8] = [
    ("bun", &["spawn", "spawnSync"]),
    ("deno", &["run", "Command"]),
    (
        "node:child_process",
        &[
            "exec",
            "execSync",
            "execFile",
            "execFileSync",
            "spawn",
            "spawnSync",
        ],
    ),
    (
        "node:vm",
        &[
            "runInThisContext",
            "runInNewContext",
            "runInContext",
            "Script",
            "compileFunction",
        ],
    ),
    ("node:process", &["execPath"]),
    ("node:module", &["createRequire"]),
    (
        "py:subprocess",
        &["run", "call", "Popen", "check_call", "check_output"],
    ),
    ("py:os", &["system", "popen"]),
];

fn api_members(api: &str) -> Option<&'static [&'static str]> {
    API_MEMBERS
        .iter()
        .find(|(module, _)| *module == api)
        .map(|(_, members)| *members)
}

fn api_member(api: Option<&str>, member: &str, bindings: &Bindings) -> Option<String> {
    let api = api?;
    if api == "js:global" {
        if bindings.contains_key(&format!("@global:{member}")) {
            return None;
        }
        return global_bindings(false)
            .get(member)
            .and_then(|tokens| tokens.first())
            .map(|token| token.text.clone());
    }
    if api == "py:builtins" && ["eval", "exec", "__import__"].contains(&member) {
        return Some(format!("py:{member}"));
    }
    if member == "default" && api.starts_with("node:") {
        return Some(api.to_owned());
    }
    if api_members(api).is_some_and(|members| members.contains(&member)) {
        let method = format!("{api}.{member}");
        if bindings.contains_key(&format!("@member:{method}")) {
            return None;
        }
        return Some(method);
    }
    if ["call", "apply", "bind"].contains(&member)
        && (re::test(
            r"^(?:js:(?:eval|Function|require)|py:(?:eval|exec|__import__))$",
            "",
            api,
        ) || API_MEMBERS
            .iter()
            .any(|(module, members)| members.iter().any(|name| api == format!("{module}.{name}"))))
    {
        return Some(format!("{api}.{member}"));
    }
    None
}

/// `apiReference`.
fn api_reference(
    tokens: &[Token],
    bindings: &Bindings,
    python: bool,
    depth: usize,
) -> Option<String> {
    if depth > MAX_EXECUTION_DEPTH || tokens.is_empty() {
        return None;
    }
    let mut index = 0;
    let awaited = tokens[0].text == "await";
    if awaited {
        index += 1;
    }
    let first = tokens.get(index)?;
    index += 1;
    let mut api: Option<String> = None;
    let mut dynamic_import = false;
    if first.kind == Kind::Api {
        api = Some(first.text.clone());
    } else if first.kind == Kind::Word {
        if awaited && first.text == "import" && !python {
            dynamic_import = true;
        } else if let Some(bound) = bindings.get(&first.text) {
            api = api_reference(bound, bindings, python, depth + 1);
        } else {
            let mut members = first.text.split('.');
            let root = members.next().unwrap_or("");
            api = api_reference(
                bindings.get(root).map_or(&[][..], Vec::as_slice),
                bindings,
                python,
                depth + 1,
            );
            for member in members {
                api = api_member(api.as_deref(), member, bindings);
            }
        }
    } else if first.text == "(" {
        let parts = argument_parts(&first.children);
        api = api_reference(parts.last().expect("a part"), bindings, python, depth + 1);
    }
    while index < tokens.len() {
        let token = &tokens[index];
        index += 1;
        if token.text == "."
            && tokens
                .get(index)
                .is_some_and(|next| next.kind == Kind::Word)
        {
            for member in tokens[index].text.split('.') {
                api = api_member(api.as_deref(), member, bindings);
            }
            index += 1;
        } else if token.text == "[" {
            let member = literal(&token.children, Some(bindings));
            api = match member {
                None => None,
                Some(member) => api_member(api.as_deref(), &member, bindings),
            };
        } else if token.text == "(" {
            let parts = argument_parts(&token.children);
            if dynamic_import
                || api.as_deref() == Some("js:require")
                || api.as_deref() == Some("py:__import__")
            {
                let python_import = api.as_deref() == Some("py:__import__");
                api = module_api(literal(&parts[0], Some(bindings)).as_deref(), python_import);
                dynamic_import = false;
            } else if api.as_deref() == Some("node:module.createRequire") {
                api = Some("js:require".to_owned());
            } else if api.as_deref().is_some_and(|api| api.ends_with(".bind")) {
                api = api.map(|api| api[..api.len() - ".bind".len()].to_owned());
            } else {
                return None;
            }
        } else {
            return None;
        }
    }
    api
}

/// `referenceStart`: where the callee expression that ends at `end` begins.
fn reference_start(tokens: &[Token], end: usize) -> usize {
    let mut start = end;
    while start > 0 {
        if tokens[start - 1].text == "." && start > 1 {
            start -= 2;
        } else if (tokens[start].text == "(" || tokens[start].text == "[")
            && (tokens[start - 1].kind == Kind::Word || tokens[start - 1].kind == Kind::Group)
            && !matches!(
                tokens[start - 1].text.as_str(),
                "return"
                    | "throw"
                    | "new"
                    | "await"
                    | "if"
                    | "while"
                    | "for"
                    | "switch"
                    | "catch"
                    | "function"
            )
        {
            start -= 1;
        } else {
            break;
        }
    }
    start
}

fn gap_has_terminator(chars: &[char], from: usize, to: usize) -> bool {
    chars[from.min(chars.len())..to.min(chars.len())]
        .iter()
        .any(|c| line_terminator(*c))
}

/// `statementEnd`: the index after the statement that starts at `start`.
fn statement_end(tokens: &[Token], start: usize, chars: &[char]) -> usize {
    let mut end = start;
    while end < tokens.len() {
        if tokens[end].text == ";" {
            break;
        }
        if end > start && gap_has_terminator(chars, tokens[end - 1].end, tokens[end].start) {
            break;
        }
        end += 1;
    }
    end
}

fn import_statement_end(tokens: &[Token], start: usize, chars: &[char], python: bool) -> usize {
    if python {
        return statement_end(tokens, start, chars);
    }
    if tokens
        .get(start + 1)
        .is_some_and(|next| next.kind == Kind::Str)
    {
        return start + 2;
    }
    let mut end = start + 1;
    while end < tokens.len() && tokens[end].text != ";" {
        if tokens[end].text == "from"
            && tokens
                .get(end + 1)
                .is_some_and(|next| next.kind == Kind::Str)
        {
            return end + 2;
        }
        end += 1;
    }
    statement_end(tokens, start, chars)
}

fn type_only_import(tokens: &[Token], start: usize) -> bool {
    tokens
        .get(start + 1)
        .is_some_and(|next| next.text == "type")
        && !matches!(
            tokens.get(start + 2).map(|token| token.text.as_str()),
            Some("from" | ",")
        )
}

fn bind_pattern(pattern: &[Token], value: &[Token], bindings: &mut Bindings, python: bool) {
    if pattern.len() != 1 {
        return;
    }
    let name = &pattern[0];
    let api = api_reference(value, bindings, python, 0);
    if name.kind == Kind::Word {
        let bound = match &api {
            Some(api) => vec![Token::api(api)],
            None => bound_value(value, Some(bindings)),
        };
        bindings.insert(name.text.clone(), bound);
    } else if name.text == "{" {
        for part in argument_parts(&name.children) {
            let Some(field) = part
                .first()
                .map(Token::value_or_text)
                .filter(|field| !field.is_empty())
            else {
                continue;
            };
            let alias = if part.get(1).is_some_and(|token| token.text == ":") {
                part.get(2)
            } else {
                part.first()
            };
            let Some(alias) = alias.filter(|alias| alias.kind == Kind::Word) else {
                continue;
            };
            let member = api_member(api.as_deref(), field, bindings);
            bindings.insert(
                alias.text.clone(),
                member.map_or_else(Vec::new, |member| vec![Token::api(&member)]),
            );
        }
    }
}

/// `importedBindings`: what an import statement binds; the index after it.
fn imported_bindings(
    tokens: &[Token],
    start: usize,
    chars: &[char],
    bindings: &mut Bindings,
    python: bool,
) -> usize {
    let end = import_statement_end(tokens, start, chars, python);
    let statement = &tokens[start..end];
    if python && tokens[start].text == "import" {
        for part in argument_parts(&statement[1..]) {
            let module = module_api(part.first().map(|token| token.text.as_str()), true);
            let alias = if part.get(1).is_some_and(|token| token.text == "as") {
                part.get(2).map(|token| token.text.clone())
            } else {
                part.first().map(|token| token.text.clone())
            };
            if let Some(alias) = alias.filter(|alias| !alias.is_empty()) {
                bindings.insert(
                    alias,
                    module.map_or_else(Vec::new, |module| vec![Token::api(&module)]),
                );
            }
        }
        return end;
    }
    let keyword = if python { "import" } else { "from" };
    let Some(from) = statement.iter().position(|token| token.text == keyword) else {
        return start;
    };
    let module = if python {
        module_api(statement.get(1).map(|token| token.text.as_str()), true)
    } else {
        module_api(literal(&statement[from + 1..], None).as_deref(), false)
    };
    let mut names: Vec<Token> = if python {
        statement[from + 1..].to_vec()
    } else {
        statement[1..from].to_vec()
    };
    if !python && type_only_import(tokens, start) {
        // Type-only imports neither execute the module nor shadow runtime
        // globals.
        return end;
    }
    if !python && names.first().is_some_and(|first| first.kind == Kind::Word) {
        bindings.insert(
            names[0].text.clone(),
            module
                .as_ref()
                .map_or_else(Vec::new, |module| vec![Token::api(module)]),
        );
        let skip = if names.get(1).is_some_and(|second| second.text == ",") {
            2
        } else {
            1
        };
        names = names[skip.min(names.len())..].to_vec();
    }
    if names.first().is_some_and(|first| first.text == "*")
        && names.get(1).is_some_and(|second| second.text == "as")
        && names.get(2).is_some()
    {
        bindings.insert(
            names[2].text.clone(),
            module
                .as_ref()
                .map_or_else(Vec::new, |module| vec![Token::api(module)]),
        );
    } else {
        if names.len() == 1 && (names[0].text == "{" || names[0].text == "(") {
            names = names[0].children.clone();
        }
        for part in argument_parts(&names) {
            let imported = part.first().map(|token| token.text.clone());
            let alias = if part.get(1).is_some_and(|token| token.text == "as") {
                part.get(2).map(|token| token.text.clone())
            } else {
                imported.clone()
            };
            let Some(alias) = alias.filter(|alias| !alias.is_empty()) else {
                continue;
            };
            let member = if imported.as_deref() == Some("default") {
                module.clone()
            } else {
                api_member(
                    module.as_deref(),
                    imported.as_deref().unwrap_or(""),
                    bindings,
                )
            };
            bindings.insert(
                alias,
                member.map_or_else(Vec::new, |member| vec![Token::api(&member)]),
            );
        }
    }
    end
}

fn local_declarations(tokens: &[Token], bindings: &mut Bindings) {
    for index in 0..tokens.len() {
        if matches!(
            tokens[index].text.as_str(),
            "const" | "let" | "var" | "function" | "class" | "def"
        ) {
            let mut pattern = tokens.get(index + 1);
            if pattern.is_some_and(|pattern| pattern.text == "*") {
                pattern = tokens.get(index + 2);
            }
            let Some(pattern) = pattern else {
                continue;
            };
            if pattern.kind == Kind::Word {
                bindings.insert(pattern.text.clone(), Vec::new());
            } else if pattern.text == "{" {
                bind_pattern(std::slice::from_ref(pattern), &[], bindings, false);
            }
        }
    }
}

fn parameter_bindings(tokens: &[Token], bindings: &mut Bindings) {
    for part in argument_parts(tokens) {
        if let Some(parameter) = part
            .iter()
            .find(|token| token.kind == Kind::Word || token.text == "{")
        {
            bind_pattern(std::slice::from_ref(parameter), &[], bindings, false);
        }
    }
}

/// `boundValue`: through parentheses and plain aliases to the value.
fn bound_value(tokens: &[Token], bindings: Option<&Bindings>) -> Vec<Token> {
    let mut tokens: Vec<Token> = tokens.to_vec();
    let mut seen: Vec<String> = Vec::new();
    while tokens.len() == 1 {
        let token = &tokens[0];
        if token.text == "(" {
            tokens = token.children.clone();
        } else if token.kind == Kind::Word
            && bindings.is_some_and(|bindings| bindings.contains_key(&token.text))
            && !seen.contains(&token.text)
        {
            seen.push(token.text.clone());
            tokens = bindings
                .and_then(|bindings| bindings.get(&token.text))
                .cloned()
                .unwrap_or_default();
        } else {
            break;
        }
    }
    tokens
}

/// `literal`: the string a value is, when it is one literal string (or
/// `process.execPath`, which is the runtime).
fn literal(tokens: &[Token], bindings: Option<&Bindings>) -> Option<String> {
    let tokens = bound_value(tokens, bindings);
    if tokens.len() == 1 && tokens[0].kind == Kind::Str {
        return tokens[0].value.clone();
    }
    let bindings = bindings?;
    (api_reference(&tokens, bindings, false, 0).as_deref() == Some("node:process.execPath"))
        .then(|| "bun".to_owned())
}

/// `arrayLiteral`: the literal strings of an array literal (an entry that
/// is not one is `None`).
fn array_literal(tokens: &[Token], bindings: Option<&Bindings>) -> Option<Vec<Option<String>>> {
    let mut tokens: Vec<Token> = tokens.to_vec();
    // Python subprocess also accepts args=[...].
    if tokens.first().is_some_and(|first| first.text == "args")
        && tokens.get(1).is_some_and(|second| second.text == "=")
    {
        tokens = tokens[2..].to_vec();
    }
    let tokens = bound_value(&tokens, bindings);
    if tokens.len() == 1 && tokens[0].text == "[" {
        Some(
            argument_parts(&tokens[0].children)
                .iter()
                .filter(|part| !part.is_empty())
                .map(|part| literal(part, bindings))
                .collect(),
        )
    } else {
        None
    }
}

fn object_field(tokens: &[Token], name: &str, bindings: Option<&Bindings>) -> Vec<Token> {
    let tokens = bound_value(tokens, bindings);
    if tokens.len() != 1 || tokens[0].text != "{" {
        return Vec::new();
    }
    argument_parts(&tokens[0].children)
        .into_iter()
        .find(|part| {
            part.first()
                .is_some_and(|first| first.value_or_text() == name)
                && part.get(1).is_some_and(|second| second.text == ":")
        })
        .map(|part| part[2..].to_vec())
        .unwrap_or_default()
}

struct Scope {
    end: usize,
    parameters: Vec<Token>,
    body: Vec<Token>,
    is_loop: bool,
}

/// `functionScopes`: the function bodies among `tokens`, by the index of
/// the token that opens each.
fn function_scopes(tokens: &[Token]) -> BTreeMap<usize, Scope> {
    let mut scopes = BTreeMap::new();
    for index in 0..tokens.len() {
        let token = &tokens[index];
        let before = index.checked_sub(1).and_then(|at| tokens.get(at));
        if token.text == "("
            && tokens.get(index + 1).is_some_and(|next| next.text == "{")
            && !matches!(
                before.map(|token| token.text.as_str()),
                Some("if" | "while" | "switch" | "with")
            )
        {
            scopes.insert(
                index + 1,
                Scope {
                    end: index + 2,
                    parameters: token.children.clone(),
                    body: tokens[index + 1].children.clone(),
                    is_loop: before.is_some_and(|token| token.text == "for"),
                },
            );
        }
        if token.text == "="
            && tokens.get(index + 1).is_some_and(|next| next.text == ">")
            && let Some(argument) = before
        {
            let parameters = if argument.text == "(" {
                argument.children.clone()
            } else {
                vec![argument.clone()]
            };
            let start = index + 2;
            if tokens.get(start).is_some_and(|token| token.text == "{") {
                scopes.insert(
                    start,
                    Scope {
                        end: start + 1,
                        parameters,
                        body: tokens[start].children.clone(),
                        is_loop: false,
                    },
                );
            } else {
                let mut end = start;
                while end < tokens.len() && tokens[end].text != ";" && tokens[end].text != "," {
                    end += 1;
                }
                scopes.insert(
                    start,
                    Scope {
                        end,
                        parameters,
                        body: tokens[start.min(end)..end].to_vec(),
                        is_loop: false,
                    },
                );
            }
        }
    }
    scopes
}

fn python_suite_end(tokens: &[Token], start: usize, header: &Token, chars: &[char]) -> usize {
    let line_start = chars[..header.start.min(chars.len())]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |at| at + 1);
    let header_indent = header.start - line_start;
    let header_end = chars[header.start.min(chars.len())..]
        .iter()
        .position(|c| *c == '\n')
        .map(|at| header.start + at);
    let same_line = tokens
        .get(start)
        .is_some_and(|token| token.start < header_end.unwrap_or(chars.len()));
    let mut end = start;
    while end < tokens.len() {
        let position = tokens[end].start;
        if same_line {
            if header_end.is_some_and(|header_end| position > header_end) {
                break;
            }
        } else {
            let prefix_start = chars[..position.min(chars.len())]
                .iter()
                .rposition(|c| *c == '\n')
                .map_or(0, |at| at + 1);
            let prefix = &chars[prefix_start..position.min(chars.len())];
            if prefix.iter().all(|c| *c == ' ' || *c == '\t') && prefix.len() <= header_indent {
                break;
            }
        }
        end += 1;
    }
    end
}

struct Pending {
    tokens: Vec<Token>,
    bindings: Bindings,
    local_python: bool,
}

/// `protectedContent`: whether a script's source reaches a hook module or
/// an execution sink that does.
pub(crate) fn protected_content(
    walk: &mut Walk,
    value: &str,
    depth: usize,
    python: bool,
    inherited: Option<&Bindings>,
) -> bool {
    if depth > MAX_EXECUTION_DEPTH {
        return false;
    }
    let chars: Vec<char> = value.chars().collect();
    let mut pending = vec![Pending {
        tokens: source_tokens(value, python),
        bindings: inherited
            .cloned()
            .unwrap_or_else(|| global_bindings(python)),
        local_python: false,
    }];
    while let Some(current) = pending.pop() {
        let tokens = current.tokens;
        let mut bindings = current.bindings;
        if !python || current.local_python {
            local_declarations(&tokens, &mut bindings);
        }
        if current.local_python {
            for index in 0..tokens.len() {
                if tokens[index].kind == Kind::Word
                    && !tokens[index].text.contains('.')
                    && tokens.get(index + 1).is_some_and(|next| next.text == "=")
                {
                    bindings.insert(tokens[index].text.clone(), Vec::new());
                }
                if tokens[index].text == "import" || tokens[index].text == "from" {
                    let mut imported = Bindings::new();
                    imported_bindings(&tokens, index, &chars, &mut imported, true);
                    for name in imported.keys() {
                        bindings.insert(name.clone(), Vec::new());
                    }
                }
            }
        }
        // ESM imports are hoisted; require/Python imports bind at their
        // statement.
        if !python {
            for index in 0..tokens.len() {
                if tokens[index].text == "import"
                    && !tokens.get(index + 1).is_some_and(|next| next.text == "(")
                {
                    imported_bindings(&tokens, index, &chars, &mut bindings, false);
                }
            }
        }
        let scopes = if python {
            BTreeMap::new()
        } else {
            function_scopes(&tokens)
        };
        let mut index = 0;
        while index < tokens.len() {
            let token = &tokens[index];
            let next = tokens.get(index + 1);
            if let Some(scope) = scopes.get(&index) {
                let mut child = bindings.clone();
                if scope.is_loop {
                    local_declarations(&scope.parameters, &mut child);
                } else {
                    parameter_bindings(&scope.parameters, &mut child);
                }
                pending.push(Pending {
                    tokens: scope.body.clone(),
                    bindings: child,
                    local_python: false,
                });
                index = scope.end;
                continue;
            }
            if python && (token.text == "def" || token.text == "class") {
                if let Some(next) = next.filter(|next| next.kind == Kind::Word) {
                    bindings.insert(next.text.clone(), Vec::new());
                }
                let mut colon = index + 2;
                while colon < tokens.len() && tokens[colon].text != ":" {
                    colon += 1;
                }
                if colon < tokens.len() {
                    let mut child = bindings.clone();
                    if token.text == "def"
                        && tokens.get(index + 2).is_some_and(|open| open.text == "(")
                    {
                        parameter_bindings(&tokens[index + 2].children, &mut child);
                    }
                    let end = python_suite_end(&tokens, colon + 1, token, &chars);
                    pending.push(Pending {
                        tokens: tokens[(colon + 1).min(end)..end].to_vec(),
                        bindings: child,
                        local_python: token.text == "def",
                    });
                    index = end;
                    continue;
                }
            }
            if python && (token.text == "import" || token.text == "from") {
                let end = imported_bindings(&tokens, index, &chars, &mut bindings, true);
                index = (index + 1).max(end);
                continue;
            }
            if !python
                && (token.text == "import" || token.text == "export")
                && !next.is_some_and(|next| next.text == "(")
            {
                let end = import_statement_end(&tokens, index, &chars, false);
                let statement = &tokens[index..end];
                let from = statement.iter().position(|part| part.text == "from");
                if token.text == "import" || from.is_some() {
                    let module = literal(
                        match from {
                            Some(from) => &statement[from + 1..],
                            None => &statement[1..],
                        },
                        Some(&bindings),
                    );
                    if module
                        .as_deref()
                        .is_some_and(|module| walk.hook_module(module))
                        && !type_only_import(&tokens, index)
                    {
                        return true;
                    }
                    index = (index + 1).max(end);
                    continue;
                }
            }
            if token.has_children {
                pending.push(Pending {
                    tokens: token.children.clone(),
                    bindings: bindings.clone(),
                    local_python: false,
                });
            }
            // Resolve simple literal variables when an execution sink consumes
            // them, not when they are stored as examples. Do not guess
            // computed expressions.
            if (token.kind == Kind::Word || token.text == "{")
                && next.is_some_and(|next| next.text == "=")
                && !tokens.get(index + 2).is_some_and(|after| after.text == ">")
            {
                let mut end = statement_end(&tokens, index + 2, &chars);
                if let Some(comma) = tokens[index + 2..end]
                    .iter()
                    .position(|part| part.text == ",")
                {
                    end = index + 2 + comma;
                }
                let previous = api_reference(std::slice::from_ref(token), &bindings, python, 0);
                if token.kind == Kind::Word
                    && token.text.contains('.')
                    && let Some(previous) = &previous
                {
                    bindings.insert(format!("@member:{previous}"), Vec::new());
                    if re::test(r"^(?:globalThis|global)\.", "", &token.text) {
                        let property = token.text.split('.').nth(1).unwrap_or("").to_owned();
                        bindings.insert(format!("@global:{property}"), Vec::new());
                        bindings.insert(property, Vec::new());
                    }
                }
                let value_tokens = tokens[index + 2..end].to_vec();
                bind_pattern(
                    std::slice::from_ref(token),
                    &value_tokens,
                    &mut bindings,
                    python,
                );
            }
            if let Some(next) = next.filter(|next| next.text == "(") {
                let expression = &tokens[reference_start(&tokens, index)..=index];
                let mut api = api_reference(expression, &bindings, python, 0);
                let mut parts = argument_parts(&next.children);
                let indirect = api.as_deref().is_some_and(|api| {
                    api.ends_with(".call") || api.ends_with(".apply") || api.ends_with(".bind")
                });
                if indirect && let Some(found) = api.clone() {
                    parts = if found.ends_with(".apply")
                        && parts
                            .get(1)
                            .and_then(|part| part.first())
                            .is_some_and(|first| first.text == "[")
                    {
                        argument_parts(&parts[1][0].children)
                    } else {
                        parts[1.min(parts.len())..].to_vec()
                    };
                    api = Some(re::replace(r"\.(?:call|apply|bind)$", "", &found, ""));
                }
                if (token.text == "import" && !python)
                    || api.as_deref() == Some("js:require")
                    || api.as_deref() == Some("py:__import__")
                {
                    let module = literal(
                        parts.first().map_or(&[][..], Vec::as_slice),
                        Some(&bindings),
                    );
                    if module
                        .as_deref()
                        .is_some_and(|module| walk.hook_module(module))
                    {
                        return true;
                    }
                }
                if let Some(found) = api.as_deref().filter(|api| {
                    re::test(
                        r"^(?:js:(?:eval|Function)|py:(?:eval|exec)|node:vm\.(?:runInThisContext|runInNewContext|runInContext|Script|compileFunction))$",
                        "",
                        api,
                    )
                }) {
                    let function_constructor = found == "js:Function";
                    let body_part = if function_constructor {
                        parts.last()
                    } else {
                        parts.first()
                    };
                    let body = literal(body_part.map_or(&[][..], Vec::as_slice), Some(&bindings));
                    let mut context = if found.starts_with("py:") || (found == "js:eval" && !indirect && token.text == "eval") {
                        bindings.clone()
                    } else {
                        global_bindings(false)
                    };
                    if function_constructor {
                        for parameter in &parts[..parts.len().saturating_sub(1)] {
                            if let Some(text) = literal(parameter, Some(&bindings)).filter(|text| !text.is_empty()) {
                                parameter_bindings(&source_tokens(&text, false), &mut context);
                            }
                        }
                    }
                    if let Some(body) = body
                        && protected_content(walk, &body, depth + 1, found.starts_with("py:"), Some(&context))
                    {
                        return true;
                    }
                }
                if api.as_deref().is_some_and(|api| {
                    re::test(
                        r"^(?:node:child_process\.(?:exec|execSync)|py:os\.(?:system|popen))$",
                        "",
                        api,
                    )
                }) {
                    let command = literal(
                        parts.first().map_or(&[][..], Vec::as_slice),
                        Some(&bindings),
                    );
                    if let Some(command) = command
                        && walk.protected_shell(&command, depth + 1, false)
                    {
                        return true;
                    }
                }
                if api.as_deref().is_some_and(|api| {
                    re::test(
                        r"^(?:bun\.(?:spawn|spawnSync)|deno\.(?:run|Command)|node:child_process\.(?:spawn|spawnSync|execFile|execFileSync)|py:subprocess\.(?:run|call|Popen|check_call|check_output))$",
                        "",
                        api,
                    )
                }) {
                    let first = parts.first().map_or(&[][..], Vec::as_slice);
                    let argv = array_literal(first, Some(&bindings))
                        .or_else(|| array_literal(&object_field(first, "cmd", Some(&bindings)), Some(&bindings)));
                    if let Some(argv) = &argv
                        && let Some(Some(executable)) = argv.first()
                        && walk.protected_invocation(executable, &argv[1..], depth + 1)
                    {
                        return true;
                    }
                    let executable = literal(first, Some(&bindings));
                    let second = parts.get(1).map_or(&[][..], Vec::as_slice);
                    let args = array_literal(second, Some(&bindings))
                        .or_else(|| array_literal(&object_field(second, "args", Some(&bindings)), Some(&bindings)))
                        .unwrap_or_default();
                    if let Some(executable) = executable
                        && walk.protected_invocation(&executable, &args, depth + 1)
                    {
                        return true;
                    }
                }
            }
            // Recognize shell commands in shell wrappers without interpreting
            // a JS string/template as shell source. Tokens inside either
            // remain opaque.
            if token.kind == Kind::Word
                && (super::interpreter(&token.text).is_some() || token.text == "focus")
                && !next.is_some_and(|next| next.text == "(")
            {
                let mut end = token.end;
                for following in &tokens[index + 1..] {
                    if following.text == ";" || gap_has_terminator(&chars, end, following.start) {
                        break;
                    }
                    end = following.end;
                }
                if walk.protected_shell(&text_of(&chars, token.start, end), depth + 1, false) {
                    return true;
                }
            }
            index += 1;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_decode_as_javascript_does() {
        assert_eq!(decode_literal(r"a\nb\x41B\u{43}\q"), "a\nbABCq");
        assert_eq!(decode_literal("a\\\nb"), "ab");
    }

    #[test]
    fn strings_groups_and_regexps_are_opaque_tokens() {
        let tokens = source_tokens("const x = `a${b}c`; f(/[/]x/g, 'y') // c\n/* d */ z", false);
        let kinds: Vec<Kind> = tokens.iter().map(|token| token.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Word,
                Kind::Word,
                Kind::Symbol,
                Kind::Str,
                Kind::Symbol,
                Kind::Word,
                Kind::Group,
                Kind::Word
            ]
        );
        assert!(tokens[3].value.is_none() && tokens[3].children.len() == 1);
        let group = &tokens[6];
        assert_eq!(group.children[0].kind, Kind::Regexp);
        assert_eq!(group.children[2].value.as_deref(), Some("y"));
    }
}

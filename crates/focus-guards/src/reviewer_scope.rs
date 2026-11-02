//! The per-unit reviewer read-scope bound, as a pure matcher: whether a tool
//! call reaches into a sibling unit's `construction/` subtree, or spans
//! siblings with a wildcard, and is not on the dispatch's exempt list.
//! Mirrors the matcher half of `aidlc-reviewer-scope.ts`
//! (`evaluateReviewerScope`, `parseDispatchRecord`, `blockReason`); the hook
//! around it is `focus-hooks`'.
//!
//! Path-shaped tools contribute their path fields; `Bash` the whole command;
//! `Glob` and `Grep` their pattern or glob plus their search root. `Grep`'s
//! `pattern` is the content regex and is deliberately not scanned.

use focus_js::path::{Flavour, is_absolute, resolve};
use serde_json::{Map, Value};

/// The conductor-written dispatch record
/// (`<record>/.focus-engine/reviewer-dispatch.json`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewerDispatch {
    /// Agent name of the dispatched reviewer.
    pub reviewer: String,
    /// Stage slug the review belongs to.
    pub stage: String,
    /// The unit under review: the one `construction/<unit>/` subtree in scope.
    pub unit: String,
    /// Paths the reviewer may touch beyond the current unit. Only entries
    /// with a `construction/` component matter to the matcher.
    pub exempt: Vec<String>,
}

/// The matcher's verdict: the offending path or token when it blocks, and
/// whether that target is a synthesized default search root (`.`) rather
/// than a token the caller typed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScopeVerdict {
    pub block: bool,
    pub target: Option<String>,
    pub defaulted: bool,
}

impl ScopeVerdict {
    fn of(target: &str) -> Self {
        Self {
            block: true,
            target: Some(target.to_owned()),
            defaulted: false,
        }
    }

    /// `markDefaulted`: a block whose target is the implied search root.
    fn defaulted(verdict: Option<Self>) -> Option<Self> {
        verdict.map(|mut verdict| {
            if verdict.block {
                verdict.defaulted = true;
            }
            verdict
        })
    }
}

/// Path context for the matcher: the record root (the parent of the
/// dispatch file's `.focus-engine/`) and the tool's cwd. Without it only the
/// lexical judgement applies.
#[derive(Debug, Clone, Default)]
pub struct ScopeContext {
    pub record_root: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Path,
    Command,
    Glob,
    SearchRoot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Target,
    SearchRoot,
}

/// Glob metacharacters: a sibling segment carrying any spans units.
fn has_wildcard(text: &str) -> bool {
    text.chars()
        .any(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}'))
}

fn flavour() -> Flavour {
    Flavour::host()
}

/// `toPosix`: separators turned forward on Windows; untouched elsewhere.
fn to_posix(path: &str) -> String {
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.to_owned()
    }
}

fn fold(text: &str) -> String {
    focus_js::text::to_lower_case(text)
}

/// `candidateStrings`: the strings of a tool call the matcher judges.
fn candidate_strings(tool: &str, input: Option<&Map<String, Value>>) -> Vec<(String, Kind)> {
    let mut out = Vec::new();
    let mut push = |value: Option<&Value>, kind: Kind| {
        if let Some(text) = value.and_then(Value::as_str)
            && !text.is_empty()
        {
            out.push((text.to_owned(), kind));
        }
    };
    let field = |name: &str| input.and_then(|input| input.get(name));
    match tool {
        "Bash" => push(field("command"), Kind::Command),
        "Read" | "NotebookRead" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
            push(field("file_path"), Kind::Path);
            push(field("notebook_path"), Kind::Path);
            push(field("path"), Kind::Path);
            if let Some(Value::Array(paths)) = field("paths") {
                for path in paths {
                    push(Some(path), Kind::Path);
                }
            }
        }
        "LS" => push(field("path"), Kind::SearchRoot),
        "Glob" => {
            push(field("pattern"), Kind::Glob);
            push(field("path"), Kind::SearchRoot);
        }
        "Grep" => {
            push(field("glob"), Kind::Glob);
            push(field("path"), Kind::SearchRoot);
        }
        _ => {}
    }
    out
}

/// `normalizedComps`: the path's components, empty and `.` dropped and `..`
/// collapsed against a parent that is not itself `..`.
fn normalized_comps(path: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for comp in to_posix(path).split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." && out.last().is_some_and(|last| last != "..") {
            out.pop();
            continue;
        }
        out.push(comp.to_owned());
    }
    out
}

fn normalized_comps_folded(path: &str) -> Vec<String> {
    normalized_comps(path)
        .iter()
        .map(|comp| fold(comp))
        .collect()
}

/// `globComponentMatchesConstruction`: a component that is `construction`,
/// or a glob (with something literal in it) that `construction` matches.
fn glob_component_matches_construction(component: &str) -> bool {
    let folded = fold(component);
    if folded == "construction" {
        return true;
    }
    if !has_wildcard(&folded) {
        return false;
    }
    if folded
        .chars()
        .all(|ch| matches!(ch, '*' | '?' | '[' | ']' | '{' | '}' | '!' | ','))
    {
        return false;
    }
    // The glob as a regular expression over `construction`: `*` any run, `?`
    // one character, everything else itself.
    let pattern: Vec<char> = folded.chars().collect();
    fn matches(pattern: &[char], text: &[char]) -> bool {
        match pattern.first() {
            None => text.is_empty(),
            Some('*') => (0..=text.len()).any(|skip| matches(&pattern[1..], &text[skip..])),
            Some('?') => !text.is_empty() && matches(&pattern[1..], &text[1..]),
            Some(ch) => text.first() == Some(ch) && matches(&pattern[1..], &text[1..]),
        }
    }
    let target: Vec<char> = "construction".chars().collect();
    matches(&pattern, &target)
}

fn construction_index(comps: &[String], allow_glob: bool) -> Option<usize> {
    comps.iter().position(|comp| {
        fold(comp) == "construction" || (allow_glob && glob_component_matches_construction(comp))
    })
}

fn canonical_suffix(comps: &[String]) -> String {
    comps
        .iter()
        .map(|comp| fold(comp))
        .collect::<Vec<_>>()
        .join("/")
}

/// `exemptSuffixOf`: the `construction/`-suffix of an exempt entry, or
/// nothing when the entry never enters `construction/`.
fn exempt_suffix_of(entry: &str) -> Option<String> {
    let comps = normalized_comps(entry);
    let index = construction_index(&comps, false)?;
    Some(canonical_suffix(&comps[index..]))
}

/// `judgeOccurrence`: whether one `construction/` occurrence is a sibling
/// read. The segment after `construction` decides: the dispatched unit
/// passes; a wildcard or a bare sweep root blocks; a concrete sibling blocks
/// unless the whole suffix is an exempt entry.
fn judge_occurrence(suffix: &[String], unit_folded: &str, exempt_suffixes: &[String]) -> bool {
    let Some(segment) = suffix.get(1).filter(|segment| !segment.is_empty()) else {
        return true;
    };
    if has_wildcard(segment) {
        return true;
    }
    if fold(segment) == unit_folded {
        return false;
    }
    !exempt_suffixes.contains(&canonical_suffix(suffix))
}

struct PreparedScope {
    unit_folded: String,
    exempt_suffixes: Vec<String>,
    exempt_paths: Vec<String>,
    construction_root: Option<String>,
    bases: Vec<String>,
}

/// `normalizeForCompare`: folded, normalized, with a leading slash kept.
fn normalize_for_compare(path: &str) -> String {
    let posix = to_posix(path);
    let body = normalized_comps_folded(&posix).join("/");
    if posix.starts_with('/') {
        format!("/{body}")
    } else {
        body
    }
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values {
        if !value.is_empty() && !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

fn resolve_path_strings(text: &str, bases: &[String]) -> Vec<String> {
    if is_absolute(flavour(), text) {
        return vec![resolve(flavour(), "/", &[text])];
    }
    bases
        .iter()
        .map(|base| resolve(flavour(), "/", &[base, text]))
        .collect()
}

fn normalize_resolved(text: &str, bases: &[String]) -> Vec<String> {
    unique_strings(
        resolve_path_strings(text, bases)
            .iter()
            .map(|path| normalize_for_compare(path))
            .collect(),
    )
}

fn contains_path(root: &str, candidate: &str) -> bool {
    candidate == root || candidate.starts_with(&format!("{root}/"))
}

fn prepare_scope(unit: &str, exempt: &[String], context: &ScopeContext) -> PreparedScope {
    let absolute = |path: &str| resolve(flavour(), "/", &[path]);
    let record_root = context
        .record_root
        .as_deref()
        .filter(|root| !root.is_empty())
        .map(absolute);
    let construction_root = record_root
        .as_deref()
        .map(|root| resolve(flavour(), "/", &[root, "construction"]));
    let unit_root = construction_root
        .as_deref()
        .map(|root| resolve(flavour(), "/", &[root, unit]));
    let cwd = context
        .cwd
        .as_deref()
        .filter(|cwd| !cwd.is_empty())
        .map(absolute);
    let bases = unique_strings(vec![
        cwd.unwrap_or_default(),
        record_root.clone().unwrap_or_default(),
        unit_root.unwrap_or_default(),
    ]);
    let mut exempt_suffixes = Vec::new();
    let mut exempt_paths = Vec::new();
    for entry in exempt {
        if let Some(suffix) = exempt_suffix_of(entry)
            && !exempt_suffixes.contains(&suffix)
        {
            exempt_suffixes.push(suffix);
        }
        for path in normalize_resolved(entry, &bases) {
            if !exempt_paths.contains(&path) {
                exempt_paths.push(path);
            }
        }
    }
    PreparedScope {
        unit_folded: fold(unit),
        exempt_suffixes,
        exempt_paths,
        construction_root: construction_root.as_deref().map(normalize_for_compare),
        bases,
    }
}

fn judge_lexical_path(text: &str, scope: &PreparedScope) -> Option<ScopeVerdict> {
    let comps = normalized_comps(text);
    for index in 0..comps.len() {
        if construction_index(&comps[index..=index], true) != Some(0) {
            continue;
        }
        if judge_occurrence(&comps[index..], &scope.unit_folded, &scope.exempt_suffixes) {
            return Some(ScopeVerdict::of(text));
        }
    }
    None
}

fn judge_resolved_path(
    text: &str,
    mode: Mode,
    scope: &PreparedScope,
    bases: &[String],
) -> Option<ScopeVerdict> {
    let construction_root = scope.construction_root.as_deref()?;
    for path in normalize_resolved(text, bases) {
        if contains_path(construction_root, &path) {
            let mut suffix = vec!["construction".to_owned()];
            if path != construction_root {
                suffix.extend(
                    path[construction_root.len() + 1..]
                        .split('/')
                        .map(str::to_owned),
                );
            }
            let exact_exempt = scope.exempt_paths.contains(&path)
                || scope.exempt_suffixes.contains(&canonical_suffix(&suffix));
            if exact_exempt {
                continue;
            }
            if judge_occurrence(&suffix, &scope.unit_folded, &scope.exempt_suffixes) {
                return Some(ScopeVerdict::of(text));
            }
        }
        // A search root above `construction/` sweeps every sibling unit even
        // when the command never spells `construction`.
        if mode == Mode::SearchRoot && contains_path(&path, construction_root) {
            return Some(ScopeVerdict::of(text));
        }
    }
    None
}

fn judge_path_access(
    text: &str,
    mode: Mode,
    scope: &PreparedScope,
    bases: &[String],
) -> Option<ScopeVerdict> {
    judge_lexical_path(text, scope).or_else(|| judge_resolved_path(text, mode, scope, bases))
}

/// `patternLimitsToCurrentUnit`: every `construction/` occurrence in the
/// pattern names the current unit or an exempt suffix, and there is one.
fn pattern_limits_to_current_unit(text: &str, scope: &PreparedScope) -> bool {
    let comps = normalized_comps(text);
    let mut saw_construction = false;
    for index in 0..comps.len() {
        if construction_index(&comps[index..=index], true) != Some(0) {
            continue;
        }
        saw_construction = true;
        let suffix = &comps[index..];
        let Some(segment) = suffix.get(1).filter(|segment| !has_wildcard(segment)) else {
            return false;
        };
        if fold(segment) != scope.unit_folded
            && !scope.exempt_suffixes.contains(&canonical_suffix(suffix))
        {
            return false;
        }
    }
    saw_construction
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ShellWord {
    text: String,
    quoted: bool,
}

enum ShellToken {
    Word(ShellWord),
    /// A separator; `pipe` only for a single `|`.
    Separator {
        pipe: bool,
    },
}

/// `shellTokens`: words and separators, quotes and backslashes consumed.
fn shell_tokens(command: &str) -> Vec<ShellToken> {
    let chars: Vec<char> = command.chars().collect();
    let mut tokens = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    let push_word = |tokens: &mut Vec<ShellToken>, text: &mut String, quoted: &mut bool| {
        if !text.is_empty() || *quoted {
            tokens.push(ShellToken::Word(ShellWord {
                text: std::mem::take(text),
                quoted: *quoted,
            }));
        }
        text.clear();
        *quoted = false;
    };
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if focus_js::text::is_js_whitespace(ch) {
            push_word(&mut tokens, &mut text, &mut quoted);
            index += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            quoted = true;
            let quote = ch;
            index += 1;
            while index < chars.len() && chars[index] != quote {
                if quote == '"' && chars[index] == '\\' && index + 1 < chars.len() {
                    index += 1;
                }
                text.push(chars[index]);
                index += 1;
            }
            index += 1;
            continue;
        }
        if ch == '\\' {
            if index + 1 < chars.len() {
                index += 1;
                text.push(chars[index]);
            }
            index += 1;
            continue;
        }
        if ";|&()".contains(ch) {
            push_word(&mut tokens, &mut text, &mut quoted);
            let doubled = (ch == '|' || ch == '&') && chars.get(index + 1) == Some(&ch);
            if doubled {
                index += 1;
            }
            tokens.push(ShellToken::Separator {
                pipe: ch == '|' && !doubled,
            });
            index += 1;
            continue;
        }
        text.push(ch);
        index += 1;
    }
    push_word(&mut tokens, &mut text, &mut quoted);
    tokens
}

struct ShellSegment {
    words: Vec<ShellWord>,
    /// The preceding separator was a single `|`.
    piped_from: bool,
}

fn shell_segments(command: &str) -> Vec<ShellSegment> {
    let mut segments = Vec::new();
    let mut current: Vec<ShellWord> = Vec::new();
    let mut piped_from = false;
    for token in shell_tokens(command) {
        match token {
            ShellToken::Separator { pipe } => {
                if !current.is_empty() {
                    segments.push(ShellSegment {
                        words: std::mem::take(&mut current),
                        piped_from,
                    });
                }
                current.clear();
                piped_from = pipe;
            }
            ShellToken::Word(word) => current.push(word),
        }
    }
    if !current.is_empty() {
        segments.push(ShellSegment {
            words: current,
            piped_from,
        });
    }
    segments
}

fn command_basename(command: &str) -> String {
    let comps = normalized_comps(command);
    fold(comps.last().map_or(command, String::as_str))
}

fn is_option(word: &str) -> bool {
    word.len() > 1 && word.starts_with('-')
}

fn first_operand(words: &[ShellWord]) -> Option<usize> {
    (1..words.len()).find(|&index| !is_option(&words[index].text))
}

const GREP_VALUE_OPTIONS: [&str; 16] = [
    "--after-context",
    "--before-context",
    "--binary-files",
    "--context",
    "--devices",
    "--directories",
    "--exclude",
    "--exclude-dir",
    "--exclude-from",
    "--file",
    "--group-separator",
    "--include",
    "--include-dir",
    "--label",
    "--max-count",
    "--regexp",
];
const RG_VALUE_OPTIONS: [&str; 35] = [
    "--after-context",
    "--before-context",
    "--color",
    "--colors",
    "--context",
    "--context-separator",
    "--dfa-size-limit",
    "--encoding",
    "--engine",
    "--field-context-separator",
    "--field-match-separator",
    "--file",
    "--generate",
    "--glob",
    "--hostname-bin",
    "--hyperlink-format",
    "--iglob",
    "--ignore-file",
    "--max-columns",
    "--max-count",
    "--max-depth",
    "--max-filesize",
    "--path-separator",
    "--pre",
    "--pre-glob",
    "--regex-size-limit",
    "--regexp",
    "--replace",
    "--sort",
    "--sortr",
    "--threads",
    "--type",
    "--type-add",
    "--type-clear",
    "--type-not",
];

struct SearchArguments {
    paths: Vec<String>,
    input_files: Vec<String>,
    globs: Vec<String>,
    searches_files: bool,
}

/// `searchArguments`: a grep or ripgrep command's paths, pattern files and
/// globs, options parsed before the positional pattern is decided.
fn search_arguments(words: &[ShellWord], ripgrep: bool) -> SearchArguments {
    let mut paths: Vec<String> = Vec::new();
    let mut input_files: Vec<String> = Vec::new();
    let mut globs: Vec<String> = Vec::new();
    let mut pattern_supplied = false;
    let mut lists_files = false;
    let mut recursive = false;
    let mut stdin_patterns = false;
    let mut options_ended = false;
    let value_options: &[&str] = if ripgrep {
        &RG_VALUE_OPTIONS
    } else {
        &GREP_VALUE_OPTIONS
    };
    let short_value_options = if ripgrep {
        "ABCEMdefgjmrtT"
    } else {
        "ABCDdefm"
    };
    let mut option = |name: &str, value: &str| {
        if name == "-e" || name == "--regexp" {
            pattern_supplied = true;
        } else if name == "-f" || name == "--file" {
            pattern_supplied = true;
            input_files.push(value.to_owned());
            stdin_patterns |= value == "-";
        } else if name == "--exclude-from" || name == "--ignore-file" {
            input_files.push(value.to_owned());
        } else if ripgrep && (name == "-g" || name == "--glob" || name == "--iglob") {
            globs.push(value.to_owned());
        } else if ripgrep && name == "--files" {
            lists_files = true;
        } else if !ripgrep {
            if ["-r", "-R", "--recursive", "--dereference-recursive"].contains(&name) {
                recursive = true;
            } else if name == "-d" || name == "--directories" {
                recursive = value == "recurse";
            }
        }
    };
    let mut index = 1;
    while index < words.len() {
        let word = words[index].text.as_str();
        if !options_ended && word == "--" {
            options_ended = true;
        } else if !options_ended && word.starts_with("--") {
            let (name, value) = match word.find('=') {
                Some(equals) => (&word[..equals], word[equals + 1..].to_owned()),
                None if value_options.contains(&word) => {
                    index += 1;
                    (
                        word,
                        words.get(index).map(|w| w.text.clone()).unwrap_or_default(),
                    )
                }
                None => (word, String::new()),
            };
            option(name, &value);
        } else if !options_ended && is_option(word) {
            let letters: Vec<char> = word.chars().collect();
            let mut at = 1;
            while at < letters.len() {
                let letter = letters[at];
                let takes_value = short_value_options.contains(letter);
                let value = if takes_value {
                    let rest: String = letters[at + 1..].iter().collect();
                    if rest.is_empty() {
                        index += 1;
                        words
                            .get(index)
                            .map(|w| w.text.clone())
                            .filter(|text| !text.is_empty())
                            .unwrap_or_default()
                    } else {
                        rest
                    }
                } else {
                    String::new()
                };
                option(&format!("-{letter}"), &value);
                if takes_value {
                    break;
                }
                at += 1;
            }
        } else {
            paths.push(word.to_owned());
        }
        index += 1;
    }
    if !pattern_supplied && !lists_files && !paths.is_empty() {
        paths.remove(0);
    }
    SearchArguments {
        paths,
        input_files,
        globs,
        searches_files: if ripgrep {
            lists_files || stdin_patterns
        } else {
            recursive
        },
    }
}

fn judge_grep_like(
    words: &[ShellWord],
    scope: &PreparedScope,
    bases: &[String],
    reads_stdin: bool,
) -> Option<ScopeVerdict> {
    let args = search_arguments(words, false);
    for file in args.input_files.iter().filter(|file| *file != "-") {
        if let Some(verdict) = judge_path_access(file, Mode::Target, scope, bases) {
            return Some(verdict);
        }
    }
    for path in args.paths.iter().filter(|path| *path != "-") {
        if let Some(verdict) = judge_path_access(path, Mode::SearchRoot, scope, bases) {
            return Some(verdict);
        }
    }
    // GNU grep's recursive mode defaults to `.` even with piped stdin; a
    // plain filter uses the pipe.
    if !args.paths.is_empty() || (reads_stdin && !args.searches_files) {
        return None;
    }
    ScopeVerdict::defaulted(judge_path_access(".", Mode::SearchRoot, scope, bases))
}

fn judge_ripgrep(
    words: &[ShellWord],
    scope: &PreparedScope,
    bases: &[String],
    reads_stdin: bool,
) -> Option<ScopeVerdict> {
    let args = search_arguments(words, true);
    for file in args.input_files.iter().filter(|file| *file != "-") {
        if let Some(verdict) = judge_path_access(file, Mode::Target, scope, bases) {
            return Some(verdict);
        }
    }
    let mut constrained_to_current = false;
    for glob in args.globs.iter().filter(|glob| !glob.is_empty()) {
        if let Some(verdict) = judge_path_access(glob, Mode::Target, scope, bases) {
            return Some(verdict);
        }
        constrained_to_current |= pattern_limits_to_current_unit(glob, scope);
    }
    for path in args.paths.iter().filter(|path| *path != "-") {
        if let Some(verdict) = judge_path_access(path, Mode::SearchRoot, scope, bases) {
            return Some(verdict);
        }
    }
    if !args.paths.is_empty() || constrained_to_current || (reads_stdin && !args.searches_files) {
        return None;
    }
    ScopeVerdict::defaulted(judge_path_access(".", Mode::SearchRoot, scope, bases))
}

fn judge_find(
    words: &[ShellWord],
    scope: &PreparedScope,
    bases: &[String],
) -> Option<ScopeVerdict> {
    let mut root_seen = false;
    for word in &words[1..] {
        let text = word.text.as_str();
        if is_option(text) || text == "!" || text == "(" || text == ")" {
            break;
        }
        root_seen = true;
        if let Some(verdict) = judge_path_access(text, Mode::SearchRoot, scope, bases) {
            return Some(verdict);
        }
    }
    if !root_seen {
        return ScopeVerdict::defaulted(judge_path_access(".", Mode::SearchRoot, scope, bases));
    }
    None
}

fn is_pathish(word: &str) -> bool {
    word == "."
        || word == ".."
        || word.contains('/')
        || has_wildcard(word)
        || fold(word) == "construction"
        || glob_component_matches_construction(word)
}

fn judge_simple_file_command(
    words: &[ShellWord],
    mode: Mode,
    scope: &PreparedScope,
    bases: &[String],
) -> Option<ScopeVerdict> {
    let mut saw_operand = false;
    for word in &words[1..] {
        if is_option(&word.text) {
            continue;
        }
        saw_operand = true;
        if let Some(verdict) = judge_path_access(&word.text, mode, scope, bases) {
            return Some(verdict);
        }
    }
    if !saw_operand && mode == Mode::SearchRoot {
        return ScopeVerdict::defaulted(judge_path_access(".", Mode::SearchRoot, scope, bases));
    }
    None
}

fn judge_generic_command(
    words: &[ShellWord],
    scope: &PreparedScope,
    bases: &[String],
) -> Option<ScopeVerdict> {
    for word in &words[1..] {
        if is_option(&word.text) || !is_pathish(&word.text) {
            continue;
        }
        if let Some(verdict) = judge_path_access(&word.text, Mode::Target, scope, bases) {
            return Some(verdict);
        }
    }
    None
}

/// `judgeCommandText`: a shell command segment by segment, `cd` moving the
/// bases the later segments resolve against.
fn judge_command_text(text: &str, scope: &PreparedScope) -> Option<ScopeVerdict> {
    let mut bases = scope.bases.clone();
    for segment in shell_segments(text) {
        let words = &segment.words;
        if words.is_empty() {
            continue;
        }
        let command = command_basename(&words[0].text);
        if command == "cd" {
            let Some(index) = first_operand(words) else {
                continue;
            };
            let next = words[index].text.as_str();
            if next == "-" {
                continue;
            }
            if let Some(verdict) = judge_path_access(next, Mode::SearchRoot, scope, &bases) {
                return Some(verdict);
            }
            bases = unique_strings(resolve_path_strings(next, &bases));
            continue;
        }
        let verdict = match command.as_str() {
            "grep" | "egrep" | "fgrep" => judge_grep_like(words, scope, &bases, segment.piped_from),
            "rg" | "ripgrep" => judge_ripgrep(words, scope, &bases, segment.piped_from),
            "find" => judge_find(words, scope, &bases),
            "ls" => judge_simple_file_command(words, Mode::SearchRoot, scope, &bases),
            "cat" | "less" | "more" | "head" | "tail" => {
                judge_simple_file_command(words, Mode::Target, scope, &bases)
            }
            _ => judge_generic_command(words, scope, &bases),
        };
        if verdict.is_some() {
            return verdict;
        }
    }
    None
}

/// `evaluateReviewerScope`: the reviewer read-scope decision. Pure: no I/O,
/// no environment beyond the host's path flavour.
#[must_use]
pub fn evaluate_reviewer_scope(
    tool: &str,
    input: Option<&Map<String, Value>>,
    unit: &str,
    exempt: &[String],
    context: &ScopeContext,
) -> ScopeVerdict {
    let scope = prepare_scope(unit, exempt, context);
    let mut glob_constrained_to_current = false;
    let mut saw_glob = false;
    let mut saw_search_root = false;
    for (text, kind) in candidate_strings(tool, input) {
        let verdict = match kind {
            Kind::Path => judge_path_access(&text, Mode::Target, &scope, &scope.bases),
            Kind::SearchRoot => {
                saw_search_root = true;
                judge_path_access(&text, Mode::SearchRoot, &scope, &scope.bases)
            }
            Kind::Glob => {
                saw_glob = true;
                let verdict = judge_path_access(&text, Mode::Target, &scope, &scope.bases);
                if verdict.is_none() {
                    glob_constrained_to_current |= pattern_limits_to_current_unit(&text, &scope);
                }
                verdict
            }
            Kind::Command => judge_command_text(&text, &scope),
        };
        if let Some(verdict) = verdict {
            return verdict;
        }
    }
    // A pathless Grep recurses from the cwd; a pathless Glob too, unless its
    // pattern constrains the search to the current unit or an exempt file.
    if tool == "Grep"
        && !saw_search_root
        && !glob_constrained_to_current
        && let Some(verdict) = ScopeVerdict::defaulted(judge_path_access(
            ".",
            Mode::SearchRoot,
            &scope,
            &scope.bases,
        ))
    {
        return verdict;
    }
    if tool == "Glob"
        && !saw_search_root
        && saw_glob
        && !glob_constrained_to_current
        && let Some(verdict) = ScopeVerdict::defaulted(judge_path_access(
            ".",
            Mode::SearchRoot,
            &scope,
            &scope.bases,
        ))
    {
        return verdict;
    }
    ScopeVerdict::default()
}

/// `parseDispatchRecord`: the record's JSON, or nothing on any shape miss.
#[must_use]
pub fn parse_dispatch_record(raw: &str) -> Option<ReviewerDispatch> {
    let Value::Object(record) = focus_js::json::parse(raw).ok()? else {
        return None;
    };
    let text = |name: &str| record.get(name).and_then(Value::as_str);
    let reviewer = text("reviewer").filter(|value| !value.is_empty())?;
    let unit = text("unit").filter(|value| !value.is_empty())?;
    let stage = text("stage")?;
    let Value::Array(exempt) = record.get("exempt")? else {
        return None;
    };
    let exempt = exempt
        .iter()
        .map(|entry| entry.as_str().map(str::to_owned))
        .collect::<Option<Vec<String>>>()?;
    Some(ReviewerDispatch {
        reviewer: reviewer.to_owned(),
        stage: stage.to_owned(),
        unit: unit.to_owned(),
        exempt,
    })
}

/// `blockReason`: the refusal handed back through the harness's PreToolUse
/// channel, naming the scope, the target and the sanctioned alternative.
#[must_use]
pub fn block_reason(target: &str, unit: &str, defaulted: bool) -> String {
    let default_note = if defaulted {
        format!(
            " (this command names no path, so \"{target}\" is the implicit recursive search root it falls back to - not a path you typed; give it an explicit in-scope path)"
        )
    } else {
        String::new()
    };
    format!(
        "This review cannot open \"{target}\"{default_note} because it belongs to another unit; the current review covers {unit}. Use the files supplied with the review and the files under this unit's construction path. If the design depends on another unit, note that integration point in the findings instead of opening its files. Write {unit} literally in shell paths because variables cannot be checked, and keep searches inside the current unit."
    )
}

/// Whether any candidate string of the call spells `construction/`: the
/// missing-record advisory's test.
#[must_use]
pub fn touches_construction(tool: &str, input: Option<&Map<String, Value>>) -> bool {
    candidate_strings(tool, input)
        .iter()
        .any(|(text, _)| to_posix(text).contains("construction/"))
}

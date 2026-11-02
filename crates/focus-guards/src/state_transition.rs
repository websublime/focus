//! The state-transition fence, as pure readers of a shell command: the
//! direct state transitions a conductor may not call (the engine owns stage
//! pinning, evidence checks and transition selection), the lifecycle
//! boundary commands, and the engine commands a delegated agent may not run
//! at all. Mirrors the matcher half of `aidlc-state-transition-guard.ts`
//! (`directStateTransition`, `isLifecycleBoundaryCommand`,
//! `delegatedLifecycleCommand`); the hook around it is `focus-hooks`'.
//!
//! Upstream's parser works in JavaScript strings: some of its scans index
//! UTF-16 code units while writing into an array of code points. Where the
//! two differ the readers here follow upstream's arithmetic, so a command
//! with a character outside the BMP is read as upstream reads it.

use focus_js::re;

/// `BLOCKED_STATE_TRANSITIONS`: the state verbs that move a stage.
pub const BLOCKED_STATE_TRANSITIONS: [&str; 13] = [
    "set",
    "checkbox",
    "advance",
    "finalize",
    "complete-workflow",
    "gate-start",
    "approve",
    "reject",
    "revise",
    "skip",
    "park",
    "refresh-unit-progress",
    "fold-unit-merge",
];

/// `DELEGATED_STATE_MUTATIONS`: the blocked transitions and every other
/// state verb that writes.
const DELEGATED_STATE_EXTRA: [&str; 16] = [
    "set-skeleton-stance",
    "set-construction-iteration",
    "set-construction-checkpoints",
    "set-construction-execution",
    "set-construction-verification-command",
    "sync-unit-scope-stage",
    "unit",
    "set-unit-ownership",
    "set-unit-gate-rhythm",
    "acknowledge-compaction",
    "reuse-artifact",
    "practices-event",
    "practices-promote",
    "fork",
    "merge",
    "unpark",
];

#[must_use]
pub fn is_blocked_state_transition(verb: &str) -> bool {
    BLOCKED_STATE_TRANSITIONS.contains(&verb)
}

#[must_use]
pub fn is_delegated_state_mutation(verb: &str) -> bool {
    is_blocked_state_transition(verb) || DELEGATED_STATE_EXTRA.contains(&verb)
}

const DELEGATED_ORCHESTRATE_VERBS: [&str; 4] = ["next", "continue", "report", "park"];
const DELEGATED_JUMP_VERBS: [&str; 2] = ["execute", "reopen"];
const DELEGATED_UNIT_VERBS: [&str; 8] = [
    "adopt",
    "claim",
    "release",
    "participate",
    "publish",
    "pin",
    "gate",
    "land",
];
const DELEGATED_BOLT_VERBS: [&str; 10] = [
    "start",
    "complete",
    "fail",
    "abort",
    "set-autonomy",
    "checkpoint",
    "swarm-checkpoint",
    "dispatch-event",
    "hold-merge",
    "release-merge",
];
const DELEGATED_SWARM_VERBS: [&str; 3] = ["prepare", "check", "finalize"];
const DELEGATED_LOG_VERBS: [&str; 4] = ["decision", "answer", "review", "link"];
const DELEGATED_LEARNINGS_VERBS: [&str; 1] = ["persist"];
const DELEGATED_RUNTIME_VERBS: [&str; 2] = ["fragment-fork", "fragment-merge"];
const DELEGATED_TESTING_POSTURE_VERBS: [&str; 2] = ["begin", "fingerprint"];
const DELEGATED_WORKTREE_VERBS: [&str; 1] = ["purge"];
const DELEGATED_AUDIT_VERBS: [&str; 2] = ["fork", "merge"];
const DELEGATED_AUDIT_SCRIPT_VERBS: [&str; 2] = ["audit-fork", "audit-merge"];
const DELEGATED_PLUGIN_SCRIPT_VERBS: [&str; 1] = ["sync"];

/// `DELEGATED_SCRIPT_VERBS`: the authored engine scripts a delegated agent
/// may not use to change stage status or routing, and their verbs.
const DELEGATED_SCRIPT_VERBS: [(&str, &[&str]); 12] = [
    ("focus-orchestrate.ts", &DELEGATED_ORCHESTRATE_VERBS),
    ("focus-jump.ts", &DELEGATED_JUMP_VERBS),
    ("focus-unit.ts", &DELEGATED_UNIT_VERBS),
    ("focus-bolt.ts", &DELEGATED_BOLT_VERBS),
    ("focus-swarm.ts", &DELEGATED_SWARM_VERBS),
    ("focus-log.ts", &DELEGATED_LOG_VERBS),
    ("focus-learnings.ts", &DELEGATED_LEARNINGS_VERBS),
    ("focus-runtime.ts", &DELEGATED_RUNTIME_VERBS),
    ("focus-testing-posture.ts", &DELEGATED_TESTING_POSTURE_VERBS),
    ("focus-worktree.ts", &DELEGATED_WORKTREE_VERBS),
    ("focus-audit.ts", &DELEGATED_AUDIT_SCRIPT_VERBS),
    ("focus-plugin.ts", &DELEGATED_PLUGIN_SCRIPT_VERBS),
];
/// Scripts no delegated agent runs at all.
const DELEGATED_WHOLE_SCRIPTS: [&str; 1] = ["focus-machine-config.ts"];

fn is_delegated_lifecycle_script(script: &str) -> bool {
    script == "focus-state.ts"
        || script == "focus-utility.ts"
        || DELEGATED_SCRIPT_VERBS
            .iter()
            .any(|(name, _)| *name == script)
}

fn script_verbs(script: &str) -> Option<&'static [&'static str]> {
    DELEGATED_SCRIPT_VERBS
        .iter()
        .find(|(name, _)| *name == script)
        .map(|(_, verbs)| *verbs)
}

/// `TESTING_POSTURE_SUBCOMMANDS`: the tool runs the first of these it
/// finds anywhere in argv.
const TESTING_POSTURE_SUBCOMMANDS: [&str; 8] = [
    "resolve",
    "render",
    "fingerprint",
    "verify",
    "begin",
    "brief",
    "reply",
    "restore",
];

/// Bare, these print the current selection instead of changing it.
const SELECTION_QUERIES: [&str; 1] = ["select-plugins"];
const DELEGATED_UTILITY_VERBS: [&str; 15] = [
    "scope-change",
    "scope-save",
    "config-change",
    "recompose",
    "intent-create",
    "state-init",
    "space-create",
    "reclassify",
    "select-plugins",
    "plugin-sync",
    "upgrade",
    "claim",
    "release",
    "participate",
    "set-status",
];

/// The same rule through the dispatcher: top-level routing groups.
const DELEGATED_DISPATCHER_GROUPS: [&str; 14] = [
    "next",
    "continue",
    "report",
    "park",
    "--resume",
    "--scope",
    "--claim",
    "--release",
    "scope-change",
    "scope-save",
    "config-change",
    "compose",
    "recompose",
    "init",
];
const DISPATCHER_SCOPE_VERBS: [&str; 2] = ["change", "save"];
const DISPATCHER_INTENT_VERBS: [&str; 1] = ["create"];
const DISPATCHER_CONFIG_VERBS: [&str; 2] = ["set", "global"];
const DISPATCHER_WORKSPACE_VERBS: [&str; 1] = ["reclassify"];
const DISPATCHER_PLUGIN_VERBS: [&str; 2] = ["select", "sync"];
/// `DELEGATED_DISPATCHER_VERBS`: the lifecycle verbs per noun.
const DELEGATED_DISPATCHER_VERBS: [(&str, &[&str]); 16] = [
    ("scope", &DISPATCHER_SCOPE_VERBS),
    ("orchestrate", &DELEGATED_ORCHESTRATE_VERBS),
    ("intent", &DISPATCHER_INTENT_VERBS),
    ("jump", &DELEGATED_JUMP_VERBS),
    ("config", &DISPATCHER_CONFIG_VERBS),
    ("workspace", &DISPATCHER_WORKSPACE_VERBS),
    ("plugin", &DISPATCHER_PLUGIN_VERBS),
    ("unit", &DELEGATED_UNIT_VERBS),
    ("bolt", &DELEGATED_BOLT_VERBS),
    ("swarm", &DELEGATED_SWARM_VERBS),
    ("log", &DELEGATED_LOG_VERBS),
    ("learnings", &DELEGATED_LEARNINGS_VERBS),
    ("runtime", &DELEGATED_RUNTIME_VERBS),
    ("testing-posture", &DELEGATED_TESTING_POSTURE_VERBS),
    ("worktree", &DELEGATED_WORKTREE_VERBS),
    ("audit", &DELEGATED_AUDIT_VERBS),
];

/// `LAUNCHER_GLOBAL_FLAGS`: dropped by the dispatcher before routing.
const LAUNCHER_GLOBAL_FLAGS: [&str; 6] = [
    "--json",
    "--quiet",
    "--no-color",
    "--yes",
    "--offline",
    "--verbose",
];

/// The reason a command is refused when its shape defeats inspection.
pub const DYNAMIC_EXECUTABLE: &str = "dynamic executable beyond guard inspection";
pub const DYNAMIC_SHELL: &str = "dynamic shell command beyond guard inspection";
pub const DYNAMIC_EVAL: &str = "dynamic eval shell command beyond guard inspection";
pub const EXECUTION_WRAPPER: &str = "execution wrapper beyond guard inspection";
pub const NESTED_LIMIT: &str = "nested shell command beyond guard inspection limit";
const UNINSPECTABLE_EXECUTION_WRAPPER: &str = "__focus_uninspectable_execution_wrapper__";

// --- Text as JavaScript holds it ------------------------------------------

/// A JavaScript string: UTF-16 code units, which is what upstream indexes.
fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn from_units(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}

fn is_ws(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(focus_js::text::is_js_whitespace)
}

const fn u(ch: char) -> u16 {
    ch as u16
}

/// `maskQuotedCommandSeparators`: inside quotes, shell separators (and in a
/// multi-line quote every character) become spaces, except the executable
/// body of a `$(...)` inside double quotes. Works over code points, as
/// upstream's `[...command]` does.
fn mask_quoted_command_separators(command: &str) -> String {
    let mut chars: Vec<char> = command.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let quote = chars[i];
        if quote != '\'' && quote != '"' && quote != '`' {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        let mut escaped = false;
        while end < chars.len() {
            let ch = chars[end];
            if quote != '\'' && !escaped && ch == '\\' {
                escaped = true;
                end += 1;
                continue;
            }
            if !escaped && ch == quote {
                break;
            }
            escaped = false;
            end += 1;
        }
        if end >= chars.len() {
            end = chars.len() - 1;
        }
        let multiline = chars[i..=end].contains(&'\n');
        let mut depth = 0;
        let mut j = i;
        while j <= end {
            let starts_sub = quote == '"'
                && chars[j] == '$'
                && chars.get(j + 1) == Some(&'(')
                && (j == 0 || chars[j - 1] != '\\');
            if starts_sub {
                depth += 1;
                j += 2;
                continue;
            }
            if quote == '"' && depth > 0 && chars[j] == ')' && (j == 0 || chars[j - 1] != '\\') {
                depth -= 1;
                j += 1;
                continue;
            }
            if depth > 0 {
                j += 1;
                continue;
            }
            if multiline {
                if chars[j] != '\n' {
                    chars[j] = ' ';
                }
            } else if matches!(chars[j], '&' | '|' | ';' | '(' | '{') {
                chars[j] = ' ';
            }
            j += 1;
        }
        i = end + 1;
    }
    chars.into_iter().collect()
}

const HEREDOC: &str = r#"<<(-)?\s*(?:'([^']+)'|"([^"]+)"|([A-Za-z_][A-Za-z0-9_]*))"#;

struct Heredoc {
    delimiter: String,
    strip_tabs: bool,
    /// An unquoted delimiter: the body is expanded, so its substitutions run.
    executable: bool,
    lines: Vec<String>,
}

fn heredocs_on(line: &str) -> Vec<Heredoc> {
    re::find_all(HEREDOC, "g", line)
        .into_iter()
        .filter_map(|found| {
            let group = |index| re::group(line, &found, index);
            let delimiter = group(2).or_else(|| group(3)).or_else(|| group(4))?;
            Some(Heredoc {
                delimiter: delimiter.to_owned(),
                strip_tabs: group(1) == Some("-"),
                executable: group(4).is_some(),
                lines: Vec::new(),
            })
        })
        .collect()
}

fn strip_tabs(line: &str) -> &str {
    line.trim_start_matches('\t')
}

/// `maskHeredocBodies`: a heredoc's body lines become blanks of their length.
fn mask_heredoc_bodies(command: &str) -> String {
    let mut lines: Vec<String> = command.split('\n').map(str::to_owned).collect();
    let mut pending: Vec<Heredoc> = Vec::new();
    for line in &mut lines {
        if let Some(active) = pending.first() {
            let candidate = if active.strip_tabs {
                strip_tabs(line).to_owned()
            } else {
                line.clone()
            };
            let length = line.encode_utf16().count();
            *line = " ".repeat(length);
            if candidate == active.delimiter {
                pending.remove(0);
            }
            continue;
        }
        pending.extend(heredocs_on(line));
    }
    lines.join("\n")
}

const FUNCTION_DEFINITION: &str = r"(?:^|[;\n])[ \t]*(?:(?:function[ \t]+)?[A-Za-z_][A-Za-z0-9_]*[ \t]*\([ \t]*\)|function[ \t]+[A-Za-z_][A-Za-z0-9_]*)[ \t\n]*\{";

/// `maskFunctionDefinitions`: a function's body, braces included, becomes
/// spaces. The definition is found on the joined text (UTF-16 indices) and
/// masked on the code points, as upstream does.
fn mask_function_definitions(command: &str) -> String {
    if !command.contains('{') {
        return command.to_owned();
    }
    let mut chars: Vec<char> = command.chars().collect();
    let pattern = re::compile(FUNCTION_DEFINITION, "g");
    let mut from = 0;
    loop {
        let source: String = chars.iter().collect();
        let source_units = units(&source);
        // `definition.exec(source())` from `lastIndex`: the first match at or
        // after it, in UTF-16 units of the joined text.
        let Some(found) = pattern
            .find_iter(&source)
            .find(|m| source[..m.range.start].encode_utf16().count() >= from)
        else {
            break;
        };
        let start_unit = source[..found.range.start].encode_utf16().count();
        let matched = &source[found.range.clone()];
        let matched_units = units(matched);
        let brace_in_match = matched_units
            .iter()
            .rposition(|&unit| unit == u('{'))
            .unwrap_or(0);
        let open = start_unit + brace_in_match;
        let mut depth = 0i32;
        let mut quote: Option<char> = None;
        let mut escaped = false;
        let mut end = open;
        while end < chars.len() {
            let ch = chars[end];
            if let Some(q) = quote {
                if q != '\'' && !escaped && ch == '\\' {
                    escaped = true;
                    end += 1;
                    continue;
                }
                if !escaped && ch == q {
                    quote = None;
                }
                escaped = false;
                end += 1;
                continue;
            }
            if ch == '\'' || ch == '"' || ch == '`' {
                quote = Some(ch);
            } else if ch == '{' {
                depth += 1;
            } else if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            end += 1;
        }
        if depth != 0 {
            break;
        }
        let leading = usize::from(
            matched_units
                .first()
                .is_some_and(|&unit| unit == u(';') || unit == u('\n')),
        );
        let start = start_unit + leading;
        let mut i = start;
        while i <= end && i < chars.len() {
            if chars[i] != '\n' {
                chars[i] = ' ';
            }
            i += 1;
        }
        from = end + 1;
        let _ = &source_units;
    }
    chars.into_iter().collect()
}

fn executable_shell_text(command: &str) -> String {
    mask_function_definitions(&mask_heredoc_bodies(&mask_quoted_command_separators(
        command,
    )))
}

const INVOCATION_HEAD: &str = r#"(?:^|&&|\|\||[;|(\n{])[ \t]*(?:(?:command|exec)\s+)?(?:env(?:\s+-[^\s]+)*\s+)?(?:[A-Za-z_][A-Za-z0-9_]*=(?:"[^"\n]*"|'[^'\n]*'|[^\s;&|]+)\s+)*"#;

fn script_invocation(scripts: &str) -> String {
    format!(
        r#"{INVOCATION_HEAD}(?:[^\s"';&|({{]+\/)?bun(?:\.exe)?(?:\s+run)?\s+(?:"[^"\n]*focus-{scripts}\.ts"|'[^'\n]*focus-{scripts}\.ts'|[^\s;&|]*focus-{scripts}\.ts)\s+([a-z][a-z0-9-]*)\b"#
    )
}

fn native_invocation(nouns: &str) -> String {
    format!(
        r#"{INVOCATION_HEAD}(?:"[^"\n]*\/focus(?:\.exe)?"|'[^'\n]*\/focus(?:\.exe)?'|[^\s"';&|({{]*focus(?:\.exe)?)[ \t]+engine[ \t]+{nouns}[ \t]+([a-z][a-z0-9-]*)\b"#
    )
}

/// `directStateTransition`: the blocked state verb a shell command calls
/// directly, at a command position, through the script, the native binary
/// or the dispatcher; or nothing.
#[must_use]
pub fn direct_state_transition(command: &str) -> Option<String> {
    let text = executable_shell_text(command);
    let script = script_invocation("state");
    for found in re::find_all(&script, "g", &text) {
        if let Some(verb) = re::group(&text, &found, 1)
            && is_blocked_state_transition(verb)
        {
            return Some(verb.to_owned());
        }
    }
    let native = native_invocation("state");
    for found in re::find_all(&native, "g", &text) {
        if let Some(verb) = re::group(&text, &found, 1)
            && is_blocked_state_transition(verb)
        {
            return Some(verb.to_owned());
        }
    }
    let delegated = delegated_lifecycle_command(command)?;
    let found = re::find(r"\bengine state ([a-z][a-z0-9-]*)$", "", &delegated)?;
    let verb = re::group(&delegated, &found, 1)?;
    is_blocked_state_transition(verb).then(|| verb.to_owned())
}

/// `isLifecycleBoundaryCommand`: an executable command that can cross a
/// stage or workflow boundary (prose in echoes, heredocs and function
/// bodies deliberately ignored).
#[must_use]
pub fn is_lifecycle_boundary_command(command: &str) -> bool {
    let text = executable_shell_text(command);
    let script = script_invocation("(orchestrate|state|jump)");
    for found in re::find_all(&script, "g", &text) {
        let tool = (1..=3).find_map(|index| re::group(&text, &found, index));
        let Some(verb) = re::group(&text, &found, 4) else {
            continue;
        };
        match tool {
            Some("orchestrate") if verb == "report" => return true,
            Some("state") if is_blocked_state_transition(verb) => return true,
            Some("jump") if verb == "execute" || verb == "reopen" => return true,
            _ => {}
        }
    }
    let native = native_invocation("(orchestrate|state|jump)");
    for found in re::find_all(&native, "g", &text) {
        let (Some(tool), Some(verb)) = (re::group(&text, &found, 1), re::group(&text, &found, 2))
        else {
            continue;
        };
        match tool {
            "orchestrate" if verb == "report" || verb == "park" => return true,
            "state" if is_blocked_state_transition(verb) => return true,
            "jump" if verb == "execute" || verb == "reopen" => return true,
            _ => {}
        }
    }
    false
}

/// `delegatedLifecycleCommand`: the engine command a delegated agent may
/// not run, named as the refusal names it; or nothing.
#[must_use]
pub fn delegated_lifecycle_command(command: &str) -> Option<String> {
    delegated_at_depth(command, 0)
}

/// `shellWords`: the words of a segment, quotes and escapes consumed.
fn shell_words(input: &str) -> Vec<String> {
    let text = units(input);
    let mut words: Vec<String> = Vec::new();
    let mut word: Vec<u16> = Vec::new();
    let mut quote: Option<u16> = None;
    let mut escaped = false;
    let mut started = false;
    let mut i = 0;
    while i < text.len() {
        let ch = text[i];
        if ch == u('\\') && text.get(i + 1) == Some(&u('\n')) {
            i += 2;
            continue;
        }
        if escaped {
            word.push(ch);
            escaped = false;
            started = true;
            i += 1;
            continue;
        }
        if ch == u('\\') && quote != Some(u('\'')) {
            escaped = true;
            started = true;
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
            }
            started = true;
            i += 1;
            continue;
        }
        if ch == u('$') && matches!(text.get(i + 1), Some(&q) if q == u('\'') || q == u('"')) {
            quote = Some(text[i + 1]);
            started = true;
            i += 2;
            continue;
        }
        if ch == u('\'') || ch == u('"') {
            quote = Some(ch);
            started = true;
        } else if is_ws(ch) {
            if started {
                words.push(from_units(&word));
                word.clear();
                started = false;
            }
        } else {
            word.push(ch);
            started = true;
        }
        i += 1;
    }
    if escaped {
        word.push(u('\\'));
    }
    if started {
        words.push(from_units(&word));
    }
    words
}

/// `maskRange`: positions `start..=end` become spaces, newlines kept.
fn mask_range(chars: &mut [char], start: usize, end: usize) {
    let mut i = start;
    while i <= end && i < chars.len() {
        if chars[i] != '\n' {
            chars[i] = ' ';
        }
        i += 1;
    }
}

/// `commandSubstitutionEnd`: the index of the `)` closing the substitution
/// opened at `open`, quotes and escapes respected; or nothing.
fn command_substitution_end(source: &[u16], open: usize) -> Option<usize> {
    let mut depth = 1;
    let mut quote: Option<u16> = None;
    let mut escaped = false;
    let mut i = open + 1;
    while i < source.len() {
        let ch = source[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if ch == u('\\') && quote != Some(u('\'')) {
            escaped = true;
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if ch == u('\'') || ch == u('"') || ch == u('`') {
            quote = Some(ch);
            i += 1;
            continue;
        }
        if ch == u('(') {
            depth += 1;
        }
        if ch == u(')') {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// `executableSubstitutions`: the bodies of `$(...)` and backtick
/// substitutions outside single quotes, and the command with each masked
/// to a `$`. The scan indexes code units and masks code points, as
/// upstream's does.
fn executable_substitutions(command: &str) -> (String, Vec<String>) {
    let text = units(command);
    let mut chars: Vec<char> = command.chars().collect();
    let mut bodies: Vec<String> = Vec::new();
    let mut quote: Option<u16> = None;
    let mut escaped = false;
    let mut i = 0;
    while i < text.len() {
        let ch = text[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if ch == u('\\') && quote != Some(u('\'')) {
            escaped = true;
            i += 1;
            continue;
        }
        if quote == Some(u('\'')) {
            if ch == u('\'') {
                quote = None;
            }
            i += 1;
            continue;
        }
        if ch == u('\'') && quote.is_none() {
            quote = Some(ch);
            i += 1;
            continue;
        }
        if ch == u('"') {
            quote = if quote == Some(u('"')) {
                None
            } else {
                Some(ch)
            };
            i += 1;
            continue;
        }
        if ch == u('`') {
            let mut end = i + 1;
            let mut inner_escaped = false;
            while end < text.len() {
                if inner_escaped {
                    inner_escaped = false;
                    end += 1;
                    continue;
                }
                if text[end] == u('\\') {
                    inner_escaped = true;
                    end += 1;
                    continue;
                }
                if text[end] == u('`') {
                    break;
                }
                end += 1;
            }
            if end >= text.len() {
                i += 1;
                continue;
            }
            bodies.push(from_units(&text[i + 1..end]));
            mask_range(&mut chars, i, end);
            if i < chars.len() {
                chars[i] = '$';
            }
            i = end + 1;
            continue;
        }
        if ch == u('$') && text.get(i + 1) == Some(&u('(')) {
            let Some(end) = command_substitution_end(&text, i + 1) else {
                i += 1;
                continue;
            };
            bodies.push(from_units(&text[i + 2..end]));
            mask_range(&mut chars, i, end);
            if i < chars.len() {
                chars[i] = '$';
            }
            i = end + 1;
            continue;
        }
        i += 1;
    }
    (chars.into_iter().collect(), bodies)
}

/// `heredocSubstitutionBodies`: the substitutions inside the bodies of
/// unquoted-delimiter heredocs, which the shell expands.
fn heredoc_substitution_bodies(command: &str) -> Vec<String> {
    let mut bodies: Vec<String> = Vec::new();
    let mut pending: Vec<Heredoc> = Vec::new();
    for line in command.split('\n') {
        if let Some(active) = pending.first_mut() {
            let candidate = if active.strip_tabs {
                strip_tabs(line)
            } else {
                line
            };
            if candidate == active.delimiter {
                if active.executable {
                    bodies.extend(executable_substitutions(&active.lines.join("\n")).1);
                }
                pending.remove(0);
            } else {
                active.lines.push(line.to_owned());
            }
            continue;
        }
        pending.extend(heredocs_on(line));
    }
    bodies
}

/// `shellCommandSegments`: the command split at separators outside quotes
/// and parameter expansions, comments dropped, each segment trimmed.
fn shell_command_segments(command: &str) -> Vec<String> {
    let text = units(command);
    let mut segments: Vec<String> = Vec::new();
    let mut start = 0;
    let mut quote: Option<u16> = None;
    let mut expansion_depth = 0;
    let mut escaped = false;
    let push = |segments: &mut Vec<String>, start: usize, end: usize| {
        if start < end {
            let segment = from_units(&text[start..end]);
            let trimmed = focus_js::text::trim(&segment);
            if !trimmed.is_empty() {
                segments.push(trimmed.to_owned());
            }
        }
    };
    let mut i = 0;
    while i < text.len() {
        let ch = text[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if ch == u('\\') && quote != Some(u('\'')) {
            escaped = true;
            i += 1;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if ch == u('$') && text.get(i + 1) == Some(&u('{')) {
            expansion_depth += 1;
            i += 2;
            continue;
        }
        if expansion_depth > 0 {
            if ch == u('}') {
                expansion_depth -= 1;
            }
            i += 1;
            continue;
        }
        if ch == u('\'') || ch == u('"') {
            quote = Some(ch);
            i += 1;
            continue;
        }
        let before_comment = i == 0
            || is_ws(text[i - 1])
            || matches!(text[i - 1], 0x3B | 0x26 | 0x7C | 0x28 | 0x29 | 0x7B | 0x7D);
        if ch == u('#') && before_comment {
            push(&mut segments, start, i);
            match text[i + 1..].iter().position(|&unit| unit == u('\n')) {
                None => {
                    start = text.len();
                    break;
                }
                Some(offset) => {
                    let newline = i + 1 + offset;
                    i = newline + 1;
                    start = newline + 1;
                    continue;
                }
            }
        }
        if matches!(ch, 0x3B | 0x7C | 0x26 | 0x0A | 0x28 | 0x29 | 0x7B | 0x7D) {
            push(&mut segments, start, i);
            if (ch == u('|') || ch == u('&')) && text.get(i + 1) == Some(&ch) {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    push(&mut segments, start, text.len());
    segments
}

fn command_basename(command: Option<&str>) -> String {
    command
        .unwrap_or("")
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn is_redirection(word: &str) -> bool {
    re::test(r"^\d*(?:<<<|<<-?|<>|>>?|<|>\||<&|>&)", "", word)
}

fn is_bare_redirection(word: &str) -> bool {
    re::test(r"^\d*(?:<<<|<<-?|<>|>>?|<|>\||<&|>&)$", "", word)
}

fn is_assignment_word(word: &str) -> bool {
    re::test(r"^[A-Za-z_][A-Za-z0-9_]*=", "", word)
}

/// `executableArgv`: the words of a segment from its executable on,
/// shell prefixes (control words, redirections, assignments) and the
/// wrappers `time`, `command`, `exec`, `env`, `nice` and `nohup` consumed.
/// Empty when the wrapper's options make the call nothing to inspect; a
/// marker when they make it impossible to inspect.
fn executable_argv(segment: &str) -> Vec<String> {
    let mut words = shell_words(segment);
    let mut cursor = 0;
    fn word_at(words: &[String], at: usize) -> &str {
        words.get(at).map_or("", String::as_str)
    }
    fn skip_redirections(words: &[String], cursor: &mut usize) {
        while is_redirection(word_at(words, *cursor)) {
            let redirection = word_at(words, *cursor).to_owned();
            *cursor += 1;
            if is_bare_redirection(&redirection) {
                *cursor += 1;
            }
        }
    }
    fn skip_prefixes(words: &[String], cursor: &mut usize) {
        let mut previous = usize::MAX;
        while *cursor != previous {
            previous = *cursor;
            while ["if", "then", "while", "until", "do", "else", "elif", "!"]
                .contains(&word_at(words, *cursor))
            {
                *cursor += 1;
            }
            skip_redirections(words, cursor);
            while is_assignment_word(word_at(words, *cursor)) {
                *cursor += 1;
            }
        }
    }
    let mut allow_shell_prefixes = true;
    while cursor < words.len() {
        if allow_shell_prefixes {
            skip_prefixes(&words, &mut cursor);
        } else {
            skip_redirections(&words, &mut cursor);
        }
        allow_shell_prefixes = false;
        let wrapper = command_basename(words.get(cursor).map(String::as_str));
        match wrapper.as_str() {
            "time" => {
                cursor += 1;
                while word_at(&words, cursor).starts_with('-') {
                    let option = word_at(&words, cursor).to_owned();
                    cursor += 1;
                    if ["-f", "--format", "-o", "--output"].contains(&option.as_str()) {
                        skip_redirections(&words, &mut cursor);
                        if cursor >= words.len() {
                            return Vec::new();
                        }
                        cursor += 1;
                    }
                }
                allow_shell_prefixes = true;
            }
            "command" | "exec" => {
                cursor += 1;
                while cursor < words.len() {
                    skip_redirections(&words, &mut cursor);
                    let option = word_at(&words, cursor).to_owned();
                    if option == "--" {
                        cursor += 1;
                        break;
                    }
                    if !option.starts_with('-') {
                        break;
                    }
                    if wrapper == "command" {
                        if re::test("[vV]", "", option.trim_start_matches('-')) {
                            return Vec::new();
                        }
                        if !re::test(r"^-p+$", "", &option) {
                            return Vec::new();
                        }
                        cursor += 1;
                        continue;
                    }
                    if option == "-a" {
                        cursor += 1;
                        skip_redirections(&words, &mut cursor);
                        if cursor >= words.len() {
                            return Vec::new();
                        }
                        cursor += 1;
                        continue;
                    }
                    if !re::test(r"^-[cl]+$", "", &option) {
                        return Vec::new();
                    }
                    cursor += 1;
                }
                allow_shell_prefixes = true;
            }
            "env" => {
                cursor += 1;
                while cursor < words.len() {
                    skip_redirections(&words, &mut cursor);
                    if cursor >= words.len() {
                        break;
                    }
                    let word = words[cursor].clone();
                    if is_assignment_word(&word) {
                        cursor += 1;
                        continue;
                    }
                    if word == "--" {
                        cursor += 1;
                        break;
                    }
                    let split_option_index = cursor;
                    let mut split: Option<String> = None;
                    let mut split_rest_index = cursor + 1;
                    if word.starts_with("-S") && word.encode_utf16().count() > 2 {
                        split = Some(word[2..].to_owned());
                    } else if let Some(rest) = word.strip_prefix("--split-string=") {
                        split = Some(rest.to_owned());
                    } else if word == "-S" || word == "--split-string" {
                        cursor += 1;
                        skip_redirections(&words, &mut cursor);
                        split = Some(word_at(&words, cursor).to_owned());
                        split_rest_index = cursor + 1;
                    }
                    if let Some(split) = split {
                        if re::test(r"[\\$`#]", "", &split) {
                            return vec![UNINSPECTABLE_EXECUTION_WRAPPER.to_owned()];
                        }
                        let mut rebuilt: Vec<String> = words[..split_option_index].to_vec();
                        rebuilt.extend(shell_words(&split));
                        rebuilt
                            .extend(words.get(split_rest_index..).unwrap_or(&[]).iter().cloned());
                        words = rebuilt;
                        cursor = split_option_index;
                        continue;
                    }
                    if ["-u", "--unset", "-C", "--chdir", "-P"].contains(&word.as_str()) {
                        cursor += 1;
                        skip_redirections(&words, &mut cursor);
                        if cursor >= words.len() {
                            return Vec::new();
                        }
                        cursor += 1;
                        continue;
                    }
                    if re::test(r"^-(?:u|C).+", "", &word)
                        || re::test(r"^(?:--unset|--chdir)=.+", "", &word)
                        || re::test(r"^-[iv]+$", "", &word)
                        || word == "-"
                        || ["--ignore-environment", "--debug", "--list-signal-handling"]
                            .contains(&word.as_str())
                        || re::test(r"^--(?:block|default|ignore)-signal(?:=.*)?$", "", &word)
                    {
                        cursor += 1;
                        continue;
                    }
                    if word == "-0" || word == "--null" {
                        return Vec::new();
                    }
                    if word == "--help" || word == "--version" {
                        return Vec::new();
                    }
                    if word.starts_with('-') {
                        return vec![UNINSPECTABLE_EXECUTION_WRAPPER.to_owned()];
                    }
                    break;
                }
            }
            "nice" => {
                cursor += 1;
                while cursor < words.len() {
                    skip_redirections(&words, &mut cursor);
                    if cursor >= words.len() {
                        break;
                    }
                    let word = words[cursor].clone();
                    if word == "--" {
                        cursor += 1;
                        break;
                    }
                    if word == "--help" || word == "--version" {
                        return Vec::new();
                    }
                    if word == "-n" || word == "--adjustment" {
                        cursor += 1;
                        skip_redirections(&words, &mut cursor);
                        if !re::test(r"^[+-]?\d+$", "", word_at(&words, cursor)) {
                            return Vec::new();
                        }
                        cursor += 1;
                        continue;
                    }
                    if re::test(r"^-n[+-]?\d+$", "", &word)
                        || re::test(r"^--adjustment=[+-]?\d+$", "", &word)
                        || re::test(r"^--?\d+$", "", &word)
                    {
                        cursor += 1;
                        continue;
                    }
                    if word.starts_with('-') {
                        return Vec::new();
                    }
                    break;
                }
            }
            "nohup" => {
                cursor += 1;
                let word = word_at(&words, cursor).to_owned();
                if word == "--help" || word == "--version" {
                    return Vec::new();
                }
                if word == "--" {
                    cursor += 1;
                } else if word.starts_with('-') {
                    return Vec::new();
                }
            }
            _ => break,
        }
    }
    words.get(cursor..).unwrap_or(&[]).to_vec()
}

/// `bunScriptInvocation`: the script a `bun` command runs and its
/// arguments; nothing for an eval or a bare `bun`.
fn bun_script_invocation(argv: &[String]) -> Option<(String, Vec<String>)> {
    const VALUE_OPTIONS: [&str; 9] = [
        "-C",
        "--cwd",
        "-r",
        "--preload",
        "--define",
        "--loader",
        "--conditions",
        "--env-file",
        "--config",
    ];
    const EVAL_OPTIONS: [&str; 4] = ["-e", "--eval", "-p", "--print"];
    let mut cursor = 1;
    let word = |at: usize| argv.get(at).map_or("", String::as_str);
    let skip_options = |cursor: &mut usize| -> bool {
        while word(*cursor).starts_with('-') {
            let option = word(*cursor);
            if option == "--" {
                *cursor += 1;
                return true;
            }
            if EVAL_OPTIONS.contains(&option) {
                return false;
            }
            *cursor += if VALUE_OPTIONS.contains(&option) && !option.contains('=') {
                2
            } else {
                1
            };
        }
        true
    };
    if !skip_options(&mut cursor) {
        return None;
    }
    if word(cursor) == "run" {
        cursor += 1;
        if !skip_options(&mut cursor) {
            return None;
        }
    }
    let script = command_basename(argv.get(cursor).map(String::as_str));
    (!script.is_empty()).then(|| (script, argv.get(cursor + 1..).unwrap_or(&[]).to_vec()))
}

fn without_project_dir(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--project-dir" {
            i += 2;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

/// `parseArgs`'s positional words: `--flag value` pairs and `--flag=value`
/// dropped, everything after `--` kept.
fn parse_args_positional(args: &[String]) -> Vec<String> {
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--" {
            positional.extend(args[i + 1..].iter().cloned());
            break;
        }
        if args[i].starts_with("--") {
            if args[i].contains('=') {
                i += 1;
                continue;
            }
            if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        positional.push(args[i].clone());
        i += 1;
    }
    positional
}

/// `scriptVerbCandidates`: the words a lifecycle script may take as its
/// verb.
fn script_verb_candidates(script: &str, args: &[String]) -> Vec<String> {
    if script == "focus-testing-posture.ts" {
        return args
            .iter()
            .find(|arg| TESTING_POSTURE_SUBCOMMANDS.contains(&arg.as_str()))
            .cloned()
            .into_iter()
            .collect();
    }
    let positional = without_project_dir(args);
    vec![
        positional.first().cloned().unwrap_or_default(),
        parse_args_positional(&positional)
            .first()
            .cloned()
            .unwrap_or_default(),
    ]
}

const RESERVED_FUTURE: [&str; 3] = ["rename", "show", "birth"];

/// `workspaceMutation`: the workspace nouns' switch, create and archive
/// forms (`parseWorkspaceCommand`), named for the refusal; nothing for a
/// list, a help, an error or a command that is not a workspace one.
fn workspace_mutation(prefix: &str, args: &[String]) -> Option<String> {
    let word = |at: usize| args.get(at).map(String::as_str);
    if word(1) == Some("--help") {
        return None;
    }
    let head = word(0)?;
    if head == "space-create" {
        if args.len() != 2 {
            return None;
        }
        return Some(format!("{prefix} space-create"));
    }
    if head != "intent" && head != "space" {
        return None;
    }
    let noun = head;
    let verb_or_name = word(1)?;
    if verb_or_name == "--json"
        || (noun == "intent" && verb_or_name == "--all")
        || verb_or_name == "help"
        || verb_or_name == "-h"
        || RESERVED_FUTURE.contains(&verb_or_name)
    {
        return None;
    }
    if noun == "intent" {
        match verb_or_name {
            "list" => return None,
            "switch" => {
                return (args.len() == 3).then(|| format!("{prefix} intent switch"));
            }
            "create" => return Some(format!("{prefix} intent create")),
            "archive" | "unarchive" => {
                let name = word(2);
                return name
                    .filter(|name| !name.starts_with("--"))
                    .map(|_| format!("{prefix} intent {verb_or_name}"));
            }
            "add-repo" | "remove-repo" => {
                return None;
            }
            _ => {}
        }
    }
    if noun == "space" {
        match verb_or_name {
            "list" => return None,
            "switch" => {
                return (args.len() == 3).then(|| format!("{prefix} space switch"));
            }
            "create" => {
                return (args.len() == 3).then(|| format!("{prefix} space create"));
            }
            _ => {}
        }
    }
    if args.iter().skip(2).any(|token| token.starts_with("--")) {
        return None;
    }
    Some(format!("{prefix} {noun} {verb_or_name}"))
}

fn without_dispatcher_flags(args: &[String]) -> Vec<String> {
    let literal = args.iter().position(|arg| arg == "--");
    args.iter()
        .enumerate()
        .filter(|(i, arg)| {
            literal.is_some_and(|at| *i >= at) || !LAUNCHER_GLOBAL_FLAGS.contains(&arg.as_str())
        })
        .map(|(_, arg)| arg.clone())
        .collect()
}

/// `delegatedDispatcherCommand`: the same rule through the dispatcher.
fn delegated_dispatcher_command(prefix: &str, raw_args: &[String]) -> Option<String> {
    let raw = without_dispatcher_flags(&without_project_dir(raw_args));
    let namespace = raw
        .first()
        .filter(|word| *word == "engine" || *word == "system")
        .cloned();
    let args: Vec<String> = if namespace.is_some() {
        raw[1..].to_vec()
    } else {
        raw.clone()
    };
    let route_prefix = match &namespace {
        Some(namespace) => format!("{prefix} {namespace}"),
        None => prefix.to_owned(),
    };
    let group = args.first().map_or("", String::as_str);
    let verb = args.get(1).map_or("", String::as_str);
    if DELEGATED_DISPATCHER_GROUPS.contains(&group) {
        return Some(format!("{route_prefix} {group}"));
    }
    if group == "plugin"
        && verb == "select"
        && parse_args_positional(args.get(2..).unwrap_or(&[])).is_empty()
    {
        return None;
    }
    if let Some((_, verbs)) = DELEGATED_DISPATCHER_VERBS
        .iter()
        .find(|(noun, _)| *noun == group)
        && verbs.contains(&verb)
    {
        return Some(format!("{route_prefix} {group} {verb}"));
    }
    if group == "state"
        && (is_delegated_state_mutation(verb) || verb == "init" || verb == "set-status")
    {
        return Some(format!("{route_prefix} state {verb}"));
    }
    workspace_mutation(&route_prefix, &args)
}

/// `delegatedUtilityCommand`: the utility's writing verbs.
fn delegated_utility_command(prefix: &str, raw_args: &[String]) -> Option<String> {
    let positional = parse_args_positional(raw_args);
    let verb = positional.first().map_or("", String::as_str);
    if SELECTION_QUERIES.contains(&verb) && positional.len() == 1 {
        return None;
    }
    if DELEGATED_UTILITY_VERBS.contains(&verb) {
        return Some(format!("{prefix} {verb}"));
    }
    workspace_mutation(prefix, &positional)
}

/// `assignment`: `NAME=value`, the value as written.
fn assignment(word: &str) -> Option<(String, String)> {
    let found = re::find(r"^([A-Za-z_][A-Za-z0-9_]*)=(.*)$", "s", word)?;
    Some((
        re::group(word, &found, 1)?.to_owned(),
        re::group(word, &found, 2).unwrap_or("").to_owned(),
    ))
}

/// `variableReference`: `$NAME` or `${NAME}`.
fn variable_reference(word: &str) -> Option<String> {
    let found = re::find(r"^\$([A-Za-z_][A-Za-z0-9_]*)$", "", word)
        .or_else(|| re::find(r"^\$\{([A-Za-z_][A-Za-z0-9_]*)\}$", "", word))?;
    re::group(word, &found, 1).map(str::to_owned)
}

fn delegated_at_depth(command: &str, depth: u32) -> Option<String> {
    if depth > 8 {
        return Some(NESTED_LIMIT.to_owned());
    }
    let heredoc_bodies = heredoc_substitution_bodies(command);
    let source = mask_heredoc_bodies(command);
    let (masked, bodies) = executable_substitutions(&source);
    for body in heredoc_bodies.iter().chain(bodies.iter()) {
        if let Some(nested) = delegated_at_depth(body, depth + 1) {
            return Some(nested);
        }
    }
    // Only standalone literal assignments survive into later segments.
    let mut assignments: Vec<(String, String)> = Vec::new();
    let lookup = |assignments: &[(String, String)], name: &str| -> Option<String> {
        assignments
            .iter()
            .rev()
            .find(|(known, _)| known == name)
            .map(|(_, value)| value.clone())
    };
    for segment in shell_command_segments(&masked) {
        let segment_words = shell_words(&segment);
        let segment_assignments: Vec<Option<(String, String)>> =
            segment_words.iter().map(|word| assignment(word)).collect();
        if !segment_assignments.is_empty() && segment_assignments.iter().all(Option::is_some) {
            for (name, value) in segment_assignments.into_iter().flatten() {
                assignments.retain(|(known, _)| *known != name);
                assignments.push((name, value));
            }
            continue;
        }
        let mut argv = executable_argv(&segment);
        let head = argv.first().cloned().unwrap_or_default();
        if let Some(variable) = variable_reference(&head) {
            let resolved = lookup(&assignments, &variable)
                .map(|value| shell_words(&value))
                .unwrap_or_default();
            if resolved.len() != 1 {
                return Some(DYNAMIC_EXECUTABLE.to_owned());
            }
            argv[0] = resolved[0].clone();
        }
        if argv.first().is_some_and(|word| word.contains('$')) {
            return Some(DYNAMIC_EXECUTABLE.to_owned());
        }
        let executable = command_basename(argv.first().map(String::as_str));
        if executable == UNINSPECTABLE_EXECUTION_WRAPPER {
            return Some(EXECUTION_WRAPPER.to_owned());
        }
        if executable == "eval" {
            let mut eval_args: Vec<String> = argv.get(1..).unwrap_or(&[]).to_vec();
            if eval_args.first().is_some_and(|word| word == "--") {
                eval_args.remove(0);
            }
            let nested = delegated_at_depth(&eval_args.join(" "), depth + 1);
            if nested.as_deref() == Some(DYNAMIC_EXECUTABLE)
                || nested.as_deref() == Some(DYNAMIC_SHELL)
            {
                return Some(DYNAMIC_EVAL.to_owned());
            }
            if nested.is_some() {
                return nested;
            }
            if re::test(r"[$`\\]", "", &segment) {
                return Some(DYNAMIC_EVAL.to_owned());
            }
            continue;
        }
        if re::test(r"^(?:ba|da|a|k|z)?sh(?:\.exe)?$", "", &executable) {
            let mut i = 1;
            while i < argv.len() {
                let option = argv[i].as_str();
                if ["-O", "+O", "-o", "+o", "--rcfile", "--init-file"].contains(&option) {
                    i += 2;
                    continue;
                }
                if option == "-c" || re::test(r"^-[A-Za-z]*c[A-Za-z]*$", "", option) {
                    let mut command_index = i + 1;
                    if argv.get(command_index).is_some_and(|word| word == "--") {
                        command_index += 1;
                    }
                    let mut nested_command = argv.get(command_index).cloned().unwrap_or_default();
                    if let Some(variable) = variable_reference(&nested_command) {
                        match lookup(&assignments, &variable) {
                            None => return Some(DYNAMIC_SHELL.to_owned()),
                            Some(value) => nested_command = value,
                        }
                    }
                    if nested_command.contains('$') {
                        return Some(DYNAMIC_SHELL.to_owned());
                    }
                    if let Some(nested) = delegated_at_depth(&nested_command, depth + 1) {
                        return Some(nested);
                    }
                    break;
                }
                if !option.starts_with('-') {
                    break;
                }
                i += 1;
            }
            continue;
        }
        let mut args: Vec<String> = argv.get(1..).unwrap_or(&[]).to_vec();
        let mut script = executable.clone();
        if re::test(r"^bun(?:\.exe)?$", "", &executable) {
            let Some((found, found_args)) = bun_script_invocation(&argv) else {
                continue;
            };
            script = found;
            args = found_args;
        }
        if DELEGATED_WHOLE_SCRIPTS.contains(&script.as_str()) {
            return Some(script);
        }
        if is_delegated_lifecycle_script(&script) {
            let refused = script_verb_candidates(&script, &args)
                .into_iter()
                .find(|verb| {
                    (script == "focus-state.ts" && is_delegated_state_mutation(verb))
                        || script_verbs(&script).is_some_and(|verbs| verbs.contains(&verb.as_str()))
                });
            if let Some(verb) = refused {
                return Some(format!("{script} {verb}"));
            }
            if script == "focus-utility.ts"
                && let Some(utility) = delegated_utility_command("focus-utility.ts", &args)
            {
                return Some(utility);
            }
            continue;
        }
        if script == "focus.ts" {
            if let Some(delegated) = delegated_dispatcher_command("focus.ts", &args) {
                return Some(delegated);
            }
            continue;
        }
        if re::test(r"^focus(?:\.exe)?$", "", &script)
            && let Some(delegated) = delegated_dispatcher_command("focus", &args)
        {
            return Some(delegated);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direct(command: &str) -> Option<&str> {
        // The answer is a verb from the table, so the borrow is of the table.
        direct_state_transition(command).map(|verb| {
            BLOCKED_STATE_TRANSITIONS
                .into_iter()
                .find(|&known| known == verb)
                .unwrap()
        })
    }

    fn delegated(command: &str) -> Option<String> {
        delegated_lifecycle_command(command)
    }

    // The corpus (`fixtures/oracle`, family `state_transition`) compares the
    // three readers with upstream on every generated command; the tests here
    // name the boundaries a reader of this module should know.

    #[test]
    fn a_direct_transition_is_a_state_verb_at_a_command_position() {
        assert_eq!(
            direct("bun .claude/tools/focus-state.ts approve x"),
            Some("approve")
        );
        assert_eq!(
            direct("cd /p && bun focus-state.ts set 'Scope=x'"),
            Some("set")
        );
        assert_eq!(
            direct("env X=1 bun run focus-state.ts advance"),
            Some("advance")
        );
        // An assignment before `env` is not a shape the direct reader knows;
        // the delegated reader looks through it.
        assert_eq!(direct("X=1 env bun run focus-state.ts advance"), None);
        assert_eq!(direct("focus engine state skip x"), Some("skip"));
        assert_eq!(direct("./focus.exe engine state park"), Some("park"));
        assert_eq!(direct("bun focus.ts engine state approve"), Some("approve"));
        assert_eq!(direct("bun focus-state.ts get Scope"), None);
        assert_eq!(direct("bun focus-state.ts resume"), None);
        assert_eq!(direct("bun focus-state.ts approvex"), None);
        assert_eq!(direct("bun other-state.ts approve"), None);
    }

    #[test]
    fn prose_heredocs_and_function_bodies_are_not_commands() {
        assert_eq!(direct("echo 'bun focus-state.ts approve x'"), None);
        assert_eq!(direct("echo \"bun focus-state.ts approve x\""), None);
        assert_eq!(direct("cat <<EOF\nbun focus-state.ts approve x\nEOF"), None);
        assert_eq!(direct("f() { bun focus-state.ts approve x; }"), None);
        assert_eq!(
            direct("f() { x; }; bun focus-state.ts approve x"),
            Some("approve")
        );
        // A substitution inside double quotes is executable.
        assert_eq!(
            direct("echo \"$(bun focus-state.ts approve x)\""),
            Some("approve")
        );
        assert_eq!(direct("echo '$(bun focus-state.ts approve x)'"), None);
        // A multi-line quote hides every character but its newlines.
        assert_eq!(direct("echo 'a\nbun focus-state.ts approve x'"), None);
        assert_eq!(
            direct("echo 'a\n'; bun focus-state.ts approve x"),
            Some("approve")
        );
    }

    #[test]
    fn the_boundary_commands_are_report_park_execute_and_reopen() {
        assert!(is_lifecycle_boundary_command(
            "bun focus-orchestrate.ts report --stage x"
        ));
        assert!(is_lifecycle_boundary_command(
            "focus engine orchestrate park"
        ));
        assert!(is_lifecycle_boundary_command("bun focus-jump.ts execute"));
        assert!(is_lifecycle_boundary_command("focus engine jump reopen"));
        assert!(is_lifecycle_boundary_command(
            "bun focus-state.ts approve x"
        ));
        // The script form of `park` is the state tool's; the orchestrator's
        // script has no park verb at a boundary.
        assert!(!is_lifecycle_boundary_command(
            "bun focus-orchestrate.ts park"
        ));
        assert!(!is_lifecycle_boundary_command(
            "bun focus-orchestrate.ts next"
        ));
        assert!(!is_lifecycle_boundary_command(
            "echo 'focus engine orchestrate report'"
        ));
    }

    #[test]
    fn a_delegated_agent_may_not_route_or_change_status() {
        assert_eq!(
            delegated("bun .claude/tools/focus-orchestrate.ts next").as_deref(),
            Some("focus-orchestrate.ts next")
        );
        assert_eq!(
            delegated("bun focus-state.ts unpark").as_deref(),
            Some("focus-state.ts unpark")
        );
        assert_eq!(
            delegated("bun focus-state.ts --project-dir /p approve").as_deref(),
            Some("focus-state.ts approve")
        );
        assert_eq!(
            delegated("bun focus-state.ts --stage x approve").as_deref(),
            Some("focus-state.ts approve")
        );
        assert_eq!(
            delegated("bun focus-testing-posture.ts --stage x begin").as_deref(),
            Some("focus-testing-posture.ts begin")
        );
        assert_eq!(
            delegated("bun focus-machine-config.ts get x").as_deref(),
            Some("focus-machine-config.ts")
        );
        assert_eq!(
            delegated("bun focus-utility.ts scope-change").as_deref(),
            Some("focus-utility.ts scope-change")
        );
        assert_eq!(delegated("bun focus-utility.ts select-plugins"), None);
        assert_eq!(
            delegated("bun focus-utility.ts select-plugins a").as_deref(),
            Some("focus-utility.ts select-plugins")
        );
        assert_eq!(delegated("bun focus-state.ts get Scope"), None);
        assert_eq!(delegated("bun focus-orchestrate.ts wait"), None);
        assert_eq!(delegated("ls -la && git status"), None);
    }

    #[test]
    fn the_dispatcher_is_read_with_its_namespace_and_global_flags() {
        assert_eq!(delegated("focus next").as_deref(), Some("focus next"));
        assert_eq!(
            delegated("focus engine orchestrate park").as_deref(),
            Some("focus engine orchestrate park")
        );
        assert_eq!(
            delegated("focus --json --project-dir /p system state init").as_deref(),
            Some("focus system state init")
        );
        assert_eq!(
            delegated("./focus.exe intent switch other").as_deref(),
            Some("focus intent switch")
        );
        assert_eq!(
            delegated("bun focus.ts space-create x").as_deref(),
            Some("focus.ts space-create")
        );
        assert_eq!(delegated("focus intent switch"), None);
        assert_eq!(delegated("focus intent list"), None);
        assert_eq!(delegated("focus intent rename a"), None);
        assert_eq!(
            delegated("focus intent x y").as_deref(),
            Some("focus intent x")
        );
        assert_eq!(delegated("focus intent x --flag"), None);
        assert_eq!(delegated("focus plugin select"), None);
        assert_eq!(
            delegated("focus plugin select a").as_deref(),
            Some("focus plugin select")
        );
        assert_eq!(delegated("focus -- next"), None);
        assert_eq!(delegated("focusx next"), None);
    }

    #[test]
    fn wrappers_are_looked_through_and_the_uninspectable_ones_named() {
        assert_eq!(
            delegated("time -p focus next").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("command -p focus next").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("exec -a name focus next").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("env -i X=1 -- focus next").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("nice -n 5 nohup focus next &").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("env -S 'focus next'").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("env -S 'focus $X'").as_deref(),
            Some(EXECUTION_WRAPPER)
        );
        assert_eq!(
            delegated("env -Z focus next").as_deref(),
            Some(EXECUTION_WRAPPER)
        );
        // Options that make the wrapper print instead of run.
        assert_eq!(delegated("command -v focus next"), None);
        assert_eq!(delegated("env --help focus next"), None);
        assert_eq!(delegated("nice -x focus next"), None);
        assert_eq!(delegated("sudo focus next"), None);
    }

    #[test]
    fn nested_shells_and_assignments_are_followed() {
        assert_eq!(
            delegated("echo $(focus next)").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("echo `focus next`").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("cat <<EOF\n$(focus next)\nEOF").as_deref(),
            Some("focus next")
        );
        assert_eq!(delegated("cat <<'EOF'\n$(focus next)\nEOF"), None);
        assert_eq!(
            delegated("sh -c 'focus next'").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("bash -lc 'focus next'").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("eval 'focus next'").as_deref(),
            Some("focus next")
        );
        // A variable standing for the whole command is one word at most;
        // one holding two words is a dynamic executable.
        assert_eq!(
            delegated("CMD=focus\n$CMD next").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("X='focus next'\n$X").as_deref(),
            Some(DYNAMIC_EXECUTABLE)
        );
        // `eval` re-reads its words, so a variable there is not resolved.
        assert_eq!(
            delegated("X='focus next'\neval \"$X\"").as_deref(),
            Some(DYNAMIC_EVAL)
        );
        assert_eq!(
            delegated("X='focus next'\nsh -c \"$X\"").as_deref(),
            Some("focus next")
        );
        assert_eq!(
            delegated("CMD=ls\nCMD=focus\n$CMD next").as_deref(),
            Some("focus next")
        );
        assert_eq!(delegated("CMD=focus\nCMD=ls\n$CMD next"), None);
        // Only the direct reader masks function bodies: a delegated agent
        // defining a function that routes is held, as upstream holds it.
        assert_eq!(
            delegated("f() { focus next; }").as_deref(),
            Some("focus next")
        );
    }

    #[test]
    fn what_cannot_be_inspected_is_named_for_the_refusal() {
        assert_eq!(delegated("$CMD next").as_deref(), Some(DYNAMIC_EXECUTABLE));
        assert_eq!(delegated("a$X next").as_deref(), Some(DYNAMIC_EXECUTABLE));
        assert_eq!(delegated("sh -c \"$X\"").as_deref(), Some(DYNAMIC_SHELL));
        assert_eq!(
            delegated("sh -c 'focus $X'").as_deref(),
            Some(DYNAMIC_SHELL)
        );
        assert_eq!(delegated("eval \"$X\"").as_deref(), Some(DYNAMIC_EVAL));
        assert_eq!(delegated("eval 'echo `x`'").as_deref(), Some(DYNAMIC_EVAL));
        assert_eq!(delegated("eval echo x"), None);
        let nine = (0..9).fold("ls".to_owned(), |inner, _| format!("echo $({inner})"));
        assert_eq!(delegated(&nine).as_deref(), Some(NESTED_LIMIT));
        let eight = (0..8).fold("ls".to_owned(), |inner, _| format!("echo $({inner})"));
        assert_eq!(delegated(&eight), None);
    }

    #[test]
    fn the_scan_indexes_code_units_and_masks_code_points_as_upstream_does() {
        // A character outside the BMP is two code units in JavaScript and
        // one code point in the array upstream masks: the mask lands one
        // position early per such character before it, and the readers
        // here follow that arithmetic (the corpus checks it on hundreds of
        // commands). The observable consequence: the masked `$` of a
        // substitution after an emoji is placed where upstream places it.
        let (masked, bodies) = executable_substitutions("echo \u{1F600} $(focus next)");
        assert_eq!(bodies, vec!["focus next".to_owned()]);
        assert_eq!(
            masked.chars().count(),
            "echo \u{1F600} $(focus next)".chars().count()
        );
        assert!(masked.contains('$'));
        assert_eq!(
            delegated("echo \u{1F600} $(focus next)").as_deref(),
            Some("focus next")
        );
    }

    #[test]
    fn shell_words_consume_quotes_and_escapes() {
        assert_eq!(
            shell_words("a 'b c' \"d e\" f\\ g"),
            ["a", "b c", "d e", "f g"]
        );
        assert_eq!(shell_words("$'a' $\"b\" c\\\nd"), ["a", "b", "cd"]);
        assert_eq!(shell_words("  a\tb  "), ["a", "b"]);
        assert_eq!(shell_words("a\\"), ["a\\"]);
        assert_eq!(shell_words("'unterminated"), ["unterminated"]);
        assert!(shell_words("").is_empty());
    }

    #[test]
    fn segments_split_at_separators_outside_quotes_and_drop_comments() {
        assert_eq!(
            shell_command_segments("a; b && c | d || e\nf & g"),
            ["a", "b", "c", "d", "e", "f", "g"]
        );
        assert_eq!(
            shell_command_segments("echo 'a;b' # c\nd"),
            ["echo 'a;b'", "d"]
        );
        assert_eq!(
            shell_command_segments("echo ${X:-a;b}; c"),
            ["echo ${X:-a;b}", "c"]
        );
        assert_eq!(shell_command_segments("x#y"), ["x#y"]);
        assert_eq!(shell_command_segments("( a ) { b; }"), ["a", "b"]);
    }

    #[test]
    fn executable_argv_skips_prefixes_and_redirections() {
        assert_eq!(
            executable_argv("if X=1 2>err focus next"),
            ["focus", "next"]
        );
        assert_eq!(executable_argv("> out focus next"), ["focus", "next"]);
        assert_eq!(
            executable_argv("time -f %e -o out focus next"),
            ["focus", "next"]
        );
        assert!(executable_argv("time -f").is_empty());
        assert_eq!(
            executable_argv("env -u X X=1 focus next"),
            ["focus", "next"]
        );
        assert_eq!(executable_argv("env -S 'a b' c"), ["a", "b", "c"]);
        assert_eq!(
            executable_argv("env -S 'a $b'"),
            [UNINSPECTABLE_EXECUTION_WRAPPER]
        );
    }

    #[test]
    fn bun_options_are_skipped_up_to_the_script() {
        let argv = |text: &str| text.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            bun_script_invocation(&argv("bun --cwd /p run -r x focus-state.ts approve")),
            Some(("focus-state.ts".to_owned(), vec!["approve".to_owned()]))
        );
        assert_eq!(
            bun_script_invocation(&argv("bun -e focus-state.ts approve")),
            None
        );
        assert_eq!(bun_script_invocation(&argv("bun")), None);
        assert_eq!(
            bun_script_invocation(&argv("bun -- /p/focus-state.ts")),
            Some(("focus-state.ts".to_owned(), vec![]))
        );
    }

    #[test]
    fn parse_args_positional_follows_the_engines_parser() {
        let args = |text: &str| text.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            parse_args_positional(&args("--stage x approve")),
            ["approve"]
        );
        assert_eq!(
            parse_args_positional(&args("--stage=x approve")),
            ["approve"]
        );
        // A flag takes the next word as its value unless that word is a flag.
        assert!(parse_args_positional(&args("--json approve")).is_empty());
        assert!(parse_args_positional(&args("--json --quiet approve")).is_empty());
        assert_eq!(
            parse_args_positional(&args("--json --quiet -- approve")),
            ["approve"]
        );
        assert_eq!(
            parse_args_positional(&args("approve -- --json")),
            ["approve", "--json"]
        );
    }

    #[test]
    fn workspace_mutations_follow_the_workspace_parser() {
        let args = |text: &str| text.split(' ').map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(
            workspace_mutation("focus", &args("intent create a")).as_deref(),
            Some("focus intent create")
        );
        // Only `--help` as the verb is the help form; a later one is an argument.
        assert_eq!(workspace_mutation("focus", &args("intent --help")), None);
        assert_eq!(
            workspace_mutation("focus", &args("intent create --help")).as_deref(),
            Some("focus intent create")
        );
        assert_eq!(
            workspace_mutation("focus", &args("intent archive a")).as_deref(),
            Some("focus intent archive")
        );
        assert_eq!(
            workspace_mutation("focus", &args("intent archive --json")),
            None
        );
        assert_eq!(
            workspace_mutation("focus", &args("intent add-repo r")),
            None
        );
        assert_eq!(
            workspace_mutation("focus", &args("space create a")).as_deref(),
            Some("focus space create")
        );
        assert_eq!(workspace_mutation("focus", &args("space create a b")), None);
        assert_eq!(
            workspace_mutation("focus", &args("space x")).as_deref(),
            Some("focus space x")
        );
        assert_eq!(workspace_mutation("focus", &args("space-create a b")), None);
        assert_eq!(workspace_mutation("focus", &args("other x")), None);
    }
}

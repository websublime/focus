//! Reading a PowerShell command line: its words in argument mode, its
//! statements and pipelines, and the groups that run commands of their own.
//!
//! This reader answers what a command line may write, so it reads every
//! line and keeps the words it cannot place as candidates. It does not
//! evaluate what PowerShell computes (variables, groups, splatting): those
//! words are read as written.
//!
//! Mirrors `powerShellQuoteEnd`, `powerShellGroupEnd`, `powerShellWord`,
//! `readPowerShell`, `powerShellCommandName` and
//! `powerShellLocationChanges` in `review-freeze-command.ts`.

use focus_js::re;

use super::cmdlet::{CmdletArg, CmdletWrite, cmdlet_write_targets};
use super::invocation::shell_executable_name;
use super::words::blank;

/// PowerShell reads the typographic quotes and dashes as ' " and -.
fn is_single_quote(ch: Option<char>) -> bool {
    ch.is_some_and(|ch| "'‘’‚‛".contains(ch))
}

fn is_double_quote(ch: Option<char>) -> bool {
    ch.is_some_and(|ch| "\"“”„".contains(ch))
}

const PARAMETER: &str = r"^[-–—―]([A-Za-z_][\w-]*)$";

fn parameter_name(value: &str) -> Option<String> {
    re::find(PARAMETER, "", value).and_then(|found| re::group(value, &found, 1).map(str::to_owned))
}

fn is_blank(ch: char) -> bool {
    ch != '\n' && ch != '\r' && blank(ch)
}

/// `command.slice(from, to)`: empty when the range is inverted.
fn text(chars: &[char], from: usize, to: usize) -> String {
    let (from, to) = (from.min(chars.len()), to.min(chars.len()));
    if from >= to {
        return String::new();
    }
    chars[from..to].iter().collect()
}

/// The index after a quoted string that opens at `start`, or the end of the
/// command when it never closes. A doubled quote is one literal quote.
fn quote_end(chars: &[char], start: usize) -> usize {
    let double = is_double_quote(chars.get(start).copied());
    let closes = |ch: Option<char>| {
        if double {
            is_double_quote(ch)
        } else {
            is_single_quote(ch)
        }
    };
    let mut i = start + 1;
    while i < chars.len() {
        let ch = chars[i];
        if double && ch == '`' {
            i += 2;
            continue;
        }
        if double && ch == '$' && chars.get(i + 1) == Some(&'(') {
            i = group_end(chars, i + 1);
            continue;
        }
        if !closes(Some(ch)) {
            i += 1;
            continue;
        }
        if closes(chars.get(i + 1).copied()) {
            i += 2;
            continue;
        }
        return i + 1;
    }
    chars.len()
}

/// The index after the (...), {...} or [...] group that opens at `start`,
/// quotes and inner groups included, or the end of an unclosed command.
fn group_end(chars: &[char], start: usize) -> usize {
    let mut closers: Vec<char> = Vec::new();
    let mut i = start;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '`' {
            i += 2;
            continue;
        }
        if is_single_quote(Some(ch)) || is_double_quote(Some(ch)) {
            i = quote_end(chars, i);
            continue;
        }
        if ch == '(' {
            closers.push(')');
        } else if ch == '{' {
            closers.push('}');
        } else if ch == '[' {
            closers.push(']');
        } else if closers.last() == Some(&ch) {
            closers.pop();
            if closers.is_empty() {
                return i + 1;
            }
        }
        i += 1;
    }
    chars.len()
}

/// One argument-mode word.
#[derive(Debug, Clone)]
pub struct Word {
    pub arg: CmdletArg,
    /// Written without quotes, variables or groups.
    pub bare: bool,
    /// Holds a variable or a group, so its value is computed.
    pub computed: bool,
    /// Holds a backtick escape, so it is never a parameter or `--%`.
    pub escaped: bool,
}

/// `/\[[^\s;|&<>\]]*\]/y` at `i`: a `[Type]` that closes within the word.
fn type_name_at(chars: &[char], i: usize) -> bool {
    if chars.get(i) != Some(&'[') {
        return false;
    }
    let mut j = i + 1;
    while j < chars.len() {
        let ch = chars[j];
        if ch == ']' {
            return true;
        }
        if blank(ch) || ";|&<>".contains(ch) {
            return false;
        }
        j += 1;
    }
    false
}

/// `powerShellWord`: one word from `start`. Backslashes are literal; a
/// backtick escapes the next character; single quotes are literal and a
/// doubled quote inside is one quote. Text inside (...), $(...), @(...) and
/// {...} runs as commands of its own, so it is handed to `nested`; the word
/// keeps the group as written. A redirect target (`as_value`) is a value,
/// whatever it looks like.
fn power_shell_word(
    chars: &[char],
    start: usize,
    nested: &mut Vec<String>,
    as_value: bool,
) -> (Word, usize) {
    let mut values: Vec<String> = Vec::new();
    let mut value = String::new();
    let mut parameter: Option<String> = None;
    let mut attached = false;
    let mut bare = true;
    let mut computed = false;
    let mut quoted = false;
    // A backtick anywhere (`-Name, -`Name) makes the word a value.
    let mut escaped = as_value;
    let mut i = start;
    while i < chars.len() {
        let ch = chars[i];
        if is_blank(ch) || ";|&<>\n\r".contains(ch) {
            break;
        }
        if ch == '`' {
            if matches!(chars.get(i + 1), Some('\n' | '\r')) {
                break;
            }
            escaped = true;
            if let Some(next) = chars.get(i + 1) {
                value.push(*next);
            }
            i += 2;
            continue;
        }
        if ch == ',' {
            // An array continues past blanks after its comma: a, b is one
            // value.
            values.push(std::mem::take(&mut value));
            i += 1;
            loop {
                match chars.get(i) {
                    Some(next) if is_blank(*next) || *next == '\n' || *next == '\r' => i += 1,
                    Some('`') if chars.get(i + 1) == Some(&'\n') => i += 2,
                    Some('`') if chars.get(i + 1) == Some(&'\r') => {
                        i += if chars.get(i + 2) == Some(&'\n') {
                            3
                        } else {
                            2
                        };
                    }
                    _ => break,
                }
            }
            continue;
        }
        if is_single_quote(Some(ch)) {
            let mut j = i + 1;
            while j < chars.len() {
                if !is_single_quote(Some(chars[j])) {
                    value.push(chars[j]);
                } else if is_single_quote(chars.get(j + 1).copied()) {
                    value.push(chars[j]);
                    j += 1;
                } else {
                    break;
                }
                j += 1;
            }
            bare = false;
            quoted = true;
            i = j + 1;
            continue;
        }
        if is_double_quote(Some(ch)) {
            let mut j = i + 1;
            while j < chars.len() {
                let inner = chars[j];
                if inner == '`' {
                    j += 1;
                    if let Some(next) = chars.get(j) {
                        value.push(*next);
                    }
                } else if inner == '$' && chars.get(j + 1) == Some(&'(') {
                    let end = group_end(chars, j + 1);
                    nested.push(text(chars, j + 2, end.saturating_sub(1)));
                    value.push_str(&text(chars, j, end));
                    computed = true;
                    j = end - 1;
                } else if is_double_quote(Some(inner)) {
                    if !is_double_quote(chars.get(j + 1).copied()) {
                        break;
                    }
                    value.push(chars[j]);
                    j += 1;
                } else {
                    if inner == '$' {
                        computed = true;
                    }
                    value.push(inner);
                }
                j += 1;
            }
            bare = false;
            quoted = true;
            i = j + 1;
            continue;
        }
        // A parameter name ends at ( { . or [: -Path(...) and -Path.x are
        // -Path and its value.
        if "({.[".contains(ch)
            && parameter.is_none()
            && bare
            && !escaped
            && values.is_empty()
            && re::test(PARAMETER, "", &value)
        {
            break;
        }
        let sigil = (ch == '$' || ch == '@') && matches!(chars.get(i + 1), Some('(' | '{'));
        // [Type] closes within the word; any other [ is an ordinary character.
        let type_name = ch == '[' && value.is_empty() && type_name_at(chars, i);
        if sigil || ch == '(' || ch == '{' || type_name {
            let open = if sigil { i + 1 } else { i };
            let end = group_end(chars, open);
            // ${name} is a variable and [Type] a type name: neither runs
            // commands.
            if !(ch == '$' && chars.get(open) == Some(&'{')) && ch != '[' {
                nested.push(text(chars, open + 1, (end.saturating_sub(1)).max(open + 1)));
            }
            // A group that holds one string literal is that string.
            let group = text(chars, open, end);
            let literal = if ch == '(' || (ch == '@' && chars.get(open) == Some(&'(')) {
                re::find(r#"^\(\s*(?:'([^']*)'|"([^"`$]*)")\s*\)$"#, "", &group).map(|found| {
                    re::group(&group, &found, 1)
                        .or_else(|| re::group(&group, &found, 2))
                        .unwrap_or("")
                        .to_owned()
                })
            } else {
                None
            };
            if let Some(literal) = literal {
                value.push_str(&literal);
                bare = false;
                quoted = true;
                i = end;
                continue;
            }
            value.push_str(&text(chars, i, end));
            bare = false;
            computed = true;
            i = end;
            continue;
        }
        // $name is a variable and @name splats one.
        if ch == '$'
            || (ch == '@'
                && value.is_empty()
                && chars.get(i + 1).is_some_and(|next| {
                    next.is_ascii_alphanumeric() || *next == '_' || *next == '?'
                }))
        {
            bare = false;
            computed = true;
        }
        if ch == ':'
            && parameter.is_none()
            && bare
            && !escaped
            && values.is_empty()
            && let Some(name) = parameter_name(&value)
        {
            parameter = Some(name);
            attached = true;
            value.clear();
            i += 1;
            continue;
        }
        value.push(ch);
        i += 1;
    }
    if !value.is_empty() || quoted || !values.is_empty() || attached {
        values.push(value);
    }
    if parameter.is_none()
        && bare
        && !escaped
        && values.len() == 1
        && let Some(name) = parameter_name(&values[0])
    {
        return (
            Word {
                arg: CmdletArg {
                    parameter: Some(name),
                    values: Vec::new(),
                    attached: false,
                    written: Some(text(chars, start, i)),
                },
                bare,
                computed,
                escaped: false,
            },
            i,
        );
    }
    // `-Name: value` attaches the next word.
    if attached && values.len() == 1 && values[0].is_empty() && !quoted {
        return (
            Word {
                arg: CmdletArg {
                    parameter,
                    values: Vec::new(),
                    attached: false,
                    written: Some(text(chars, start, i)),
                },
                bare,
                computed,
                escaped: false,
            },
            i,
        );
    }
    let arg = if attached {
        CmdletArg {
            parameter,
            values,
            attached: true,
            written: Some(text(chars, start, i)),
        }
    } else {
        CmdletArg {
            values,
            ..Default::default()
        }
    };
    (
        Word {
            arg,
            bare,
            computed,
            escaped,
        },
        i,
    )
}

/// A command of a PowerShell line.
#[derive(Debug, Clone)]
pub struct Command {
    /// The command name, or `None` for an expression or a computed name.
    pub name: Option<String>,
    pub args: Vec<CmdletArg>,
    /// Receives pipeline input.
    pub piped: bool,
}

/// What `readPowerShell` answers.
pub struct Reading {
    pub commands: Vec<Command>,
    pub redirects: Vec<String>,
}

/// `readPowerShell`: the commands and redirect targets of a PowerShell
/// command line. Statements end at ; && || a newline or a background &; a
/// pipeline joins commands with |. Groups go to `nested`.
#[must_use]
pub fn read_power_shell(command: &str, nested: &mut Vec<String>) -> Reading {
    let chars: Vec<char> = command.chars().collect();
    let mut commands: Vec<Command> = Vec::new();
    let mut redirects: Vec<String> = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut call = false;
    let mut piped = false;
    let finish = |words: &mut Vec<Word>,
                  call: &mut bool,
                  piped: &mut bool,
                  pipe_next: bool,
                  commands: &mut Vec<Command>| {
        // `$x = command ...` runs the command on the right: a variable target
        // ($x, $x.y, ${x}, $a[0], [type]$x) and operator (=, +=, ??=), glued
        // or spaced, through a chain of up to eight assignments.
        let right_of = |text: &str, rest: &[Word]| -> Vec<Word> {
            if text.is_empty() {
                return rest.to_vec();
            }
            let computed = re::test(r"^[$[]", "", text);
            let mut out = vec![Word {
                arg: CmdletArg {
                    values: vec![text.to_owned()],
                    ..Default::default()
                },
                bare: !computed,
                computed,
                escaped: false,
            }];
            out.extend(rest.iter().cloned());
            out
        };
        let mut chain = 0;
        while chain < 8 && !*call && !words.is_empty() {
            let first = &words[0];
            if first.arg.parameter.is_some() || !first.computed {
                break;
            }
            let joined = first.arg.values.join(",");
            let glued = re::find(
                r"^(?:\[[^\]]*\])*\$[^=\s]*?(?:[-+*/%]|\?\?)?=(.*)$",
                "s",
                &joined,
            )
            .map(|found| re::group(&joined, &found, 1).unwrap_or("").to_owned());
            let second_text = words
                .get(1)
                .filter(|second| second.arg.parameter.is_none())
                .map(|second| second.arg.values.join(","));
            let spaced = second_text.as_ref().and_then(|second| {
                re::find(r"^(?:[-+*/%]|\?\?)?=(.*)$", "s", second)
                    .map(|found| re::group(second, &found, 1).unwrap_or("").to_owned())
            });
            if let Some(glued) = glued {
                *words = right_of(&glued, &words[1..]);
            } else if let Some(spaced) = spaced {
                *words = right_of(&spaced, &words[2..]);
            } else {
                break;
            }
            chain += 1;
        }
        if let Some((first, rest)) = words.split_first() {
            let named = first.arg.parameter.is_none()
                && first.arg.values.len() == 1
                && if *call {
                    !first.computed
                } else {
                    first.bare && !re::test(r"^[\d.]+$", "", &first.arg.values[0])
                };
            commands.push(Command {
                name: named.then(|| shell_executable_name(&first.arg.values[0])),
                args: if named {
                    rest.iter().map(|word| word.arg.clone()).collect()
                } else {
                    Vec::new()
                },
                piped: *piped,
            });
        }
        words.clear();
        *call = false;
        *piped = pipe_next;
    };
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if is_blank(ch) {
            i += 1;
            continue;
        }
        if ch == '`' && matches!(chars.get(i + 1), Some('\n' | '\r')) {
            i += if chars.get(i + 1) == Some(&'\r') && chars.get(i + 2) == Some(&'\n') {
                3
            } else {
                2
            };
            continue;
        }
        if ch == '\n' || ch == '\r' || ch == ';' {
            // A newline after | continues the pipeline on the next line.
            if ch == ';' || !piped || !words.is_empty() {
                finish(&mut words, &mut call, &mut piped, false, &mut commands);
            }
            i += 1;
            continue;
        }
        if ch == '|' {
            let or = chars.get(i + 1) == Some(&'|');
            finish(&mut words, &mut call, &mut piped, !or, &mut commands);
            i += if or { 2 } else { 1 };
            continue;
        }
        if ch == '&' {
            if chars.get(i + 1) == Some(&'&') {
                finish(&mut words, &mut call, &mut piped, false, &mut commands);
                i += 2;
            } else if words.is_empty() && !call {
                call = true;
                i += 1;
            } else {
                finish(&mut words, &mut call, &mut piped, false, &mut commands);
                i += 1;
            }
            continue;
        }
        if ch == '#' {
            while i < chars.len() && chars[i] != '\n' && chars[i] != '\r' {
                i += 1;
            }
            continue;
        }
        if ch == '<' && chars.get(i + 1) == Some(&'#') {
            let rest = text(&chars, i + 2, chars.len());
            i = match rest.find("#>") {
                None => chars.len(),
                Some(at) => i + 2 + rest[..at].chars().count() + 2,
            };
            continue;
        }
        if ch == '<' {
            i += 1;
            continue;
        }
        let ahead = text(&chars, i, (i + 6).min(chars.len()));
        if let Some(redirect) = re::find(r"^[1-6*]?>>?(&[1-6])?", "", &ahead) {
            let merges = redirect.group(1).is_some();
            i += ahead[redirect.range()].chars().count();
            // n>&1 merges one stream into another; it writes no file.
            if merges {
                continue;
            }
            while i < chars.len() && is_blank(chars[i]) {
                i += 1;
            }
            let (target, end) = power_shell_word(&chars, i, nested, true);
            i = end;
            if !target.arg.values.is_empty() {
                redirects.push(target.arg.values.join(","));
            }
            continue;
        }
        let word_start = i;
        let (word, end) = power_shell_word(&chars, i, nested, false);
        if end == i {
            // A character no word takes: skip it.
            i += 1;
            continue;
        }
        i = end;
        if word.bare
            && !word.escaped
            && word.arg.parameter.is_none()
            && word.arg.values.first().map(String::as_str) == Some("--%")
        {
            // The stop-parsing token hands the rest of the line over as
            // written.
            let mut stop = i;
            while stop < chars.len() && chars[stop] != '\n' && chars[stop] != '\r' {
                stop += 1;
            }
            let rest = text(&chars, i, stop);
            for piece in rest.split(blank).filter(|piece| !piece.is_empty()) {
                words.push(Word {
                    arg: CmdletArg {
                        values: vec![piece.to_owned()],
                        ..Default::default()
                    },
                    bare: true,
                    computed: false,
                    escaped: false,
                });
            }
            i = stop;
            continue;
        }
        // `a ,b` and `a , b` continue the array that a began.
        if chars[word_start] == ',' && words.len() > 1 {
            let previous = words.last_mut().expect("two words at least");
            if previous.arg.parameter.is_none() || previous.arg.attached {
                previous
                    .arg
                    .values
                    .extend(word.arg.values.iter().skip(1).cloned());
                continue;
            }
        }
        words.push(word);
    }
    finish(&mut words, &mut call, &mut piped, false, &mut commands);
    Reading {
        commands,
        redirects,
    }
}

/// PowerShell aliases of the commands the targets reader knows. Aliases
/// that share a name with a native program (cp, mv, rm, tee) keep the native
/// reading unless the arguments are PowerShell's.
const ALIASES: [(&str, &str); 8] = [
    ("ac", "add-content"),
    ("clc", "clear-content"),
    ("copy", "copy-item"),
    ("cpi", "copy-item"),
    ("ni", "new-item"),
    ("sc", "set-content"),
    ("si", "set-item"),
    ("tee", "tee-object"),
];

/// `powerShellCommandName`.
#[must_use]
pub fn power_shell_command_name(name: &str, args: &[CmdletArg]) -> String {
    if let Some((_, target)) = ALIASES.iter().find(|(alias, _)| *alias == name) {
        return (*target).to_owned();
    }
    // Copy-Item's -Destination is not cp's -t.
    if name == "cp"
        && args.iter().any(|arg| {
            arg.parameter
                .as_ref()
                .is_some_and(|parameter| parameter.chars().count() > 1)
        })
    {
        return "copy-item".to_owned();
    }
    name.to_owned()
}

/// The mutating cmdlets that take their path from the pipeline when none is
/// written (Get-ChildItem x | Remove-Item).
pub const PIPELINE_PATH_COMMANDS: [&str; 17] = [
    "remove-item",
    "ri",
    "del",
    "erase",
    "rd",
    "rm",
    "rmdir",
    "clear-item",
    "cli",
    "move-item",
    "mi",
    "move",
    "mv",
    "rename-item",
    "rni",
    "ren",
    "copy-item",
];

/// How Set-Location and Push-Location bind the directory they move to.
const LOCATION_CHANGE: CmdletWrite = CmdletWrite {
    positional: &[&["path", "literalpath"]],
    paths: &["path", "literalpath"],
    child: None,
    fileless: None,
    valued: &["stackname"],
    switches: &["passthru"],
    aliases: &[("pspath", "literalpath"), ("lp", "literalpath")],
    pipeline_path: false,
};

fn location_command(name: &str) -> Option<&'static str> {
    match name {
        "set-location" | "cd" | "chdir" | "sl" => Some("set-location"),
        "push-location" | "pushd" => Some("push-location"),
        _ => None,
    }
}

/// `powerShellLocationChanges`: the directory operands of a command line's
/// location changes, in groups too, with `\` read as a separator; `None`
/// for a bare Set-Location, read as each home. `-` and `+` (the location
/// history) add nothing.
#[must_use]
pub fn location_changes(command: &str, depth: usize) -> Vec<Option<String>> {
    if depth > 8 {
        return Vec::new();
    }
    let mut nested = Vec::new();
    let mut out: Vec<Option<String>> = Vec::new();
    for Command { name, args, .. } in read_power_shell(command, &mut nested).commands {
        let Some(location) = name.as_deref().and_then(location_command) else {
            continue;
        };
        // The binding lists a positional path twice (its slot, and every
        // positional).
        let mut targets: Vec<String> = Vec::new();
        for target in cmdlet_write_targets(&LOCATION_CHANGE, &args, true).targets {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        if targets.is_empty() && location == "set-location" {
            out.push(None);
        }
        for target in targets {
            if target != "-" && target != "+" {
                out.push(Some(target.replace('\\', "/")));
            }
        }
    }
    for inner in nested {
        out.extend(location_changes(&inner, depth + 1));
    }
    out
}

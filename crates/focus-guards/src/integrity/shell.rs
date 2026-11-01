//! The walk over a shell command: the substitutions it runs, the heredocs
//! it feeds to interpreters, the variables it sets, the scripts it runs,
//! and whether any of it reaches a runtime record, the audit trail, an
//! installed enforcement file, or a hook.
//!
//! Mirrors `protectedShell` and its helpers, `protectedInvocation`,
//! `protectedScriptFile` and `unresolvedAuditTrailWrite` in
//! `runtime-integrity.ts`.

use std::collections::BTreeMap;

use focus_js::path::{Flavour, is_absolute};
use focus_js::re;
use focus_js::text::is_js_whitespace;

use super::source::protected_content;
use super::{Integrity, MAX_EXECUTION_DEPTH, executable_name, interpreter};
use crate::shell::{
    CommandShell, DirectoryChange, shell_command_invocation_details, shell_directory_changes,
    shell_write_targets,
};

const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;
const MODULE_OPTION: &str =
    r"^(?:--(?:require|import|preload|loader|experimental-loader)(?:=(.*))?|-r(.*))$";
const DATA_OPTION: &str = r"^(?:--cwd|--config|--conditions|--env-file)$";
const SCRIPT_EXTENSION: &str = r"\.(?:ts|js|mjs|cjs|sh|py)$";
pub(super) const PROSE_EXTENSION: &str = r"\.(?:md|markdown|mdown|txt|rst|adoc|asciidoc)$";
/// `protectedShell` replaces a command substitution it cannot evaluate with
/// this placeholder, so it marks a computed word as surely as `$` does.
const UNRESOLVED_WORD: &str = r"[$`*?]|__substitution__";
const AUDIT_SEGMENT: &str = r"(?:^|[\\/])audit(?:[\\/]|$)";
const MAX_ROOTS: usize = 64;
/// Each variable keeps every value the command gives it. A write is judged
/// against all of them, because order alone cannot say which value is live
/// at that write.
const MAX_EXPANSIONS: usize = 64;
/// The workspace tree the audit trail lives in, not the framework's own
/// tool names: a framework command redirected to a variable must stay
/// allowed.
const WORKSPACE: &str = r#"(?:^|[\\/\s"'=])focus[\\/]+spaces(?:[\\/]|$)"#;
const LITERAL_ASSIGNMENT: &str = r#"(?:^|[;&|\n(]|\s)(?:export\s+|local\s+|readonly\s+|declare\s+(?:-[A-Za-z]+\s+)*)?([A-Za-z_][A-Za-z0-9_]*)(\+?)=((?:"(?:[^"\\]|\\.)*"|'[^']*'|\\.|[^\s;&|)"'\\])*)"#;

/// The variables that carry the engine's authority or turn a guard off: the
/// session and presence overrides, the direct state and audit authorities,
/// the human-turn token, and every recordable bypass. Only the person sets
/// them, outside the agent. A terminal can set one many ways, so each is
/// recognized: POSIX assignments and builtins, PowerShell's env: drive,
/// .NET calls, and hashtables, and cmd's set and setx, in any letter case
/// because Windows names ignore case.
fn harness_control_name() -> String {
    let names: Vec<&str> = [
        "FOCUS_SESSION_OVERRIDE",
        "FOCUS_SESSION_OVERRIDE_SOURCE",
        "CODEX_THREAD_ID",
        "CODEX_SESSION_ID",
        "FOCUS_UNATTENDED",
        "FOCUS_ALLOW_DIRECT_STATE_TRANSITIONS",
        "FOCUS_STATE_TRANSITION_OWNER",
        "FOCUS_ALLOW_DIRECT_AUDIT_EVENTS",
        "FOCUS_INTERNAL_HUMAN_TURN_TOKEN",
        "FOCUS_SKIP_REVIEWER_GATE_GUARD",
    ]
    .into_iter()
    .chain(
        focus_workspace::settings::RECORDABLE_BYPASSES
            .iter()
            .copied(),
    )
    .collect();
    format!("(?:{})", names.join("|"))
}

fn harness_control_assignments() -> Vec<String> {
    let name = harness_control_name();
    vec![
        // NAME=1, set NAME=1, $env:NAME = 1, ${env:NAME} += 1, ${NAME:=1}, @{ "NAME" = 1 }
        format!(r#"\b{name}\b['"]?[;\]]?\s*(?:[-+*/%:]|\?\?)?=(?!=)"#),
        // read NAME, printf -v NAME, export NAME, declare NAME, and their kin,
        // however the word is reached (${x:-read} runs read too), but not a
        // --local flag or a path segment such as AppData\Local
        format!(
            r"(?<!--)(?<![\w.~:-][\\/])\b(?:read|export|declare|typeset|local|readonly|mapfile|readarray|printf\s+-v)\b[^\n;&|]*\b{name}\b"
        ),
        // Set-Item env:NAME, New-Item -Name NAME, Rename-Item -NewName NAME
        format!(r"(?:^|[^$\w;])env:[\\/]*{name}\b"),
        format!(r#"-(?:new)?name[\s:'"]+{name}\b"#),
        // [Environment]::SetEnvironmentVariable("NAME", ...), $psi.Environment.Add("NAME", ...)
        format!(r#"(?:SetEnvironmentVariable|\.Add)[\s;('"]*{name}\b"#),
        // setx NAME 1, which every later terminal inherits
        format!(r"\bsetx(?:\.exe)?\b[^\n]*\b{name}\b"),
    ]
}

/// `interpreter(name)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Language {
    Shell,
    Python,
    JavaScript,
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// One tool call's walk: the host, and the audit-trail flag the walk sets
/// when the hit that ended it was an audit-trail target.
pub(crate) struct Walk<'a> {
    pub(super) integrity: &'a Integrity,
    pub(super) cwd: String,
    pub(super) audit_trail_matched: bool,
}

impl Walk<'_> {
    fn flavour(&self) -> Flavour {
        self.integrity.flavour()
    }

    /// `HOOK_MODULE.test(module)`.
    pub(super) fn hook_module(&self, module: &str) -> bool {
        re::test(super::paths::HOOK_MODULE, "", module)
    }

    fn write_targets(&self, command: &str) -> Vec<String> {
        shell_write_targets(
            &self.integrity.host,
            command,
            &self.cwd,
            None,
            CommandShell::Posix,
        )
    }

    /// `protectedWriteTarget`.
    pub(super) fn protected_write_target(&mut self, path: &str) -> bool {
        let cwd = self.cwd.clone();
        if self.integrity.protected_runtime_path(path, &cwd)
            || self.integrity.protected_installed_path(path, &cwd, false)
        {
            return true;
        }
        if self.integrity.protected_audit_trail_path(path, &cwd) {
            self.audit_trail_matched = true;
            return true;
        }
        false
    }

    fn protected_directory_target(&mut self, path: &str) -> bool {
        let cwd = self.cwd.clone();
        if self.integrity.protected_runtime_path(path, &cwd) {
            return true;
        }
        if self.integrity.protected_audit_trail_path(path, &cwd) {
            self.audit_trail_matched = true;
            return true;
        }
        false
    }

    /// `protectedInvocation`: a program and its arguments that run a hook,
    /// the engine's hook routes, or an interpreter on protected code.
    pub(super) fn protected_invocation(
        &mut self,
        executable: &str,
        args: &[Option<String>],
        depth: usize,
    ) -> bool {
        if depth > MAX_EXECUTION_DEPTH {
            return false;
        }
        if re::test(super::paths::HOOK_FILE, "", executable) {
            return true;
        }
        let name = executable_name(executable);
        if re::test(r"^focus(?:\.ts)?$", "", &name) {
            // Host hooks reach these routes without a tool call; `engine
            // adapter` is the per-harness hook entry, so a model shell running
            // it would mint the human turn the host hook records. Global flags
            // may precede the route. A route word this parser cannot resolve
            // ($A, $(...), a glob, a spawn element that is not a string
            // literal) could expand to either, so it fails closed; routine
            // commands spell their route out.
            let route: Vec<&Option<String>> = args
                .iter()
                .enumerate()
                .filter(|(index, arg)| {
                    !re::test(
                        r"^--(?:json|quiet|no-color|yes|offline|verbose|project-dir)$",
                        "",
                        arg.as_deref().unwrap_or(""),
                    ) && !(*index > 0 && args[index - 1].as_deref() == Some("--project-dir"))
                })
                .map(|(_, arg)| arg)
                .collect();
            let computed = |index: usize| -> bool {
                match route.get(index) {
                    None => false,
                    Some(None) => true,
                    Some(Some(word)) => {
                        !re::test(r"^[A-Za-z0-9._-]+$", "", word)
                            || re::test(UNRESOLVED_WORD, "", word)
                    }
                }
            };
            let word = |index: usize| route.get(index).and_then(|arg| arg.as_deref());
            return computed(0)
                || (word(0) == Some("engine")
                    && (word(1) == Some("hook") || word(1) == Some("adapter") || computed(1)))
                || args
                    .iter()
                    .any(|arg| arg.as_deref() == Some("--internal-focus-record-human-turn"));
        }
        let Some(kind) = interpreter(&name) else {
            return false;
        };
        let mut inline_mode = false;
        let mut check_mode = false;
        let mut index = 0;
        while index < args.len() {
            let Some(arg) = args[index].as_deref() else {
                return false;
            };
            let inline = if kind == Language::JavaScript {
                re::find(
                    r"^(?:--(?:eval|print)(?:=(.*))?|-i*(?:pe|ep|e|p)(.*))$",
                    "",
                    arg,
                )
            } else {
                re::find(r"^(?:-c(.*)|-[a-z]*c)$", "", arg)
            };
            if let Some(inline) = inline {
                let group = |idx: usize| re::group(arg, &inline, idx).map(str::to_owned);
                let source: Option<String> = if arg.starts_with("--") && arg.contains('=') {
                    Some(group(1).unwrap_or_default())
                } else {
                    match group(1)
                        .filter(|value| !value.is_empty())
                        .or_else(|| group(2).filter(|value| !value.is_empty()))
                    {
                        Some(value) => Some(value),
                        None => {
                            index += 1;
                            args.get(index).cloned().flatten()
                        }
                    }
                };
                let Some(source) = source else {
                    return false;
                };
                let protected = re::test(super::paths::RUNTIME_RECORD_MENTION, "i", &source)
                    || if kind == Language::Shell {
                        self.protected_shell(&source, depth + 1, false)
                    } else {
                        protected_content(self, &source, depth + 1, kind == Language::Python, None)
                    };
                if protected {
                    return true;
                }
                inline_mode = true;
                index += 1;
                continue;
            }
            if kind == Language::JavaScript
                && name != "deno"
                && re::test(r"^(?:--check|-c)$", "", arg)
            {
                check_mode = true;
                index += 1;
                continue;
            }
            if let Some(option) = re::find(MODULE_OPTION, "", arg) {
                let module = match re::group(arg, &option, 1) {
                    Some(attached) => Some(attached.to_owned()),
                    None => match re::group(arg, &option, 2).filter(|value| !value.is_empty()) {
                        Some(short) => Some(short.to_owned()),
                        None => {
                            index += 1;
                            args.get(index).cloned().flatten()
                        }
                    },
                };
                if let Some(module) = module.filter(|module| !module.is_empty())
                    && (self.hook_module(&module)
                        || self.protected_script_file(
                            &module,
                            depth + 1,
                            Some(Language::JavaScript),
                        ))
                {
                    return true;
                }
                index += 1;
                continue;
            }
            if re::test(DATA_OPTION, "", arg) {
                index += 2;
                continue;
            }
            if arg == "run" && matches!(name.as_str(), "bun" | "deno" | "tsx") {
                index += 1;
                continue;
            }
            if arg == "--" {
                if inline_mode || check_mode {
                    return false;
                }
                let Some(Some(file)) = args.get(index + 1).cloned() else {
                    return false;
                };
                let rest: Vec<Option<String>> = args[(index + 2).min(args.len())..].to_vec();
                return self.protected_invocation(&file, &rest, depth + 1)
                    || self.protected_script_file(&file, depth + 1, Some(kind));
            }
            if arg.starts_with('-') {
                index += 1;
                continue;
            }
            if inline_mode || check_mode {
                return false;
            }
            let rest: Vec<Option<String>> = args[index + 1..].to_vec();
            let file = arg.to_owned();
            return self.protected_invocation(&file, &rest, depth + 1)
                || self.protected_script_file(&file, depth + 1, Some(kind));
        }
        false
    }

    /// `protectedScriptFile`: a script on disk, read and walked as its
    /// language; an unreadable or missing script is outside this lexical
    /// check's reach.
    pub(super) fn protected_script_file(
        &mut self,
        path: &str,
        depth: usize,
        language: Option<Language>,
    ) -> bool {
        if depth > MAX_EXECUTION_DEPTH {
            return false;
        }
        let absolute = self.integrity.resolve(&[&self.cwd, path]);
        let Ok(meta) = std::fs::metadata(&absolute) else {
            return false;
        };
        if !meta.is_file() || meta.len() > MAX_SCRIPT_BYTES {
            return false;
        }
        if self
            .integrity
            .trusted_installed_script(&absolute, &self.cwd)
        {
            return false;
        }
        let Ok(bytes) = std::fs::read(&absolute) else {
            return false;
        };
        let source = String::from_utf8_lossy(&bytes).into_owned();
        let shell = match language {
            Some(language) => language == Language::Shell,
            None => {
                path.ends_with(".sh")
                    || (!re::test(SCRIPT_EXTENSION, "", path)
                        && !re::test(r"^#![^\n]*\b(?:bun|node|python[\d.]*)\b", "", &source))
            }
        };
        if shell {
            self.protected_shell(&source, depth + 1, false)
        } else {
            let python = match language {
                Some(language) => language == Language::Python,
                None => path.ends_with(".py"),
            };
            protected_content(self, &source, depth + 1, python, None)
        }
    }

    /// `literalShellOutput`: what `echo`/`printf %s` prints, when that is
    /// all a substitution does.
    fn literal_shell_output(command: &str) -> Option<String> {
        let calls = shell_command_invocation_details(command);
        if calls.len() != 1 {
            return None;
        }
        let call = &calls[0];
        if call.name == "echo" && !call.args.first().is_some_and(|arg| arg.starts_with('-')) {
            return Some(call.args.join(" "));
        }
        if call.name == "printf" && call.args.first().map(String::as_str) == Some("%s") {
            return Some(call.args[1..].concat());
        }
        None
    }

    /// `interpreterReadsCode`: whether an interpreter invocation reads its
    /// program from stdin.
    fn interpreter_reads_code(name: &str, args: &[String]) -> bool {
        let Some(kind) = interpreter(name) else {
            return false;
        };
        let mut index = 0;
        while index < args.len() {
            let arg = args[index].as_str();
            if re::test(r"^(?:--help|--version|-h|-v|-V)$", "", arg) {
                return false;
            }
            let inline = if kind == Language::JavaScript {
                re::test(r"^(?:--(?:eval|print|check)(?:=|$)|-i*(?:e|p|c))", "", arg)
            } else {
                re::test(r"^(?:-c|-[a-z]*c$)", "", arg)
            };
            if inline {
                return false;
            }
            if let Some(option) = re::find(MODULE_OPTION, "", arg) {
                if re::group(arg, &option, 1).is_none()
                    && re::group(arg, &option, 2).is_none_or(str::is_empty)
                {
                    index += 1;
                }
                index += 1;
                continue;
            }
            if re::test(DATA_OPTION, "", arg) {
                index += 2;
                continue;
            }
            if kind == Language::Shell && re::test(r"^-[a-z]*s$", "", arg) {
                return true;
            }
            if arg == "-" || re::test(r"^/(?:dev/(?:stdin|fd/0)|proc/self/fd/0)$", "", arg) {
                return true;
            }
            if arg == "--" {
                return args.get(index + 1).is_none_or(|next| next == "-");
            }
            if arg == "run" && matches!(name, "bun" | "deno" | "tsx") {
                index += 1;
                continue;
            }
            if !arg.starts_with('-') {
                return false;
            }
            index += 1;
        }
        true
    }

    /// `heredocPipeline`: the pipeline around a heredoc's command, with the
    /// redirections blanked for the execution reading.
    fn heredoc_pipeline(header: &[char], here_position: usize) -> (String, String) {
        let mut quote: Option<char> = None;
        let mut start = 0;
        let mut end = header.len();
        let mut redirects: Vec<(usize, usize)> = Vec::new();
        let mut index = 0;
        while index < header.len() {
            let ch = header[index];
            if ch == '\\' && quote != Some('\'') {
                index += 2;
                continue;
            }
            if let Some(open) = quote {
                if ch == open {
                    quote = None;
                }
                index += 1;
                continue;
            }
            if ch == '\'' || ch == '"' {
                quote = Some(ch);
                index += 1;
                continue;
            }
            let ahead: String = header[index..].iter().collect();
            let redirect = re::find(r"^(?:\d*)?(?:>>?|<|&>)(?:&)?[ \t]*", "", &ahead);
            let before_ok = index == 0
                || header
                    .get(index - 1)
                    .is_some_and(|before| is_js_whitespace(*before) || ";|&".contains(*before));
            if let Some(redirect) = redirect
                && (ch == '<' || ch == '>' || ch == '&' || before_ok)
            {
                let begin = index;
                index += ahead[redirect.range()].chars().count();
                let mut target_quote: Option<char> = None;
                while index < header.len() {
                    let current = header[index];
                    if current == '\\' && target_quote != Some('\'') {
                        index += 2;
                        continue;
                    }
                    if let Some(open) = target_quote {
                        if current == open {
                            target_quote = None;
                        }
                    } else if current == '\'' || current == '"' {
                        target_quote = Some(current);
                    } else if is_js_whitespace(current) || ";|&<>".contains(current) {
                        break;
                    }
                    index += 1;
                }
                redirects.push((begin, index));
                continue;
            }
            if !";|&\n".contains(ch) {
                index += 1;
                continue;
            }
            let pair: String = header[index..(index + 2).min(header.len())]
                .iter()
                .collect();
            let width = if ["&&", "||", "|&"].contains(&pair.as_str()) {
                2
            } else {
                1
            };
            if index < here_position {
                start = index + width;
            } else if ch != '|' || pair == "||" {
                end = index;
                break;
            }
            index += width;
        }
        let mut execution: Vec<char> = header.to_vec();
        let length = execution.len();
        for (begin, after) in redirects {
            for slot in execution.iter_mut().take(after.min(length)).skip(begin) {
                *slot = ' ';
            }
        }
        let (start, end) = (start.min(header.len()), end.min(header.len()));
        if start > end {
            return (String::new(), String::new());
        }
        (
            header[start..end].iter().collect(),
            execution[start..end].iter().collect(),
        )
    }

    /// `protectedHeredoc`: a heredoc body fed to an interpreter on stdin, or
    /// written to a script, read as that language.
    fn protected_heredoc(
        &mut self,
        body: &str,
        header: &[char],
        here_position: usize,
        stdin: bool,
        depth: usize,
    ) -> bool {
        let (raw, execution) = Self::heredoc_pipeline(header, here_position);
        let mut kinds: Vec<Language> = Vec::new();
        let mut add = |kind: Language| {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        };
        if stdin {
            for call in shell_command_invocation_details(&execution) {
                if let Some(kind) = interpreter(&call.name)
                    && Self::interpreter_reads_code(&call.name, &call.args)
                {
                    add(kind);
                }
            }
        }
        for target in self.write_targets(&raw) {
            if re::test(PROSE_EXTENSION, "i", &target) {
                continue;
            }
            let mut script = re::test(SCRIPT_EXTENSION, "", &target);
            if let Ok(meta) = std::fs::metadata(self.integrity.resolve(&[&self.cwd, &target])) {
                script = script || (meta.is_file() && executable_bits(&meta));
            }
            if script {
                add(
                    if target.ends_with(".sh")
                        || re::test(r"^#![^\n]*\b(?:sh|bash|zsh)\b", "", body)
                    {
                        Language::Shell
                    } else if target.ends_with(".py") {
                        Language::Python
                    } else {
                        Language::JavaScript
                    },
                );
            }
        }
        kinds.into_iter().any(|kind| match kind {
            Language::Shell => self.protected_shell(body, depth + 1, false),
            other => protected_content(self, body, depth + 1, other == Language::Python, None),
        })
    }

    /// `protectedShell`. Shell quoting has different semantics from source
    /// strings: substitutions execute inside double quotes, and interpreter
    /// -c/-e arguments become code. Comments and heredoc bodies are stripped
    /// before the shared invocation parser is asked about executable
    /// positions; arbitrary argv text is not searched for hook names.
    pub(super) fn protected_shell(
        &mut self,
        command: &str,
        depth: usize,
        expansions_only: bool,
    ) -> bool {
        if depth > MAX_EXECUTION_DEPTH {
            return false;
        }
        let chars: Vec<char> = command.chars().collect();
        let mut quote: Option<char> = None;
        let mut visible = String::new();
        // Open `${` parameter expansions: their braces are part of a word,
        // not a group.
        let mut parameter = 0usize;
        let mut index = 0;
        while index < chars.len() {
            let ch = chars[index];
            if ch == '\\' && quote != Some('\'') {
                visible.extend(&chars[index..(index + 2).min(chars.len())]);
                index += 2;
                continue;
            }
            if quote == Some('\'') {
                visible.push(ch);
                if ch == '\'' {
                    quote = None;
                }
                index += 1;
                continue;
            }
            if !expansions_only && ch == '\'' && quote.is_none() {
                quote = Some(ch);
                visible.push(ch);
                index += 1;
                continue;
            }
            if !expansions_only && ch == '"' {
                quote = if quote.is_some() { None } else { Some(ch) };
                visible.push(ch);
                index += 1;
                continue;
            }
            if quote.is_none()
                && !expansions_only
                && ch == '#'
                && (index == 0
                    || chars
                        .get(index - 1)
                        .is_some_and(|before| is_js_whitespace(*before) || ";|&".contains(*before)))
            {
                match chars[index..].iter().position(|c| *c == '\n') {
                    Some(at) => index += at,
                    None => index = chars.len(),
                }
                continue;
            }
            if ch == '`' || (ch == '$' && chars.get(index + 1) == Some(&'(')) {
                let start = index + if ch == '`' { 1 } else { 2 };
                let end = if ch == '`' {
                    let mut end = start;
                    while end < chars.len() && chars[end] != '`' {
                        if chars[end] == '\\' {
                            end += 1;
                        }
                        end += 1;
                    }
                    end
                } else {
                    shell_substitution_end(&chars, index + 1)
                };
                let body: String = chars
                    [start.min(chars.len())..end.min(chars.len()).max(start.min(chars.len()))]
                    .iter()
                    .collect();
                if self.protected_shell(&body, depth + 1, false) {
                    return true;
                }
                let output = Self::literal_shell_output(&body)
                    .unwrap_or_else(|| "__substitution__".to_owned());
                if quote == Some('"') {
                    // `output.replace(/["\\]/g, "\\$&")`.
                    for c in output.chars() {
                        if c == '"' || c == '\\' {
                            visible.push('\\');
                        }
                        visible.push(c);
                    }
                } else {
                    let quoted: Vec<String> =
                        output.split(is_js_whitespace).map(shell_quote).collect();
                    visible.push_str(&quoted.join(" "));
                }
                // Past the closing backtick or parenthesis.
                index = end + 1;
                continue;
            }
            if quote.is_none()
                && !expansions_only
                && chars.get(index) == Some(&'<')
                && chars.get(index + 1) == Some(&'<')
                && chars.get(index + 2) != Some(&'<')
            {
                let ahead: String = chars[index..].iter().collect();
                if let Some(here) = re::find(
                    r#"^<<(-)?[ \t]*(?:'([^']+)'|"([^"]+)"|([A-Za-z_]\w*))"#,
                    "",
                    &ahead,
                ) {
                    let strip_tabs = re::group(&ahead, &here, 1).is_some();
                    let delimiter = re::group(&ahead, &here, 2)
                        .or_else(|| re::group(&ahead, &here, 3))
                        .or_else(|| re::group(&ahead, &here, 4))
                        .unwrap_or("")
                        .to_owned();
                    let bare_delimiter = re::group(&ahead, &here, 4).is_some();
                    let here_len = ahead[here.range()].chars().count();
                    let after_here = index + here_len;
                    if let Some(newline) = chars[after_here.min(chars.len())..]
                        .iter()
                        .position(|c| *c == '\n')
                        .map(|at| after_here + at)
                    {
                        let mut end = newline + 1;
                        let body_start = end;
                        while end < chars.len() {
                            let next_line = chars[end..]
                                .iter()
                                .position(|c| *c == '\n')
                                .map(|at| end + at);
                            let line_end = next_line.unwrap_or(chars.len());
                            let line: String = chars[end..line_end].iter().collect();
                            let line = line.strip_suffix('\r').unwrap_or(&line).to_owned();
                            let compared = if strip_tabs {
                                line.trim_start_matches('\t').to_owned()
                            } else {
                                line
                            };
                            if compared == delimiter {
                                break;
                            }
                            end = next_line.map_or(chars.len(), |at| at + 1);
                        }
                        let body: String = chars[body_start.min(end)..end].iter().collect();
                        let descriptor = re::find(r"(?:^|[ \t])(\d+)$", "", &visible)
                            .and_then(|found| re::group(&visible, &found, 1).map(str::to_owned));
                        let prefix: String = match &descriptor {
                            Some(digits) => visible[..visible.len() - digits.len()].to_owned(),
                            None => visible.clone(),
                        };
                        let header_tail: String =
                            chars[after_here.min(newline)..newline].iter().collect();
                        let header = format!("{prefix}{header_tail}");
                        let header_chars: Vec<char> = header.chars().collect();
                        let stdin = descriptor.as_deref().is_none_or(|digits| digits == "0");
                        if self.protected_heredoc(
                            &body,
                            &header_chars,
                            prefix.chars().count(),
                            stdin,
                            depth,
                        ) || self.protected_shell(&header, depth + 1, false)
                            || (bare_delimiter && self.protected_shell(&body, depth + 1, true))
                        {
                            return true;
                        }
                        let after = chars[end.min(chars.len())..]
                            .iter()
                            .position(|c| *c == '\n')
                            .map(|at| end + at);
                        visible = format!("{header}\n");
                        index = after.unwrap_or(chars.len());
                        continue;
                    }
                }
            }
            // `${ cmd; }` and `${| cmd; }` (bash 5.3) run a command, as `$(...)`
            // does, so their braces stay separators.
            if quote.is_none()
                && ch == '{'
                && visible.ends_with('$')
                && !chars
                    .get(index + 1)
                    .is_some_and(|next| is_js_whitespace(*next) || *next == '|')
            {
                parameter += 1;
                visible.push(ch);
                index += 1;
                continue;
            }
            if quote.is_none() && ch == '}' && parameter > 0 {
                parameter -= 1;
                visible.push(ch);
                index += 1;
                continue;
            }
            visible.push(
                if quote.is_none() && !expansions_only && "(){}".contains(ch) {
                    ';'
                } else {
                    ch
                },
            );
            index += 1;
        }
        if expansions_only {
            return false;
        }
        if harness_control_assignments()
            .iter()
            .any(|pattern| re::test(pattern, "i", &visible))
        {
            return true;
        }
        for path in self.write_targets(&visible) {
            if self.protected_write_target(&path) {
                return true;
            }
        }
        if self.unresolved_audit_trail_write(&visible, command) {
            self.audit_trail_matched = true;
            return true;
        }
        for invocation in shell_command_invocation_details(&visible) {
            let name = invocation.name.as_str();
            let args = &invocation.args;
            if name == "mkdir"
                && args
                    .iter()
                    .any(|path| self.protected_directory_target(path))
            {
                return true;
            }
            if ["rm", "mv", "rmdir"].contains(&name) {
                let quoted: Vec<String> = std::iter::once(name.to_owned())
                    .chain(args.iter().cloned())
                    .map(|word| shell_quote(&word))
                    .collect();
                let cwd = self.cwd.clone();
                if self
                    .write_targets(&quoted.join(" "))
                    .iter()
                    .any(|path| self.integrity.protected_installed_path(path, &cwd, true))
                {
                    return true;
                }
            }
            if name == "alias"
                && args.iter().any(|arg| {
                    arg.find('=').is_some_and(|equals| {
                        self.protected_shell(&arg[equals + 1..], depth + 1, false)
                    })
                })
            {
                return true;
            }
            if name == "eval" && self.protected_shell(&args.join(" "), depth + 1, false) {
                return true;
            }
            let as_options: Vec<Option<String>> = args.iter().cloned().map(Some).collect();
            if let Some(executable) = &invocation.executable {
                if self.protected_invocation(executable, &as_options, depth + 1) {
                    return true;
                }
                if (executable.starts_with("./") || re::test(SCRIPT_EXTENSION, "", executable))
                    && self.protected_script_file(executable, depth + 1, None)
                {
                    return true;
                }
            }
        }
        false
    }

    /// `unresolvedAuditTrailWrite`: the shared parser resolves a relative
    /// write against the hook's cwd and drops a word it cannot resolve. For
    /// the audit trail that gap is too wide, so this pass re-reads each write
    /// word: literal assignments made earlier in the command are expanded, a
    /// relative word is resolved against every directory a literal cd or
    /// pushd could leave the shell in, and each result is checked against
    /// the anchored audit-trail path. A word that still cannot be decided is
    /// refused if the command could be aiming at the audit trail at all. A
    /// backslash `\audit\` spelling is refused outright.
    fn unresolved_audit_trail_write(&mut self, visible: &str, command: &str) -> bool {
        // The walk's `visible` text turns unquoted braces into separators,
        // which mangles `${NAME}`, so words, assignments and directory
        // changes are also read from the command as written.
        let mut words: Vec<String> = Vec::new();
        let _ = shell_write_targets(
            &self.integrity.host,
            visible,
            &self.cwd,
            Some(&mut words),
            CommandShell::Posix,
        );
        let _ = shell_write_targets(
            &self.integrity.host,
            command,
            &self.cwd,
            Some(&mut words),
            CommandShell::Posix,
        );
        if words.is_empty() {
            return false;
        }
        if re::test(r"\\audit\\", "i", command) {
            return true;
        }
        let values = self.literal_assignments(command);
        // Every directory the shell could be in when a write runs.
        let mut roots: Vec<String> = vec![self.cwd.clone()];
        let mut computed_root = false;
        // The shared reading of each directory change; $HOME and a directory
        // the command cannot see both stay undecided here.
        let moves: Vec<DirectoryChange> = shell_directory_changes(visible)
            .into_iter()
            .chain(shell_directory_changes(command))
            .collect();
        for change in moves {
            let expansions: Vec<Option<String>> = match &change {
                DirectoryChange::Stay => continue,
                DirectoryChange::Operand(operand) => expand_word(operand, &values),
                DirectoryChange::Home | DirectoryChange::Unknown => vec![None],
            };
            for expanded in expansions {
                match expanded {
                    Some(expanded) if roots.len() <= MAX_ROOTS => {
                        let more: Vec<String> = roots
                            .iter()
                            .map(|root| self.integrity.resolve(&[root, &expanded]))
                            .collect();
                        roots.extend(more);
                    }
                    _ => computed_root = true,
                }
            }
        }
        // Quotes and escapes can split the segment (`aud""it`, `au\\dit`),
        // and a command substitution can assemble it (`$(printf au)dit`),
        // which no text test sees. An undecided word therefore also fails
        // closed whenever the command reaches into the workspace or already
        // runs inside it.
        let plain = command.replace(['"', '\'', '\\'], "");
        let names_audit = re::test("audit", "i", &plain)
            || re::test(WORKSPACE, "i", &plain)
            || re::test(WORKSPACE, "i", &self.cwd);
        let cwd = self.cwd.clone();
        words.iter().any(|word| {
            expand_word(word, &values)
                .into_iter()
                .any(|expanded| match expanded {
                    None => re::test(AUDIT_SEGMENT, "i", word) || names_audit,
                    Some(expanded) => {
                        if roots.iter().any(|root| {
                            self.integrity.protected_audit_trail_path(
                                &self.integrity.resolve(&[root, &expanded]),
                                &cwd,
                            )
                        }) {
                            return true;
                        }
                        computed_root
                            && !is_absolute(self.flavour(), &expanded)
                            && re::test(AUDIT_SEGMENT, "i", &expanded)
                    }
                })
        })
    }

    /// `literalAssignments`: every literal value each `NAME=value` gives
    /// NAME; `None` marks a computed value.
    fn literal_assignments(&self, command: &str) -> BTreeMap<String, Vec<Option<String>>> {
        let mut values: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
        values.insert("PWD".to_owned(), vec![Some(self.cwd.clone())]);
        for found in re::compile(LITERAL_ASSIGNMENT, "").find_iter(command) {
            let name = re::group(command, &found, 1).unwrap_or("").to_owned();
            let appends = re::group(command, &found, 2) == Some("+");
            let raw = re::group(command, &found, 3).unwrap_or("");
            let mut assigned: Vec<Option<String>> = match dequote_shell_word(raw) {
                None => vec![None],
                Some(dequoted) => expand_word(&dequoted, &values),
            };
            // `NAME+=value` appends to whatever NAME already holds.
            if appends {
                let before = values
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| vec![Some(String::new())]);
                assigned = before
                    .iter()
                    .flat_map(|prefix| {
                        assigned.iter().map(move |suffix| match (prefix, suffix) {
                            (Some(prefix), Some(suffix)) => Some(format!("{prefix}{suffix}")),
                            _ => None,
                        })
                    })
                    .collect();
                if assigned.len() > MAX_EXPANSIONS {
                    assigned = vec![None];
                }
            }
            values.entry(name).or_default().extend(assigned);
        }
        values
    }
}

/// `shellSubstitutionEnd`: the index of the `)` that closes the `$(` whose
/// `(` is at `open`.
fn shell_substitution_end(chars: &[char], open: usize) -> usize {
    let mut nesting = 1;
    let mut quote: Option<char> = None;
    let mut index = open + 1;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\\' && quote != Some('\'') {
            index += 2;
            continue;
        }
        if let Some(open_quote) = quote {
            if ch == open_quote {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' || ch == '`' {
            quote = Some(ch);
        } else if ch == '(' {
            nesting += 1;
        } else if ch == ')' {
            nesting -= 1;
            if nesting == 0 {
                return index;
            }
        }
        index += 1;
    }
    chars.len()
}

/// `dequoteShellWord`: the value the shell builds from a word's quoting.
/// `None` when the quoting is unbalanced, or when a single-quoted part
/// holds a `$` that expansion here could not tell from a real reference.
fn dequote_shell_word(raw: &str) -> Option<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        if ch == '\'' {
            let end = chars[index + 1..].iter().position(|c| *c == '\'')? + index + 1;
            let part: String = chars[index + 1..end].iter().collect();
            if part.contains('$') {
                return None;
            }
            out.push_str(&part);
            index = end + 1;
        } else if ch == '"' {
            index += 1;
            while index < chars.len() && chars[index] != '"' {
                if chars[index] == '\\'
                    && chars
                        .get(index + 1)
                        .is_some_and(|next| "$`\"\\\n".contains(*next))
                {
                    out.push(chars[index + 1]);
                    index += 2;
                } else {
                    out.push(chars[index]);
                    index += 1;
                }
            }
            if index >= chars.len() {
                return None;
            }
            index += 1;
        } else if ch == '\\' {
            if let Some(next) = chars.get(index + 1) {
                out.push(*next);
            }
            index += 2;
        } else {
            out.push(ch);
            index += 1;
        }
    }
    Some(out)
}

/// `expandWord`: every expansion of `$NAME` and `${NAME}` in `word` over
/// the values the command assigns; a `None` entry marks one the command
/// cannot decide.
fn expand_word(word: &str, values: &BTreeMap<String, Vec<Option<String>>>) -> Vec<Option<String>> {
    let mut results: Vec<Option<String>> = vec![Some(String::new())];
    let mut last = 0;
    for found in re::compile(
        r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)",
        "",
    )
    .find_iter(word)
    {
        let literal = &word[last..found.start()];
        let name = re::group(word, &found, 1)
            .or_else(|| re::group(word, &found, 2))
            .unwrap_or("");
        let unknown = vec![None];
        let options = values.get(name).unwrap_or(&unknown);
        results = results
            .iter()
            .flat_map(|prefix| {
                options.iter().map(move |option| match (prefix, option) {
                    (Some(prefix), Some(option)) => Some(format!("{prefix}{literal}{option}")),
                    _ => None,
                })
            })
            .collect();
        if results.len() > MAX_EXPANSIONS {
            return vec![None];
        }
        last = found.end();
    }
    let tail = &word[last..];
    results
        .into_iter()
        .map(|prefix| {
            let expanded = format!("{}{tail}", prefix?);
            (!re::test(UNRESOLVED_WORD, "", &expanded)).then_some(expanded)
        })
        .collect()
}

#[cfg(unix)]
fn executable_bits(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable_bits(_meta: &std::fs::Metadata) -> bool {
    // Node reports no execute bits on Windows.
    false
}

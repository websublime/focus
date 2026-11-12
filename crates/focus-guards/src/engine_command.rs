//! The engine's own invocations as a hook reads them off a shell command:
//! the one spelling the detectors classify (`canonicalEngineCommand`), the
//! cheap gate of the runtime-graph recompile
//! (`classifyRuntimeCompileCommand`), and the literal one-command grammar
//! that proves a single engine invocation ran (`parseLiteralShellInvocation`,
//! `engineInvocationFromWords`, `literalOrchestrateVerb`).
//!
//! Mirrors the "engine command detectors" seam of `aidlc-lib.ts`. These are
//! lexical: an English mention of a command matches too, and that direction
//! fails closed (over-detection nudges, never releases).

use focus_js::re;

use crate::shell::words::shell_command_segments;

pub use focus_workspace::names::KNOWN_HARNESS_DIRS;

const HARNESS_PATTERN: &str = r"\.claude|\.kiro|\.codex|\.focus|\.cursor";
const DISPATCHER_PATH: &str =
    r"(?:\.claude|\.kiro|\.codex|\.focus|\.cursor)[/\\]tools[/\\]focus\.ts";

/// `sourceEngineDispatcher`: the Bun dispatcher's executable token, quoted
/// project roots included, followed by `engine`.
const SOURCE_DISPATCHER: &str = r#"\bbun[ \t]+(?:"(?:[^"\r\n]*[/\\])?(?:\.claude|\.kiro|\.codex|\.focus|\.cursor)[/\\]tools[/\\]focus\.ts"|'(?:[^'\r\n]*[/\\])?(?:\.claude|\.kiro|\.codex|\.focus|\.cursor)[/\\]tools[/\\]focus\.ts'|(?:[^\s"';&|<>]*[/\\])?(?:\.claude|\.kiro|\.codex|\.focus|\.cursor)[/\\]tools[/\\]focus\.ts)[ \t]+(?=engine\b)"#;

/// `canonicalEngineCommand`: the Bun and native spellings of the engine
/// tools these detectors own, as one.
#[must_use]
pub fn canonical_engine_command(text: &str) -> String {
    let text = re::replace(SOURCE_DISPATCHER, "g", text, "focus ");
    let text = re::replace(
        r"\bfocus\s+engine\s+orchestrate\s+help\b",
        "g",
        &text,
        "focus help",
    );
    re::replace(
        r"\bfocus\s+engine\s+(orchestrate|state|jump|bolt|swarm|scope|config|status|recompose)\b",
        "g",
        &text,
        "focus $1",
    )
}

/// What the recompile hook makes of a shell command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileCommand {
    /// The runtime tool itself: never recompile on its own calls.
    Reject,
    /// A transition-class tool: read the audit tail.
    Fire,
    /// Anything else.
    Pass,
}

impl CompileCommand {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Reject => "reject",
            Self::Fire => "fire",
            Self::Pass => "pass",
        }
    }
}

/// `classifyRuntimeCompileCommand`.
#[must_use]
pub fn classify_runtime_compile_command(command: &str) -> CompileCommand {
    let canonical = canonical_engine_command(command);
    let invokes_runtime = shell_command_segments(command).iter().any(|segment| {
        re::test(
            r"^\s*focus\s+engine\s+runtime\s+compile\b",
            "",
            &canonical_engine_command(segment),
        )
    });
    let self_tool = format!(r"\bbun\b.*(?:{HARNESS_PATTERN})/tools/focus-runtime\.ts\b");
    if re::test(&self_tool, "", command) || invokes_runtime {
        return CompileCommand::Reject;
    }
    let tool =
        format!(r"\bbun\b.*(?:{HARNESS_PATTERN})/tools/focus-(state|jump|bolt|unit|utility)\.ts\b");
    let report =
        format!(r"\bbun\b.*(?:{HARNESS_PATTERN})/tools/focus-orchestrate\.ts\b.*\breport\b");
    if re::test(&tool, "", &canonical)
        || re::test(&report, "", &canonical)
        || re::test(
            r"\bfocus\s+(?:state|jump|bolt|unit|recompose)\b|\bfocus\s+(?:status|doctor|version|help)\b|\bfocus\s+scope\s+change\b|\bfocus\s+config\s+set\b",
            "",
            &canonical,
        )
        || re::test(
            r"\bfocus\s+report\b|\bfocus\s+orchestrate\s+report\b|\bfocus\s+next\b.*\breport\b",
            "",
            &canonical,
        )
    {
        return CompileCommand::Fire;
    }
    CompileCommand::Pass
}

/// One literal shell invocation: a command, optionally after
/// `cd [--] <absolute directory> &&`, with an optional trailing `2>&1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiteralInvocation {
    pub directory: Option<String>,
    /// The command's text from its first word.
    pub command: String,
    /// The words with their quotes removed.
    pub argv: Vec<String>,
    /// The words as written.
    pub raw_words: Vec<String>,
}

struct Token {
    value: String,
    raw: String,
    start: usize,
    and: bool,
}

/// `parseLiteralShellInvocation`: a deliberately small literal shell
/// grammar. No execution, expansion, relative CDPATH lookup, or other shell
/// operators; ASCII blanks delimit words; backslash-newline is refused.
#[must_use]
pub fn parse_literal_shell_invocation(text: &str) -> Option<LiteralInvocation> {
    if re::test(r"[\0\r\n]|\$\(|`", "", text) {
        return None;
    }
    let text = re::replace(r"^[ \t]+|[ \t]+$", "g", text, "");
    let text = re::replace(r"[ \t]+2>&1$", "", &text, "");
    let chars: Vec<char> = text.chars().collect();
    // Byte offsets of each char, for slicing the text as written.
    let offsets: Vec<usize> = text
        .char_indices()
        .map(|(at, _)| at)
        .chain([text.len()])
        .collect();
    let mut tokens: Vec<Token> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == ' ' || chars[i] == '\t' {
            i += 1;
            continue;
        }
        if chars[i] == '&' && chars.get(i + 1) == Some(&'&') {
            tokens.push(Token {
                value: "&&".to_owned(),
                raw: "&&".to_owned(),
                start: offsets[i],
                and: true,
            });
            i += 2;
            continue;
        }
        let start = i;
        let mut value = String::new();
        let mut quote: Option<char> = None;
        while i < chars.len() {
            let ch = chars[i];
            match quote {
                Some('\'') => {
                    if ch == '\'' {
                        quote = None;
                    } else {
                        value.push(ch);
                    }
                    i += 1;
                }
                Some(_) => {
                    if ch == '"' {
                        quote = None;
                        i += 1;
                    } else if ch == '$' {
                        return None;
                    } else if ch == '\\' && matches!(chars.get(i + 1), Some('"' | '\\' | '$' | '`'))
                    {
                        value.push(chars[i + 1]);
                        i += 2;
                    } else {
                        value.push(ch);
                        i += 1;
                    }
                }
                None => {
                    if ch == ' ' || ch == '\t' || (ch == '&' && chars.get(i + 1) == Some(&'&')) {
                        break;
                    }
                    if ch == '\'' || ch == '"' {
                        quote = Some(ch);
                        i += 1;
                    } else if ch == '\\'
                        || "()[]{}*?;|<>&$#".contains(ch)
                        || (ch == '~' && i == start)
                    {
                        return None;
                    } else {
                        value.push(ch);
                        i += 1;
                    }
                }
            }
        }
        if quote.is_some() {
            return None;
        }
        tokens.push(Token {
            value,
            raw: text[offsets[start]..offsets[i]].to_owned(),
            start: offsets[start],
            and: false,
        });
    }
    let separators: Vec<usize> = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| token.and.then_some(index))
        .collect();
    let mut directory: Option<String> = None;
    let mut words: &[Token] = &tokens;
    if !separators.is_empty() {
        if separators.len() != 1 {
            return None;
        }
        let at = separators[0];
        let prefix = &tokens[..at];
        if prefix.first().map(|token| token.raw.as_str()) != Some("cd")
            || !(prefix.len() == 2 || (prefix.len() == 3 && prefix[1].raw == "--"))
        {
            return None;
        }
        let dir = prefix.last()?.value.clone();
        if !dir.starts_with('/')
            && !re::test(r"^[A-Za-z]:[/\\]", "", &dir)
            && !re::test(r"^\\\\[^/\\]+[/\\][^/\\]+", "", &dir)
        {
            return None;
        }
        directory = Some(dir);
        words = &tokens[at + 1..];
    }
    if words.is_empty() || words.iter().any(|word| word.and) {
        return None;
    }
    Some(LiteralInvocation {
        directory,
        command: text[words[0].start..].to_owned(),
        argv: words.iter().map(|word| word.value.clone()).collect(),
        raw_words: words.iter().map(|word| word.raw.clone()).collect(),
    })
}

/// What a literal invocation's words name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineInvocation {
    /// An engine tool, with its arguments after the global flags, and the
    /// `--project-dir` it named.
    Found {
        command: String,
        args: Vec<String>,
        project_dir: Option<String>,
    },
    /// A wrapper took an option the reader does not follow.
    Uncertain,
    /// Another interpreter or script: conservative.
    Opaque,
}

fn base(token: &str) -> String {
    token
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_owned()
}

fn is_assignment(token: &str) -> bool {
    re::test(r"^[A-Za-z_][A-Za-z0-9_]*=", "", token)
}

/// `engineInvocationFromWords`: `None` when no executable position is
/// established, or a global flag misses its value.
#[must_use]
pub fn engine_invocation_from_words(
    tokens: &[String],
    raw_tokens: &[String],
) -> Option<EngineInvocation> {
    let at = |index: usize| tokens.get(index).map(String::as_str);
    let mut index = 0;
    while raw_tokens.get(index).is_some_and(|raw| is_assignment(raw)) {
        index += 1;
    }
    if matches!(at(index), Some("command" | "exec")) {
        index += 1;
        if at(index) == Some("--") {
            index += 1;
        }
        if at(index).is_some_and(|token| token.starts_with('-')) {
            return Some(EngineInvocation::Uncertain);
        }
    }
    if base(at(index).unwrap_or("")) == "env" {
        index += 1;
        if at(index) == Some("--") {
            index += 1;
        }
        if at(index).is_some_and(|token| token.starts_with('-')) {
            return Some(EngineInvocation::Uncertain);
        }
        while at(index).is_some_and(is_assignment) {
            index += 1;
        }
    }
    if index >= tokens.len() {
        return None;
    }
    let via_bun = base(&tokens[index]) == "bun";
    if via_bun {
        index += 1;
    }
    if index >= tokens.len() {
        return None;
    }
    if !re::test(
        r"^(?:focus|focus\.ts|focus-(?:orchestrate|state|jump|bolt|swarm|unit)(?:\.ts)?)$",
        "",
        &base(&tokens[index]),
    ) {
        // Unsupported interpreters do not become Bun transports merely because
        // a later argument names focus.ts.
        return Some(EngineInvocation::Opaque);
    }
    let mut command = base(&tokens[index]);
    if command == "focus.ts" {
        let previous = if index == 0 {
            ""
        } else {
            tokens[index - 1].as_str()
        };
        if !re::test(r"(?:^|[/\\])bun$", "", previous)
            || !re::test(&format!("{DISPATCHER_PATH}$"), "", &tokens[index])
        {
            return Some(EngineInvocation::Opaque);
        }
        command = "focus".to_owned();
    }
    let native = command == "focus";
    let mut project_dir: Option<String> = None;
    // The dispatcher's global extraction, then the orchestrator's own attempt
    // selector; neither consumes options after the literal delimiter.
    let mut strip_globals = |args: &[String], attempt: bool| -> Option<Vec<String>> {
        let mut clean: Vec<String> = Vec::new();
        let mut literal = false;
        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            if arg == "--" {
                literal = true;
            }
            if !literal && (arg == "--project-dir" || (attempt && arg == "--focus-attempt-id")) {
                if i + 1 >= args.len() {
                    return None;
                }
                if arg == "--project-dir" {
                    project_dir = Some(args[i + 1].clone());
                }
                i += 1;
            } else if let Some(dir) = arg.strip_prefix("--project-dir=").filter(|_| !literal) {
                project_dir = Some(dir.to_owned());
            } else if literal
                || !native
                || ![
                    "--json",
                    "--quiet",
                    "--no-color",
                    "--yes",
                    "--offline",
                    "--verbose",
                ]
                .contains(&arg)
            {
                clean.push(arg.to_owned());
            }
            i += 1;
        }
        Some(clean)
    };
    let mut args = strip_globals(&tokens[index + 1..], !native)?;
    if native && args.first().map(String::as_str) == Some("engine") {
        args.remove(0);
    }
    if native
        && matches!(
            args.first().map(String::as_str),
            Some("orchestrate" | "next" | "report" | "park")
        )
    {
        args = strip_globals(&args, true)?;
    }
    Some(EngineInvocation::Found {
        command,
        args,
        project_dir,
    })
}

const ORCHESTRATE_RELAY_VERBS: [&str; 4] = ["next", "continue", "report", "park"];

/// `literalOrchestrateVerb`: the orchestrate verb of one literal engine
/// invocation, in any shipped spelling; `None` for chains, pipes,
/// redirections, expansions and every other engine tool.
#[must_use]
pub fn literal_orchestrate_verb(command: &str) -> Option<String> {
    let literal = parse_literal_shell_invocation(command)?;
    let Some(EngineInvocation::Found { command, args, .. }) =
        engine_invocation_from_words(&literal.argv, &literal.raw_words)
    else {
        return None;
    };
    let mut index = 0;
    if command == "focus" {
        if args.first().map(String::as_str) == Some("orchestrate") {
            index += 1;
        }
    } else if !re::test(r"^focus-orchestrate(?:\.ts)?$", "", &command) {
        return None;
    }
    args.get(index)
        .filter(|verb| ORCHESTRATE_RELAY_VERBS.contains(&verb.as_str()))
        .cloned()
}

/// `shellToolResponseText`: the shell result's text, in Claude Code's
/// `{stdout, stderr, interrupted}` shape or the plain string other hosts
/// deliver.
#[must_use]
pub fn shell_tool_response_text(response: Option<&serde_json::Value>) -> Option<&str> {
    match response? {
        serde_json::Value::String(text) => Some(text),
        serde_json::Value::Object(map) => map.get("stdout").and_then(serde_json::Value::as_str),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bun_and_native_spellings_classify_alike() {
        for command in [
            "bun .claude/tools/focus-state.ts approve x",
            "bun '/p q/.kiro/tools/focus.ts' engine state approve x",
            "focus engine jump execute --target x",
            "focus status",
            "focus next --resume && focus report",
            "bun .claude/tools/focus-orchestrate.ts report --result done",
            "focus scope change",
            "focus config set x y",
            "echo 'focus state approve'",
        ] {
            assert_eq!(
                classify_runtime_compile_command(command),
                CompileCommand::Fire,
                "{command}"
            );
        }
        for command in [
            "echo hi",
            "focus intent create",
            "focus engine next",
            "focus config get x",
            "bun .claude/tools/focus-log.ts decision x",
            "bun .gemini/tools/focus.ts engine state approve x",
            "focusx state",
            "",
        ] {
            assert_eq!(
                classify_runtime_compile_command(command),
                CompileCommand::Pass,
                "{command}"
            );
        }
        for command in [
            "focus engine runtime compile",
            "bun .claude/tools/focus-runtime.ts compile",
            "focus state approve x && focus engine runtime compile",
            "bun .claude/tools/focus.ts engine runtime compile",
        ] {
            assert_eq!(
                classify_runtime_compile_command(command),
                CompileCommand::Reject,
                "{command}"
            );
        }
        // The runtime tool named in a quoted echo is not a segment that runs it.
        assert_eq!(
            classify_runtime_compile_command("echo 'focus engine runtime compile'"),
            CompileCommand::Pass
        );
    }

    #[test]
    fn a_literal_invocation_is_one_command_with_at_most_a_cd_before_it() {
        let literal = parse_literal_shell_invocation(
            "cd -- /p && bun '.claude/tools/focus.ts' engine orchestrate next 2>&1",
        )
        .unwrap();
        assert_eq!(literal.directory.as_deref(), Some("/p"));
        assert_eq!(
            literal.command,
            "bun '.claude/tools/focus.ts' engine orchestrate next"
        );
        assert_eq!(
            literal.argv,
            [
                "bun",
                ".claude/tools/focus.ts",
                "engine",
                "orchestrate",
                "next"
            ]
        );
        assert_eq!(literal.raw_words[1], "'.claude/tools/focus.ts'");
        assert_eq!(
            parse_literal_shell_invocation("focus \"ne\\\"xt\"")
                .unwrap()
                .argv,
            ["focus", "ne\"xt"]
        );
        assert_eq!(
            parse_literal_shell_invocation("cd 'C:\\p' && focus next")
                .unwrap()
                .directory
                .as_deref(),
            Some("C:\\p")
        );
        for text in [
            "",
            "   ",
            "cd /p &&",
            "&& focus next",
            "cd rel && focus next",
            "cd /p && cd /q && focus next",
            "cd /p || focus next",
            "focus next | cat",
            "focus next > out",
            "focus next; echo",
            "focus $(x)",
            "focus `x`",
            "focus #x",
            "focus next\n",
            "focus ~/x",
            "focus a\\ b",
            "focus 'open",
            "focus \"a$b\"",
            "focus (x)",
            "focus *",
            "x\0y",
            "cd /p -- && focus next",
        ] {
            assert!(parse_literal_shell_invocation(text).is_none(), "{text:?}");
        }
        assert!(parse_literal_shell_invocation("focus x~").is_some());
        assert_eq!(
            parse_literal_shell_invocation("focus\tnext").unwrap().argv,
            ["focus", "next"]
        );
    }

    #[test]
    fn the_engine_tool_is_found_behind_transparent_wrappers_only() {
        let words = |text: &str| {
            let literal = parse_literal_shell_invocation(text).unwrap();
            engine_invocation_from_words(&literal.argv, &literal.raw_words)
        };
        let found = |command: &str, args: &[&str], project_dir: Option<&str>| {
            Some(EngineInvocation::Found {
                command: command.to_owned(),
                args: args.iter().map(|arg| (*arg).to_owned()).collect(),
                project_dir: project_dir.map(str::to_owned),
            })
        };
        assert_eq!(
            words("focus engine orchestrate next --json"),
            found("focus", &["orchestrate", "next"], None)
        );
        assert_eq!(
            words("env X=1 focus --project-dir /p next --focus-attempt-id a1"),
            found("focus", &["next"], Some("/p"))
        );
        assert_eq!(
            words("X=1 command -- focus --project-dir=/q report"),
            found("focus", &["report"], Some("/q"))
        );
        assert_eq!(
            words("bun .claude/tools/focus.ts engine state approve"),
            found("focus", &["state", "approve"], None)
        );
        assert_eq!(
            words("bun .claude/tools/focus-orchestrate.ts --json next"),
            found("focus-orchestrate.ts", &["--json", "next"], None)
        );
        assert_eq!(
            words("focus next -- --json"),
            found("focus", &["next", "--", "--json"], None)
        );
        assert_eq!(
            words("env -i focus next"),
            Some(EngineInvocation::Uncertain)
        );
        // `--` after the assignments an env takes is a word, not the engine.
        assert_eq!(
            words("env X=1 -- focus next"),
            Some(EngineInvocation::Opaque)
        );
        assert_eq!(words("env -- focus next"), found("focus", &["next"], None));
        assert_eq!(
            words("command -v focus next"),
            Some(EngineInvocation::Uncertain)
        );
        assert_eq!(
            words("node .claude/tools/focus.ts engine orchestrate next"),
            Some(EngineInvocation::Opaque)
        );
        assert_eq!(
            words("bun focus.ts engine orchestrate next"),
            Some(EngineInvocation::Opaque)
        );
        assert_eq!(
            words("bun .gemini/tools/focus.ts engine next"),
            Some(EngineInvocation::Opaque)
        );
        assert_eq!(words("sh -c x"), Some(EngineInvocation::Opaque));
        assert_eq!(words("focus --project-dir"), None);
        assert_eq!(words("X=1"), None);
        assert_eq!(words("env"), None);
    }

    #[test]
    fn the_relayed_verbs_are_the_orchestrators_four() {
        assert_eq!(
            literal_orchestrate_verb("focus next").as_deref(),
            Some("next")
        );
        assert_eq!(
            literal_orchestrate_verb("focus engine orchestrate continue").as_deref(),
            Some("continue")
        );
        assert_eq!(
            literal_orchestrate_verb(
                "cd /p && bun .claude/tools/focus.ts engine orchestrate report --result x 2>&1"
            )
            .as_deref(),
            Some("report")
        );
        assert_eq!(
            literal_orchestrate_verb("bun .claude/tools/focus-orchestrate.ts park").as_deref(),
            Some("park")
        );
        for command in [
            "focus next | cat",
            "focus state approve",
            "focus orchestrate help",
            "bun .claude/tools/focus-state.ts next",
            "focus",
            "focus engine orchestrate",
        ] {
            assert_eq!(literal_orchestrate_verb(command), None, "{command}");
        }
    }

    #[test]
    fn a_shell_result_reads_as_text_or_its_stdout() {
        let text = serde_json::json!("out");
        let object = serde_json::json!({"stdout": "o", "stderr": "e"});
        let other = serde_json::json!({"output": "o"});
        assert_eq!(shell_tool_response_text(Some(&text)), Some("out"));
        assert_eq!(shell_tool_response_text(Some(&object)), Some("o"));
        assert_eq!(shell_tool_response_text(Some(&other)), None);
        assert_eq!(shell_tool_response_text(Some(&serde_json::json!(5))), None);
        assert_eq!(shell_tool_response_text(None), None);
    }
}

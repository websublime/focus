//! What runs while a plan waits (`aidlc-plan-approval-guard.ts`, the
//! hook's private readers of a command): the engine commands the Plan
//! Approval guard admits before the person has approved, and how a command
//! line is read for them. The build itself always waits; these are the
//! steps that change nothing a plan governs, the open question's own
//! transport, and the moves a person asks for.
//!
//! Everything here is pure: what depends on the project (the ledger's
//! record of a person having spoken, a recorded switch, the installed tool
//! files) is passed in or read by the hook (slices 4e-ii-b and 4e-ii-c).
//!
//! Held to upstream's answers by the oracle corpus (`every_helper`,
//! `plan_prerequisite`, `engine_directed`, `read_only_diagnostic`,
//! `plain_powershell`, `dynamic_evaluation`), asked through an export shim
//! over the hook's own source: the functions are upstream's, unmodified.

use focus_js::path::{Flavour, basename};
use focus_js::re;
use focus_js::text::to_lower_case;

use crate::dispatcher::resolve_engine_action;
use crate::guard_operation::{
    RenderShell, is_guard_recovery_engine_invocation, quote_command_argument,
};
use crate::plan_approval::GUARDED_STAGE;
use crate::shell::invocation::{ShellInvocationDetails, shell_command_invocation_details};
use crate::state_transition::delegated_lifecycle_command;

/// `SKELETON_STANCES`: the walking-skeleton stances `report
/// --skeleton-stance` records.
pub const SKELETON_STANCES: [&str; 3] = ["on", "off", "scope-dependent"];

/// `CONSTRUCTION_ENTRY_SETTERS`: the Construction entry choices the person
/// makes before the first Unit's plan exists.
const CONSTRUCTION_ENTRY_SETTERS: [&str; 4] = [
    "set-construction-checkpoints",
    "set-construction-execution",
    "set-construction-iteration",
    "set-construction-verification-command",
];

/// `READ_ONLY_POWERSHELL_CMDLETS`: cmdlets that read and never write,
/// whatever their parameters. They count only where the command is
/// PowerShell.
pub const READ_ONLY_POWERSHELL_CMDLETS: [&str; 16] = [
    "convertfrom-json",
    "format-custom",
    "format-hex",
    "format-list",
    "format-table",
    "format-wide",
    "get-childitem",
    "get-content",
    "get-item",
    "measure-object",
    "out-string",
    "resolve-path",
    "select-object",
    "select-string",
    "test-path",
    "write-output",
];

/// `DELEGATE_ADMITTED_VERBS` (`aidlc-state-transition-guard.ts`): what
/// every delegated agent may run, by script and verb: reads, and the work a
/// persona is dispatched to do.
pub const DELEGATE_ADMITTED_VERBS: [(&str, &[&str]); 18] = [
    (
        "focus-utility.ts",
        &[
            "help",
            "version",
            "now",
            "status",
            "detect",
            "project-description",
            "codekb-path",
            "codekb-scope-diff",
            "codekb-snapshot",
            "codekb-publish",
            "plugin-list",
            "plugin-validate",
            "config-get",
            "config-list",
            "resolve-env-scope",
            "scope-table",
            "stage-table",
            "select-plugins",
            "document-input",
        ],
    ),
    ("focus-state.ts", &["get", "count", "resume", "lookup"]),
    ("focus-orchestrate.ts", &["wait", "team-board"]),
    ("focus-jump.ts", &["resolve"]),
    ("focus-unit.ts", &["merge-status", "status"]),
    ("focus-log.ts", &["answers"]),
    ("focus-learnings.ts", &["surface"]),
    ("focus-runtime.ts", &["read", "summary"]),
    (
        "focus-testing-posture.ts",
        &["resolve", "render", "verify", "reply", "brief"],
    ),
    ("focus-worktree.ts", &["list", "verify", "info"]),
    ("focus-audit.ts", &["history"]),
    ("focus-plugin.ts", &["list"]),
    (
        "focus-graph.ts",
        &[
            "artifacts",
            "producers",
            "consumers",
            "topo",
            "cycles",
            "scope",
            "validate-scope",
            "validate-grid",
            "ars",
            "export",
        ],
    ),
    ("focus-sensor.ts", &["list", "describe", "fire"]),
    ("focus-validate.ts", &["outputs"]),
    ("focus-review-brief.ts", &["review", "context", "summary"]),
    ("focus-attest.ts", &["resolve"]),
    ("focus-knowledge.ts", &["list", "show"]),
];

fn word(args: &[String], at: usize) -> Option<&str> {
    args.get(at).map(String::as_str)
}

fn tail(args: &[String], from: usize) -> &[String] {
    args.get(from..).unwrap_or(&[])
}

/// `normalizedCommandName`: a command's base name, lower-cased, without
/// `.exe`.
#[must_use]
pub fn normalized_command_name(flavour: Flavour, name: &str) -> String {
    let lower = to_lower_case(&basename(flavour, name));
    lower.strip_suffix(".exe").unwrap_or(&lower).to_owned()
}

/// `lastFlagValue`: the value of the flag's last occurrence; nothing when
/// any occurrence has no value, or one that is itself a flag.
#[must_use]
pub fn last_flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    let mut value = None;
    let mut index = 0;
    while index < args.len() {
        if args[index] == flag {
            let candidate = args.get(index + 1).filter(|next| !next.is_empty())?;
            if candidate.starts_with("--") {
                return None;
            }
            value = Some(candidate.as_str());
            // The value is skipped as upstream skips it; it cannot be the
            // flag itself, which opens with `--`.
            index += 1;
        }
        index += 1;
    }
    value
}

/// `isReadOnlyDiagnostic`: diagnostics that change nothing a plan governs.
#[must_use]
pub fn is_read_only_diagnostic(args: &[String]) -> bool {
    let head = word(args, 0).unwrap_or("");
    if [
        "status",
        "--status",
        "version",
        "--version",
        "help",
        "--help",
    ]
    .contains(&head)
    {
        return true;
    }
    // The engine's clock, for a time a document asks for.
    if head == "engine" && args.len() == 2 && args[1] == "now" {
        return true;
    }
    head == "doctor" || head == "--doctor"
}

/// `isReadOnlyToolDiagnostic`: the same diagnostics through their source
/// tools.
#[must_use]
pub fn is_read_only_tool_diagnostic(stem: &str, args: &[String]) -> bool {
    if stem == "doctor" {
        return word(args, 0) == Some("doctor") && is_read_only_diagnostic(args);
    }
    stem == "utility"
        && (matches!(word(args, 0), Some("status" | "version"))
            || (word(args, 0) == Some("now") && args.len() == 1))
}

/// `asEngineRoute`: the dispatcher's top-level park runs `engine
/// orchestrate park`.
#[must_use]
pub fn as_engine_route(args: &[String]) -> Vec<String> {
    if word(args, 0) == Some("park") {
        ["engine", "orchestrate"]
            .iter()
            .map(|word| (*word).to_owned())
            .chain(args.iter().cloned())
            .collect()
    } else {
        args.to_vec()
    }
}

/// `onlyFlags`: only these flags, each with its value.
fn only_flags(args: &[String], allowed: &[&str]) -> bool {
    let mut index = 0;
    while index < args.len() {
        let (flag, inline) = match args[index].split_once('=') {
            Some((flag, _)) => (flag, true),
            None => (args[index].as_str(), false),
        };
        if !allowed.contains(&flag) {
            return false;
        }
        if !inline {
            index += 1;
        }
        index += 1;
    }
    true
}

/// The settings a scope or setting change the person asked for may carry.
const PLAN_WAIT_SETTINGS: [&str; 6] = [
    "depth",
    "test-strategy",
    "review",
    "sensors",
    "learnings",
    "collaborators",
];
const PLAN_WAIT_SETTING_FLAGS: [&str; 6] = [
    "--depth",
    "--test-strategy",
    "--review",
    "--sensors",
    "--learnings",
    "--collaborators",
];

/// `engineDirectedWhilePlanWaits`: what the engine names while a plan
/// waits, other than building it: steps that change nothing a plan
/// governs, admitted at any time, and moves a person asks for, admitted
/// once a person has spoken since the last decision (`person_asked`).
pub fn engine_directed_while_plan_waits(
    args: &[String],
    person_asked: &mut dyn FnMut() -> bool,
) -> bool {
    if word(args, 0) != Some("engine") {
        return false;
    }
    let noun = word(args, 1).unwrap_or("");
    let verb = word(args, 2).unwrap_or("");
    let after_noun = tail(args, 2);
    let after_verb = tail(args, 3);
    let has = |flag: &str| after_noun.iter().any(|arg| arg == flag);
    // Each route in upstream's order: its verbs, its own check, then
    // whether a person must have asked.
    let scope_flags: Vec<&str> = std::iter::once("--scope")
        .chain(PLAN_WAIT_SETTING_FLAGS)
        .collect();
    let routes: [(&str, Option<&[&str]>, bool, bool); 13] = [
        (
            "review-brief",
            Some(&["review", "context", "summary"]),
            true,
            false,
        ),
        (
            "log",
            Some(&["decision", "answer"]),
            last_flag_value(after_verb, "--stage") == Some(GUARDED_STAGE)
                && !after_noun
                    .iter()
                    .any(|arg| arg == "--checkpoint" || arg.starts_with("--checkpoint=")),
            false,
        ),
        ("status", None, true, false),
        ("orchestrate", Some(&["help"]), true, false),
        (
            "orchestrate",
            Some(&["report"]),
            last_flag_value(after_verb, "--result") == Some("resumed") && !has("--stage"),
            false,
        ),
        ("jump", Some(&["execute", "reopen"]), true, true),
        (
            "recompose",
            None,
            only_flags(after_noun, &["--skip", "--add", "--reason"]),
            true,
        ),
        (
            "scope",
            Some(&["change"]),
            only_flags(after_verb, &scope_flags),
            true,
        ),
        (
            "config",
            Some(&["set"]),
            PLAN_WAIT_SETTINGS.contains(&word(after_noun, 1).unwrap_or(""))
                && after_noun.len() > 2
                && only_flags(tail(after_noun, 3), &PLAN_WAIT_SETTING_FLAGS),
            true,
        ),
        ("intent", Some(&["create"]), true, true),
        (
            "workspace",
            Some(&["reclassify", "codekb-scope-diff"]),
            true,
            true,
        ),
        ("log", Some(&["review"]), true, true),
        // Kept last so the table's length names every route above.
        ("", Some(&[]), false, false),
    ];
    routes.iter().any(|(route, verbs, admits, asked)| {
        *route == noun
            && verbs.is_none_or(|verbs| verbs.contains(&verb))
            && *admits
            && (!*asked || person_asked())
    })
}

/// `intentRepoChangeAdmitted`: a sibling repo added to or removed from
/// this piece of work, and nothing else, once the person has spoken.
pub fn intent_repo_change_admitted(
    args: &[String],
    person_spoke: &mut dyn FnMut() -> bool,
) -> bool {
    let rest = if word(args, 0) == Some("engine") && word(args, 1) == Some("utility") {
        tail(args, 2)
    } else if word(args, 0) == Some("engine") {
        tail(args, 1)
    } else {
        return false;
    };
    if rest.len() != 3 || rest[0] != "intent" {
        return false;
    }
    if rest[1] != "add-repo" && rest[1] != "remove-repo" {
        return false;
    }
    person_spoke()
}

/// `isPlanApprovalPrerequisite`: the engine commands that carry a plan to
/// its approval, or record what the person decided, and build nothing.
/// `gate_held`: the durable state holds the Code Generation completion gate
/// open. `person_spoke`: a person has spoken since the last decision.
pub fn is_plan_approval_prerequisite(
    args: &[String],
    gate_held: bool,
    person_spoke: &mut dyn FnMut() -> bool,
) -> bool {
    if word(args, 0) != Some("engine") {
        return false;
    }
    // Direct refusals can offer the abort or the fence switch without
    // publishing a selection marker.
    if is_guard_recovery_engine_invocation(args) {
        return true;
    }
    let noun = word(args, 1).unwrap_or("");
    let verb = word(args, 2).unwrap_or("");
    let route_args = tail(args, 3);
    let has = |flag: &str| route_args.iter().any(|arg| arg == flag);
    // The conductor re-enters through next on each human turn, and continue
    // delivers the remaining stage rules.
    if noun == "orchestrate" && (verb == "next" || verb == "continue") {
        return true;
    }
    // Stopping for now and coming back are the person's call at any point.
    if noun == "orchestrate" && verb == "park" {
        return true;
    }
    if noun == "state" && verb == "unpark" {
        return true;
    }
    // The open Code Generation gate belongs to the human: their answer is
    // the only move left, and the engine generates nothing for it.
    if noun == "orchestrate" && verb == "report" && gate_held {
        return last_flag_value(route_args, "--stage") == Some(GUARDED_STAGE)
            && matches!(
                last_flag_value(route_args, "--result"),
                Some("approved" | "rejected")
            );
    }
    if noun == "testing-posture"
        && [
            "resolve",
            "render",
            "fingerprint",
            "verify",
            "reply",
            "restore",
        ]
        .contains(&verb)
    {
        return true;
    }
    // Reads of the runtime graph, the state file and the audit trail.
    if noun == "runtime" && verb == "summary" {
        return true;
    }
    if noun == "log" && verb == "answers" {
        return true;
    }
    if noun == "audit" && verb == "history" {
        return true;
    }
    // Checkpoint review owns its own audit, readiness and human authority.
    // Verification executes a supplied command and still requires approval.
    if noun == "bolt" && (verb == "checkpoint" || verb == "swarm-checkpoint") {
        return match last_flag_value(route_args, "--action") {
            None => !has("--action"),
            Some(action) => ["status", "ask", "approve", "reject"].contains(&action),
        };
    }
    // Unit lifecycle receipts and the Construction entry choices record
    // what the person decided and what the Unit did; none writes source.
    if noun == "state" && verb == "unit" {
        return matches!(word(args, 3), Some("start" | "pause" | "resume"));
    }
    if noun == "state" && CONSTRUCTION_ENTRY_SETTERS.contains(&verb) {
        return true;
    }
    if noun == "bolt" && verb == "set-autonomy" {
        return true;
    }
    // Plan approval on or off for this piece of work is the person's call:
    // on only adds the stop; off once a person has spoken.
    if noun == "config"
        && verb == "set"
        && args.len() == 5
        && matches!(word(args, 3), Some("plan-approval" | "guard.plan-approval"))
        && (args[4] == "on" || (args[4] == "off" && person_spoke()))
    {
        return true;
    }
    // Recording the remedy the person picked on the engine's recovery
    // question writes nothing in the workspace.
    if noun == "log"
        && verb == "answer"
        && last_flag_value(route_args, "--checkpoint") == Some("guard-recovery")
    {
        return true;
    }
    // The walking-skeleton stance, recorded through report without a stage
    // result.
    if noun == "orchestrate" && verb == "report" {
        return last_flag_value(route_args, "--skeleton-stance")
            .is_some_and(|stance| SKELETON_STANCES.contains(&stance))
            && !has("--result")
            && !has("--stage");
    }
    // Generation start refuses itself without the human's receipt-backed
    // approval, so the owner answers with the precise reason.
    if noun == "testing-posture" && verb == "begin" {
        return true;
    }
    if noun != "log" || (verb != "decision" && verb != "answer") {
        return false;
    }
    last_flag_value(route_args, "--stage") == Some(GUARDED_STAGE)
        && last_flag_value(route_args, "--checkpoint") == Some("plan-approval")
}

/// `everyHelperMayRun`: what every helper may run, whoever runs it: the
/// verbs the state-transition guard admits every delegated agent, in a
/// spelling that guard does not refuse a helper. A route is judged by the
/// script and verb the dispatcher runs for it, a tool script
/// (`engine <stem> …`) by its own name. `project_dir` and `cwd` are the
/// dispatcher's (see [`resolve_engine_action`]).
#[must_use]
pub fn every_helper_may_run(
    engine_args: &[String],
    harness_dir: &str,
    project_dir: &str,
    cwd: &str,
) -> bool {
    if word(engine_args, 0) != Some("engine") {
        return false;
    }
    let (tool, args) = match resolve_engine_action(engine_args, project_dir, cwd) {
        Some(found) => (found.tool, found.args),
        None => {
            let Some(stem) = word(engine_args, 1) else {
                return false;
            };
            (format!("focus-{stem}.ts"), tail(engine_args, 2).to_vec())
        }
    };
    let verb = args
        .iter()
        .enumerate()
        .find(|(index, arg)| {
            *arg != "--project-dir" && (*index == 0 || args[index - 1] != "--project-dir")
        })
        .map_or("", |(_, arg)| arg.as_str());
    let Some((_, verbs)) = DELEGATE_ADMITTED_VERBS
        .iter()
        .find(|(script, _)| *script == tool)
    else {
        return false;
    };
    if !verbs.contains(&verb) {
        return false;
    }
    let mut command = vec![format!("bun {harness_dir}/tools/{tool}")];
    command.extend(
        args.iter()
            .map(|arg| quote_command_argument(arg, RenderShell::Posix)),
    );
    delegated_lifecycle_command(&command.join(" ")).is_none()
}

/// A plain PowerShell command line: its commands' words, and the same
/// commands rendered as POSIX words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlainPowerShell {
    pub commands: Vec<Vec<String>>,
    pub rendering: String,
}

/// A PowerShell stream redirect that writes no file: to `$null`, or into
/// output.
const POWERSHELL_NULL_REDIRECT: &str = r"^(?:[1-6*]?>>?[ ]*\$null|[2-6*]>&1)(?=[ ;|]|$)";

/// `plainPowerShell`: reads a plain PowerShell command line: literal
/// words, commands joined by `;` or `|`, a leading `&` call operator, and
/// redirects that write no file. Nothing for anything PowerShell would
/// evaluate (variables, subexpressions, script blocks, splatting, comments)
/// and for words Windows PowerShell 5.1 hands a native program differently
/// than written.
#[must_use]
pub fn plain_power_shell(command: &str) -> Option<PlainPowerShell> {
    // Controls, white space other than a space, and the typographic dashes
    // and quotes PowerShell reads as `-` and as quotes.
    if re::test(
        r"[\x00-\x1f\x7f-\x9f\u2013-\u2015\u2018-\u201f]|[^\S ]",
        "u",
        command,
    ) {
        return None;
    }
    let chars: Vec<char> = command.chars().collect();
    let text = |from: usize, to: usize| chars[from..to].iter().collect::<String>();
    let mut commands: Vec<Vec<String>> = Vec::new();
    let mut joins: Vec<char> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut call = false;
    fn finish(words: &mut Vec<String>, commands: &mut Vec<Vec<String>>, call: &mut bool) -> bool {
        if words.is_empty() {
            return false;
        }
        if !READ_ONLY_POWERSHELL_CMDLETS.contains(&to_lower_case(&words[0]).as_str())
            && words[1..]
                .iter()
                .any(|word| word.is_empty() || word.contains('"') || word.ends_with('\\'))
        {
            return false;
        }
        commands.push(std::mem::take(words));
        *call = false;
        true
    }
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == ' ' {
            i += 1;
            continue;
        }
        if ch == ';' || ch == '|' {
            // `||` ends here too: its second bar finds no command before it.
            if !finish(&mut words, &mut commands, &mut call) {
                return None;
            }
            joins.push(ch);
            i += 1;
            continue;
        }
        let ahead = text(i, chars.len());
        if let Some(redirect) = re::find(POWERSHELL_NULL_REDIRECT, "i", &ahead) {
            i += ahead[..redirect.end()].chars().count();
            continue;
        }
        if ch == '&' {
            if call || !words.is_empty() || chars.get(i + 1) != Some(&' ') {
                return None;
            }
            call = true;
            i += 1;
            continue;
        }
        let first = words.is_empty();
        let word: String;
        if ch == '\'' || ch == '"' {
            let close = (i + 1..chars.len()).find(|at| chars[*at] == ch);
            // Without `&`, a quoted first word is an expression, not a
            // command.
            let close = close.filter(|_| !first || call)?;
            word = text(i + 1, close);
            if ch == '"' && (word.contains('$') || word.contains('`')) {
                return None;
            }
            i = close + 1;
        } else {
            let end = (i..chars.len())
                .find(|at| [' ', ';', '|', '&', '\'', '"'].contains(&chars[*at]))
                .unwrap_or(chars.len());
            word = text(i, end);
            if re::test(r"[`$@(){}#<>,%^!\[\]]", "", &word)
                || re::test(r"\\(?![A-Za-z0-9._-])", "", &word)
                || re::test(r"^-[^-].*\.", "", &word)
                || (first && !call && !re::test(r"^[A-Za-z][A-Za-z0-9._:\\/-]*$", "", &word))
            {
                return None;
            }
            i = end;
        }
        // A quote or `&` inside a word escapes or joins beyond this reading.
        if i < chars.len() && ![' ', ';', '|'].contains(&chars[i]) {
            return None;
        }
        if first && (word.is_empty() || word.contains('=')) {
            return None;
        }
        words.push(word);
    }
    if !finish(&mut words, &mut commands, &mut call) {
        return None;
    }
    let posix_word = |word: &String| {
        if re::test(r"^[A-Za-z0-9._/:+,@%-]+$", "", word) {
            word.clone()
        } else {
            format!("'{}'", word.replace('\'', "'\\''"))
        }
    };
    let rendering = commands
        .iter()
        .enumerate()
        .map(|(index, command)| {
            format!(
                "{}{}",
                if index > 0 {
                    format!(" {} ", joins[index - 1])
                } else {
                    String::new()
                },
                command.iter().map(posix_word).collect::<Vec<_>>().join(" ")
            )
        })
        .collect::<String>();
    Some(PlainPowerShell {
        commands,
        rendering,
    })
}

/// `powerShellReading`: the POSIX reading of a plain PowerShell command,
/// where that reading runs the same commands with the same arguments.
/// PowerShell runs each command's first word: a word the POSIX lexer reads
/// as a wrapper, keyword or assignment would put a different program under
/// review.
#[must_use]
pub fn power_shell_reading(command: &str) -> Option<String> {
    let plain = plain_power_shell(command)?;
    let invocations: Vec<ShellInvocationDetails> =
        shell_command_invocation_details(&plain.rendering);
    // The launchers, the executable and the arguments each tell a wrapper
    // on their own; upstream asks all three.
    let faithful = invocations.len() == plain.commands.len()
        && invocations
            .iter()
            .zip(&plain.commands)
            .all(|(invocation, words)| {
                !invocation.ambiguous
                    && invocation.launchers.is_empty()
                    && !invocation.data_driven
                    && !invocation.executable_resolution_changed
                    && invocation.executable.as_deref() == Some(words[0].as_str())
                    && invocation.args == words[1..]
            });
    faithful.then_some(plain.rendering)
}

/// `shellUsesDynamicEvaluation`: a variable, a substitution or a backtick
/// outside single quotes: text the lexer cannot follow.
#[must_use]
pub fn shell_uses_dynamic_evaluation(command: &str) -> bool {
    let chars: Vec<char> = command.chars().collect();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (index, ch) in chars.iter().copied().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            }
            continue;
        }
        if ch == '"' {
            quote = if quote == Some('"') { None } else { Some('"') };
            continue;
        }
        if ch == '\'' && quote.is_none() {
            quote = Some('\'');
            continue;
        }
        if ch == '`' || ch == '$' {
            return true;
        }
        if (ch == '<' || ch == '>') && chars.get(index + 1) == Some(&'(') {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    fn prerequisite(text: &str, gate_held: bool, spoke: bool) -> bool {
        is_plan_approval_prerequisite(&words(text), gate_held, &mut || spoke)
    }

    #[test]
    fn the_transport_of_a_plan_is_admitted_and_the_build_is_not() {
        assert!(prerequisite("engine orchestrate next", false, false));
        assert!(prerequisite("engine orchestrate park --x", false, false));
        assert!(prerequisite(
            "engine state unit start --unit a",
            false,
            false
        ));
        assert!(!prerequisite(
            "engine state unit complete --unit a",
            false,
            false
        ));
        assert!(!prerequisite("engine state approve s", false, false));
        assert!(prerequisite(
            "engine config set plan-approval on",
            false,
            false
        ));
        assert!(!prerequisite(
            "engine config set plan-approval off",
            false,
            false
        ));
        assert!(prerequisite(
            "engine config set plan-approval off",
            false,
            true
        ));
        // The fence's own switch is a recovery a refusal may offer.
        assert!(prerequisite(
            "engine config set guard.plan-approval off",
            false,
            false
        ));
        assert!(prerequisite(
            "engine config set guard.review-freeze off",
            false,
            false
        ));
        assert!(!prerequisite(
            "engine orchestrate report --stage code-generation --result approved",
            false,
            false
        ));
        assert!(prerequisite(
            "engine orchestrate report --stage code-generation --result approved",
            true,
            false
        ));
        assert!(prerequisite(
            "engine orchestrate report --skeleton-stance on",
            false,
            false
        ));
        assert!(!prerequisite(
            "engine orchestrate report --skeleton-stance on --stage x",
            false,
            false
        ));
        assert!(prerequisite("engine bolt checkpoint", false, false));
        assert!(prerequisite(
            "engine bolt checkpoint --action ask",
            false,
            false
        ));
        assert!(!prerequisite(
            "engine bolt checkpoint --action verify",
            false,
            false
        ));
        assert!(!prerequisite(
            "engine bolt checkpoint --action",
            false,
            false
        ));
        assert!(prerequisite(
            "engine log answer --stage code-generation --checkpoint plan-approval",
            false,
            false
        ));
        assert!(!prerequisite(
            "engine log answer --stage code-generation",
            false,
            false
        ));
        assert!(!prerequisite("orchestrate next", false, false));
    }

    #[test]
    fn what_the_engine_names_is_admitted_once_asked_for() {
        let directed =
            |text: &str, asked: bool| engine_directed_while_plan_waits(&words(text), &mut || asked);
        assert!(directed("engine review-brief review", false));
        assert!(directed("engine status", false));
        assert!(directed(
            "engine log decision --stage code-generation",
            false
        ));
        assert!(!directed(
            "engine log decision --stage code-generation --checkpoint x",
            false
        ));
        assert!(!directed("engine jump execute --target a", false));
        assert!(directed("engine jump execute --target a", true));
        assert!(directed("engine recompose --skip a --reason=b", true));
        assert!(!directed("engine recompose --skip a --force", true));
        assert!(directed("engine config set depth standard", true));
        assert!(!directed("engine config set guard-policy off", true));
        assert!(directed(
            "engine scope change --scope feature --depth standard",
            true
        ));
        assert!(directed(
            "engine orchestrate report --result resumed",
            false
        ));
        assert!(!directed(
            "engine orchestrate report --result resumed --stage a",
            false
        ));
        let repo =
            |text: &str, spoke: bool| intent_repo_change_admitted(&words(text), &mut || spoke);
        assert!(repo("engine intent add-repo api", true));
        assert!(repo("engine utility intent remove-repo api", true));
        assert!(!repo("engine intent add-repo api", false));
        assert!(!repo("engine intent add-repo api x", true));
        assert!(is_read_only_diagnostic(&words("doctor --export")));
        assert!(is_read_only_diagnostic(&words("engine now")));
        assert!(!is_read_only_diagnostic(&words("engine now x")));
        assert!(is_read_only_tool_diagnostic("utility", &words("now")));
        assert!(!is_read_only_tool_diagnostic("doctor", &words("status")));
        assert_eq!(
            as_engine_route(&words("park x")),
            words("engine orchestrate park x")
        );
    }

    #[test]
    fn every_helper_runs_reads_and_its_own_work() {
        let may = |text: &str| every_helper_may_run(&words(text), ".claude", "/p", "/p");
        assert!(may("engine graph topo"));
        assert!(may("engine utility detect"));
        assert!(may("engine workspace detect"));
        assert!(may("engine state get Scope"));
        assert!(!may("engine state approve s"));
        assert!(!may("engine graph compile"));
        assert!(!may("engine"));
        assert!(!may("graph topo"));
        assert_eq!(last_flag_value(&words("--a 1 --a 2"), "--a"), Some("2"));
        assert_eq!(last_flag_value(&words("--a 1 --a --b"), "--a"), None);
        assert_eq!(
            normalized_command_name(Flavour::Posix, "/usr/bin/Bun.EXE"),
            "bun"
        );
        assert_eq!(normalized_command_name(Flavour::Posix, "bin/cat/"), "cat");
        assert_eq!(
            normalized_command_name(Flavour::Win32, r"C:\x\Cat.exe"),
            "cat"
        );
        assert_eq!(
            normalized_command_name(Flavour::Posix, r"C:\x\cat.exe"),
            r"c:\x\cat"
        );
    }

    #[test]
    fn a_plain_powershell_line_is_read_and_anything_evaluated_is_not() {
        let plain = plain_power_shell("Get-Content a.txt | Select-Object -First 1").unwrap();
        assert_eq!(plain.commands.len(), 2);
        assert_eq!(
            plain.rendering,
            "Get-Content a.txt | Select-Object -First 1"
        );
        assert_eq!(
            plain_power_shell("& 'C:\\Program Files\\x.exe' 'it''s' 2>$null")
                .map(|read| read.rendering),
            None
        );
        assert_eq!(
            plain_power_shell("& 'my tool' a 2>$null; ls")
                .map(|read| read.rendering)
                .as_deref(),
            Some("'my tool' a ; ls")
        );
        assert!(plain_power_shell("echo $env:X").is_none());
        assert!(plain_power_shell("a || b").is_none());
        assert!(plain_power_shell("'quoted' a").is_none());
        assert!(plain_power_shell("tool -x.y").is_none());
        assert!(plain_power_shell("a\tb").is_none());
        assert_eq!(
            power_shell_reading("git status").as_deref(),
            Some("git status")
        );
        assert!(power_shell_reading("env git status").is_none());
        assert!(shell_uses_dynamic_evaluation("echo $HOME"));
        assert!(shell_uses_dynamic_evaluation("cat <(ls)"));
        assert!(!shell_uses_dynamic_evaluation("echo '$HOME' \\$x"));
        assert!(shell_uses_dynamic_evaluation("echo \"$HOME\""));
    }
}

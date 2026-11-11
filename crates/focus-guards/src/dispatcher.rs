//! The engine namespace of upstream's dispatcher (`aidlc.ts`,
//! `resolveAction`): which tool script, with which arguments, an
//! `engine <noun> <verb> …` command runs as. A guard that judges a command
//! by the script and verb the dispatcher would run for it reads it here;
//! the routes are upstream's table, as data.
//!
//! Only the delegation is resolved: a command the dispatcher would answer
//! itself (help, a hook, the status line, an adapter, an error) resolves to
//! nothing. Focus's own command line is not routed through this table.
//!
//! Held to upstream's answers by the oracle corpus (`engine_route`).

use focus_js::path::{Flavour, is_absolute, resolve};

/// The tool script a command is delegated to, and the arguments it gets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delegate {
    pub tool: String,
    pub args: Vec<String>,
}

/// `LAUNCHER_GLOBAL_FLAGS`: dropped wherever they appear before `--`, then
/// handed to the delegate.
const GLOBAL_FLAGS: [&str; 6] = [
    "--json",
    "--quiet",
    "--no-color",
    "--yes",
    "--offline",
    "--verbose",
];

const UTILITY: &str = "focus-utility.ts";
const ORCHESTRATE: &str = "focus-orchestrate.ts";
const RUNNER_GEN: &str = "focus-runner-gen.ts";
const PLUGIN: &str = "focus-plugin.ts";

/// How a route of the engine namespace resolves.
#[derive(Clone, Copy)]
enum Kind {
    /// `<tool> <verb> <rest>`, for the verbs listed.
    Passthrough(&'static str, &'static [&'static str]),
    /// `<tool> <target> <rest>`, the target looked up by verb.
    Map(&'static str, &'static [(&'static str, &'static str)]),
    /// `<tool> <rest>`, whatever follows the noun.
    Tool(&'static str),
    /// The workspace nouns (`intent`, `space`).
    Workspace,
    Config,
    Plugin,
    Gen,
}

/// The engine namespace's routes by noun (`ROUTES`), in upstream's order
/// within a noun. `top` holds what resolves on the first word alone.
const ROUTES: &[(&str, Kind)] = &[
    (
        "state",
        Kind::Passthrough(
            "focus-state.ts",
            &[
                "get",
                "set",
                "set-skeleton-stance",
                "set-construction-iteration",
                "set-construction-checkpoints",
                "set-construction-execution",
                "set-construction-verification-command",
                "checkbox",
                "count",
                "advance",
                "finalize",
                "complete-workflow",
                "gate-start",
                "approve",
                "reject",
                "revise",
                "skip",
                "resume",
                "acknowledge-compaction",
                "reuse-artifact",
                "lookup",
                "practices-event",
                "practices-promote",
                "set-unit-ownership",
                "set-unit-gate-rhythm",
                "refresh-unit-progress",
                "sync-unit-scope-stage",
                "fold-unit-merge",
                "fork",
                "merge",
                "park",
                "unpark",
                "unit",
            ],
        ),
    ),
    (
        "state",
        Kind::Map(
            UTILITY,
            &[("set-status", "set-status"), ("init", "state-init")],
        ),
    ),
    (
        "audit",
        Kind::Passthrough(
            "focus-audit.ts",
            &["append", "append-batch", "append-raw", "history"],
        ),
    ),
    (
        "audit",
        Kind::Map(
            "focus-audit.ts",
            &[("fork", "audit-fork"), ("merge", "audit-merge")],
        ),
    ),
    (
        "graph",
        Kind::Passthrough(
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
                "compile",
                "resolve",
                "export",
            ],
        ),
    ),
    (
        "runtime",
        Kind::Passthrough(
            "focus-runtime.ts",
            &[
                "compile",
                "read",
                "summary",
                "fragment-fork",
                "fragment-merge",
            ],
        ),
    ),
    (
        "sensor",
        Kind::Passthrough("focus-sensor.ts", &["list", "describe", "fire"]),
    ),
    (
        "swarm",
        Kind::Passthrough("focus-swarm.ts", &["prepare", "check", "finalize"]),
    ),
    (
        "bolt",
        Kind::Passthrough(
            "focus-bolt.ts",
            &[
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
            ],
        ),
    ),
    (
        "worktree",
        Kind::Passthrough(
            "focus-worktree.ts",
            &[
                "create", "merge", "discard", "restore", "purge", "list", "verify", "info",
            ],
        ),
    ),
    (
        "jump",
        Kind::Passthrough("focus-jump.ts", &["resolve", "execute", "reopen"]),
    ),
    (
        "log",
        Kind::Passthrough(
            "focus-log.ts",
            &["decision", "answer", "answers", "review", "link"],
        ),
    ),
    (
        "learnings",
        Kind::Passthrough("focus-learnings.ts", &["surface", "persist"]),
    ),
    (
        "testing-posture",
        Kind::Passthrough(
            "focus-testing-posture.ts",
            &[
                "resolve",
                "render",
                "fingerprint",
                "verify",
                "begin",
                "brief",
                "reply",
                "restore",
            ],
        ),
    ),
    (
        "validate",
        Kind::Passthrough("focus-validate.ts", &["outputs"]),
    ),
    ("intent", Kind::Workspace),
    ("space", Kind::Workspace),
    (
        "scope",
        Kind::Map(
            UTILITY,
            &[
                ("change", "scope-change"),
                ("save", "scope-save"),
                ("detect", "detect-scope"),
                ("resolve-env", "resolve-env-scope"),
            ],
        ),
    ),
    ("config", Kind::Config),
    ("plugin", Kind::Plugin),
    (
        "knowledge",
        Kind::Passthrough(
            "focus-knowledge.ts",
            &[
                "onboard",
                "sync",
                "list",
                "show",
                "associate",
                "dissociate",
                "rebind",
                "summarize",
                "help",
            ],
        ),
    ),
    ("gen", Kind::Gen),
    (
        "workspace",
        Kind::Map(
            UTILITY,
            &[
                ("detect", "detect"),
                ("reclassify", "reclassify"),
                ("codekb", "codekb-path"),
                ("codekb-scope-diff", "codekb-scope-diff"),
                ("codekb-snapshot", "codekb-snapshot"),
                ("codekb-publish", "codekb-publish"),
                ("project-description", "project-description"),
                ("document-input", "document-input"),
            ],
        ),
    ),
    (
        "review-brief",
        Kind::Passthrough("focus-review-brief.ts", &["review", "context", "summary"]),
    ),
    (
        "orchestrate",
        Kind::Passthrough(
            ORCHESTRATE,
            &["next", "continue", "report", "park", "wait", "team-board"],
        ),
    ),
    ("orchestrate", Kind::Map(UTILITY, &[("help", "help")])),
    (
        "sensor-claim-sources",
        Kind::Tool("focus-sensor-claim-sources.ts"),
    ),
    ("sensor-linter", Kind::Tool("focus-sensor-linter.ts")),
    (
        "sensor-required-sections",
        Kind::Tool("focus-sensor-required-sections.ts"),
    ),
    (
        "sensor-traceability",
        Kind::Tool("focus-sensor-traceability.ts"),
    ),
    (
        "sensor-type-check",
        Kind::Tool("focus-sensor-type-check.ts"),
    ),
    (
        "sensor-upstream-coverage",
        Kind::Tool("focus-sensor-upstream-coverage.ts"),
    ),
    // `hook`, and `top` (the group of what resolves on the first word
    // alone, whose last routes are the status line and the adapters), are
    // the dispatcher's own: no row, so they resolve to nothing.
];

/// The engine namespace's first-word routes that delegate (`status`,
/// `recompose`, `now`).
const TOP_PASSTHROUGH: [&str; 3] = ["status", "recompose", "now"];

/// The `config set <key>` keys that delegate to `config-change`.
const CONFIG_SET_KEYS: [&str; 14] = [
    "depth",
    "test-strategy",
    "review",
    "guard-policy",
    "change-control",
    "sensors",
    "learnings",
    "collaborators",
    "summary-confirmation",
    "plan-approval",
    "guard.plan-approval",
    "guard.review-freeze",
    "guard.state-transition",
    "guard.reviewer-scope",
];

const SENSOR_SCRIPTS: [(&str, &str); 6] = [
    ("claim-sources", "focus-sensor-claim-sources.ts"),
    ("linter", "focus-sensor-linter.ts"),
    ("required-sections", "focus-sensor-required-sections.ts"),
    ("traceability", "focus-sensor-traceability.ts"),
    ("type-check", "focus-sensor-type-check.ts"),
    ("upstream-coverage", "focus-sensor-upstream-coverage.ts"),
];

const RESERVED_FUTURE: [&str; 3] = ["rename", "show", "birth"];
fn delegate(tool: &str, args: Vec<String>) -> Option<Delegate> {
    Some(Delegate {
        tool: tool.to_owned(),
        args,
    })
}

fn with_head(head: &[&str], rest: &[String]) -> Vec<String> {
    head.iter()
        .map(|word| (*word).to_owned())
        .chain(rest.iter().cloned())
        .collect()
}

fn rest(argv: &[String], from: usize) -> &[String] {
    argv.get(from..).unwrap_or(&[])
}

/// `handleWorkspace`: `parseDispatcherWorkspaceCommand` and
/// `dispatcherWorkspaceUtilityArgv` in one, for `intent` and `space`.
fn workspace(argv: &[String]) -> Option<Delegate> {
    let noun = argv[0].as_str();
    let word = |at: usize| argv.get(at).map(String::as_str);
    let list = |flags: &[String]| {
        let mut args = vec![noun.to_owned()];
        if flags.iter().any(|flag| flag == "--json") {
            args.push("--json".to_owned());
        }
        if noun == "intent" && flags.iter().any(|flag| flag == "--all") {
            args.push("--all".to_owned());
        }
        delegate(UTILITY, args)
    };
    let Some(verb) = word(1) else {
        return delegate(UTILITY, vec![noun.to_owned()]);
    };
    if verb == "--json" || (noun == "intent" && verb == "--all") {
        return list(rest(argv, 1));
    }
    if verb == "help" || verb == "-h" {
        return delegate(UTILITY, vec!["help".to_owned()]);
    }
    if RESERVED_FUTURE.contains(&verb) {
        return None;
    }
    match (noun, verb) {
        (_, "list") => return list(rest(argv, 2)),
        (_, "switch") => {
            return word(2)
                .and_then(|name| delegate(UTILITY, with_head(&[noun, "switch", name], &[])));
        }
        ("intent", "create") => {
            return delegate(UTILITY, with_head(&["intent-create"], rest(argv, 2)));
        }
        ("intent", "archive" | "unarchive" | "add-repo" | "remove-repo") => {
            return word(2)
                .filter(|name| !name.starts_with("--"))
                .and_then(|name| delegate(UTILITY, with_head(&[noun, verb, name], rest(argv, 3))));
        }
        ("space", "create") => {
            return word(2)
                .and_then(|name| delegate(UTILITY, with_head(&["space-create", name], &[])));
        }
        _ => {}
    }
    // Every verb of the noun was answered above (upstream keeps an error
    // for one that was not, which none reaches): a bare name selects the
    // record of that name.
    delegate(UTILITY, with_head(&[noun, verb], &[]))
}

/// `handleConfig`.
fn config(argv: &[String]) -> Option<Delegate> {
    let word = |at: usize| argv.get(at).map(String::as_str);
    match word(1) {
        Some("get") => delegate(UTILITY, with_head(&["config-get"], rest(argv, 2))),
        Some("list") => delegate(UTILITY, with_head(&["config-list"], rest(argv, 2))),
        Some("set") => {
            let key = word(2).filter(|key| CONFIG_SET_KEYS.contains(key))?;
            let value = word(3).filter(|value| !value.is_empty())?;
            delegate(
                UTILITY,
                with_head(
                    &["config-change", &format!("--{key}"), value],
                    rest(argv, 4),
                ),
            )
        }
        _ => None,
    }
}

/// `handlePlugin`.
fn plugin(argv: &[String]) -> Option<Delegate> {
    let target = match argv.get(1).map(String::as_str)? {
        verb @ ("list" | "sync") => return delegate(PLUGIN, with_head(&[verb], rest(argv, 2))),
        "select" => "select-plugins",
        "validate" => "plugin-validate",
        "build" => "plugin-build",
        _ => return None,
    };
    delegate(UTILITY, with_head(&[target], rest(argv, 2)))
}

/// `handleGen`.
fn generate(argv: &[String]) -> Option<Delegate> {
    match argv.get(1).map(String::as_str)? {
        "runners" if argv.get(2).map(String::as_str) == Some("--check") => {
            delegate(RUNNER_GEN, with_head(&["check"], rest(argv, 3)))
        }
        "runners" => delegate(RUNNER_GEN, with_head(&["write"], rest(argv, 2))),
        "runner-list" => delegate(RUNNER_GEN, with_head(&["list"], rest(argv, 2))),
        "runner-scopes" => delegate(RUNNER_GEN, with_head(&["scopes"], rest(argv, 2))),
        verb @ ("stage-table" | "scope-table") => {
            delegate(UTILITY, with_head(&[verb], rest(argv, 2)))
        }
        _ => None,
    }
}

/// `resolveAlias`, for the engine namespace. `__sensor-script-file` and
/// `--version` are the dispatcher's own and name no route, so they resolve
/// to nothing with every other unknown word.
fn alias(argv: &[String]) -> Option<Option<Delegate>> {
    let tail = |head: &[&str], tool: &str| Some(delegate(tool, with_head(head, rest(argv, 1))));
    match argv[0].as_str() {
        "__sensor-script" => {
            let id = argv.get(1).map_or("", String::as_str);
            Some(
                SENSOR_SCRIPTS
                    .iter()
                    .find(|(name, _)| *name == id)
                    .and_then(|(_, tool)| delegate(tool, rest(argv, 2).to_vec())),
            )
        }
        "--status" => tail(&["status"], UTILITY),
        "--claim" => tail(&["claim"], UTILITY),
        "--release" => tail(&["release"], UTILITY),
        "--doctor" => tail(&["doctor"], "focus-doctor.ts"),
        "--resume" => tail(&["next", "--resume"], ORCHESTRATE),
        "--scope" => tail(&["next", "--scope"], ORCHESTRATE),
        _ => None,
    }
}

/// `resolveEngine`: the words after `engine`.
fn engine(argv: &[String]) -> Option<Delegate> {
    let head = argv.first().map(String::as_str)?;
    // Help (`--help`, `-h`, `help`) is the dispatcher's own, and names no
    // route below.
    if let Some(aliased) = alias(argv) {
        return aliased;
    }
    // `resolveNoun`: the noun's routes in order, then its custom handler.
    let routes: Vec<Kind> = ROUTES
        .iter()
        .filter(|(noun, _)| *noun == head)
        .map(|(_, kind)| *kind)
        .collect();
    if !routes.is_empty() {
        let verb = argv.get(1).map(String::as_str);
        for route in &routes {
            match route {
                Kind::Tool(tool) => return delegate(tool, rest(argv, 1).to_vec()),
                Kind::Passthrough(tool, verbs) => {
                    if let Some(verb) = verb.filter(|verb| verbs.contains(verb)) {
                        return delegate(tool, with_head(&[verb], rest(argv, 2)));
                    }
                }
                Kind::Map(tool, targets) => {
                    if let Some((_, target)) = targets.iter().find(|(name, _)| Some(*name) == verb)
                    {
                        return delegate(tool, with_head(&[target], rest(argv, 2)));
                    }
                }
                Kind::Workspace | Kind::Config | Kind::Plugin | Kind::Gen => {}
            }
        }
        return match routes.iter().find(|kind| {
            matches!(
                kind,
                Kind::Workspace | Kind::Config | Kind::Plugin | Kind::Gen
            )
        }) {
            Some(Kind::Workspace) => workspace(argv),
            Some(Kind::Config) => config(argv),
            Some(Kind::Plugin) => plugin(argv),
            Some(Kind::Gen) => generate(argv),
            _ => None,
        };
    }
    if TOP_PASSTHROUGH.contains(&head) {
        return delegate(UTILITY, argv.to_vec());
    }
    // `statusline`, `adapter`, or a word the engine does not know.
    None
}

/// `resolveAction`, for a command line whose first word is `engine`: the
/// tool and arguments it is delegated to, or nothing when the dispatcher
/// answers it itself. `project_dir` is the dispatcher's own project (its
/// environment's, else its working directory) and `cwd` its working
/// directory: a literal tail after `--` gets the first pinned ahead of it,
/// and a relative `--project-dir` is resolved against the second.
#[must_use]
pub fn resolve_engine_action(raw: &[String], project_dir: &str, cwd: &str) -> Option<Delegate> {
    let mut clean: Vec<String> = Vec::new();
    let mut global: Vec<String> = Vec::new();
    let mut named: Option<String> = None;
    let mut literal = false;
    let mut index = 0;
    while index < raw.len() {
        let arg = &raw[index];
        index += 1;
        if arg == "--" {
            literal = true;
            clean.push(arg.clone());
            continue;
        }
        if !literal && GLOBAL_FLAGS.contains(&arg.as_str()) {
            global.push(arg.clone());
            continue;
        }
        if !literal && arg.starts_with("--project-dir=") {
            return None;
        }
        if literal || arg != "--project-dir" {
            clean.push(arg.clone());
            continue;
        }
        let value = raw
            .get(index)
            .filter(|value| !value.is_empty() && !value.starts_with("--"))?;
        index += 1;
        if named.is_some() {
            return None;
        }
        named = Some(value.clone());
    }
    if clean.first().map(String::as_str) != Some("engine") {
        return None;
    }
    let mut action = engine(&clean[1..])?;
    let pinned = match &named {
        Some(dir) if is_absolute(Flavour::host(), dir) => Some(dir.clone()),
        Some(dir) => Some(resolve(Flavour::host(), cwd, &[dir])),
        None => None,
    };
    let delimiter = |args: &[String]| args.iter().position(|arg| arg == "--");
    let pin = |args: &mut Vec<String>, dir: &str| {
        let flag = ["--project-dir".to_owned(), dir.to_owned()];
        match delimiter(args) {
            Some(at) => {
                args.splice(at..at, flag);
            }
            None => args.extend(flag),
        }
    };
    match &pinned {
        Some(dir) => pin(&mut action.args, dir),
        // Literal text after `--` may itself hold `--project-dir`: the
        // dispatcher's own directory goes ahead of the delimiter.
        None if delimiter(&action.args).is_some() => pin(&mut action.args, project_dir),
        None => {}
    }
    match delimiter(&action.args) {
        Some(at) => {
            action.args.splice(at..at, global);
        }
        None => action.args.extend(global),
    }
    Some(action)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split(' ').map(str::to_owned).collect()
    }

    fn resolved(text: &str) -> Option<(String, String)> {
        resolve_engine_action(&words(text), "/p", "/cwd")
            .map(|found| (found.tool, found.args.join(" ")))
    }

    fn is(text: &str, tool: &str, args: &str) {
        assert_eq!(
            resolved(text),
            Some((tool.to_owned(), args.to_owned())),
            "{text}"
        );
    }

    #[test]
    fn nouns_delegate_to_their_tools() {
        is("engine graph topo --json", "focus-graph.ts", "topo --json");
        is("engine state init x", UTILITY, "state-init x");
        is("engine state approve s", "focus-state.ts", "approve s");
        is("engine orchestrate help", UTILITY, "help");
        is("engine scope resolve-env", UTILITY, "resolve-env-scope");
        is("engine status", UTILITY, "status");
        is("engine --resume x", ORCHESTRATE, "next --resume x");
        is("engine sensor-linter a b", "focus-sensor-linter.ts", "a b");
        is(
            "engine __sensor-script linter a",
            "focus-sensor-linter.ts",
            "a",
        );
        assert_eq!(resolved("engine graph nope"), None);
        assert_eq!(resolved("engine hook x"), None);
        assert_eq!(resolved("engine top status"), None);
        assert_eq!(resolved("engine statusline"), None);
        assert_eq!(resolved("engine"), None);
        assert_eq!(resolved("engine help"), None);
        assert_eq!(resolved("status"), None);
    }

    #[test]
    fn the_custom_nouns_follow_their_parsers() {
        is("engine intent", UTILITY, "intent");
        // `--json` is a global flag: dropped before the noun is read, handed on
        // after it.
        is("engine intent --all --json", UTILITY, "intent --all --json");
        is("engine space list --all --json", UTILITY, "space --json");
        is("engine intent switch a", UTILITY, "intent switch a");
        is("engine intent a", UTILITY, "intent a");
        is(
            "engine intent create a --scope x",
            UTILITY,
            "intent-create a --scope x",
        );
        is(
            "engine intent archive a --reason r",
            UTILITY,
            "intent archive a --reason r",
        );
        is("engine space create a b", UTILITY, "space-create a");
        assert_eq!(resolved("engine intent archive --x"), None);
        assert_eq!(resolved("engine intent rename"), None);
        // A word that is no verb of the noun is a record's name.
        is("engine space archive", UTILITY, "space archive");
        assert_eq!(resolved("engine intent switch"), None);
        assert_eq!(resolved("engine space create"), None);
        is(
            "engine config set guard.review-freeze off x",
            UTILITY,
            "config-change --guard.review-freeze off x",
        );
        is("engine config get depth", UTILITY, "config-get depth");
        assert_eq!(resolved("engine config set guard.human-presence off"), None);
        assert_eq!(resolved("engine config set depth"), None);
        is("engine plugin list a", PLUGIN, "list a");
        is("engine plugin select a", UTILITY, "select-plugins a");
        assert_eq!(resolved("engine plugin"), None);
        is("engine gen runners --check x", RUNNER_GEN, "check x");
        is("engine gen runners x", RUNNER_GEN, "write x");
        is("engine gen scope-table", UTILITY, "scope-table");
    }

    #[test]
    fn global_flags_and_the_project_are_handed_on() {
        is("engine --json graph topo", "focus-graph.ts", "topo --json");
        is(
            "engine graph topo --project-dir /x --yes",
            "focus-graph.ts",
            "topo --project-dir /x --yes",
        );
        is(
            "engine graph topo --project-dir x",
            "focus-graph.ts",
            "topo --project-dir /cwd/x",
        );
        is(
            "engine log answer --json -- --project-dir y --json",
            "focus-log.ts",
            "answer --project-dir /p --json -- --project-dir y --json",
        );
        is(
            "engine log answer --project-dir /x -- a",
            "focus-log.ts",
            "answer --project-dir /x -- a",
        );
        assert_eq!(resolved("engine graph topo --project-dir=/x"), None);
        assert_eq!(resolved("engine graph topo --project-dir"), None);
        assert_eq!(resolved("engine graph topo --project-dir --x"), None);
        assert_eq!(
            resolved("engine graph topo --project-dir /x --project-dir /y"),
            None
        );
    }
}

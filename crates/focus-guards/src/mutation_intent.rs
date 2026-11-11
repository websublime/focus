//! What a tool call would change while a plan waits
//! (`aidlc-plan-approval-guard.ts`: `mutationIntent`, what it calls, and
//! the predicates the hook asks of each write target).
//!
//! The Plan Approval guard lets a call through when it can see that the
//! call changes nothing a plan governs. For a shell command that is three
//! readings of the same line: the files it names as written to, whether any
//! command on it could change something the lexer cannot see (an *opaque*
//! shell), and whether the lexer read the whole line at all (*readable*).
//! For a file tool it is the paths the tool names. A target is then judged
//! by where it lies: outside the project, the composer's proposal, the text
//! of a person's answer, the stage's own diary.
//!
//! What depends on the project is passed in ([`Project`], [`PlanWait`]):
//! the folders are resolved by whoever selected the piece of work, and a
//! person having spoken is the ledger's to say, which this crate does not
//! read. The active-directive marker is M5's: its admissions (the open
//! question's own answers, a selected restart, a swarm's units) are absent
//! here, as they are upstream while no marker is on disk.
//!
//! Two of upstream's spellings of the engine are not Focus's and are not
//! admitted (each a deliberate deviation, in the plan and in the tests):
//! the absolute path of the Bun running the hook (`isThisBun`), which
//! Focus's hook has none of, and the absolute path of the installed
//! launcher or active executable (`installedEngine`), which belongs to the
//! distribution and is refused until it is ported.
//!
//! Held to upstream's answers by the oracle corpus (`mutation_intent`,
//! `mutation_target`), asked over scratch project trees through the export
//! shim over the hook's own source.

use std::path::Path;

use focus_js::path::{Flavour, basename, dirname, is_absolute, join, relative, resolve};
use focus_js::re;
use focus_js::text::to_lower_case;
use focus_state::progress::{CheckboxState, parse_checkboxes};
use focus_workspace::files::{absent_or_plain_file, same_directory};
use focus_workspace::links::chain_is_link_free;
use focus_workspace::names::{ENGINE_DIR, STATE_FILE};
use focus_workspace::project::normalize_drive_letter;
use focus_workspace::settings::RECORDABLE_BYPASSES;
use serde_json::{Map, Value, json};

use crate::plan_approval::{GUARDED_STAGE, normalize_stage_name};
use crate::plan_wait::{
    READ_ONLY_POWERSHELL_CMDLETS, as_engine_route, engine_directed_while_plan_waits,
    every_helper_may_run, intent_repo_change_admitted, is_plan_approval_prerequisite,
    is_read_only_diagnostic, is_read_only_tool_diagnostic, normalized_command_name,
    power_shell_reading, shell_uses_dynamic_evaluation,
};
use crate::shell::invocation::{
    ShellInvocationDetails, shell_command_alters_executable_resolution,
    shell_command_invocation_details,
};
use crate::shell::{CommandShell, Host, shell_write_targets};

/// `SHELL_INTERPRETERS`: programs that run text the lexer cannot see.
const SHELL_INTERPRETERS: [&str; 14] = [
    ".",
    "bash",
    "busybox",
    "cmd",
    "dash",
    "eval",
    "exec",
    "fish",
    "ksh",
    "powershell",
    "pwsh",
    "sh",
    "source",
    "zsh",
];

/// `INLINE_CODE_RUNTIMES` and `INLINE_CODE_FLAGS`: a runtime handed its
/// program on the command line.
const INLINE_CODE_RUNTIMES: [&str; 9] = [
    "bun", "deno", "node", "perl", "php", "python", "python2", "python3", "ruby",
];
const INLINE_CODE_FLAGS: [&str; 8] = ["-c", "-e", "-E", "-p", "-r", "--eval", "--print", "eval"];

/// `WRITE_TOOLS`: the file tools.
pub const WRITE_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// `READ_ONLY_SHELL_COMMANDS`: commands that write nothing by themselves.
const READ_ONLY_SHELL_COMMANDS: [&str; 28] = [
    "[", "basename", "cat", "cmp", "cut", "diff", "dirname", "echo", "file", "grep", "head", "ls",
    "more", "printf", "pwd", "readlink", "realpath", "rg", "sort", "stat", "tail", "test", "tr",
    "type", "uniq", "wc", "where", "which",
];

/// `TRACKED_SHELL_MUTATORS`: commands whose writes the lexer places among
/// the targets.
const TRACKED_SHELL_MUTATORS: [&str; 11] = [
    "cp", "dd", "install", "mv", "perl", "rm", "sed", "tee", "touch", "truncate", "unlink",
];

/// `READ_ONLY_GIT_SUBCOMMANDS`.
const READ_ONLY_GIT_SUBCOMMANDS: [&str; 8] = [
    "branch",
    "diff",
    "grep",
    "log",
    "ls-files",
    "rev-parse",
    "show",
    "status",
];

/// `POWERSHELL_SET_LOCATION`: PowerShell's names for a directory change.
const POWERSHELL_SET_LOCATION: [&str; 3] = ["chdir", "set-location", "sl"];

/// `RECORDED_SWITCH_OPTIONS`: options that only choose a layer, confirm,
/// or shape the output.
const RECORDED_SWITCH_OPTIONS: [&str; 8] = [
    "--local",
    "--project",
    "--global",
    "--yes",
    "--json",
    "--quiet",
    "--no-color",
    "--verbose",
];

/// `PERSON_CHECK_SWITCH_VALUES`: the person's checks a setter switches for
/// this piece of work, with the values that turn each one on (only adding
/// a stop) and off (the person's call).
const PERSON_CHECK_SWITCH_VALUES: [(&str, &[&str], &[&str]); 7] = [
    ("plan-approval", &["on"], &["off"]),
    ("guard.plan-approval", &["on"], &["off"]),
    ("summary-confirmation", &["on"], &["off"]),
    ("guard.review-freeze", &["on"], &["off"]),
    ("guard.state-transition", &["on"], &["off"]),
    ("guard.reviewer-scope", &["on"], &["off"]),
    ("guard-policy", &["strict"], &["relaxed", "off"]),
];

/// `ANSWER_TEXT_DIR`: where the agent writes a person's answer text for
/// `log answer --details-file`, under the record.
const ANSWER_TEXT: &str = "answer-text";

/// The project a call is judged in: its folders, resolved by whoever
/// selected the piece of work, and the host the paths are read on.
#[derive(Debug, Clone, Copy)]
pub struct Project<'a> {
    pub host: &'a Host,
    /// `projectDir`.
    pub dir: &'a str,
    /// `harnessDir()`: the harness's folder, relative to the project.
    pub harness_dir: &'a str,
    /// `spacesRoot(projectDir)`.
    pub spaces_root: &'a str,
    /// `docsRoot(projectDir)`: the selected record, or its space's
    /// `intents` folder; nothing when no selection resolves.
    pub docs_root: Option<&'a str>,
    /// `composerProposalPath(projectDir)`.
    pub composer_proposal: &'a str,
}

impl Project<'_> {
    fn flavour(&self) -> Flavour {
        self.host.flavour()
    }

    /// `resolve(...parts)`.
    fn abs(&self, parts: &[&str]) -> String {
        resolve(self.flavour(), &self.host.cwd, parts)
    }

    /// `relative(from, to)`.
    fn rel(&self, from: &str, to: &str) -> String {
        relative(self.flavour(), &self.host.cwd, from, to)
    }

    /// `isWithinDir`.
    fn within(&self, path: &str, dir: &str) -> bool {
        let rel = self.rel(dir, path);
        rel.is_empty()
            || (!is_absolute(self.flavour(), &rel)
                && rel != ".."
                && !rel.starts_with(&format!("..{}", self.flavour().separator())))
    }

    /// The way from the project down to `target` goes through no link:
    /// `assertNoSymlinkInChainOrThrow(realpathSync(project), relative(project,
    /// target))`. A project that cannot be resolved trusts nothing.
    fn reached_through_no_link(&self, target_abs: &str) -> bool {
        let lexical = self.abs(&[self.dir]);
        real_path(&lexical).is_some_and(|real| {
            chain_is_link_free(Path::new(&real), &self.rel(&lexical, target_abs))
        })
    }

    /// `isOutsideProject`: the path as written lies outside the project,
    /// and so does the real path of its deepest existing ancestor, so a
    /// link from outside into the project counts as inside (a dangling
    /// link resolves to nothing and counts as inside too).
    #[must_use]
    pub fn is_outside(&self, target: &str) -> bool {
        let lexical = self.abs(&[self.dir]);
        let Some(real) = real_path(&lexical) else {
            return false;
        };
        let target_abs = self.abs(&[target]);
        if self.within(&target_abs, &lexical) || self.within(&target_abs, &real) {
            return false;
        }
        let mut suffix: Vec<String> = Vec::new();
        let mut cursor = target_abs;
        loop {
            match std::fs::symlink_metadata(&cursor) {
                Ok(_) => break,
                Err(error) if is_no_entry(&error) => {}
                Err(_) => return false,
            }
            let parent = dirname(self.flavour(), &cursor);
            if parent == cursor {
                return false;
            }
            suffix.insert(0, basename(self.flavour(), &cursor));
            cursor = parent;
        }
        let Some(existing) = real_path(&cursor) else {
            return false;
        };
        let mut parts: Vec<&str> = vec![existing.as_str()];
        parts.extend(suffix.iter().map(String::as_str));
        let real_target = join(self.flavour(), &parts);
        !self.within(&real_target, &lexical) && !self.within(&real_target, &real)
    }

    /// `isStageRecordOutput`: a stage's own record output inside the
    /// records (`focus/spaces`): reached through no link, not hard-linked
    /// to another file, and not the work's state, audit trail or engine
    /// control files, which only the engine writes.
    #[must_use]
    pub fn is_stage_record_output(&self, target: &str) -> bool {
        let target_abs = self.abs(&[target]);
        let inside = self.rel(&self.abs(&[self.spaces_root]), &target_abs);
        if inside.is_empty() || inside.starts_with("..") || is_absolute(self.flavour(), &inside) {
            return false;
        }
        let name = basename(self.flavour(), &target_abs);
        if name == STATE_FILE
            || name == "intents.json"
            || inside
                .split(['\\', '/'])
                .any(|segment| segment == ENGINE_DIR || segment == "audit")
        {
            return false;
        }
        self.reached_through_no_link(&target_abs) && absent_or_plain_file(Path::new(&target_abs))
    }

    /// `isTrustedRecordTarget`: `target` is under `record_dir`, and neither
    /// is reached through a link.
    fn is_trusted_record_target(&self, target: &str, record_dir: &str) -> bool {
        let target_abs = self.abs(&[target]);
        let record_abs = self.abs(&[record_dir]);
        self.reached_through_no_link(&record_abs)
            && self.reached_through_no_link(&target_abs)
            && self.within(&target_abs, &record_abs)
    }

    /// `isAnswerTextTarget`: a plain file in the record folder where the
    /// agent writes a person's answer text. Writing it changes nothing and
    /// builds nothing.
    #[must_use]
    pub fn is_answer_text_target(&self, target: &str) -> bool {
        let Some(record) = self.docs_root.filter(|record| !record.is_empty()) else {
            return false;
        };
        let folder = join(self.flavour(), &[record, ENGINE_DIR, ANSWER_TEXT]);
        self.is_trusted_record_target(target, &folder)
            && absent_or_plain_file(Path::new(&self.abs(&[target])))
    }

    /// `isStageDiaryTarget`: the Code Generation stage's learnings diary
    /// (`<record>/construction/code-generation/memory.md`), exactly, as a
    /// plain file inside the records.
    #[must_use]
    pub fn is_stage_diary_target(&self, target: &str) -> bool {
        let Some(record) = self.docs_root else {
            return false;
        };
        let diary = join(
            self.flavour(),
            &[record, "construction", GUARDED_STAGE, "memory.md"],
        );
        normalize_drive_letter(&self.abs(&[target])) == normalize_drive_letter(&self.abs(&[&diary]))
            && self.is_stage_record_output(target)
    }

    /// `isComposerProposalTarget`: the composer's grid proposal, exactly,
    /// reached through no link and not hard-linked to another file. It is
    /// engine scratch that only `validate-grid` reads.
    #[must_use]
    pub fn is_composer_proposal_target(&self, target: &str) -> bool {
        let target_abs = self.abs(&[target]);
        normalize_drive_letter(&target_abs)
            == normalize_drive_letter(&self.abs(&[self.composer_proposal]))
            && self.reached_through_no_link(&target_abs)
            && absent_or_plain_file(Path::new(&target_abs))
    }

    /// `isTrustedToolFile`: the script is a real file in this harness's
    /// installed tools directory, with no link anywhere in its path, so the
    /// installed tool is what runs.
    fn is_trusted_tool_file(&self, cwd: &str, script: &str) -> bool {
        if script.is_empty() {
            return false;
        }
        let lexical = self.abs(&[self.dir]);
        let absolute = self.abs(&[cwd, script]);
        let tools = self.abs(&[&lexical, self.harness_dir, "tools"]);
        let folder = dirname(self.flavour(), &absolute);
        if !self.rel(&tools, &folder).is_empty() || !self.reached_through_no_link(&absolute) {
            return false;
        }
        // A case-only difference in spelling must name the same directory:
        // a distinct case-sensitive one does not inherit the tool's
        // authority.
        if folder != tools && !same_directory(Path::new(&folder), Path::new(&tools)) {
            return false;
        }
        std::fs::symlink_metadata(&absolute).is_ok_and(|meta| meta.is_file())
    }
}

/// `lstat` with `throwIfNoEntry: false`: the errors that read as nothing
/// there.
fn is_no_entry(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

/// `realpathSync`, spelled as a person reads it: Windows reports a
/// verbatim path, Node's `realpath` does not.
fn real_path(path: &str) -> Option<String> {
    let real = std::fs::canonicalize(path).ok()?;
    let text = real.to_string_lossy();
    Some(match text.strip_prefix(r"\\?\UNC\") {
        Some(share) => format!(r"\\{share}"),
        None => text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned(),
    })
}

/// `codeGenerationGateHeld`: the durable state holds the Code Generation
/// completion gate open: the stage is still current and its checkbox reads
/// awaiting-approval.
#[must_use]
pub fn code_generation_gate_held(state: &str) -> bool {
    normalize_stage_name(focus_state::get_field(state, "Current Stage").unwrap_or_default())
        == GUARDED_STAGE
        && parse_checkboxes(state).iter().any(|entry| {
            entry.slug == GUARDED_STAGE && entry.state == CheckboxState::AwaitingApproval
        })
}

/// `writesThroughOption`: a file a command writes through an option or a
/// second operand, which the lexer does not place among the write targets:
/// it is a write all the same.
#[must_use]
pub fn writes_through_option(name: &str, args: &[String]) -> bool {
    match name {
        "sort" | "git" => args.iter().any(|arg| {
            (name == "sort" && arg == "-o") || arg == "--output" || arg.starts_with("--output=")
        }),
        "uniq" => args.iter().filter(|arg| !arg.starts_with('-')).count() >= 2,
        _ => false,
    }
}

/// `gitSubcommand`: the first word of a git command line that is neither
/// an option nor an option's value.
#[must_use]
pub fn git_subcommand(args: &[String]) -> Option<&str> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if ["-C", "--git-dir", "--work-tree", "--namespace"].contains(&arg) {
            index += 2;
            continue;
        }
        if !arg.starts_with('-') {
            return Some(arg);
        }
        index += 1;
    }
    None
}

/// How a shell command line is read (`ShellDialect`). Every harness keeps
/// the POSIX reading unless its adapter says the command runs in
/// PowerShell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellDialect {
    /// PowerShell's read-only cmdlets and `Set-Location` are real commands.
    pub power_shell: bool,
    /// Command words kept their backslashes. A path then names only the
    /// installed engine: under the POSIX reading `C:\x\cat.exe` was never
    /// `cat`.
    pub paths_as_written: bool,
    /// `focus.cmd` names the engine, as it does on Windows.
    pub engine_paths: bool,
}

/// A plan waiting in a project, and what the ledger says of the person:
/// everything the admissions of a command are judged by.
#[derive(Clone, Copy)]
pub struct PlanWait<'a> {
    pub project: Project<'a>,
    /// [`code_generation_gate_held`] of the durable state.
    pub gate_held: bool,
    /// `personSpokeSinceGate(projectDir, { requests: true })`: a person has
    /// spoken since the last decision, with anything but a question about
    /// a switch.
    pub person_spoke: &'a dyn Fn() -> bool,
    /// `personAskedSinceGate(projectDir)`: the person asked for what the
    /// agent runs now, and no unattended driver stands in for them.
    pub person_asked: &'a dyn Fn() -> bool,
    /// `lastStepAdmitsPersonsMoves(projectDir)`: the work is parked, or the
    /// engine's last step is current and not its recovery question.
    pub last_step_admits: &'a dyn Fn() -> bool,
}

impl PlanWait<'_> {
    /// `recordedSwitchChangeAdmitted`: a recorded switch turned off or back
    /// on, and nothing else: `config flags` with `--bypass` and
    /// `--clear-bypass` pairs, an optional layer, this project, and output
    /// options. Turning a check back on never waits; turning one off is the
    /// person's call.
    fn recorded_switch_change_admitted(&self, args: &[String]) -> bool {
        if args.first().map(String::as_str) != Some("config")
            || args.get(1).map(String::as_str) != Some("flags")
        {
            return false;
        }
        let mut changes = false;
        let mut lowers = false;
        let mut words = args[2..].iter();
        while let Some(arg) = words.next() {
            if arg == "--bypass" || arg == "--clear-bypass" {
                let name = words.next().map_or("", String::as_str);
                if !RECORDABLE_BYPASSES.contains(&name) {
                    return false;
                }
                changes = true;
                lowers |= arg == "--bypass";
            } else if arg == "--project-dir" {
                // The way back the engine prints names this project when it
                // ran elsewhere: the same folder by identity, however it is
                // spelled.
                let Some(dir) = words.next() else {
                    return false;
                };
                if !same_directory(
                    Path::new(&self.project.abs(&[dir])),
                    Path::new(self.project.dir),
                ) {
                    return false;
                }
            } else if !RECORDED_SWITCH_OPTIONS.contains(&arg.as_str()) {
                return false;
            }
        }
        changes && (!lowers || (self.person_asked)())
    }

    /// `personCheckSwitchAllowed`: `config set <key> <value>` switches one
    /// of the person's checks the way the setter would carry it out now:
    /// on always, off only once the person asked since the last decision.
    fn person_check_switch_allowed(&self, key: &str, value: &str) -> bool {
        PERSON_CHECK_SWITCH_VALUES
            .iter()
            .find(|(name, _, _)| *name == key)
            .is_some_and(|(_, on, off)| {
                on.contains(&value) || (off.contains(&value) && (self.person_asked)())
            })
    }

    /// `chatSwitchChangeAdmitted`: a check for this piece of work turned
    /// off or back on with `engine config set`, and nothing else. Plan
    /// approval itself is admitted beside the other prerequisites.
    fn chat_switch_change_admitted(&self, args: &[String]) -> bool {
        args.len() == 5
            && args[0] == "engine"
            && args[1] == "config"
            && args[2] == "set"
            && args[3] != "plan-approval"
            && args[3] != "guard.plan-approval"
            && self.person_check_switch_allowed(&args[3], &args[4])
    }

    /// `planWaitAdmits`: everything admitted while a plan waits, in one
    /// place: the prerequisites, read-only diagnostics, a recorded switch,
    /// a chat switch, a sibling repo, what every helper may run, and what
    /// the engine names. The open question's own answers are the marker's
    /// (M5).
    #[must_use]
    pub fn admits(&self, engine_args: &[String]) -> bool {
        let project = &self.project;
        is_plan_approval_prerequisite(engine_args, self.gate_held, &mut || (self.person_spoke)())
            || is_read_only_diagnostic(engine_args)
            || self.recorded_switch_change_admitted(engine_args)
            || self.chat_switch_change_admitted(engine_args)
            || intent_repo_change_admitted(engine_args, &mut || (self.person_spoke)())
            || every_helper_may_run(
                engine_args,
                project.harness_dir,
                project.dir,
                &project.host.cwd,
            )
            || engine_directed_while_plan_waits(engine_args, &mut || {
                (self.person_spoke)() && (self.last_step_admits)()
            })
    }

    /// `isNativePlanApprovalPrerequisite`: the native engine, by name or
    /// (with `engine_paths`) by its `.cmd` launcher, running a command the
    /// wait admits.
    fn is_native_prerequisite(&self, name: &str, args: &[String], engine_paths: bool) -> bool {
        let command = to_lower_case(name);
        if command == "focus" || command == "focus.exe" {
            return self.admits(&as_engine_route(args));
        }
        // cmd.exe parses a .cmd launcher's arguments again, where these
        // characters expand variables or start another command. An absolute
        // path to the installed engine is the distribution's to recognise.
        engine_paths
            && self.admits(&as_engine_route(args))
            && command == "focus.cmd"
            && args
                .iter()
                .all(|arg| !arg.is_empty() && !re::test(r#"["%&<>^|!\r\n]"#, "", arg))
    }

    /// `isFrameworkToolInvocation`: the engine itself, or one of its
    /// installed tool scripts run by Bun, doing what the wait admits. A
    /// per-tool script gets the verdict of the engine route it implements:
    /// `focus-<route>.ts <args>` is judged as `engine <route> <args>`.
    fn is_framework_tool_invocation(
        &self,
        cwd: &str,
        invocation: &ShellInvocationDetails,
        engine_paths: bool,
    ) -> bool {
        let project = &self.project;
        let name = invocation.executable.as_deref().unwrap_or(&invocation.name);
        let args = &invocation.args;
        // A wrapper (env -C, sudo -D, xargs) can run it against another
        // directory than the one these admissions were judged for.
        let direct = !invocation.executable_resolution_changed
            && !invocation.data_driven
            && invocation.launchers.is_empty();
        if self.is_native_prerequisite(name, args, engine_paths) {
            return direct;
        }
        if normalized_command_name(project.flavour(), name) != "bun" {
            return false;
        }
        if args.iter().any(|arg| {
            arg == "-r"
                || arg == "--require"
                || arg == "--preload"
                || arg.starts_with("--require=")
                || arg.starts_with("--preload=")
        }) {
            return false;
        }
        let script_index = usize::from(args.first().map(String::as_str) == Some("run"));
        let Some(script) = args
            .get(script_index)
            .filter(|script| !script.is_empty() && !script.starts_with('-'))
        else {
            return false;
        };
        let absolute = project.abs(&[cwd, script]);
        let tools = project.abs(&[project.dir, project.harness_dir, "tools"]);
        let file = basename(project.flavour(), &absolute);
        let unified = file == "focus.ts";
        let stem = if unified {
            None
        } else {
            re::find(r"^focus-([A-Za-z0-9._-]+)\.ts$", "", &file)
                .and_then(|found| re::group(&file, &found, 1).map(str::to_owned))
        };
        if !project
            .rel(&tools, &dirname(project.flavour(), &absolute))
            .is_empty()
            || (!unified && stem.is_none())
        {
            return false;
        }
        // Bun by name, run directly. The Bun running the hook, named by its
        // absolute path, is not something Focus's hook has.
        if !["bun", "bun.exe"].contains(&to_lower_case(name).as_str()) || !direct {
            return false;
        }
        let tool_args = &args[script_index + 1..];
        let admitted = match &stem {
            // The unified entry point dispatches both planning and mutation
            // routes: it gets the native planning exceptions only.
            None => self.admits(&as_engine_route(tool_args)) || is_read_only_diagnostic(tool_args),
            Some(stem) => {
                let mut route = vec!["engine".to_owned(), stem.clone()];
                route.extend(tool_args.iter().cloned());
                self.admits(&route) || is_read_only_tool_diagnostic(stem, tool_args)
            }
        };
        admitted && project.is_trusted_tool_file(cwd, script)
    }

    /// `shellInvocationNeedsApproval`: whether one command of a line could
    /// change something the plan governs without the lexer seeing it.
    /// `has_concrete_targets`: the line names files it writes.
    fn invocation_needs_approval(
        &self,
        cwd: &str,
        invocation: &ShellInvocationDetails,
        has_concrete_targets: bool,
        raw_command: &str,
        dialect: ShellDialect,
    ) -> bool {
        let project = &self.project;
        let flavour = project.flavour();
        let name = normalized_command_name(flavour, &invocation.name);
        let executable = invocation.executable.as_deref().unwrap_or(&invocation.name);
        let unwrapped = invocation.launchers.is_empty()
            && !invocation.data_driven
            && !invocation.executable_resolution_changed;
        if dialect.paths_as_written
            && executable.contains(['\\', '/'])
            && !self.is_native_prerequisite(executable, &invocation.args, true)
        {
            return true;
        }
        let set_location = dialect.power_shell && POWERSHELL_SET_LOCATION.contains(&name.as_str());
        if name == "cd" || set_location {
            // The shared lexer is not a full shell parser: no exception
            // where its whitespace or continuation decoding differs.
            if unread_whitespace(raw_command) || raw_command.contains("\\\n") {
                return true;
            }
            // A literal, absolute return to the current directory changes no
            // execution context. Every actual change, wrapper and dynamic
            // operand stays opaque.
            let first = invocation.args.first().map_or("", String::as_str);
            let skip = first == "--"
                || (dialect.power_shell && re::test(r"^-(?:literal)?path$", "i", first));
            let args = &invocation.args[usize::from(skip)..];
            let direct = unwrapped
                && if dialect.power_shell {
                    let spelled = to_lower_case(executable);
                    spelled == "cd" || POWERSHELL_SET_LOCATION.contains(&spelled.as_str())
                } else {
                    executable == "cd"
                };
            let target = args.first().map_or("", String::as_str);
            if !direct
                || args.len() != 1
                || target.is_empty()
                || !is_absolute(flavour, target)
                || target.contains(['*', '?', '[', ']', '{', '}'])
                || target
                    .split(['\\', '/'])
                    .any(|part| part == "." || part == "..")
            {
                return true;
            }
            let current = project.abs(&[cwd]);
            let destination = project.abs(&[target]);
            return !project.rel(&current, &destination).is_empty()
                || !same_directory(Path::new(&current), Path::new(&destination));
        }
        if dialect.power_shell && READ_ONLY_POWERSHELL_CMDLETS.contains(&name.as_str()) {
            // A path, extension or wrapper would name some other program.
            return to_lower_case(executable) != name || !unwrapped;
        }
        if name == "sort" || name == "uniq" {
            return writes_through_option(&name, &invocation.args);
        }
        if READ_ONLY_SHELL_COMMANDS.contains(&name.as_str()) {
            return false;
        }
        if name == "git" {
            if writes_through_option(&name, &invocation.args) {
                return true;
            }
            return match git_subcommand(&invocation.args) {
                Some("branch") => !invocation.args.iter().any(|arg| arg == "--show-current"),
                Some(subcommand) => !READ_ONLY_GIT_SUBCOMMANDS.contains(&subcommand),
                None => true,
            };
        }
        if self.is_framework_tool_invocation(cwd, invocation, dialect.engine_paths) {
            return false;
        }
        !(TRACKED_SHELL_MUTATORS.contains(&name.as_str())
            && has_concrete_targets
            && !invocation
                .args
                .iter()
                .any(|arg| arg.contains(['$', '`', '*', '?'])))
    }

    /// `mutationIntent`: what a tool call would change. `power_shell_hint`:
    /// the adapter that ran the tool says the command runs in PowerShell.
    #[must_use]
    pub fn mutation_intent(
        &self,
        tool_name: &str,
        tool_input: Option<&Map<String, Value>>,
        cwd: &str,
        power_shell_hint: bool,
    ) -> MutationIntent {
        let project = &self.project;
        let mut intent = MutationIntent {
            readable: true,
            ..MutationIntent::default()
        };
        let mut targets: Vec<String> = Vec::new();
        if tool_name == "Bash" {
            let Some(command) = tool_input
                .and_then(|input| input.get("command"))
                .and_then(Value::as_str)
            else {
                return intent;
            };
            intent.shell_command = Some(command.to_owned());
            // A command the adapter ran in PowerShell is read as PowerShell
            // when it is plain. Anything else, and every unhinted command,
            // keeps the POSIX reading.
            let power_shell_command = if power_shell_hint {
                power_shell_reading(command)
            } else {
                None
            };
            let read_as_power_shell = power_shell_command.is_some();
            let analysed = power_shell_command.as_deref().unwrap_or(command);
            let dialect = ShellDialect {
                power_shell: read_as_power_shell,
                paths_as_written: read_as_power_shell,
                engine_paths: read_as_power_shell || project.host.windows,
            };
            targets = shell_write_targets(project.host, analysed, cwd, None, CommandShell::Posix);
            let invocations = shell_command_invocation_details(analysed);
            let dynamic = shell_uses_dynamic_evaluation(analysed)
                || shell_command_alters_executable_resolution(analysed);
            intent.opaque_shell = dynamic
                || invocations.iter().any(|invocation| {
                    self.invocation_needs_approval(
                        cwd,
                        invocation,
                        !targets.is_empty(),
                        analysed,
                        dialect,
                    )
                });
            let flavour = project.flavour();
            intent.runs_engine = invocations.iter().any(|invocation| {
                re::replace(
                    r"\.(?:cmd|ps1)$",
                    "",
                    &normalized_command_name(flavour, &invocation.name),
                    "",
                ) == "focus"
                    || invocation.args.iter().any(|arg| {
                        re::test(
                            r"^focus(?:-[A-Za-z0-9._-]+)?\.ts$",
                            "",
                            &basename(flavour, arg),
                        )
                    })
            }) || (dynamic && re::test(r"\bfocus\b", "i", analysed));
            // An escaped operator is read as the lexer decodes it, which is
            // not trusted here; a variable, substitution or backtick can hide
            // a command or a write target, and a shell script run by name
            // hides commands the lexer never sees.
            intent.readable = !dynamic
                && !unread_whitespace(command)
                && !command.contains("\\\n")
                && !re::test(r"\\[<>&|;]", "", command)
                && !(power_shell_hint && !read_as_power_shell)
                && !invocations.iter().any(|invocation| {
                    let program = normalized_command_name(
                        flavour,
                        invocation.executable.as_deref().unwrap_or(&invocation.name),
                    );
                    SHELL_INTERPRETERS.contains(&program.as_str())
                        || writes_through_option(&program, &invocation.args)
                        || re::test(r"\.(?:ps1|sh|bash|zsh|ksh|cmd|bat)$", "", &program)
                        || (INLINE_CODE_RUNTIMES.contains(&program.as_str())
                            && invocation
                                .args
                                .iter()
                                .any(|arg| INLINE_CODE_FLAGS.contains(&arg.as_str())))
                });
        } else if WRITE_TOOLS.contains(&tool_name) {
            let mut add = |value: Option<&Value>| {
                if let Some(path) = value
                    .and_then(Value::as_str)
                    .filter(|path| !path.is_empty())
                {
                    targets.push(path.to_owned());
                }
            };
            let field = |name: &str| tool_input.and_then(|input| input.get(name));
            add(field("file_path"));
            add(field("notebook_path"));
            add(field("path"));
            for path in field("paths")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                add(Some(path));
            }
        }
        intent.targets = targets
            .iter()
            .map(|target| project.abs(&[cwd, target]))
            .collect();
        intent
    }
}

/// Whitespace the lexer does not decode: anything JavaScript calls
/// whitespace other than a space, a tab or a line feed.
fn unread_whitespace(command: &str) -> bool {
    re::test(r"[^\S \t\n]", "u", command)
}

/// `MutationIntent`: what a tool call would change.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MutationIntent {
    /// The files the call names as written to, absolute.
    pub targets: Vec<String>,
    /// A command on the line could change something the lexer cannot see.
    pub opaque_shell: bool,
    /// The command line, for a shell call.
    pub shell_command: Option<String>,
    /// The command runs the engine itself, or may (a dynamic command
    /// naming it).
    pub runs_engine: bool,
    /// False when the lexer could not read the whole command: a variable
    /// or substitution, whitespace or an operator it does not decode, a
    /// shell or evaluator that runs text it cannot see, a write through an
    /// option, or a PowerShell command not read as plain.
    pub readable: bool,
}

impl MutationIntent {
    /// The object upstream builds, key for key: the engine and an unread
    /// command are said only when they are so.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("targets".to_owned(), json!(self.targets));
        out.insert("opaqueShell".to_owned(), json!(self.opaque_shell));
        out.insert("shellCommand".to_owned(), json!(self.shell_command));
        if self.runs_engine {
            out.insert("runsFocus".to_owned(), json!(true));
        }
        if !self.readable {
            out.insert("readable".to_owned(), json!(false));
        }
        Value::Object(out)
    }
}

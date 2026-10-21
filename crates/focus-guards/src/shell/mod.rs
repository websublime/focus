//! What a shell command may write, and where the shell may be when it
//! writes: the reader the review freeze, runtime integrity and the
//! plan-approval guard put a `Bash` tool call through before deciding.
//!
//! The reader answers conservatively. It does not run the command, model
//! the order of its segments, or evaluate what the shell computes: a word
//! it cannot place is kept as a candidate, a relative target is resolved
//! from every directory a `cd` anywhere in the command could leave the
//! shell in, and a wrapper it cannot read through (`sudo --weird`) names
//! the working directory itself. The guards treat a candidate as a write.
//!
//! Ported from `core/hooks/review-freeze-command.ts`, and held to its
//! answers by the oracle corpus (`shell_invocations`, `shell_targets` and
//! `write_targets` cases). What the host decides — its working directory,
//! its home, whether it is Windows — comes in through [`Host`], so the
//! corpus can ask for any host and the hooks pass the real one.

pub mod cmdlet;
pub mod invocation;
pub mod powershell;
pub mod words;

use std::collections::BTreeSet;

use focus_js::path::{Flavour, basename, is_absolute, join, resolve};
use focus_js::re;
use serde_json::{Map, Value, json};

pub use invocation::{
    ShellInvocation, ShellInvocationDetails, shell_command_alters_executable_resolution,
    shell_command_invocation_details, shell_command_invocations,
};

use cmdlet::{
    PATH_OPTIONS, attached_path_option_values, cmdlet_args, cmdlet_write, cmdlet_write_targets,
};
use invocation::{
    STATIC_MOVE_COMMANDS, STATIC_REMOVE_COMMANDS, find_traversal_roots, invocation_may_mutate,
    parse_shell_args, segment_invocation,
};
use powershell::{
    PIPELINE_PATH_COMMANDS, location_changes, power_shell_command_name, read_power_shell,
};
use words::{shell_command_segments, shell_word_at, without_redirections};

/// The shell a command line is written for. The adapter says when a command
/// runs in PowerShell; every other command is read as POSIX shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandShell {
    Posix,
    PowerShell,
}

/// The host the command is read on: what upstream takes from `process`
/// and `os`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    /// `process.cwd()`.
    pub cwd: String,
    /// `process.platform === "win32"`: `nul` is the null device, PowerShell
    /// takes its homes from the user profile, and paths follow Windows.
    pub windows: bool,
    /// `process.env.HOME`.
    pub home: Option<String>,
    /// `os.homedir()`.
    pub os_home: String,
    /// `process.env.USERPROFILE`, `HOMEDRIVE` and `HOMEPATH`.
    pub user_profile: Option<String>,
    pub home_drive: Option<String>,
    pub home_path: Option<String>,
}

impl Host {
    /// The host this process runs on.
    #[must_use]
    pub fn current() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            cwd: std::env::current_dir()
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default(),
            windows: cfg!(windows),
            home: var("HOME"),
            os_home: std::env::home_dir()
                .map(|dir| dir.to_string_lossy().into_owned())
                .unwrap_or_default(),
            user_profile: var("USERPROFILE"),
            home_drive: var("HOMEDRIVE"),
            home_path: var("HOMEPATH"),
        }
    }

    /// A POSIX host at `cwd` with `home`, for tests and the corpus.
    #[must_use]
    pub fn posix(cwd: &str, home: &str) -> Self {
        Self {
            cwd: cwd.to_owned(),
            windows: false,
            home: Some(home.to_owned()),
            os_home: home.to_owned(),
            user_profile: None,
            home_drive: None,
            home_path: None,
        }
    }

    pub(crate) fn flavour(&self) -> Flavour {
        if self.windows {
            Flavour::Win32
        } else {
            Flavour::Posix
        }
    }

    /// `path.resolve(...parts)` on this host.
    fn resolve(&self, parts: &[&str]) -> String {
        resolve(self.flavour(), &self.cwd, parts)
    }

    /// `shellHomes`: the directories a bare `cd` or a leading `~` or $HOME
    /// name: HOME, else the OS home. PowerShell on Windows takes them from
    /// its user profile instead (USERPROFILE, or HOMEDRIVE and HOMEPATH),
    /// never from HOME.
    fn shell_homes(&self, power_shell: bool) -> Vec<String> {
        if !power_shell || !self.windows {
            return vec![self.resolve(&[self.home.as_deref().unwrap_or(&self.os_home)])];
        }
        let mut homes = vec![
            self.user_profile
                .clone()
                .unwrap_or_else(|| self.os_home.clone()),
        ];
        if let (Some(drive), Some(path)) = (&self.home_drive, &self.home_path) {
            homes.push(format!("{drive}{path}"));
        }
        let mut out: Vec<String> = Vec::new();
        for home in homes {
            let resolved = self.resolve(&[&home]);
            if !out.contains(&resolved) {
                out.push(resolved);
            }
        }
        out
    }

    /// `homeReadings`: a word's readings with a leading `~`, `$HOME` or
    /// `${HOME}` expanded to each home, or none. A PowerShell word also
    /// takes `\` as a separator and its variable names in any case.
    fn home_readings(&self, word: &str, power_shell: bool) -> Vec<String> {
        let text = if power_shell {
            word.replace('\\', "/")
        } else {
            word.to_owned()
        };
        let pattern = r"^(?:~|\$HOME|\$\{HOME\})";
        let Some(lead) = re::find(pattern, if power_shell { "i" } else { "" }, &text) else {
            return Vec::new();
        };
        let rest = &text[lead.range().end..];
        if !rest.is_empty() && !rest.starts_with('/') {
            return Vec::new();
        }
        self.shell_homes(power_shell)
            .iter()
            .map(|home| self.resolve(&[&join(self.flavour(), &[home, rest])]))
            .collect()
    }

    /// `isNullDevice`: output sent to the null device is discarded, never
    /// written to a file.
    fn is_null_device(&self, raw: &str) -> bool {
        raw == "/dev/null" || (self.windows && re::test(r"^nul$", "i", raw))
    }

    /// `normalizeShellTarget`: a target word as an absolute path, `""` for
    /// one the shell would compute.
    fn normalize_shell_target(&self, target: &str, cwd: &str) -> String {
        let flavour = self.flavour();
        let punctuation = |ch: char| ",:[]{}()".contains(ch);
        let mut cleaned = target
            .strip_prefix("of=")
            .unwrap_or(target)
            .trim_start_matches(punctuation)
            .trim_end_matches(punctuation)
            .to_owned();
        if cleaned == "$PWD" || cleaned == "${PWD}" {
            cleaned = cwd.to_owned();
        } else if let Some(rest) = cleaned.strip_prefix("$PWD/") {
            cleaned = join(flavour, &[cwd, rest]);
        } else if let Some(rest) = cleaned.strip_prefix("${PWD}/") {
            cleaned = join(flavour, &[cwd, rest]);
        }
        if cleaned.is_empty() || cleaned.contains(['$', '`', '*', '?']) {
            return String::new();
        }
        if is_absolute(flavour, &cleaned) {
            self.resolve(&[&cleaned])
        } else {
            self.resolve(&[cwd, &cleaned])
        }
    }
}

/// Where a POSIX directory change moves the shell: to a literal operand, to
/// $HOME (a bare `cd` or `chdir`), nowhere (`pushd -n`), or to a directory
/// the command cannot see (`cd -`, pushd's stack, a bare `pushd`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryChange {
    Operand(String),
    Home,
    Stay,
    Unknown,
}

impl DirectoryChange {
    /// The value upstream answers: `{operand}`, `"home"`, `"stay"` or
    /// `"unknown"`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        match self {
            Self::Operand(operand) => json!({ "operand": operand }),
            Self::Home => json!("home"),
            Self::Stay => json!("stay"),
            Self::Unknown => json!("unknown"),
        }
    }
}

/// Commands that change the shell's working directory for the commands
/// after them.
pub const SHELL_DIRECTORY_CHANGES: [&str; 5] = ["cd", "pushd", "chdir", "set-location", "sl"];
const MAX_SHELL_ROOTS: usize = 64;

/// `directoryChange`. A word after `--` is the operand, `-` aside, which is
/// still $OLDPWD. Only pushd reads +N as a stack entry in bash, whose cd
/// takes it as a directory.
fn directory_change(name: &str, args: &[String]) -> DirectoryChange {
    let command = name.to_lowercase();
    let end = args.iter().position(|arg| arg == "--");
    let options = match end {
        Some(end) => &args[..end],
        None => args,
    };
    let pushd = command == "pushd";
    if pushd && options.iter().any(|arg| arg == "-n") {
        return DirectoryChange::Stay;
    }
    let operand = match end {
        Some(end) => args.get(end + 1),
        None => args
            .iter()
            .find(|arg| !arg.starts_with('-') && !(pushd && re::test(r"^\+\d*$", "", arg))),
    };
    if operand.map(String::as_str) == Some("-") {
        return DirectoryChange::Unknown;
    }
    if let Some(operand) = operand {
        return DirectoryChange::Operand(operand.clone());
    }
    let previous = options.iter().any(|arg| re::test(r"^[+-]\d*$", "", arg));
    if !previous && (command == "cd" || command == "chdir") {
        DirectoryChange::Home
    } else {
        DirectoryChange::Unknown
    }
}

/// `shellDirectoryChanges`: where each directory change in a POSIX command
/// moves the shell, in the order written, read with its redirections
/// removed, so `cd 2>/dev/null dir` moves to `dir`.
#[must_use]
pub fn shell_directory_changes(command: &str) -> Vec<DirectoryChange> {
    let mut out = Vec::new();
    for segment in shell_command_segments(command) {
        if let Some(invocation) = segment_invocation(&without_redirections(&segment))
            && SHELL_DIRECTORY_CHANGES.contains(&invocation.name.to_lowercase().as_str())
        {
            out.push(directory_change(&invocation.name, &invocation.args));
        }
    }
    out
}

/// `shellDirectoryRoots`: the directories the shell can be in when the
/// command's writes run: `cwd`, and each literal `cd`/`pushd`/`chdir`/
/// `Set-Location` target resolved from every directory collected anywhere
/// in the command. Past the cap the oldest collected directories are
/// dropped, never `cwd`, a home or the newest.
#[must_use]
pub fn shell_directory_roots(
    host: &Host,
    command: &str,
    cwd: &str,
    shell: CommandShell,
) -> Vec<String> {
    let power_shell = shell == CommandShell::PowerShell;
    let homes = host.shell_homes(power_shell);
    let mut roots = vec![host.resolve(&[cwd])];
    let mut pinned: BTreeSet<String> = roots.iter().cloned().collect();
    let add = |roots: &mut Vec<String>, pinned: &mut BTreeSet<String>, dir: String, pin: bool| {
        if pin {
            pinned.insert(dir.clone());
        }
        match roots.iter().position(|root| *root == dir) {
            Some(0) => return,
            Some(at) => {
                roots.remove(at);
            }
            None => {}
        }
        roots.push(dir);
        while roots.len() - pinned.len() > MAX_SHELL_ROOTS {
            let Some(oldest) = roots.iter().position(|root| !pinned.contains(root)) else {
                break;
            };
            roots.remove(oldest);
        }
    };
    // A literal operand is read from every directory collected so far.
    let change = |roots: &mut Vec<String>, pinned: &mut BTreeSet<String>, operand: &str| {
        let mut next: Vec<String> = Vec::new();
        for root in roots.iter() {
            let dir = host.normalize_shell_target(operand, root);
            if !dir.is_empty() && !next.contains(&dir) {
                next.push(dir);
            }
        }
        for reading in host.home_readings(operand, power_shell) {
            if !next.contains(&reading) {
                next.push(reading);
            }
        }
        for dir in next {
            let pin = homes.contains(&dir);
            add(roots, pinned, dir, pin);
        }
    };
    if power_shell {
        for operand in location_changes(command, 0) {
            match operand {
                None => {
                    for home in &homes {
                        add(&mut roots, &mut pinned, home.clone(), true);
                    }
                }
                Some(operand) => change(&mut roots, &mut pinned, &operand),
            }
        }
        return roots;
    }
    for change_ in shell_directory_changes(command) {
        match change_ {
            DirectoryChange::Operand(operand) => change(&mut roots, &mut pinned, &operand),
            DirectoryChange::Home => {
                for home in &homes {
                    add(&mut roots, &mut pinned, home.clone(), true);
                }
            }
            DirectoryChange::Stay | DirectoryChange::Unknown => {}
        }
    }
    roots
}

/// `shellWriteTargets`: concrete filesystem targets of a mutation-capable
/// shell command. A relative target is resolved from each directory
/// `shell_directory_roots` collects; the reading from `cwd` stays among
/// them. `raw_words` receives every target word as written, before
/// resolution, including the words resolution drops ($VAR, globs).
#[must_use]
pub fn shell_write_targets(
    host: &Host,
    command: &str,
    cwd: &str,
    mut raw_words: Option<&mut Vec<String>>,
    shell: CommandShell,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (index, root) in shell_directory_roots(host, command, cwd, shell)
        .iter()
        .enumerate()
    {
        let raw = if index == 0 {
            raw_words.as_deref_mut()
        } else {
            None
        };
        for target in shell_write_targets_from(host, command, root, raw, shell) {
            if !out.contains(&target) {
                out.push(target);
            }
        }
    }
    out
}

/// The destination/in-place operands one POSIX command writes. Commands
/// that also have read-only source operands contribute only those.
struct Collector<'a> {
    host: &'a Host,
    cwd: &'a str,
    power_shell: bool,
    raw_words: Option<&'a mut Vec<String>>,
    out: Vec<String>,
}

impl Collector<'_> {
    /// PowerShell's file system provider takes \ as a separator on every
    /// host, and its variable names ignore case: $pwd is $PWD.
    fn resolve_target(&self, raw: &str) -> String {
        if self.power_shell {
            let slashed = raw.replace('\\', "/");
            let pwd = re::replace(r"^\$(?:\{pwd\}|pwd)(?=\/|$)", "i", &slashed, "$$PWD");
            self.host.normalize_shell_target(&pwd, self.cwd)
        } else {
            self.host.normalize_shell_target(raw, self.cwd)
        }
    }

    fn add(&mut self, raw: Option<&str>) {
        let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
            return;
        };
        if self.host.is_null_device(raw) || (self.power_shell && re::test(r"^\$null$", "i", raw)) {
            return;
        }
        if let Some(words) = self.raw_words.as_deref_mut() {
            words.push(raw.to_owned());
        }
        let target = self.resolve_target(raw);
        if !target.is_empty() {
            self.out.push(target);
        }
        self.out
            .extend(self.host.home_readings(raw, self.power_shell));
    }

    fn is_directory(&self, raw: Option<&str>) -> bool {
        let Some(raw) = raw.filter(|raw| !raw.is_empty()) else {
            return false;
        };
        let target = self.resolve_target(raw);
        if target.is_empty() {
            return false;
        }
        std::fs::metadata(&target).is_ok_and(|meta| meta.is_dir())
    }

    /// cp/install/mv accept a directory destination. Each concrete child
    /// candidate is added as well as the destination itself, without
    /// consulting the pre-command filesystem, which may not contain the
    /// directory yet.
    fn add_destination(&mut self, destination: Option<&str>, sources: &[String], directory: bool) {
        self.add(destination);
        let Some(destination) = destination.filter(|raw| !raw.is_empty() && directory) else {
            return;
        };
        let destination = self.resolve_target(destination);
        if destination.is_empty() {
            return;
        }
        let flavour = self.host.flavour();
        for raw_source in sources {
            let source = self.resolve_target(raw_source);
            if !source.is_empty() {
                self.add(Some(&join(
                    flavour,
                    &[&destination, &basename(flavour, &source)],
                )));
            }
        }
    }

    /// `invocationTargets`.
    fn invocation_targets(&mut self, command_name: &str, args: &[String]) {
        if command_name == "dd" {
            for arg in args {
                if arg.starts_with("of=") {
                    self.add(Some(arg));
                }
            }
            return;
        }
        if let Some(spec) = cmdlet_write(command_name) {
            for target in cmdlet_write_targets(spec, &cmdlet_args(args), false).targets {
                self.add(Some(&target));
            }
            return;
        }

        let basic = parse_shell_args(args, &[], &[]);
        let operands = basic.operands.clone();
        let attached_paths = attached_path_option_values(args, &PATH_OPTIONS);
        if operands.is_empty() && attached_paths.is_empty() && command_name != "find" {
            return;
        }

        let last = |values: &[String]| values.last().cloned();
        match command_name {
            "cp" | "mv" => {
                let parsed =
                    parse_shell_args(args, &["-S", "-t"], &["--suffix", "--target-directory"]);
                let target_directory = parsed
                    .values("-t")
                    .iter()
                    .chain(parsed.values("--target-directory").iter())
                    .last()
                    .cloned();
                let destination = target_directory.clone().or_else(|| last(&parsed.operands));
                let has_target_directory = target_directory.is_some();
                let sources: Vec<String> = if has_target_directory {
                    parsed.operands.clone()
                } else {
                    parsed.operands[..parsed.operands.len().saturating_sub(1)].to_vec()
                };
                if command_name == "mv" {
                    for source in &sources {
                        self.add(Some(source));
                    }
                }
                let directory = has_target_directory
                    || sources.len() > 1
                    || self.is_directory(destination.as_deref());
                self.add_destination(destination.as_deref(), &sources, directory);
            }
            "install" => {
                let parsed = parse_shell_args(
                    args,
                    &["-g", "-m", "-o", "-S", "-t"],
                    &[
                        "--group",
                        "--mode",
                        "--owner",
                        "--suffix",
                        "--target-directory",
                    ],
                );
                let target_directory = parsed
                    .values("-t")
                    .iter()
                    .chain(parsed.values("--target-directory").iter())
                    .last()
                    .cloned();
                if parsed.options.contains("-d") || parsed.options.contains("--directory") {
                    for operand in &parsed.operands {
                        self.add(Some(operand));
                    }
                } else {
                    let destination = target_directory.clone().or_else(|| last(&parsed.operands));
                    let has_target_directory = target_directory.is_some();
                    let sources: Vec<String> = if has_target_directory {
                        parsed.operands.clone()
                    } else {
                        parsed.operands[..parsed.operands.len().saturating_sub(1)].to_vec()
                    };
                    let directory = has_target_directory
                        || sources.len() > 1
                        || self.is_directory(destination.as_deref());
                    self.add_destination(destination.as_deref(), &sources, directory);
                }
            }
            "rm" | "tee" | "touch" | "truncate" | "unlink" => {
                let parsed = match command_name {
                    "touch" => parse_shell_args(
                        args,
                        &["-d", "-r", "-t"],
                        &["--date", "--reference", "--time"],
                    ),
                    "truncate" => parse_shell_args(args, &["-r", "-s"], &["--reference", "--size"]),
                    _ => basic,
                };
                for operand in &parsed.operands {
                    self.add(Some(operand));
                }
            }
            "sed" => {
                let parsed = parse_shell_args(
                    args,
                    &["-e", "-f", "-l"],
                    &["--expression", "--file", "--line-length"],
                );
                if !parsed.options.contains("-i") && !parsed.options.contains("--in-place") {
                    return;
                }
                let program_from_option = ["-e", "-f", "--expression", "--file"]
                    .iter()
                    .any(|name| parsed.option_values.contains_key(*name));
                for operand in parsed
                    .operands
                    .iter()
                    .skip(usize::from(!program_from_option))
                {
                    self.add(Some(operand));
                }
            }
            "perl" => {
                let parsed = parse_shell_args(args, &["-E", "-F", "-I", "-M", "-e", "-m"], &[]);
                if !parsed.options.contains("-i") && !parsed.options.contains("--in-place") {
                    return;
                }
                let program_from_option = parsed.option_values.contains_key("-e")
                    || parsed.option_values.contains_key("-E");
                for operand in parsed
                    .operands
                    .iter()
                    .skip(usize::from(!program_from_option))
                {
                    self.add(Some(operand));
                }
            }
            "find" => {
                if args.iter().any(|arg| arg == "-delete") {
                    for root in find_traversal_roots(args) {
                        self.add(Some(&root));
                    }
                }
                let mut index = 0;
                while index < args.len() {
                    if ["-fprint", "-fprint0", "-fls"].contains(&args[index].as_str()) {
                        index += 1;
                        self.add(args.get(index).map(String::as_str));
                    } else if args[index] == "-fprintf" {
                        index += 1;
                        self.add(args.get(index).map(String::as_str));
                        index += 1;
                    }
                    index += 1;
                }
            }
            name if STATIC_REMOVE_COMMANDS.contains(&name)
                || STATIC_MOVE_COMMANDS.contains(&name) =>
            {
                for operand in &operands {
                    self.add(Some(operand));
                }
                for value in &attached_paths {
                    self.add(Some(value));
                }
            }
            "copy-item" => {
                self.add(last(&operands).as_deref());
                for value in attached_path_option_values(args, &["destination"]) {
                    self.add(Some(&value));
                }
            }
            "rsync" => {
                let destination = last(&operands);
                self.add(destination.as_deref());
                if basic.options.contains("--remove-source-files") {
                    for source in &operands[..operands.len().saturating_sub(1)] {
                        self.add(Some(source));
                    }
                }
            }
            _ => {}
        }
    }

    /// The PowerShell reading, groups included.
    fn read_power_shell_targets(&mut self, text: &str, depth: usize) {
        if depth > 8 {
            return;
        }
        let mut nested = Vec::new();
        let reading = read_power_shell(text, &mut nested);
        for target in &reading.redirects {
            self.add(Some(target));
        }
        for command in &reading.commands {
            let Some(name) = &command.name else {
                continue;
            };
            let command_name = power_shell_command_name(name, &command.args);
            if let Some(spec) = cmdlet_write(&command_name) {
                let bound = cmdlet_write_targets(spec, &command.args, true);
                for target in &bound.targets {
                    self.add(Some(target));
                }
                if command.piped && !bound.path_bound && spec.pipeline_path {
                    let cwd = self.cwd.to_owned();
                    self.add(Some(&cwd));
                }
                continue;
            }
            // An attached array (-Path:a,b) is one word per value.
            let words: Vec<String> = command
                .args
                .iter()
                .flat_map(|arg| match &arg.parameter {
                    None => arg.values.clone(),
                    Some(parameter) if arg.attached => arg
                        .values
                        .iter()
                        .map(|value| format!("-{parameter}:{value}"))
                        .collect(),
                    Some(parameter) => vec![format!("-{parameter}")],
                })
                .collect();
            let before = self.out.len();
            self.invocation_targets(&command_name, &words);
            // Piped items are what Copy-Item reads: a destination it found
            // is the write. Elsewhere only a path named in full replaces the
            // pipeline's.
            let takes_pipeline = if command_name == "copy-item" {
                self.out.len() == before
            } else {
                !command.args.iter().any(|arg| {
                    arg.parameter.as_deref().is_some_and(|parameter| {
                        re::test(r"^(?:path|literalpath|pspath|lp)$", "i", parameter)
                    })
                })
            };
            if command.piped
                && PIPELINE_PATH_COMMANDS.contains(&command_name.as_str())
                && takes_pipeline
            {
                let cwd = self.cwd.to_owned();
                self.add(Some(&cwd));
            }
        }
        for inner in nested {
            self.read_power_shell_targets(&inner, depth + 1);
        }
    }

    /// Output redirections outside quotes. This catches compact forms such
    /// as `printf x>>file` as well as quoted targets and $PWD-relative paths.
    fn posix_redirections(&mut self, command: &str) {
        let chars: Vec<char> = command.chars().collect();
        let mut quote: Option<char> = None;
        let mut escaped = false;
        let mut i = 0;
        while i < chars.len() {
            let ch = chars[i];
            if escaped {
                escaped = false;
                i += 1;
                continue;
            }
            if ch == '\\' && quote != Some('\'') {
                escaped = true;
                i += 1;
                continue;
            }
            if let Some(open) = quote {
                if ch == open {
                    quote = None;
                }
                i += 1;
                continue;
            }
            if ch == '\'' || ch == '"' {
                quote = Some(ch);
                i += 1;
                continue;
            }
            if ch != '>' {
                i += 1;
                continue;
            }
            let mut target_start = i + 1;
            if matches!(chars.get(target_start), Some('>' | '|')) {
                target_start += 1;
            }
            while chars
                .get(target_start)
                .is_some_and(|next| words::blank(*next))
            {
                target_start += 1;
            }
            // `2>&1` and `2>&-` duplicate/close descriptors; `>&file` writes
            // a file.
            if chars.get(target_start) == Some(&'&') {
                let Some((word, end)) = shell_word_at(&chars, target_start + 1) else {
                    i += 1;
                    continue;
                };
                if re::test(r"^\d+$|^-$", "", &word) {
                    i += 1;
                    continue;
                }
                self.add(Some(&word));
                i = end;
                continue;
            }
            let Some((word, end)) = shell_word_at(&chars, target_start) else {
                i += 1;
                continue;
            };
            self.add(Some(&word));
            i = end;
        }
    }
}

/// `shellWriteTargetsFrom`.
fn shell_write_targets_from(
    host: &Host,
    command: &str,
    cwd: &str,
    raw_words: Option<&mut Vec<String>>,
    shell: CommandShell,
) -> Vec<String> {
    let power_shell = shell == CommandShell::PowerShell;
    let mut collector = Collector {
        host,
        cwd,
        power_shell,
        raw_words,
        out: Vec::new(),
    };
    if power_shell {
        collector.read_power_shell_targets(command, 0);
    } else {
        collector.posix_redirections(command);
        // Parse each command segment independently so a mutator never
        // claims a later read-only command's operands.
        for invocation in shell_command_invocation_details(command) {
            if invocation.ambiguous {
                collector.add(Some(cwd));
                continue;
            }
            if invocation.data_driven && invocation_may_mutate(&invocation.name, &invocation.args) {
                collector.add(Some(cwd));
            }
            collector.invocation_targets(&invocation.name, &invocation.args);
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for target in collector.out {
        if !unique.contains(&target) {
            unique.push(target);
        }
    }
    unique
}

/// The file-writing tools whose targets the freeze inspects. Read-only
/// tools never invalidate a receipt.
const WRITE_TOOLS: [&str; 4] = ["Write", "Edit", "MultiEdit", "NotebookEdit"];

/// `writeTargets`: target paths of a write-tool call.
#[must_use]
pub fn write_targets(
    host: &Host,
    tool_name: &str,
    tool_input: Option<&Map<String, Value>>,
    cwd: &str,
    shell: CommandShell,
) -> Vec<String> {
    if tool_name == "Bash" {
        return match tool_input
            .and_then(|input| input.get("command"))
            .and_then(Value::as_str)
        {
            Some(command) => shell_write_targets(host, command, cwd, None, shell),
            None => Vec::new(),
        };
    }
    if !WRITE_TOOLS.contains(&tool_name) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut push = |value: Option<&Value>| {
        if let Some(text) = value
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            out.push(text.to_owned());
        }
    };
    let empty = Map::new();
    let input = tool_input.unwrap_or(&empty);
    push(input.get("file_path"));
    push(input.get("notebook_path"));
    push(input.get("path"));
    if let Some(paths) = input.get("paths").and_then(Value::as_array) {
        for path in paths {
            push(Some(path));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> Host {
        Host::posix("/work/project", "/home/me")
    }

    #[test]
    fn a_redirection_and_a_mutator_name_their_targets() {
        let targets = shell_write_targets(
            &host(),
            "echo x > out.txt && rm -rf build",
            "/work/project",
            None,
            CommandShell::Posix,
        );
        assert_eq!(targets, ["/work/project/out.txt", "/work/project/build"]);
    }

    #[test]
    fn a_relative_target_is_read_from_every_directory_a_cd_names() {
        let targets = shell_write_targets(
            &host(),
            "cd .kiro && echo x > hooks/y",
            "/work/project",
            None,
            CommandShell::Posix,
        );
        assert_eq!(
            targets,
            ["/work/project/hooks/y", "/work/project/.kiro/hooks/y"]
        );
        assert_eq!(
            shell_directory_roots(&host(), "cd ~/x; cd", "/work/project", CommandShell::Posix),
            [
                "/work/project",
                "/work/project/~/x",
                "/home/me/x",
                "/home/me"
            ]
        );
    }

    #[test]
    fn a_power_shell_cmdlet_binds_its_path() {
        let targets = shell_write_targets(
            &host(),
            "Set-Content -Path 'a b.txt' -Value x",
            "/work/project",
            None,
            CommandShell::PowerShell,
        );
        assert_eq!(targets, ["/work/project/a b.txt"]);
    }

    /// `process.platform === "win32"`: the branches the corpus, recorded on
    /// Linux, cannot reach.
    fn windows() -> Host {
        Host {
            cwd: "C:\\work\\project".to_owned(),
            windows: true,
            home: Some("/home/me".to_owned()),
            os_home: "C:\\Users\\me".to_owned(),
            user_profile: Some("C:\\Users\\me".to_owned()),
            home_drive: Some("D:".to_owned()),
            home_path: Some("\\Homes\\me".to_owned()),
        }
    }

    #[test]
    fn on_windows_power_shell_homes_come_from_the_profile_and_nul_is_the_null_device() {
        let host = windows();
        assert_eq!(host.shell_homes(false), ["C:\\home\\me"]);
        assert_eq!(host.shell_homes(true), ["C:\\Users\\me", "D:\\Homes\\me"]);
        assert!(host.is_null_device("NUL"));
        assert!(!Host::posix("/w", "/home/me").is_null_device("nul"));
        let targets = shell_write_targets(
            &host,
            "echo x > nul; Set-Content -Path $pwd\\a.txt -Value y; cd ~; rm b",
            "C:\\work\\project",
            None,
            CommandShell::PowerShell,
        );
        // `$pwd` is each directory the shell may be in: the project, the
        // literal `~` and both profile homes.
        assert_eq!(
            targets,
            [
                "C:\\work\\project\\a.txt",
                "C:\\work\\project\\b",
                "C:\\work\\project\\~\\a.txt",
                "C:\\work\\project\\~\\b",
                "C:\\Users\\me\\a.txt",
                "C:\\Users\\me\\b",
                "D:\\Homes\\me\\a.txt",
                "D:\\Homes\\me\\b"
            ]
        );
        assert_eq!(
            shell_directory_roots(&host, "cd ~/x", "C:\\work\\project", CommandShell::Posix),
            [
                "C:\\work\\project",
                "C:\\work\\project\\~\\x",
                "C:\\home\\me\\x"
            ]
        );
    }

    #[test]
    fn a_write_tool_names_its_file() {
        let input: Map<String, Value> =
            serde_json::from_str(r#"{"file_path": "/x/y", "paths": ["a", ""]}"#).unwrap();
        assert_eq!(
            write_targets(&host(), "Edit", Some(&input), "/work", CommandShell::Posix),
            ["/x/y", "a"]
        );
        assert!(
            write_targets(&host(), "Read", Some(&input), "/work", CommandShell::Posix).is_empty()
        );
    }
}

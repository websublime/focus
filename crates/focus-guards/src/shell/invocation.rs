//! The program a POSIX segment runs, through the wrappers that launch it
//! (`sudo`, `env`, `xargs`, `timeout`, `command`, shell control words), and
//! the argument parsing the mutators share.
//!
//! Mirrors `shellInvocation` and its helpers, `parseShellArgs`,
//! `findTraversalRoots` and `invocationMayMutate` in
//! `review-freeze-command.ts`.

use std::collections::{BTreeMap, BTreeSet};

use focus_js::re;
use serde_json::{Map, Value, json};

use super::cmdlet::CMDLET_WRITES;
use super::words::{shell_command_segments, shell_words};

/// A program and its arguments, as `shellCommandInvocations` answers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellInvocation {
    pub name: String,
    pub args: Vec<String>,
    pub ambiguous: bool,
}

/// `ShellInvocationDetails`: an invocation with how it was launched.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellInvocationDetails {
    pub name: String,
    pub args: Vec<String>,
    pub ambiguous: bool,
    pub executable: Option<String>,
    pub launchers: Vec<String>,
    pub data_driven: bool,
    pub data_driven_mutation: bool,
    pub executable_resolution_changed: bool,
}

impl ShellInvocationDetails {
    fn ambiguous() -> Self {
        Self {
            ambiguous: true,
            ..Self::default()
        }
    }

    /// The object upstream builds, key for key, for a comparison as text.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("name".to_owned(), json!(self.name));
        out.insert("args".to_owned(), json!(self.args));
        if self.ambiguous {
            out.insert("ambiguous".to_owned(), json!(true));
        }
        if let Some(executable) = &self.executable {
            out.insert("executable".to_owned(), json!(executable));
        }
        if !self.launchers.is_empty() {
            out.insert("launchers".to_owned(), json!(self.launchers));
        }
        if self.data_driven {
            out.insert("dataDriven".to_owned(), json!(true));
        }
        if self.data_driven_mutation {
            out.insert("dataDrivenMutation".to_owned(), json!(true));
        }
        if self.executable_resolution_changed {
            out.insert("executableResolutionChanged".to_owned(), json!(true));
        }
        Value::Object(out)
    }

    /// The invocation without its launch details (`shellCommandInvocations`).
    #[must_use]
    pub fn invocation(&self) -> ShellInvocation {
        ShellInvocation {
            name: self.name.clone(),
            args: self.args.clone(),
            ambiguous: self.ambiguous,
        }
    }
}

impl ShellInvocation {
    /// The object upstream builds, key for key.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut out = Map::new();
        out.insert("name".to_owned(), json!(self.name));
        out.insert("args".to_owned(), json!(self.args));
        if self.ambiguous {
            out.insert("ambiguous".to_owned(), json!(true));
        }
        Value::Object(out)
    }
}

/// `shellExecutableName`: the program's leaf name, a Windows executable
/// suffix dropped, in lower case.
pub(crate) fn shell_executable_name(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let leaf = match normalized.rfind('/') {
        Some(at) => &normalized[at + 1..],
        None => normalized.as_str(),
    };
    re::replace(r"\.(?:exe|com|cmd|bat)$", "i", leaf, "").to_lowercase()
}

#[derive(Default)]
struct WrapperOptionSpec {
    short_values: &'static [&'static str],
    long_values: &'static [&'static str],
    short_optional_values: &'static [&'static str],
    long_optional_values: &'static [&'static str],
    short_flags: &'static [&'static str],
    long_flags: &'static [&'static str],
    numeric_short_value: bool,
}

fn changes_executable_resolution(name: &str) -> bool {
    re::test(r"^(?:PATH|PATHEXT)$", "i", name)
}

/// `consumeWrapperOptions`: the index of the first word after the
/// wrapper's options, or `None` when an option cannot be placed.
fn consume_wrapper_options(
    words: &[String],
    mut index: usize,
    spec: &WrapperOptionSpec,
) -> Option<usize> {
    while index < words.len() {
        let option = words[index].as_str();
        if option == "--" {
            return Some(index + 1);
        }
        if option == "-" || !option.starts_with('-') {
            return Some(index);
        }
        if option.starts_with("--") {
            let name = match option.find('=') {
                Some(equals) => &option[..equals],
                None => option,
            };
            if spec.long_values.contains(&name) {
                if option.contains('=') {
                    index += 1;
                } else if index + 1 < words.len() {
                    index += 2;
                } else {
                    return None;
                }
                continue;
            }
            if spec.long_optional_values.contains(&name) || spec.long_flags.contains(&name) {
                index += 1;
                continue;
            }
            return None;
        }
        if spec.numeric_short_value && re::test(r"^-\d+$", "", option) {
            index += 1;
            continue;
        }
        let name: String = option.chars().take(2).collect();
        if spec.short_values.contains(&name.as_str()) {
            if option.chars().count() > 2 {
                index += 1;
            } else if index + 1 < words.len() {
                index += 2;
            } else {
                return None;
            }
            continue;
        }
        if spec.short_optional_values.contains(&name.as_str()) {
            index += 1;
            continue;
        }
        if option.chars().count() > 1
            && option
                .chars()
                .skip(1)
                .all(|flag| spec.short_flags.contains(&format!("-{flag}").as_str()))
        {
            index += 1;
            continue;
        }
        return None;
    }
    Some(index)
}

fn simple_wrapper(wrapper: &str) -> Option<WrapperOptionSpec> {
    Some(match wrapper {
        "exec" => WrapperOptionSpec {
            short_values: &["-a"],
            short_flags: &["-c", "-l"],
            ..Default::default()
        },
        "nohup" => WrapperOptionSpec {
            long_flags: &["--help", "--version"],
            ..Default::default()
        },
        "nice" => WrapperOptionSpec {
            short_values: &["-n"],
            long_values: &["--adjustment"],
            long_flags: &["--help", "--version"],
            numeric_short_value: true,
            ..Default::default()
        },
        "ionice" => WrapperOptionSpec {
            short_values: &["-c", "-n", "-p", "-P", "-u"],
            long_values: &["--class", "--classdata", "--pid", "--pgid", "--uid"],
            short_flags: &["-t"],
            long_flags: &["--ignore", "--help", "--version"],
            ..Default::default()
        },
        "stdbuf" => WrapperOptionSpec {
            short_values: &["-i", "-o", "-e"],
            long_values: &["--input", "--output", "--error"],
            long_flags: &["--help", "--version"],
            ..Default::default()
        },
        "setsid" => WrapperOptionSpec {
            short_flags: &["-c", "-f", "-w"],
            long_flags: &["--ctty", "--fork", "--wait", "--help", "--version"],
            ..Default::default()
        },
        "sudo" => WrapperOptionSpec {
            short_values: &["-C", "-D", "-g", "-h", "-p", "-r", "-t", "-T", "-u"],
            long_values: &[
                "--chdir",
                "--close-from",
                "--group",
                "--host",
                "--prompt",
                "--role",
                "--type",
                "--user",
            ],
            short_optional_values: &["-E"],
            long_optional_values: &["--preserve-env"],
            short_flags: &[
                "-A", "-b", "-e", "-H", "-K", "-k", "-l", "-n", "-P", "-S", "-s", "-V", "-v",
            ],
            long_flags: &[
                "--askpass",
                "--background",
                "--edit",
                "--help",
                "--login",
                "--non-interactive",
                "--remove-timestamp",
                "--reset-timestamp",
                "--set-home",
                "--shell",
                "--stdin",
                "--validate",
                "--version",
            ],
            numeric_short_value: false,
        },
        "doas" => WrapperOptionSpec {
            short_values: &["-C", "-u"],
            short_flags: &["-L", "-n", "-s"],
            ..Default::default()
        },
        "xargs" => WrapperOptionSpec {
            short_values: &["-a", "-d", "-E", "-I", "-J", "-L", "-n", "-P", "-s"],
            long_values: &[
                "--arg-file",
                "--delimiter",
                "--max-args",
                "--max-procs",
                "--max-chars",
                "--process-slot-var",
            ],
            short_optional_values: &["-e", "-i", "-l"],
            long_optional_values: &["--eof", "--replace", "--max-lines"],
            short_flags: &["-0", "-o", "-p", "-r", "-t", "-x"],
            long_flags: &[
                "--null",
                "--open-tty",
                "--interactive",
                "--no-run-if-empty",
                "--show-limits",
                "--verbose",
                "--exit",
                "--help",
                "--version",
            ],
            numeric_short_value: false,
        },
        "time" => WrapperOptionSpec {
            short_values: &["-f", "-o"],
            long_values: &["--format", "--output"],
            short_flags: &["-a", "-p", "-v"],
            long_flags: &[
                "--append",
                "--portability",
                "--verbose",
                "--help",
                "--version",
            ],
            ..Default::default()
        },
        "unbuffer" => WrapperOptionSpec {
            short_flags: &["-p"],
            ..Default::default()
        },
        _ => return None,
    })
}

const TIMEOUT: WrapperOptionSpec = WrapperOptionSpec {
    short_values: &["-k", "-s"],
    long_values: &["--kill-after", "--signal"],
    short_optional_values: &[],
    long_optional_values: &[],
    short_flags: &[],
    long_flags: &[
        "--foreground",
        "--preserve-status",
        "--verbose",
        "--help",
        "--version",
    ],
    numeric_short_value: false,
};

struct ParseState {
    executable_resolution_changed: bool,
}

/// `shellInvocation`.
fn shell_invocation(
    words: &[String],
    depth: usize,
    mut data_driven: bool,
    launchers: &mut Vec<String>,
    state: &mut ParseState,
) -> Option<ShellInvocationDetails> {
    if depth > 8 {
        return None;
    }
    let mut index = 0;
    let skip_assignments = |index: &mut usize, state: &mut ParseState| {
        while *index < words.len() {
            let Some(found) = re::find(r"^([A-Za-z_][A-Za-z0-9_]*)=", "", &words[*index]) else {
                break;
            };
            if changes_executable_resolution(re::group(&words[*index], &found, 1).unwrap_or("")) {
                state.executable_resolution_changed = true;
            }
            *index += 1;
        }
    };
    skip_assignments(&mut index, state);

    while index < words.len() {
        let wrapper = shell_executable_name(&words[index]);
        if ["}", "fi", "done", "esac"].contains(&wrapper.as_str()) {
            return None;
        }
        if ["{", "then", "else", "do", "!"].contains(&wrapper.as_str()) {
            index += 1;
            skip_assignments(&mut index, state);
            continue;
        }
        if ["for", "select", "case"].contains(&wrapper.as_str()) {
            return None;
        }
        if ["if", "elif", "while", "until"].contains(&wrapper.as_str()) {
            index += 1;
            skip_assignments(&mut index, state);
            continue;
        }
        if wrapper == "command" {
            launchers.push(words[index].clone());
            index += 1;
            while index < words.len() && words[index].starts_with('-') {
                let option = words[index].clone();
                index += 1;
                if option == "--" {
                    break;
                }
                // `command -v/-V` queries a name; it does not execute the
                // following word.
                if option.contains('v') || option.contains('V') {
                    return None;
                }
                if !option.chars().skip(1).all(|flag| flag == 'p') {
                    return Some(ShellInvocationDetails::ambiguous());
                }
            }
            skip_assignments(&mut index, state);
            continue;
        }
        if wrapper == "builtin" {
            launchers.push(words[index].clone());
            index += 1;
            if words.get(index).map(String::as_str) == Some("--") {
                index += 1;
            } else if words.get(index).is_some_and(|word| word.starts_with('-')) {
                return Some(ShellInvocationDetails::ambiguous());
            }
            skip_assignments(&mut index, state);
            continue;
        }
        if wrapper == "env" {
            launchers.push(words[index].clone());
            index += 1;
            let mut split_command: Vec<String> = Vec::new();
            while index < words.len() {
                let option = words[index].as_str();
                if option == "--" {
                    index += 1;
                    break;
                }
                if option == "-S" || option == "--split-string" {
                    let value = words.get(index + 1).filter(|value| !value.is_empty())?;
                    split_command = shell_words(value);
                    index += 2;
                    continue;
                }
                if let Some(value) = option.strip_prefix("--split-string=") {
                    split_command = shell_words(value);
                    index += 1;
                    continue;
                }
                if option.starts_with("-S") && option.chars().count() > 2 {
                    split_command = shell_words(&option[2..]);
                    index += 1;
                    continue;
                }
                if re::test(r"^-(?:u|C|a).+", "", option) {
                    if option.starts_with("-u") && changes_executable_resolution(&option[2..]) {
                        state.executable_resolution_changed = true;
                    }
                    index += 1;
                    continue;
                }
                if re::test(r"^(?:-u|--unset|-C|--chdir|-a|--argv0)$", "", option) {
                    if (option == "-u" || option == "--unset")
                        && changes_executable_resolution(
                            words.get(index + 1).map_or("", String::as_str),
                        )
                    {
                        state.executable_resolution_changed = true;
                    }
                    index += 2;
                    continue;
                }
                if let Some(name) = option.strip_prefix("--unset=") {
                    if changes_executable_resolution(name) {
                        state.executable_resolution_changed = true;
                    }
                    index += 1;
                    continue;
                }
                if re::test(r"^(?:--chdir|--argv0)=", "", option)
                    || option == "-"
                    || option == "-i"
                    || option == "--ignore-environment"
                    || option == "-0"
                    || option == "--null"
                {
                    if option == "-" || option == "-i" || option == "--ignore-environment" {
                        state.executable_resolution_changed = true;
                    }
                    index += 1;
                    continue;
                }
                if re::test(r"^-[i0v]+$", "", option) {
                    if option.contains('i') {
                        state.executable_resolution_changed = true;
                    }
                    index += 1;
                    continue;
                }
                if option == "-v"
                    || option == "--debug"
                    || option == "--list-signal-handling"
                    || option == "--help"
                    || option == "--version"
                    || re::test(
                        r"^(?:--default-signal|--ignore-signal|--block-signal)(?:=.*)?$",
                        "",
                        option,
                    )
                {
                    index += 1;
                    continue;
                }
                if option.starts_with('-') {
                    return Some(ShellInvocationDetails::ambiguous());
                }
                break;
            }
            skip_assignments(&mut index, state);
            if !split_command.is_empty() {
                let mut rest = split_command;
                rest.extend(words[index..].iter().cloned());
                return shell_invocation(&rest, depth + 1, data_driven, launchers, state);
            }
            continue;
        }

        if wrapper == "busybox" || wrapper == "toybox" {
            launchers.push(words[index].clone());
            index += 1;
            if words
                .get(index)
                .is_none_or(|word| word.is_empty() || word.starts_with('-'))
            {
                return Some(ShellInvocationDetails {
                    ambiguous: true,
                    launchers: launchers.clone(),
                    ..Default::default()
                });
            }
            continue;
        }

        if let Some(spec) = simple_wrapper(&wrapper) {
            launchers.push(words[index].clone());
            let Some(consumed) = consume_wrapper_options(words, index + 1, &spec) else {
                return Some(ShellInvocationDetails::ambiguous());
            };
            index = consumed;
            data_driven = data_driven || wrapper == "xargs";
            skip_assignments(&mut index, state);
            continue;
        }

        if wrapper == "timeout" {
            launchers.push(words[index].clone());
            let Some(consumed) = consume_wrapper_options(words, index + 1, &TIMEOUT) else {
                return Some(ShellInvocationDetails::ambiguous());
            };
            index = consumed;
            if index < words.len() {
                index += 1; // the duration
            }
            skip_assignments(&mut index, state);
            continue;
        }

        break;
    }

    let executable = words.get(index).filter(|word| !word.is_empty())?;
    let name = shell_executable_name(executable);
    let args: Vec<String> = words[index + 1..].to_vec();
    let data_driven_mutation = data_driven && invocation_may_mutate(&name, &args);
    Some(ShellInvocationDetails {
        name,
        args,
        ambiguous: false,
        executable: Some(executable.clone()),
        launchers: launchers.clone(),
        data_driven,
        data_driven_mutation,
        executable_resolution_changed: state.executable_resolution_changed,
    })
}

/// `shellInvocation(shellWords(segment))`: the program one segment runs.
pub(crate) fn segment_invocation(segment: &str) -> Option<ShellInvocationDetails> {
    let mut launchers = Vec::new();
    let mut state = ParseState {
        executable_resolution_changed: false,
    };
    shell_invocation(&shell_words(segment), 0, false, &mut launchers, &mut state)
}

/// `shellCommandInvocationDetails`.
#[must_use]
pub fn shell_command_invocation_details(command: &str) -> Vec<ShellInvocationDetails> {
    shell_command_segments(command)
        .iter()
        .filter_map(|segment| segment_invocation(segment))
        .collect()
}

/// `shellCommandAltersExecutableResolution`: a segment sets or unsets
/// `PATH`/`PATHEXT`, or clears the environment.
#[must_use]
pub fn shell_command_alters_executable_resolution(command: &str) -> bool {
    for segment in shell_command_segments(command) {
        let mut launchers = Vec::new();
        let mut state = ParseState {
            executable_resolution_changed: false,
        };
        shell_invocation(&shell_words(&segment), 0, false, &mut launchers, &mut state);
        if state.executable_resolution_changed {
            return true;
        }
    }
    false
}

/// `shellCommandInvocations`.
#[must_use]
pub fn shell_command_invocations(command: &str) -> Vec<ShellInvocation> {
    shell_command_invocation_details(command)
        .iter()
        .map(ShellInvocationDetails::invocation)
        .collect()
}

/// `ParsedShellArgs`.
pub(crate) struct ParsedShellArgs {
    pub operands: Vec<String>,
    pub options: BTreeSet<String>,
    pub option_values: BTreeMap<String, Vec<String>>,
}

impl ParsedShellArgs {
    pub(crate) fn values(&self, name: &str) -> &[String] {
        self.option_values.get(name).map_or(&[], Vec::as_slice)
    }
}

/// `parseShellArgs`: operands, options and the values of the options that
/// take one. Short options may be clustered; a value-taking option consumes
/// the cluster remainder (`-t/tmp`) or the next word (`-t /tmp`).
pub(crate) fn parse_shell_args(
    args: &[String],
    short_value_options: &[&str],
    long_value_options: &[&str],
) -> ParsedShellArgs {
    let mut operands = Vec::new();
    let mut options = BTreeSet::new();
    let mut option_values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut options_ended = false;
    let mut record = |name: &str, value: Option<&String>| {
        if let Some(value) = value {
            option_values
                .entry(name.to_owned())
                .or_default()
                .push(value.clone());
        }
    };
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if !options_ended && arg == "--" {
            options_ended = true;
            i += 1;
            continue;
        }
        if options_ended || arg == "-" || !arg.starts_with('-') {
            operands.push(arg.to_owned());
            i += 1;
            continue;
        }
        if arg.starts_with("--") {
            let equals = arg.find('=');
            let name = equals.map_or(arg, |at| &arg[..at]);
            options.insert(name.to_owned());
            if long_value_options.contains(&name) {
                match equals {
                    Some(at) => record(name, Some(&arg[at + 1..].to_owned())),
                    None => {
                        i += 1;
                        record(name, args.get(i));
                    }
                }
            }
            i += 1;
            continue;
        }
        let chars: Vec<char> = arg.chars().collect();
        for j in 1..chars.len() {
            let name = format!("-{}", chars[j]);
            options.insert(name.clone());
            if !short_value_options.contains(&name.as_str()) {
                continue;
            }
            let attached: String = chars[j + 1..].iter().collect();
            if attached.is_empty() {
                i += 1;
                record(&name, args.get(i));
            } else {
                record(&name, Some(&attached));
            }
            break;
        }
        i += 1;
    }
    ParsedShellArgs {
        operands,
        options,
        option_values,
    }
}

/// `findTraversalRoots`: the starting points of a `find`, `.` when none is
/// written.
pub(crate) fn find_traversal_roots(args: &[String]) -> Vec<String> {
    let mut index = 0;
    let at = |index: usize| args.get(index).map_or("", String::as_str);
    while ["-H", "-L", "-P"].contains(&at(index)) {
        index += 1;
    }
    while at(index).starts_with("-D") {
        if at(index) == "-D" {
            index += 2;
        } else {
            index += 1;
        }
    }
    if re::test(r"^-O\d+$", "", at(index)) {
        index += 1;
    }
    let mut roots = Vec::new();
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "!" || arg == "(" || arg == ")" || arg.starts_with('-') || arg == "," {
            break;
        }
        roots.push(arg.to_owned());
        index += 1;
    }
    if roots.is_empty() {
        vec![".".to_owned()]
    } else {
        roots
    }
}

pub(crate) const STATIC_REMOVE_COMMANDS: [&str; 9] = [
    "rmdir",
    "rd",
    "del",
    "erase",
    "shred",
    "remove-item",
    "clear-item",
    "ri",
    "cli",
];

pub(crate) const STATIC_MOVE_COMMANDS: [&str; 7] = [
    "move",
    "rename",
    "move-item",
    "rename-item",
    "mi",
    "ren",
    "rni",
];

/// `invocationMayMutate`.
pub(crate) fn invocation_may_mutate(command_name: &str, args: &[String]) -> bool {
    if [
        "cp",
        "dd",
        "install",
        "mv",
        "rm",
        "rsync",
        "tee",
        "touch",
        "truncate",
        "unlink",
        "copy-item",
    ]
    .contains(&command_name)
        || STATIC_REMOVE_COMMANDS.contains(&command_name)
        || STATIC_MOVE_COMMANDS.contains(&command_name)
        || CMDLET_WRITES.iter().any(|(name, _)| *name == command_name)
    {
        return true;
    }
    if command_name == "sed" {
        let parsed = parse_shell_args(
            args,
            &["-e", "-f", "-l"],
            &["--expression", "--file", "--line-length"],
        );
        return parsed.options.contains("-i") || parsed.options.contains("--in-place");
    }
    if command_name == "perl" {
        let parsed = parse_shell_args(args, &["-E", "-F", "-I", "-M", "-e", "-m"], &[]);
        return parsed.options.contains("-i") || parsed.options.contains("--in-place");
    }
    command_name == "find"
        && args.iter().any(|arg| {
            ["-delete", "-fprint", "-fprint0", "-fprintf", "-fls"].contains(&arg.as_str())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(command: &str) -> Vec<String> {
        shell_command_invocations(command)
            .into_iter()
            .map(|invocation| invocation.name)
            .collect()
    }

    #[test]
    fn wrappers_are_seen_through() {
        assert_eq!(
            names("sudo -u root rm -rf x; FOO=1 nice -n 5 cp a b"),
            ["rm", "cp"]
        );
        assert_eq!(names("env -S 'mv a b' c"), ["mv"]);
        assert_eq!(names("if true; then touch x; fi"), ["true", "touch"]);
        assert_eq!(names("command -v rm"), Vec::<String>::new());
        assert!(shell_command_alters_executable_resolution("PATH=/x rm y"));
        assert!(!shell_command_alters_executable_resolution("FOO=/x rm y"));
    }

    #[test]
    fn an_unknown_wrapper_option_is_ambiguous() {
        let details = shell_command_invocation_details("sudo --weird rm x");
        assert!(details[0].ambiguous);
        let details = shell_command_invocation_details("xargs -0 rm");
        assert!(details[0].data_driven && details[0].data_driven_mutation);
    }
}

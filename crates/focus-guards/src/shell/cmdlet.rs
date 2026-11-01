//! How a PowerShell cmdlet that writes files binds its arguments, and the
//! paths that binding names.
//!
//! Mirrors `CmdletArg`, `CmdletWrite`, `CMDLET_WRITES`, `cmdletWriteTargets`,
//! `cmdletArgs` and `attachedPathOptionValues` in `review-freeze-command.ts`.

use focus_js::path::{Flavour, join};
use focus_js::re;

/// A PowerShell command argument: a parameter (`-Name`, or `-Name:value`
/// with the value attached) or a value. A value written `a,b` is an array.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CmdletArg {
    pub parameter: Option<String>,
    pub values: Vec<String>,
    pub attached: bool,
    /// The word as written, for a parameter that may turn out to be a value.
    pub written: Option<String>,
}

impl CmdletArg {
    /// The parameter as written, or rebuilt from its name.
    fn as_written(&self) -> String {
        match (&self.written, &self.parameter) {
            (Some(written), _) => written.clone(),
            (None, Some(parameter)) => format!("-{parameter}"),
            (None, None) => String::new(),
        }
    }
}

/// How a cmdlet that writes file content binds its arguments.
pub struct CmdletWrite {
    /// Positional slots in binding order. A slot is filled by its first name,
    /// or by any later name bound explicitly (-LiteralPath fills -Path's slot).
    pub positional: &'static [&'static [&'static str]],
    /// The parameters whose values are the paths written.
    pub paths: &'static [&'static str],
    /// The parameter whose value names a child of each path (New-Item -Name).
    pub child: Option<&'static str>,
    /// A parameter whose set writes no file (Tee-Object -Variable).
    pub fileless: Option<&'static str>,
    /// Other parameters that take a value.
    pub valued: &'static [&'static str],
    /// Parameters that take no value.
    pub switches: &'static [&'static str],
    pub aliases: &'static [(&'static str, &'static str)],
    /// The path can arrive from the pipeline (Get-Item a | Set-Content -Value x).
    pub pipeline_path: bool,
}

const COMMON_VALUED_PARAMETERS: [&str; 10] = [
    "erroraction",
    "errorvariable",
    "informationaction",
    "informationvariable",
    "outbuffer",
    "outvariable",
    "pipelinevariable",
    "progressaction",
    "warningaction",
    "warningvariable",
];
const COMMON_SWITCH_PARAMETERS: [&str; 4] = ["confirm", "debug", "verbose", "whatif"];
const COMMON_PARAMETER_ALIASES: [(&str, &str); 14] = [
    ("cf", "confirm"),
    ("db", "debug"),
    ("ea", "erroraction"),
    ("ev", "errorvariable"),
    ("infa", "informationaction"),
    ("iv", "informationvariable"),
    ("ob", "outbuffer"),
    ("ov", "outvariable"),
    ("proga", "progressaction"),
    ("pv", "pipelinevariable"),
    ("vb", "verbose"),
    ("wa", "warningaction"),
    ("wi", "whatif"),
    ("wv", "warningvariable"),
];

const CONTENT_WRITE: CmdletWrite = CmdletWrite {
    positional: &[&["path", "literalpath"], &["value"]],
    paths: &["path", "literalpath"],
    child: None,
    fileless: None,
    valued: &[
        "value",
        "encoding",
        "filter",
        "include",
        "exclude",
        "credential",
        "stream",
    ],
    switches: &["passthru", "force", "nonewline", "asbytestream"],
    aliases: &[("pspath", "literalpath"), ("lp", "literalpath")],
    pipeline_path: true,
};

/// `CMDLET_WRITES`, in upstream's order.
pub const CMDLET_WRITES: [(&str, &CmdletWrite); 7] = [
    ("set-content", &CONTENT_WRITE),
    ("add-content", &CONTENT_WRITE),
    (
        "clear-content",
        &CmdletWrite {
            positional: &[&["path", "literalpath"]],
            paths: &["path", "literalpath"],
            child: None,
            fileless: None,
            valued: &["filter", "include", "exclude", "credential", "stream"],
            switches: &["force"],
            aliases: &[("pspath", "literalpath"), ("lp", "literalpath")],
            pipeline_path: true,
        },
    ),
    (
        "out-file",
        &CmdletWrite {
            positional: &[&["filepath", "literalpath"], &["encoding"]],
            paths: &["filepath", "literalpath"],
            child: None,
            fileless: None,
            valued: &["encoding", "inputobject", "width"],
            switches: &["append", "force", "noclobber", "nonewline"],
            aliases: &[
                ("path", "filepath"),
                ("pspath", "literalpath"),
                ("lp", "literalpath"),
                ("nooverwrite", "noclobber"),
            ],
            pipeline_path: false,
        },
    ),
    (
        "tee-object",
        &CmdletWrite {
            positional: &[&["filepath", "literalpath"]],
            paths: &["filepath", "literalpath"],
            child: None,
            fileless: Some("variable"),
            valued: &["encoding", "inputobject", "variable"],
            switches: &["append"],
            aliases: &[
                ("path", "filepath"),
                ("pspath", "literalpath"),
                ("lp", "literalpath"),
            ],
            pipeline_path: false,
        },
    ),
    (
        "new-item",
        &CmdletWrite {
            positional: &[&["path"]],
            paths: &["path"],
            child: Some("name"),
            fileless: None,
            valued: &["name", "itemtype", "value", "credential"],
            switches: &["force"],
            aliases: &[("type", "itemtype"), ("target", "value")],
            pipeline_path: true,
        },
    ),
    (
        "set-item",
        &CmdletWrite {
            positional: &[&["path", "literalpath"], &["value"]],
            paths: &["path", "literalpath"],
            child: None,
            fileless: None,
            valued: &["value", "filter", "include", "exclude", "credential"],
            switches: &["force", "passthru"],
            aliases: &[("pspath", "literalpath"), ("lp", "literalpath")],
            pipeline_path: true,
        },
    ),
];

/// The write a cmdlet name binds as, if it is one of `CMDLET_WRITES`.
#[must_use]
pub fn cmdlet_write(name: &str) -> Option<&'static CmdletWrite> {
    CMDLET_WRITES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, spec)| *spec)
}

/// A bare `--`: PowerShell reads every later word as a value.
fn is_end_of_parameters(arg: &CmdletArg) -> bool {
    arg.parameter.is_none() && arg.values.len() == 1 && arg.values[0] == "--"
}

/// What `cmdletWriteTargets` answers.
pub struct CmdletTargets {
    pub targets: Vec<String>,
    pub path_bound: bool,
}

/// `cmdletWriteTargets`: the paths a content cmdlet writes. Named parameters
/// bind by name or unambiguous prefix, so a value passed by name is not a
/// target; every positional value is. With `exact` the quotes are known;
/// a POSIX reading has removed them and every value counts, as well as the
/// paths bound.
#[must_use]
pub fn cmdlet_write_targets(spec: &CmdletWrite, args: &[CmdletArg], exact: bool) -> CmdletTargets {
    let valued: Vec<&str> = spec
        .valued
        .iter()
        .chain(spec.paths.iter())
        .chain(COMMON_VALUED_PARAMETERS.iter())
        .copied()
        .collect();
    let switches: Vec<&str> = spec
        .switches
        .iter()
        .chain(COMMON_SWITCH_PARAMETERS.iter())
        .copied()
        .collect();
    // `new Set([...])` keeps first occurrences: a name is matched once.
    let mut names: Vec<&str> = Vec::new();
    for name in valued.iter().chain(switches.iter()) {
        if !names.contains(name) {
            names.push(name);
        }
    }
    let resolve_name = |written: &str| -> Option<String> {
        let name = written.to_lowercase();
        if valued.contains(&name.as_str()) || switches.contains(&name.as_str()) {
            return Some(name);
        }
        // A cmdlet's own alias shadows a common one.
        if let Some((_, target)) = spec
            .aliases
            .iter()
            .chain(COMMON_PARAMETER_ALIASES.iter())
            .find(|(alias, _)| *alias == name)
        {
            return Some((*target).to_owned());
        }
        let matches: Vec<&&str> = names
            .iter()
            .filter(|candidate| candidate.starts_with(name.as_str()))
            .collect();
        if matches.len() == 1 {
            Some((*matches[0]).to_owned())
        } else {
            None
        }
    };
    let mut bound: Vec<(String, Vec<String>)> = Vec::new();
    let mut positionals: Vec<Vec<String>> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut unknown_parameter = false;
    let mut end_of_parameters = false;
    // Parameters that take a value but met a word that looks like a
    // parameter or nothing: in a dequoted reading each may have been a
    // quoted value.
    let mut starved: Vec<String> = Vec::new();
    let has =
        |bound: &Vec<(String, Vec<String>)>, name: &str| bound.iter().any(|(key, _)| key == name);
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if end_of_parameters || arg.parameter.is_none() {
            // A bare -- ends the parameters: every later word is a value.
            if !end_of_parameters && is_end_of_parameters(arg) {
                end_of_parameters = true;
                index += 1;
                continue;
            }
            // After --, a word that looked like a parameter is the value as
            // written.
            positionals.push(if arg.parameter.is_none() {
                arg.values.clone()
            } else {
                let mut values = vec![arg.as_written()];
                values.extend(arg.values.iter().cloned());
                values
            });
            index += 1;
            continue;
        }
        let Some(name) = resolve_name(arg.parameter.as_deref().unwrap_or("")) else {
            unknown_parameter = true;
            unknown.extend(arg.values.iter().cloned());
            index += 1;
            continue;
        };
        if switches.contains(&name.as_str()) {
            // `-Switch: value` gives the switch the next word, whatever it is.
            if !arg.attached
                && arg
                    .written
                    .as_deref()
                    .is_some_and(|written| written.ends_with(':'))
                && index + 1 < args.len()
            {
                index += 1;
                if !exact {
                    starved.push(arg.written.clone().unwrap_or_default());
                }
            }
            index += 1;
            continue;
        }
        // Dequoted, '-Variable:x' may have been a quoted path on a drive
        // named -Variable, so an attached fileless value leaves the binding
        // in doubt.
        if !exact && arg.attached && Some(name.as_str()) == spec.fileless {
            unknown_parameter = true;
        }
        let mut values = arg.values.clone();
        if !arg.attached {
            let next = args.get(index + 1);
            match next {
                Some(next) if next.parameter.is_none() => {
                    values = next.values.clone();
                    index += 1;
                }
                _ => {
                    values = Vec::new();
                    if !exact {
                        starved.push(arg.as_written());
                    }
                }
            }
        }
        match bound.iter_mut().find(|(key, _)| *key == name) {
            Some((_, existing)) => existing.extend(values),
            None => bound.push((name, values)),
        }
        index += 1;
    }
    let end = args.iter().position(is_end_of_parameters);
    // A dequoted reading knows the binding is fileless only when nothing in
    // it is in doubt; with the quotes known, a bound -Variable settles it.
    if let Some(fileless) = spec.fileless {
        let fileless_bound = bound
            .iter()
            .find(|(key, _)| key == fileless)
            .is_some_and(|(_, values)| !values.is_empty());
        if fileless_bound
            && !spec.paths.iter().any(|name| has(&bound, name))
            && (exact
                || (starved.is_empty()
                    && !unknown_parameter
                    && positionals.is_empty()
                    && end.is_none()))
        {
            return CmdletTargets {
                targets: Vec::new(),
                path_bound: false,
            };
        }
    }
    let mut next = 0;
    for slot in spec.positional {
        if slot.iter().any(|name| has(&bound, name)) {
            continue;
        }
        if next >= positionals.len() {
            break;
        }
        let values = positionals[next].clone();
        next += 1;
        match bound.iter_mut().find(|(key, _)| key == slot[0]) {
            Some((_, existing)) => *existing = values,
            None => bound.push((slot[0].to_owned(), values)),
        }
    }
    let path_bound = spec.paths.iter().any(|name| has(&bound, name));
    let bound_values = |name: &str| -> Vec<String> {
        bound
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, values)| values.clone())
            .unwrap_or_default()
    };
    let mut targets: Vec<String> = spec
        .paths
        .iter()
        .flat_map(|name| bound_values(name))
        .collect();
    if let Some(child) = spec.child
        && let Some((_, children)) = bound.iter().find(|(key, _)| key == child)
    {
        let parents = if targets.is_empty() {
            vec![".".to_owned()]
        } else {
            targets.clone()
        };
        targets = parents
            .iter()
            .flat_map(|parent| {
                children
                    .iter()
                    .map(move |child| join(Flavour::host(), &[parent, child]))
            })
            .collect();
    }
    targets.extend(positionals.iter().flatten().cloned());
    if unknown_parameter {
        targets.extend(unknown);
    }
    if !exact {
        targets.extend(args.iter().flat_map(|arg| arg.values.iter().cloned()));
        targets.extend(starved);
        // A -- the binding gave to a parameter still ends the parameters.
        if let Some(end) = end {
            for arg in &args[end + 1..] {
                if arg.parameter.is_some() {
                    targets.push(arg.as_written());
                }
            }
        }
    }
    CmdletTargets {
        targets,
        path_bound,
    }
}

/// `[...new Set([value, ...value.split(",")])].filter(part => part !== "")`.
fn parts(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in std::iter::once(value).chain(value.split(',')) {
        if !part.is_empty() && !out.iter().any(|seen| seen == part) {
            out.push(part.to_owned());
        }
    }
    out
}

/// An option with its value attached: -name=value, --name:value.
const ATTACHED_OPTION: &str = r"^-{1,2}([^:=]+)[:=](.+)$";

/// `cmdletArgs`: POSIX shell words read as PowerShell arguments. A value is
/// also read as each of its comma-separated parts, since `a,b` may be an
/// array; a value written -Name=x or --Name:x also as x; and -Name.x,
/// -Name(x), -Name[x] or -Name{x} also as what follows the name, where
/// PowerShell ends it.
#[must_use]
pub fn cmdlet_args(words: &[String]) -> Vec<CmdletArg> {
    words
        .iter()
        .map(|word| {
            let Some(found) = re::find(r"^-([A-Za-z_][\w-]*)(?::(.*))?$", "s", word) else {
                let attached_value = re::find(ATTACHED_OPTION, "s", word)
                    .and_then(|found| re::group(word, &found, 2).map(str::to_owned))
                    .or_else(|| {
                        re::find(r"^-[A-Za-z_][\w-]*([.([{].*)$", "s", word)
                            .and_then(|found| re::group(word, &found, 1).map(str::to_owned))
                    });
                let mut values = parts(word);
                if let Some(attached_value) = attached_value {
                    for part in parts(&attached_value) {
                        if !values.contains(&part) {
                            values.push(part);
                        }
                    }
                }
                return CmdletArg {
                    values,
                    ..Default::default()
                };
            };
            let parameter = re::group(word, &found, 1).unwrap_or("").to_owned();
            // `-Name: value` takes the next word, as `-Name value` does.
            match re::group(word, &found, 2).filter(|value| !value.is_empty()) {
                None => CmdletArg {
                    parameter: Some(parameter),
                    values: Vec::new(),
                    attached: false,
                    written: Some(word.clone()),
                },
                Some(value) => CmdletArg {
                    parameter: Some(parameter),
                    values: parts(value),
                    attached: true,
                    written: Some(word.clone()),
                },
            }
        })
        .collect()
}

/// `attachedPathOptionValues`: the values of the path options written with
/// their value attached.
#[must_use]
pub fn attached_path_option_values(args: &[String], path_options: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    for arg in args {
        if let Some(found) = re::find(ATTACHED_OPTION, "s", arg) {
            let name = re::group(arg, &found, 1).unwrap_or("").to_lowercase();
            if path_options.contains(&name.as_str()) {
                out.push(re::group(arg, &found, 2).unwrap_or("").to_owned());
            }
        }
    }
    out
}

/// The path options `attachedPathOptionValues` reads by default.
pub const PATH_OPTIONS: [&str; 5] = ["path", "literalpath", "destination", "newname", "filepath"];

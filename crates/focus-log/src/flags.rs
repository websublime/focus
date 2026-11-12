//! The flags of a `log` verb, and the refusal of a value that reached the
//! engine in pieces (`parseFlags` and `refuseSplitValues` in `aidlc-log.ts`).

use focus_js::{Result, bail, json};
use serde_json::Value;

/// The flags that carry no value.
const VALUELESS: [&str; 4] = ["--single", "--retry-pending", "--stage-level", "--park"];

/// The flags a verb was given, by name without the dashes. A flag given
/// twice keeps its last value.
#[derive(Debug, Default)]
pub struct Flags(Vec<(String, String)>);

impl Flags {
    /// The value of `name`, as last given.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The value of `name` when it is not empty: what upstream reads as
    /// truthy.
    #[must_use]
    pub fn given(&self, name: &str) -> Option<&str> {
        self.get(name).filter(|value| !value.is_empty())
    }
}

/// `parseFlags`: every `--name value` pair, the valueless flags as `true`,
/// and whatever is neither.
///
/// # Errors
///
/// A flag that expects a value is the last argument, or is followed by
/// another flag.
pub fn parse(args: &[String]) -> Result<(Vec<String>, Flags)> {
    let mut positional = Vec::new();
    let mut flags = Flags::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        index += 1;
        let Some(name) = arg.strip_prefix("--") else {
            positional.push(arg.clone());
            continue;
        };
        if VALUELESS.contains(&arg.as_str()) {
            flags.0.push((name.to_owned(), "true".to_owned()));
            continue;
        }
        let Some(value) = args.get(index) else {
            bail!("{arg} expects a value, got end of arguments.");
        };
        if value.starts_with("--") {
            bail!(
                "{arg} expects a value, got another flag: \"{value}\". Did you forget the value?"
            );
        }
        flags.0.push((name.to_owned(), value.clone()));
        index += 1;
    }
    Ok((positional, flags))
}

/// `LOG_INTERACTION_OPTIONS`: what `decision` and `answer` both read.
const INTERACTION_OPTIONS: [&str; 19] = [
    "--project-dir",
    "--stage",
    "--unit",
    "--stage-level",
    "--single",
    "--checkpoint",
    "--session",
    "--questions-file",
    "--message",
    "--batch-file",
    "--command",
    "--command-file",
    "--field",
    "--value",
    "--override",
    "--override-file",
    "--options",
    "--hash-option-labels",
    "--legacy-directive-options",
];
const DECISION_OPTIONS: [&str; 3] = ["--decision", "--rationale", "--exact-option-labels"];
const ANSWER_OPTIONS: [&str; 7] = [
    "--details",
    "--units",
    "--reason",
    "--park",
    "--on-instruction",
    "--details-file",
    "--on-instruction-file",
];

/// The verbs whose every value is the person's or the agent's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interaction {
    Decision,
    Answer,
}

impl Interaction {
    fn verb(self) -> &'static str {
        match self {
            Self::Decision => "decision",
            Self::Answer => "answer",
        }
    }

    fn options(self) -> Vec<&'static str> {
        let own: &[&str] = match self {
            Self::Decision => &DECISION_OPTIONS,
            Self::Answer => &ANSWER_OPTIONS,
        };
        let mut options: Vec<&str> = INTERACTION_OPTIONS.iter().chain(own).copied().collect();
        // Upstream sorts the set as text; every option is ASCII.
        options.sort_unstable();
        options
    }
}

fn quoted(text: &str) -> String {
    json::stringify(&Value::String(text.to_owned()))
}

/// One flag of the raw arguments, its value, and the words that followed
/// the value without a flag of their own.
struct Open<'a> {
    flag: &'a str,
    value: &'a str,
    words: Vec<&'a str>,
}

/// `refuseSplitValues`: `decision` and `answer` read only their flags, so a
/// word outside any flag would vanish and the record would keep a value cut
/// short. A value arrives split when it was not quoted as one argument, or
/// when a shell removed the double quotes inside it. Refused before
/// anything is recorded, over the raw arguments (the `--project-dir` pair
/// included), so the words are named with the flag they really followed.
///
/// # Errors
///
/// A `--` token is not an option of the verb; a word belongs to no flag; a
/// value was followed by words of its own.
pub fn refuse_split_values(interaction: Interaction, raw: &[String]) -> Result<()> {
    let subcommand = interaction.verb();
    let what = match interaction {
        Interaction::Decision => "this decision",
        Interaction::Answer => "this answer",
    };
    let text_flag = match interaction {
        Interaction::Decision => "--decision",
        Interaction::Answer => "--details",
    };
    let answer_file = |holding: &str| -> String {
        if interaction == Interaction::Answer {
            format!(
                " An answer holding {holding} goes in a file instead: write it to <record>/.focus-engine/answer-text/answer.txt with your file tool and pass --details-file .focus-engine/answer-text/answer.txt."
            )
        } else {
            String::new()
        }
    };
    let how_to_pass = |example: &str| -> String {
        format!(
            "Run the command again with each value as one argument, in the person's exact words; in Windows PowerShell write each double quote inside a value as \\\" (for example {text_flag} '{example}'), or as a single quote ('') when the value also holds &, |, <, > or ^.{}",
            answer_file("any of those, or a quote, $, %, ! or a line break,")
        )
    };
    let options = interaction.options();
    let mut first: Option<Open> = None;
    let mut open: Option<Open> = None;
    let mut unattached: Vec<&str> = Vec::new();
    let mut seen_subcommand = false;
    // The first value that words followed is the one named.
    fn close<'a>(open: &mut Option<Open<'a>>, first: &mut Option<Open<'a>>) {
        if let Some(closed) = open.take()
            && first.is_none()
            && !closed.words.is_empty()
        {
            *first = Some(closed);
        }
    }
    let mut index = 0;
    while index < raw.len() {
        let arg = raw[index].as_str();
        index += 1;
        if arg.starts_with("--") {
            if !options.contains(&arg) {
                // A wrong flag is named as one first; the split case keeps
                // its way out second.
                bail!(
                    "Cannot record {what}: {} is not an option of log {subcommand}. It takes {}. If {} is part of a value that a bare double quote split: {}",
                    quoted(arg),
                    options.join(", "),
                    quoted(arg),
                    how_to_pass("Run \\\"todo --help\\\" first")
                );
            }
            close(&mut open, &mut first);
            let next = raw.get(index).map(String::as_str);
            let Some(next) = next else { continue };
            if VALUELESS.contains(&arg) || (next.starts_with("--") && arg != "--project-dir") {
                continue;
            }
            open = Some(Open {
                flag: arg,
                value: next,
                words: Vec::new(),
            });
            index += 1;
        } else if !seen_subcommand && arg == subcommand {
            close(&mut open, &mut first);
            seen_subcommand = true;
        } else if let Some(open) = open.as_mut() {
            open.words.push(arg);
        } else {
            unattached.push(arg);
        }
    }
    close(&mut open, &mut first);
    if !unattached.is_empty() {
        bail!(
            "Cannot record {what}: {} is not the value of any flag. Remove it, or put it right after the flag it belongs to, as one argument.",
            quoted(&unattached.join(" "))
        );
    }
    let Some(first) = first else {
        return Ok(());
    };
    let split = format!(
        "Cannot record {what}: {} arrived as a separate argument after {} {}, so only {} would be recorded. ",
        quoted(&first.words.join(" ")),
        first.flag,
        quoted(first.value),
        quoted(first.value)
    );
    // Command Prompt passes single quotes on as part of the words, so the
    // value's own quotes on both ends name the shell and its fix.
    if first.value.starts_with('\'') && first.words.last().is_some_and(|word| word.ends_with('\''))
    {
        bail!(
            "{split}The single quotes reached Focus as part of the words, so this shell is Command Prompt, where single quotes do not hold words together. Run the command again with the value in double quotes, in the person's exact words (for example {text_flag} \"Chose Option A for auth\").{}",
            answer_file("a double quote, &, |, <, >, ^, % or a line break")
        );
    }
    bail!(
        "{split}A value splits like this when it is not quoted as one argument, or when Windows PowerShell passes a bare double quote inside it (it removes those quotes). {}",
        how_to_pass("Chose \\\"Option A\\\" for auth")
    );
}

//! Minimal argument cursor: positionals in order, `--flag value` anywhere.

/// The arguments not yet consumed.
pub struct Args {
    rest: Vec<String>,
}

impl Args {
    /// The process's arguments.
    ///
    /// # Errors
    ///
    /// An argument is not valid Unicode. Every message the engine prints
    /// quotes its arguments, so one it cannot print is refused up front.
    pub fn from_env() -> Result<Self, String> {
        let rest = std::env::args_os()
            .skip(1)
            .enumerate()
            .map(|(idx, arg)| {
                arg.into_string().map_err(|raw| {
                    format!(
                        "Argument {} is not valid Unicode: {}",
                        idx + 1,
                        raw.to_string_lossy()
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { rest })
    }

    /// Take the next argument, whatever it is.
    pub fn next_positional(&mut self) -> Option<String> {
        if self.rest.is_empty() {
            None
        } else {
            Some(self.rest.remove(0))
        }
    }

    /// Take the next argument that is not a flag.
    pub fn positional(&mut self, label: &str) -> Result<String, String> {
        match self.rest.iter().position(|arg| !arg.starts_with("--")) {
            Some(idx) => Ok(self.rest.remove(idx)),
            None => Err(format!("Missing required argument: <{label}>")),
        }
    }

    /// Whether any argument is `flag`, without consuming it.
    pub fn has(&self, flag: &str) -> bool {
        self.rest.iter().any(|arg| arg == flag)
    }

    /// Take a flag that carries no value; `true` when it was present.
    pub fn flag(&mut self, flag: &str) -> bool {
        match self.rest.iter().position(|arg| arg == flag) {
            Some(idx) => {
                self.rest.remove(idx);
                true
            }
            None => false,
        }
    }

    /// Take `--flag <value>`; `None` when the flag is absent.
    pub fn value(&mut self, flag: &str) -> Result<Option<String>, String> {
        let Some(idx) = self.rest.iter().position(|arg| arg == flag) else {
            return Ok(None);
        };
        if idx + 1 >= self.rest.len() || self.rest[idx + 1].starts_with("--") {
            return Err(format!("Missing required flag: {flag} <value>"));
        }
        self.rest.remove(idx);
        Ok(Some(self.rest.remove(idx)))
    }

    /// Take `--flag <value>`, or `--flag` alone as `Some(None)`.
    pub fn optional_value(&mut self, flag: &str) -> Option<Option<String>> {
        let idx = self.rest.iter().position(|arg| arg == flag)?;
        self.rest.remove(idx);
        let has_value = self
            .rest
            .get(idx)
            .is_some_and(|next| !next.starts_with("--"));
        Some(has_value.then(|| self.rest.remove(idx)))
    }

    /// Take `--flag` and whatever follows it, even another flag; `Some(None)`
    /// when the flag is the last argument. For flags whose handler reports a
    /// wrong value by quoting it.
    pub fn following(&mut self, flag: &str) -> Option<Option<String>> {
        let idx = self.rest.iter().position(|arg| arg == flag)?;
        self.rest.remove(idx);
        Some((idx < self.rest.len()).then(|| self.rest.remove(idx)))
    }

    /// Everything that is left, in order, without taking it.
    pub fn remaining(&self) -> &[String] {
        &self.rest
    }

    /// The next argument, without taking it.
    pub fn peek(&self) -> Option<&str> {
        self.rest.first().map(String::as_str)
    }

    /// Take everything that is left, in order.
    pub fn take_rest(&mut self) -> Vec<String> {
        std::mem::take(&mut self.rest)
    }

    /// Fail on anything left over, so a mistyped flag is never ignored.
    pub fn finish(&self) -> Result<(), String> {
        match self.rest.first() {
            None => Ok(()),
            Some(extra) => Err(format!("Unexpected argument: {extra}")),
        }
    }
}

/// Arguments as upstream's utility tool reads them (`parseArgs` in
/// `aidlc-lib.ts`): positionals in order, `--key value`, `--key=value`, and
/// `--key` alone as `true`; `--` ends the flags.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub positional: Vec<String>,
    /// In the order first given; a flag given twice keeps its last value.
    pub flags: Vec<(String, String)>,
    /// Flags given with no value (`--json`), whose value reads `true`.
    pub bare: Vec<String>,
    /// Flags whose value is blank (`--scope ""`, `--scope=`).
    pub blank: Vec<String>,
}

impl Parsed {
    #[must_use]
    pub fn from_args(args: &[String]) -> Self {
        let mut parsed = Self::default();
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if arg == "--" {
                parsed.positional.extend(args[i + 1..].iter().cloned());
                break;
            }
            if let Some(token) = arg.strip_prefix("--") {
                if let Some((key, value)) = token.split_once('=') {
                    parsed.set(key, value);
                    i += 1;
                    continue;
                }
                match args.get(i + 1) {
                    Some(value) if !value.starts_with("--") => {
                        parsed.set(token, value);
                        i += 2;
                    }
                    _ => {
                        parsed.set(token, "true");
                        parsed.bare.push(token.to_owned());
                        i += 1;
                    }
                }
            } else {
                parsed.positional.push(arg.clone());
                i += 1;
            }
        }
        parsed
    }

    fn set(&mut self, key: &str, value: &str) {
        match self.flags.iter_mut().find(|(known, _)| known == key) {
            Some(known) => known.1 = value.to_owned(),
            None => self.flags.push((key.to_owned(), value.to_owned())),
        }
        if focus_js::text::trim(value).is_empty() {
            self.blank.push(key.to_owned());
        }
    }

    /// The value of a flag, as last given.
    #[must_use]
    pub fn flag(&self, key: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(known, _)| known == key)
            .map(|(_, value)| value.as_str())
    }

    /// A flag given with a value: not bare, not blank.
    #[must_use]
    pub fn valued(&self, key: &str) -> Option<&str> {
        let missing = self.bare.iter().chain(&self.blank).any(|name| name == key);
        if missing { None } else { self.flag(key) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Parsed {
        Parsed::from_args(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn flags_are_read_as_upstreams_utility_reads_them() {
        let parsed = parse(&[
            "detect",
            "--json",
            "--scope=",
            "--label",
            " ",
            "--space",
            "x",
            "--space",
            "y",
            "--",
            "--not-a-flag",
        ]);
        assert_eq!(parsed.positional, ["detect", "--not-a-flag"]);
        assert_eq!(parsed.flag("json"), Some("true"));
        assert_eq!(parsed.flag("space"), Some("y"));
        assert_eq!(parsed.bare, ["json"]);
        assert_eq!(parsed.blank, ["scope", "label"]);
        assert_eq!(parsed.valued("space"), Some("y"));
        assert_eq!(parsed.valued("json"), None);
        assert_eq!(parsed.valued("label"), None);
        // A value that starts with `--` is the next flag; `--k=v` keeps `=`
        // in the value.
        let parsed = parse(&["--a", "--b=c=d", "--c", "-d"]);
        assert_eq!(parsed.flag("a"), Some("true"));
        assert_eq!(parsed.flag("b"), Some("c=d"));
        assert_eq!(parsed.flag("c"), Some("-d"));
        assert_eq!(parsed.bare, ["a"]);
    }
}

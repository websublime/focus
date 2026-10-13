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

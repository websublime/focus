//! The oracle protocol for this crate's share of the corpus (see
//! `focus-graph`'s `oracle` module for the protocol).
//!
//! `node_path` cases run one of Node's path functions, in the POSIX or the
//! Windows flavour, as Bun answers it.

use serde_json::{Value, json};

use crate::path::{Flavour, basename, dirname, is_absolute, join, normalize, relative, resolve};

/// The kinds of case this crate answers.
pub const KINDS: [&str; 1] = ["node_path"];

/// Answer one oracle case. An unknown `kind` answers `{"unsupported": kind}`.
#[must_use]
pub fn run_case(case: &Value) -> Value {
    let text = |key: &str| case.get(key).and_then(Value::as_str).unwrap_or_default();
    match text("kind") {
        "node_path" => {
            let flavour = if text("flavour") == "win32" {
                Flavour::Win32
            } else {
                Flavour::Posix
            };
            let args: Vec<&str> = case["args"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|arg| arg.as_str().unwrap_or_default())
                .collect();
            let first = args.first().copied().unwrap_or_default();
            let value = match text("op") {
                "resolve" => json!(resolve(flavour, text("cwd"), &args)),
                "join" => json!(join(flavour, &args)),
                "normalize" => json!(normalize(flavour, first)),
                "isAbsolute" => json!(is_absolute(flavour, first)),
                "basename" => json!(basename(flavour, first)),
                "dirname" => json!(dirname(flavour, first)),
                "relative" => json!(relative(
                    flavour,
                    text("cwd"),
                    first,
                    args.get(1).copied().unwrap_or_default()
                )),
                other => json!({ "unsupported": other }),
            };
            json!({ "value": value })
        }
        other => json!({ "unsupported": other }),
    }
}

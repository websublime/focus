//! `JSON.parse` and `JSON.stringify`.

use serde_json::{Map, Value};

use crate::num;
use crate::order::js_key_order;

/// `JSON.stringify(value, null, 2)`.
///
/// Byte-identical to JS: `[]` and `{}` stay on one line, control characters
/// use the same short and `\u00xx` escapes, and numbers are laid out as
/// `Number::toString` lays them out (`1`, not `1.0`; `1e+21`, not `1e21`).
#[must_use]
pub fn stringify_pretty(value: &Value) -> String {
    let mut out = String::new();
    write(&mut out, value, Some(0));
    out
}

/// `JSON.stringify(value)`.
#[must_use]
pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write(&mut out, value, None);
    out
}

/// `depth` is the nesting level when pretty-printing, `None` when compact.
fn write(out: &mut String, value: &Value, depth: Option<usize>) {
    let newline = |out: &mut String, level: usize| {
        out.push('\n');
        out.push_str(&"  ".repeat(level));
    };
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n)),
        Value::String(s) => out.push_str(&quote(s)),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                if let Some(level) = depth {
                    newline(out, level + 1);
                }
                write(out, item, depth.map(|level| level + 1));
            }
            if let Some(level) = depth {
                newline(out, level);
            }
            out.push(']');
        }
        Value::Object(map) => {
            if map.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (idx, (key, item)) in map.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                if let Some(level) = depth {
                    newline(out, level + 1);
                }
                out.push_str(&quote(key));
                out.push(':');
                if depth.is_some() {
                    out.push(' ');
                }
                write(out, item, depth.map(|level| level + 1));
            }
            if let Some(level) = depth {
                newline(out, level);
            }
            out.push('}');
        }
    }
}

/// A JS number is always a double, so every JSON number is printed as the
/// double it parses to: `12345678901234567890` prints as
/// `12345678901234567000`.
fn number(n: &serde_json::Number) -> String {
    match n.as_f64() {
        Some(x) if x.is_finite() => num::to_string(x),
        _ => "null".to_owned(),
    }
}

fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("serializing a string into memory cannot fail")
}

/// `JSON.parse(text)`, as far as the result's own shape goes: object keys are
/// in the order a JS object enumerates them (integer-like keys first,
/// ascending), at every level, and a repeated key keeps its first position
/// with its last value.
///
/// Three texts that `JSON.parse` reads are refused here, because the value
/// they stand for is one this reader cannot hold: a number no double holds
/// (`1e400`, `Infinity` to JavaScript), an escaped half of a surrogate pair
/// (`"\ud800"`), and nesting deeper than 127 levels.
///
/// # Errors
///
/// The parser's message when `text` is not JSON, or is one of those three.
/// The wording is `serde_json`'s, not the JS engine's.
pub fn parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str::<Value>(text).map(js_order)
}

fn js_order(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(js_order).collect()),
        Value::Object(map) => Value::Object(js_key_order(
            map.into_iter()
                .map(|(k, v)| (k, js_order(v)))
                .collect::<Map<_, _>>(),
        )),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn pretty_matches_json_stringify_indent_2() {
        let value = json!({
            "slug": "a",
            "empty_list": [],
            "empty_object": {},
            "nested": [{ "k": true }, null, 2],
            "text": "tab\tquote\" back\\ nl\n bell\u{0007} dash \u{2014} del\u{007f}",
        });
        // Captured from `JSON.stringify(value, null, 2)` under Bun.
        let expected = "{\n  \"slug\": \"a\",\n  \"empty_list\": [],\n  \"empty_object\": {},\n  \"nested\": [\n    {\n      \"k\": true\n    },\n    null,\n    2\n  ],\n  \"text\": \"tab\\tquote\\\" back\\\\ nl\\n bell\\u0007 dash \u{2014} del\u{007f}\"\n}";
        assert_eq!(stringify_pretty(&value), expected);
    }

    #[test]
    fn compact_keeps_insertion_order() {
        let value = json!({ "b": 1, "a": [1, 2] });
        assert_eq!(stringify(&value), r#"{"b":1,"a":[1,2]}"#);
    }

    #[test]
    fn numbers_print_as_js_prints_them() {
        // Captured from `JSON.stringify(JSON.parse(text))` under Bun.
        let text =
            "[1.0, 0.30, 62.75, 1e21, 1e-7, -0, 12345678901234567890, 20.499999999999996, 100]";
        let expected = "[1,0.3,62.75,1e+21,1e-7,0,12345678901234567000,20.499999999999996,100]";
        assert_eq!(stringify(&parse(text).unwrap()), expected);
    }

    #[test]
    fn parse_orders_keys_as_a_js_object_does() {
        // Captured from `JSON.stringify(JSON.parse(text))` under Bun.
        let text = r#"{"b":{"10":1,"x":2,"9":3},"2":0,"a":1,"b":{"z":1,"1":2}}"#;
        let expected = r#"{"2":0,"b":{"1":2,"z":1},"a":1}"#;
        assert_eq!(stringify(&parse(text).unwrap()), expected);
    }

    /// Deliberate deviation. `JSON.parse` reads a number no double holds as
    /// `Infinity`, an escaped half of a surrogate pair as that half, and
    /// arrays or objects nested to any depth the stack allows. This reader
    /// holds none of the three, and says the text is not JSON it reads.
    #[test]
    fn three_texts_javascript_reads_are_refused() {
        for text in ["1e400", "[-1e400]", r#""\ud800""#, r#"{"k": "a\udc00b"}"#] {
            assert!(parse(text).is_err(), "{text}");
        }
        let nested = |levels: usize| "[".repeat(levels) + &"]".repeat(levels);
        assert!(parse(&nested(127)).is_ok());
        assert!(parse(&nested(128)).is_err());
        // What is near each of them is read.
        for (text, printed) in [
            ("1e308", "1e+308"),
            ("1e-400", "0"),
            (r#""\ud83d\ude00""#, "\"\u{1F600}\""),
        ] {
            assert_eq!(stringify(&parse(text).unwrap()), printed, "{text}");
        }
    }
}

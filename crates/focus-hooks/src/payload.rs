//! The host's payload: JSON on stdin, read as upstream reads it.

use focus_js::{EngineError, Result};
use serde_json::{Map, Value};

/// `isClaudeCodeHookInput`: a plain object.
#[must_use]
pub fn parse(input: &str) -> Option<Map<String, Value>> {
    match focus_js::json::parse(input) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// `payload.session_id ?? payload.sessionId`, when it is text.
#[must_use]
pub fn session_id(payload: Option<&Map<String, Value>>) -> Option<String> {
    let payload = payload?;
    payload
        .get("session_id")
        .filter(|value| !value.is_null())
        .or_else(|| payload.get("sessionId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// A text field, or the empty string.
#[must_use]
pub fn text<'a>(map: Option<&'a Map<String, Value>>, key: &str) -> &'a str {
    map.and_then(|map| map.get(key))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// A field upstream reads as `value ?? ""` and then calls a string method
/// on: text as it is; an absent, null or falsy value as empty; anything
/// else is a value upstream falls over on (`.match is not a function`),
/// which Focus refuses by name instead.
///
/// # Errors
///
/// The field holds a truthy value that is not text.
pub fn text_field<'a>(
    map: Option<&'a Map<String, Value>>,
    key: &str,
    hook: &str,
) -> Result<&'a str> {
    match map.and_then(|map| map.get(key)) {
        None | Some(Value::Null) => Ok(""),
        Some(Value::String(text)) => Ok(text),
        Some(value) if !truthy(Some(value)) => Ok(""),
        Some(value) => Err(EngineError::new(format!(
            "{hook}: the payload's {key} is not text ({}).",
            focus_js::json::stringify(value)
        ))),
    }
}

/// `Boolean(value)`: what JavaScript takes as true.
#[must_use]
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// `String(value)`.
#[must_use]
pub fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(n) => focus_js::num::to_string(n.as_f64().unwrap_or(0.0)),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(other),
            })
            .collect::<Vec<String>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(text: &str) -> Option<Map<String, Value>> {
        parse(text)
    }

    #[test]
    fn only_a_plain_object_is_a_payload() {
        assert!(map("{}").is_some());
        assert!(map("{\"a\":1}").is_some());
        for text in ["", " ", "null", "3", "\"x\"", "[1]", "{nope", "{\"a\":1},"] {
            assert!(map(text).is_none(), "{text:?}");
        }
    }

    #[test]
    fn the_session_is_read_under_either_name_when_it_is_text() {
        assert_eq!(
            session_id(map("{\"session_id\":\"s1\"}").as_ref()).as_deref(),
            Some("s1")
        );
        assert_eq!(
            session_id(map("{\"sessionId\":\"s2\"}").as_ref()).as_deref(),
            Some("s2")
        );
        assert_eq!(
            session_id(map("{\"session_id\":\"s1\",\"sessionId\":\"s2\"}").as_ref()).as_deref(),
            Some("s1")
        );
        // `??` only passes over null and absence: a number under the first
        // name is taken, and is not text.
        assert_eq!(
            session_id(map("{\"session_id\":5,\"sessionId\":\"s2\"}").as_ref()),
            None
        );
        assert_eq!(
            session_id(map("{\"session_id\":null,\"sessionId\":\"s2\"}").as_ref()).as_deref(),
            Some("s2")
        );
        assert_eq!(session_id(None), None);
    }

    #[test]
    fn a_text_field_is_text_or_empty() {
        let fields = map("{\"a\":\"x\",\"b\":5,\"c\":null}");
        assert_eq!(text(fields.as_ref(), "a"), "x");
        assert_eq!(text(fields.as_ref(), "b"), "");
        assert_eq!(text(fields.as_ref(), "c"), "");
        assert_eq!(text(fields.as_ref(), "d"), "");
        assert_eq!(text(None, "a"), "");
    }

    #[test]
    fn a_field_of_the_wrong_kind_is_refused_unless_falsy() {
        let fields = map(
            "{\"a\":\"x\",\"b\":5,\"c\":null,\"d\":0,\"e\":false,\"f\":[],\"g\":{},\"h\":\"\"}",
        );
        let read = |key: &str| text_field(fields.as_ref(), key, "hook").map(str::to_owned);
        assert_eq!(read("a").unwrap(), "x");
        assert_eq!(read("c").unwrap(), "");
        assert_eq!(read("d").unwrap(), "");
        assert_eq!(read("e").unwrap(), "");
        assert_eq!(read("h").unwrap(), "");
        assert_eq!(read("none").unwrap(), "");
        assert_eq!(
            read("b").unwrap_err().to_string(),
            "hook: the payload's b is not text (5)."
        );
        assert_eq!(
            read("f").unwrap_err().to_string(),
            "hook: the payload's f is not text ([])."
        );
        assert_eq!(
            read("g").unwrap_err().to_string(),
            "hook: the payload's g is not text ({})."
        );
    }

    #[test]
    fn truthiness_is_javascripts() {
        for falsy in [json!(null), json!(false), json!(0), json!(-0.0), json!("")] {
            assert!(!truthy(Some(&falsy)), "{falsy}");
        }
        for truthy_ in [
            json!(true),
            json!(1),
            json!(-1),
            json!(" "),
            json!([]),
            json!({}),
            json!("0"),
        ] {
            assert!(truthy(Some(&truthy_)), "{truthy_}");
        }
        assert!(!truthy(None));
    }

    #[test]
    fn a_value_prints_as_the_runtime_prints_it() {
        assert_eq!(js_string(&json!("x")), "x");
        assert_eq!(js_string(&json!(7)), "7");
        assert_eq!(js_string(&json!(2.5)), "2.5");
        assert_eq!(js_string(&json!(1e21)), "1e+21");
        assert_eq!(js_string(&json!(true)), "true");
        assert_eq!(js_string(&json!(null)), "null");
        assert_eq!(js_string(&json!({"a": 1})), "[object Object]");
        // An array joins its items with commas, a null item as nothing, and
        // a nested array flattens the same way.
        assert_eq!(
            js_string(&json!(["a", null, 2, [["b", "c"], {"k": 1}], false])),
            "a,,2,b,c,[object Object],false"
        );
        assert_eq!(js_string(&json!([])), "");
    }
}

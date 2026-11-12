//! The registry row of a piece of work, as a completion updates it.
//!
//! Mirrors `updateIntentStatus` and `intentRepos` in `aidlc-lib.ts`.

use focus_js::{Result, json};
use focus_workspace::atomic::write_atomic;
use focus_workspace::records::{read_intent_registry, row_names_record};
use focus_workspace::{Layout, links, observer};
use serde_json::Value;

/// `updateIntentStatus`: set the `status` of the record's row to `status`.
/// Whether the file changed: a row already there says so, and a record with
/// no row (the flat legacy layout) changes nothing.
///
/// # Errors
///
/// The process is an observer, the registry sits behind a link, or the file
/// cannot be written.
pub fn update_intent_status(
    layout: &Layout,
    space: &str,
    dir_name: &str,
    status: &str,
) -> Result<bool> {
    let path = layout.intents_dir(space).join("intents.json");
    let mut list = read_intent_registry(layout, space);
    let mut changed = false;
    for row in &mut list {
        if !row_names_record(row, dir_name) {
            continue;
        }
        if row.get("status").and_then(Value::as_str) != Some(status)
            && let Some(object) = row.as_object_mut()
        {
            object.insert("status".to_owned(), Value::String(status.to_owned()));
            changed = true;
        }
        break;
    }
    if changed {
        observer::refuse_write(observer::WRITE_FILE_ATOMIC)?;
        links::refuse_link_on_the_way(&layout.project, &path)?;
        std::fs::create_dir_all(path.parent().unwrap_or(&path))
            .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))?;
        write_atomic(
            &path,
            format!("{}\n", json::stringify_pretty(&Value::Array(list))).as_bytes(),
        )
        .map_err(|err| focus_js::EngineError::new(format!("{}: {err}", path.display())))?;
    }
    Ok(changed)
}

/// `intentRepos`: the repositories the record's row lists; none when it
/// lists none or has no row.
#[must_use]
pub fn intent_repos(layout: &Layout, space: &str, dir_name: Option<&str>) -> Vec<String> {
    let Some(dir_name) = dir_name else {
        return Vec::new();
    };
    read_intent_registry(layout, space)
        .iter()
        .find(|row| row_names_record(row, dir_name))
        .and_then(|row| row.get("repos"))
        .and_then(Value::as_array)
        .map(|repos| {
            repos
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// `intentRepos` for a reader that walks them as source: the same list,
/// and a row whose `repos` is not one is refused.
///
/// # Errors
///
/// The row's `repos` is not a list of names. Upstream takes whatever is
/// there and falls over where it first joins a path, with what its runtime
/// says of the value; an empty name it reads as the project itself, engine
/// and harness included. Focus refuses the row by name.
pub fn source_repos(layout: &Layout, space: &str, dir_name: Option<&str>) -> Result<Vec<String>> {
    let Some(dir_name) = dir_name else {
        return Ok(Vec::new());
    };
    let rows = read_intent_registry(layout, space);
    let listed = rows
        .iter()
        .find(|row| row_names_record(row, dir_name))
        .and_then(|row| row.get("repos"));
    let names: Option<Vec<String>> = match listed {
        None | Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(repos)) => repos
            .iter()
            .map(|name| {
                name.as_str()
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
            })
            .collect(),
        Some(_) => None,
    };
    names.ok_or_else(|| {
        focus_js::EngineError::new(format!(
            "{} lists the repositories of {dir_name} as something that is not a list of names, so the source of that work cannot be told. Fix the row's `repos`: a list of the member repositories' directory names, or nothing for a project that is one repository.",
            layout.intents_dir(space).join("intents.json").display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use focus_harness::Harness;

    #[test]
    fn the_rows_status_is_set_once() {
        let root = std::env::temp_dir().join(format!("focus-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = Layout::new(root.clone(), Harness::Claude);
        let intents = layout.intents_dir("default");
        std::fs::create_dir_all(&intents).unwrap();
        std::fs::write(
            intents.join("intents.json"),
            "[{\"uuid\":\"u\",\"slug\":\"old\",\"dirName\":\"250101-old\",\"status\":\"done\",\"repos\":[\"api\"]},{\"uuid\":\"v\",\"slug\":\"add-login\",\"dirName\":\"261014-add-login\",\"status\":\"in-flight\"}]\n",
        )
        .unwrap();
        assert!(update_intent_status(&layout, "default", "261014-add-login", "complete").unwrap());
        assert!(!update_intent_status(&layout, "default", "261014-add-login", "complete").unwrap());
        assert!(!update_intent_status(&layout, "default", "nope", "complete").unwrap());
        let text = std::fs::read_to_string(intents.join("intents.json")).unwrap();
        assert!(text.contains("\"status\": \"complete\""), "{text}");
        assert!(text.contains("\"status\": \"done\""), "{text}");
        assert_eq!(
            intent_repos(&layout, "default", Some("250101-old")),
            ["api"]
        );
        assert!(intent_repos(&layout, "default", Some("261014-add-login")).is_empty());
        assert!(intent_repos(&layout, "default", None).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Deliberate deviation: a row whose `repos` is not a list of names.
    /// Upstream falls over on it where it first joins a path (what its
    /// runtime says of the value), and reads an empty name as the project
    /// itself. A reader that walks the repositories as source refuses the
    /// row by name.
    #[test]
    fn repositories_that_are_not_a_list_of_names_are_refused() {
        let root =
            std::env::temp_dir().join(format!("focus-registry-repos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = Layout::new(root.clone(), Harness::Claude);
        let intents = layout.intents_dir("default");
        std::fs::create_dir_all(&intents).unwrap();
        let listed = |repos: &str| {
            std::fs::write(
                intents.join("intents.json"),
                format!(
                    "[{{\"uuid\":\"u\",\"slug\":\"old\",\"dirName\":\"250101-old\",\"status\":\"done\",\"repos\":[\"lib\"]}},{{\"uuid\":\"v\",\"slug\":\"add-login\",\"dirName\":\"261014-add-login\",\"status\":\"in-flight\"{repos}}}]\n"
                ),
            )
            .unwrap();
            source_repos(&layout, "default", Some("261014-add-login"))
        };
        assert_eq!(
            listed(",\"repos\":[\"app\",\"api\"]").unwrap(),
            ["app", "api"]
        );
        assert!(listed("").unwrap().is_empty());
        assert!(listed(",\"repos\":null").unwrap().is_empty());
        assert!(listed(",\"repos\":[]").unwrap().is_empty());
        for repos in [
            "[\"app\",7]",
            "[null]",
            "[\"app\",\"\"]",
            "[[\"app\"]]",
            "\"app\"",
            "\"\"",
            "7",
            "{}",
            "false",
        ] {
            let refusal = listed(&format!(",\"repos\":{repos}"))
                .unwrap_err()
                .to_string();
            assert_eq!(
                refusal,
                format!(
                    "{} lists the repositories of 261014-add-login as something that is not a list of names, so the source of that work cannot be told. Fix the row's `repos`: a list of the member repositories' directory names, or nothing for a project that is one repository.",
                    intents.join("intents.json").display()
                ),
                "{repos}"
            );
        }
        assert!(source_repos(&layout, "default", None).unwrap().is_empty());
        assert!(
            source_repos(&layout, "default", Some("nope"))
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

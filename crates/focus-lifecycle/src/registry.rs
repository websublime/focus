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
///
/// # Errors
///
/// The row's `repos` is not a list of text. Upstream takes whatever is
/// there, and what happens depends on who reads it: most readers fall over
/// where they first use a name, with what the runtime says of the value.
/// Focus refuses the row by name, whoever reads it.
pub fn intent_repos(layout: &Layout, space: &str, dir_name: Option<&str>) -> Result<Vec<String>> {
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
            .map(|name| name.as_str().map(str::to_owned))
            .collect(),
        Some(_) => None,
    };
    names.ok_or_else(|| not_names(layout, space, dir_name))
}

fn not_names(layout: &Layout, space: &str, dir_name: &str) -> focus_js::EngineError {
    focus_js::EngineError::new(format!(
        "{} lists the repositories of {dir_name} as something that is not a list of names. Fix the row's `repos`: a list of the member repositories' directory names, or nothing for a project that is one repository.",
        layout.intents_dir(space).join("intents.json").display()
    ))
}

/// `intentRepos` for a reader that walks the repositories as source.
///
/// # Errors
///
/// As [`intent_repos`]; and a name is empty, which upstream reads as the
/// project itself, engine and harness included.
pub fn source_repos(layout: &Layout, space: &str, dir_name: Option<&str>) -> Result<Vec<String>> {
    let repos = intent_repos(layout, space, dir_name)?;
    match dir_name {
        Some(dir_name) if repos.iter().any(String::is_empty) => {
            Err(not_names(layout, space, dir_name))
        }
        _ => Ok(repos),
    }
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
        let repos = |dir_name| intent_repos(&layout, "default", dir_name).unwrap();
        assert_eq!(repos(Some("250101-old")), ["api"]);
        assert!(repos(Some("261014-add-login")).is_empty());
        assert!(repos(None).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Deliberate deviation: a row whose `repos` is not a list of names.
    /// Upstream takes what is there, and most readers fall over where they
    /// first use a name (what the runtime says of the value); an empty name
    /// is the project itself to a reader that walks the repositories as
    /// source. Focus refuses the row by name: what is not text for every
    /// reader, an empty name for the one that walks.
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
            (
                intent_repos(&layout, "default", Some("261014-add-login")),
                source_repos(&layout, "default", Some("261014-add-login")),
            )
        };
        let refusal = format!(
            "{} lists the repositories of 261014-add-login as something that is not a list of names. Fix the row's `repos`: a list of the member repositories' directory names, or nothing for a project that is one repository.",
            intents.join("intents.json").display()
        );
        for (repos, names) in [
            (",\"repos\":[\"app\",\"api\"]", vec!["app", "api"]),
            ("", vec![]),
            (",\"repos\":null", vec![]),
            (",\"repos\":[]", vec![]),
        ] {
            let (read, walked) = listed(repos);
            assert_eq!(read.unwrap(), names, "{repos}");
            assert_eq!(walked.unwrap(), names, "{repos}");
        }
        for repos in [
            "[\"app\",7]",
            "[null]",
            "[[\"app\"]]",
            "\"app\"",
            "\"\"",
            "7",
            "{}",
            "false",
        ] {
            let (read, walked) = listed(&format!(",\"repos\":{repos}"));
            assert_eq!(read.unwrap_err().to_string(), refusal, "{repos}");
            assert_eq!(walked.unwrap_err().to_string(), refusal, "{repos}");
        }
        // An empty name is one a reader may pass over; a walk would take it for the project.
        let (read, walked) = listed(",\"repos\":[\"app\",\"\"]");
        assert_eq!(read.unwrap(), ["app", ""]);
        assert_eq!(walked.unwrap_err().to_string(), refusal);
        assert!(intent_repos(&layout, "default", None).unwrap().is_empty());
        assert!(
            intent_repos(&layout, "default", Some("nope"))
                .unwrap()
                .is_empty()
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

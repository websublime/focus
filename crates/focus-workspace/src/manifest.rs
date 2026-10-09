//! The harness manifest: `<harness>/tools/data/harness.json`.
//!
//! It announces a directory as a harness tree and carries the few settings an
//! install can change. Mirrors `readShippedHarnessData` and
//! `isHarnessShellManifest` in `aidlc-lib.ts` (L314, L440), for the fields
//! Focus reads so far.

use std::path::Path;

use focus_js::text::trim;
use focus_js::{Result, bail, json};
use serde_json::Value;

use crate::names::CLI;

/// Where a harness tree announces itself, relative to the tree.
pub const MANIFEST_REL: [&str; 3] = ["tools", "data", "harness.json"];

/// What Focus reads from a harness manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessManifest {
    /// The plugin selection: `None` when the install selects nothing, which
    /// enables every plugin. `Some` lists the enabled plugins without repeats,
    /// in the order the manifest names them; core is the plugin `focus`, and
    /// leaving it out of the list disables core's stages and scopes.
    pub plugins: Option<Vec<String>>,
}

impl HarnessManifest {
    /// Read the manifest at `path`.
    ///
    /// A missing or unparseable file reads as the defaults: a source checkout
    /// has no manifest, and upstream treats both the same way.
    ///
    /// # Errors
    ///
    /// The file parses but carries retired policy keys, or a `plugins` value
    /// that is not a list of non-empty strings. A half-understood selection
    /// would silently enable or disable stages, so it is refused.
    pub fn read(path: &Path) -> Result<Self> {
        let Ok(bytes) = std::fs::read(path) else {
            return Ok(Self::default());
        };
        let Ok(Value::Object(parsed)) = json::parse(&String::from_utf8_lossy(&bytes)) else {
            return Ok(Self::default());
        };
        let shown = path.display();

        let policy_keys: Vec<&str> = ["models", "flags"]
            .into_iter()
            .filter(|key| parsed.contains_key(*key))
            .collect();
        if !policy_keys.is_empty() {
            let keys = policy_keys.join(", ");
            bail!(
                "{shown}: harness.json contains legacy policy key(s) {keys}. Remove {keys} from {shown}, then run '{CLI} config' to record policy in focus.settings.json."
            );
        }

        let plugins = match parsed.get("plugins") {
            None => None,
            Some(Value::Array(entries)) => {
                let mut names: Vec<String> = Vec::new();
                for (idx, entry) in entries.iter().enumerate() {
                    let name = entry.as_str().map(trim).unwrap_or_default();
                    if name.is_empty() {
                        bail!(
                            "{shown}: harness.json field \"plugins\" entry {idx} must be a non-empty string."
                        );
                    }
                    if !names.iter().any(|seen| seen == name) {
                        names.push(name.to_owned());
                    }
                }
                Some(names)
            }
            Some(_) => bail!(
                "{shown}: harness.json field \"plugins\" must be an array of non-empty strings."
            ),
        };
        Ok(Self { plugins })
    }

    /// Whether `plugin` is enabled under this manifest's selection.
    #[must_use]
    pub fn plugin_enabled(&self, plugin: &str) -> bool {
        self.plugins
            .as_ref()
            .is_none_or(|selected| selected.iter().any(|name| name == plugin))
    }
}

/// The harness name a manifest announces, when `path` is a harness manifest:
/// a JSON object of at most 64 KiB whose `name` is a non-empty string.
///
/// This is the test that tells a harness tree from an ordinary dot-directory
/// that happens to share its name (`.claude` exists in many projects that
/// never installed the engine).
#[must_use]
pub fn announced_name(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() > 64 * 1024 {
        return None;
    }
    let parsed = json::parse(&String::from_utf8_lossy(&bytes)).ok()?;
    let name = parsed.get("name")?.as_str()?;
    (!trim(name).is_empty()).then(|| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(body: &str) -> Result<HarnessManifest> {
        let dir = std::env::temp_dir().join(format!(
            "focus-manifest-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("harness.json");
        std::fs::write(&path, body).unwrap();
        let out = HarnessManifest::read(&path);
        std::fs::remove_dir_all(&dir).unwrap();
        out
    }

    #[test]
    fn no_selection_enables_every_plugin() {
        let m = manifest(r#"{"name":"claude"}"#).unwrap();
        assert_eq!(m.plugins, None);
        assert!(m.plugin_enabled("focus") && m.plugin_enabled("anything"));
    }

    #[test]
    fn a_selection_is_trimmed_and_deduplicated() {
        let m = manifest(r#"{"plugins":[" acme ","acme","focus"]}"#).unwrap();
        assert_eq!(m.plugins, Some(vec!["acme".to_owned(), "focus".to_owned()]));
        assert!(m.plugin_enabled("acme") && !m.plugin_enabled("other"));
        // An empty selection is a selection: nothing is enabled.
        assert!(
            !manifest(r#"{"plugins":[]}"#)
                .unwrap()
                .plugin_enabled("focus")
        );
    }

    #[test]
    fn a_malformed_selection_is_refused() {
        let message = |body| manifest(body).unwrap_err().message().to_owned();
        assert!(
            message(r#"{"plugins":"acme"}"#)
                .ends_with("harness.json field \"plugins\" must be an array of non-empty strings.")
        );
        assert!(
            message(r#"{"plugins":["ok","  "]}"#)
                .ends_with("harness.json field \"plugins\" entry 1 must be a non-empty string.")
        );
        assert!(
            message(r#"{"plugins":[7]}"#)
                .ends_with("harness.json field \"plugins\" entry 0 must be a non-empty string.")
        );
        assert!(message(r#"{"flags":{},"models":{}}"#).contains(
            "harness.json contains legacy policy key(s) models, flags. Remove models, flags from"
        ));
    }

    #[test]
    fn an_unreadable_manifest_reads_as_defaults() {
        assert_eq!(manifest("not json").unwrap(), HarnessManifest::default());
        assert_eq!(manifest("[1]").unwrap(), HarnessManifest::default());
        assert_eq!(
            HarnessManifest::read(Path::new("/nonexistent/harness.json")).unwrap(),
            HarnessManifest::default()
        );
    }
}

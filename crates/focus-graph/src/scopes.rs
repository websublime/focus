//! Scope identity files: one `<owner>-<name>.md` per scope.
//!
//! A scope is a production line through the stage graph. Which stages it runs
//! comes from the stages themselves (`scopes:` in stage frontmatter, transposed
//! into the grid at compile time); this file carries the scope's identity and
//! its defaults. Mirrors `loadScopeMetadataAll` / `loadScopeMetadata` in
//! `aidlc-lib.ts` (L36169–36363).

use std::path::Path;

use focus_frontmatter as fm;
use focus_js::order::{array_index, cmp_utf16};
use focus_js::text::trim;
use focus_js::{Result, bail, re};
use focus_workspace::manifest::HarnessManifest;
use focus_workspace::names::CORE_PLUGIN;

use crate::fsx::{read_dir_sorted, read_text};

/// The per-scope ceremony switches, each `on` or `off`.
pub const CEREMONY_KEYS: [&str; 5] = [
    "sensors",
    "learnings",
    "summary_confirmation",
    "plan_approval",
    "collaborators",
];

/// A scope name becomes part of file names and of commands the engine prints,
/// so it is one word every shell and file system takes as written.
pub const SCOPE_NAME_RE: &str = r"^[A-Za-z0-9][A-Za-z0-9._+@-]*$";
pub const SCOPE_NAME_RULE: &str =
    "letters, digits, and . _ - + @ only, starting with a letter or digit";

#[must_use]
pub fn is_scope_name(name: &str) -> bool {
    re::test(SCOPE_NAME_RE, "", name)
}

/// How strictly guards fence undirected agent work under a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardPolicy {
    Strict,
    Relaxed,
    Off,
}

impl GuardPolicy {
    /// The three values, strictest first.
    pub const VALUES: [GuardPolicy; 3] = [Self::Strict, Self::Relaxed, Self::Off];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Relaxed => "relaxed",
            Self::Off => "off",
        }
    }

    /// Whether this policy holds at least the fences `floor` holds.
    #[must_use]
    pub fn at_least(self, floor: Self) -> bool {
        let rank = |policy: Self| Self::VALUES.iter().position(|v| *v == policy);
        rank(self) <= rank(floor)
    }

    /// `parseGuardPolicy` (L38521): case-insensitive, Markdown emphasis
    /// characters ignored; `None` for anything but the three words.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let cleaned: String = raw
            .to_lowercase()
            .chars()
            .filter(|c| !matches!(c, '`' | '*' | '_'))
            .collect();
        match trim(&cleaned) {
            "strict" => Some(Self::Strict),
            "relaxed" => Some(Self::Relaxed),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// What a scope identity file declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeMetadata {
    pub name: String,
    /// The owning plugin; `None` means core.
    pub plugin: Option<String>,
    pub depth: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub test_strategy: Option<String>,
    pub runner: Option<bool>,
    pub skeleton: bool,
    pub existing_code: bool,
    /// Ceiling on stage reviews: `adversarial`, `advisory` or `none`.
    pub review_cap: Option<String>,
    pub freeform_default: bool,
    pub guard_policy: Option<GuardPolicy>,
    /// Ceremony switches the file sets, as `(key, "on" | "off")`.
    pub ceremony: Vec<(&'static str, String)>,
}

impl ScopeMetadata {
    /// The plugin that owns this scope; core when the file names none.
    #[must_use]
    pub fn owner(&self) -> &str {
        self.plugin.as_deref().unwrap_or(CORE_PLUGIN)
    }

    /// A ceremony switch as the runtime resolves it: on unless the file says
    /// off.
    #[must_use]
    pub fn ceremony_setting(&self, key: &str) -> &str {
        self.ceremony
            .iter()
            .find(|(k, _)| *k == key)
            .map_or("on", |(_, value)| value.as_str())
    }
}

/// Load every scope identity file in `dir`. An unreadable directory reads as
/// no scopes.
///
/// The result is in the order a JS object keyed by scope name enumerates:
/// integer-like names first, ascending, then the rest in file-name order.
/// Upstream keeps the scopes in such an object, and that order reaches the
/// designer export.
///
/// # Errors
///
/// A file without frontmatter or a name, an invalid name or setting value, or
/// two files declaring the same name.
pub fn load_all(dir: &Path) -> Result<Vec<ScopeMetadata>> {
    let files: Vec<String> = read_dir_sorted(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|f| f.ends_with(".md"))
        .collect();
    let mut out: Vec<ScopeMetadata> = Vec::new();
    let mut declared: Vec<(String, String)> = Vec::new();
    for file in files {
        let path = dir.join(&file);
        let shown = path.display().to_string();
        let body = read_text(&path)?;
        let Some(block) = fm::frontmatter_block(&body) else {
            bail!("Scope file missing frontmatter: {shown}");
        };
        let field = |key: &str| fm::scalar_field(block, key);

        let name = field("name");
        if name.is_empty() {
            bail!("Scope file {shown} missing required frontmatter: name");
        }
        if !is_scope_name(&name) {
            bail!(
                "Scope file {shown} has a name a scope cannot have. Rename the scope to {SCOPE_NAME_RULE}."
            );
        }
        if let Some((_, previous)) = declared.iter().find(|(n, _)| *n == name) {
            bail!(
                "Duplicate scope name \"{name}\" in {shown}: already declared in {previous}. Rename one of them."
            );
        }
        declared.push((name.clone(), shown.clone()));

        let plugin = Some(field("plugin")).filter(|p| !p.is_empty());
        if let Some(plugin) = plugin.as_deref().filter(|p| p.starts_with("focus-")) {
            bail!(
                "Scope file {shown} declares plugin \"{plugin}\"; the \"focus-\" prefix is reserved for core (it collides with core runner paths). Rename the plugin."
            );
        }
        let runner = match field("runner").as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        let skeleton = on_off(&shown, "skeleton", &field("skeleton"))?.is_some_and(|v| v == "on");
        let existing_code = match field("existing_code").as_str() {
            "" | "false" => false,
            "true" => true,
            other => bail!(
                "Scope file {shown} has invalid existing_code value \"{other}\". Expected \"true\" or \"false\"."
            ),
        };
        let review_cap = Some(field("review_cap")).filter(|v| !v.is_empty());
        if let Some(cap) = review_cap
            .as_deref()
            .filter(|c| !matches!(*c, "adversarial" | "advisory" | "none"))
        {
            bail!(
                "Scope file {shown} has invalid review_cap value \"{cap}\". Expected \"adversarial\", \"advisory\", or \"none\"."
            );
        }

        // `guard_policy` is the key; `change_control` is its retired spelling,
        // still read. A file naming both must agree.
        let guard_policy_raw = field("guard_policy");
        let change_control_raw = field("change_control");
        for (key, raw) in [
            ("guard_policy", &guard_policy_raw),
            ("change_control", &change_control_raw),
        ] {
            if !raw.is_empty() && GuardPolicy::parse(raw).is_none() {
                bail!(
                    "Scope file {shown} has invalid {key} value \"{raw}\". Expected \"strict\", \"relaxed\", or \"off\"."
                );
            }
        }
        if !guard_policy_raw.is_empty()
            && !change_control_raw.is_empty()
            && guard_policy_raw != change_control_raw
        {
            bail!(
                "Scope file {shown} names both guard_policy (\"{guard_policy_raw}\") and change_control (\"{change_control_raw}\") with different values. Keep guard_policy only."
            );
        }
        let guard_policy = GuardPolicy::parse(if guard_policy_raw.is_empty() {
            &change_control_raw
        } else {
            &guard_policy_raw
        });

        let mut ceremony = Vec::new();
        for key in CEREMONY_KEYS {
            if let Some(value) = on_off(&shown, key, &field(key))? {
                ceremony.push((key, value));
            }
        }

        out.push(ScopeMetadata {
            name,
            plugin,
            depth: field("depth"),
            description: field("description"),
            keywords: fm::list_field(block, "keywords"),
            test_strategy: Some(field("testStrategy")).filter(|v| !v.is_empty()),
            runner,
            skeleton,
            existing_code,
            review_cap,
            freeform_default: field("freeform_default") == "true",
            guard_policy,
            ceremony,
        });
    }
    let (mut indexed, named): (Vec<_>, Vec<_>) = out
        .into_iter()
        .partition(|scope| array_index(&scope.name).is_some());
    indexed.sort_by_key(|scope| array_index(&scope.name));
    Ok(indexed.into_iter().chain(named).collect())
}

/// The scopes a plugin selection leaves enabled (`loadScopeMetadata`).
#[must_use]
pub fn enabled(all: &[ScopeMetadata], manifest: &HarnessManifest) -> Vec<ScopeMetadata> {
    all.iter()
        .filter(|scope| manifest.plugin_enabled(scope.owner()))
        .cloned()
        .collect()
}

/// An `on`/`off` setting: `None` when absent, an error for any other word.
fn on_off(shown: &str, key: &str, value: &str) -> Result<Option<String>> {
    match value {
        "" => Ok(None),
        "on" | "off" => Ok(Some(value.to_owned())),
        other => bail!(
            "Scope file {shown} has invalid {key} value \"{other}\". Expected \"on\" or \"off\"."
        ),
    }
}

/// The invariant `loadScopeMetadata` enforces over the enabled scopes: at most
/// one may nominate itself the freeform default.
///
/// # Errors
///
/// More than one scope sets `freeform_default: true`.
pub fn check_single_freeform_default(scopes: &[ScopeMetadata]) -> Result<()> {
    let mut nominated: Vec<&str> = scopes
        .iter()
        .filter(|s| s.freeform_default)
        .map(|s| s.name.as_str())
        .collect();
    nominated.sort_by(|a, b| cmp_utf16(a, b));
    if nominated.len() > 1 {
        bail!(
            "Multiple enabled scopes declare freeform_default: true ({}). At most one enabled scope may nominate the freeform default.",
            nominated.join(", ")
        );
    }
    Ok(())
}

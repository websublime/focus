//! The method ("memory") files a stage runs under.
//!
//! The layered practice files live at `focus/spaces/<space>/memory/`:
//! `org.md`, `team.md`, `project.md`, and `phases/<phase>.md`. The compile
//! resolves, per stage, the list of files that apply to it and bakes their
//! display paths into the graph. Mirrors `loadRules` / `resolveRulesForStage`
//! in `aidlc-graph.ts` and `aidlc-rule-schema.ts`.

use std::cmp::Ordering;
use std::path::Path;

use focus_frontmatter as fm;
use focus_js::order::locale_compare;
use focus_js::{Result, bail, re};

use crate::fsx::{posix_join, read_dir_sorted, read_text};

/// Where the compiled graph says the method files live. Pinned to the
/// `default` space: these are display paths frozen at package time, not
/// content, and are never re-resolved at runtime.
pub const MEMORY_DISPLAY_ROOT: &str = "focus/spaces/default/memory";

/// The layer a method file belongs to. Variant order is chain order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleScope {
    Org,
    Team,
    Project,
    Phase,
}

impl RuleScope {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Team => "team",
            Self::Project => "project",
            Self::Phase => "phase",
        }
    }
}

/// The optional frontmatter of a method file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleFrontmatter {
    /// `feedforward-only`, or the id of the sensor that checks this rule.
    pub pairing: Option<String>,
    pub status: Option<String>,
    pub stale_after: Option<String>,
}

/// One method file as the compile sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleFile {
    /// Workspace-relative display path, e.g. `focus/spaces/default/memory/org.md`.
    pub path: String,
    pub scope: RuleScope,
    /// The phase a phase-scoped file applies to.
    pub phase: Option<String>,
    pub frontmatter: RuleFrontmatter,
}

/// `parseRuleFrontmatter`: method files usually carry no frontmatter at all,
/// which reads as the default.
#[must_use]
pub fn parse_frontmatter(raw: &str) -> RuleFrontmatter {
    let Some(block) = fm::frontmatter_block(fm::strip_bom(raw)) else {
        return RuleFrontmatter::default();
    };
    let present_or_set = |key: &str| {
        let value = fm::scalar_field(block, key);
        (!value.is_empty() || fm::has_top_level_field(block, key)).then_some(value)
    };
    RuleFrontmatter {
        pairing: Some(fm::scalar_field(block, "pairing")).filter(|v| !v.is_empty()),
        status: present_or_set("status"),
        stale_after: present_or_set("stale_after"),
    }
}

/// `validateRuleFrontmatter`.
///
/// # Errors
///
/// The first violation, prefixed with `file`.
pub fn validate_frontmatter(obj: &RuleFrontmatter, file: &str) -> Result<()> {
    if let Some(pairing) = &obj.pairing {
        if pairing.is_empty() {
            bail!("{file}: pairing must be a non-empty string");
        }
        if pairing != "feedforward-only" && !pairing.starts_with("focus-") {
            bail!(
                "{file}: pairing must be \"feedforward-only\" or start with \"focus-\" (sensor id shape); got \"{pairing}\""
            );
        }
    }
    if let Some(status) = obj
        .status
        .as_deref()
        .filter(|s| !matches!(*s, "active" | "deprecated" | "draft"))
    {
        bail!(
            "{file}: status must be one of \"active\", \"deprecated\", or \"draft\"; got \"{status}\""
        );
    }
    if let Some(stale_after) = obj.stale_after.as_deref().filter(|s| !is_calendar_date(s)) {
        bail!(
            "{file}: stale_after must be a real calendar date in YYYY-MM-DD format; got \"{stale_after}\""
        );
    }
    Ok(())
}

/// `YYYY-MM-DD` naming a day that exists (proleptic Gregorian, as `Date`).
fn is_calendar_date(s: &str) -> bool {
    if !re::test(r"^\d{4}-\d{2}-\d{2}$", "", s) {
        return false;
    }
    let number = |range: std::ops::Range<usize>| s[range].parse::<u32>().unwrap_or(0);
    let (year, month, day) = (number(0..4), number(5..7), number(8..10));
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day)
}

/// Load the method files under `dir` in chain order (org, team, project, then
/// phase files by path). A missing directory reads as no rules.
///
/// # Errors
///
/// An unreadable file or invalid frontmatter.
pub fn load_rules(dir: &Path) -> Result<Vec<RuleFile>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut rules: Vec<RuleFile> = Vec::new();
    let mut load =
        |file_path: &Path, rel: String, scope: RuleScope, phase: Option<String>| -> Result<()> {
            let frontmatter = parse_frontmatter(&read_text(file_path)?);
            validate_frontmatter(&frontmatter, &file_path.display().to_string())?;
            rules.push(RuleFile {
                path: posix_join(&[MEMORY_DISPLAY_ROOT, &rel]),
                scope,
                phase,
                frontmatter,
            });
            Ok(())
        };

    // Anything that is not one of these names is ignored, including a team's
    // own overlay files.
    for name in read_dir_sorted(dir)? {
        let scope = match name.as_str() {
            "org.md" => RuleScope::Org,
            "team.md" => RuleScope::Team,
            "project.md" => RuleScope::Project,
            _ => continue,
        };
        load(&dir.join(&name), name, scope, None)?;
    }
    let phases_dir = dir.join("phases");
    if phases_dir.exists() {
        for name in read_dir_sorted(&phases_dir)? {
            let Some(m) = re::find(r"^([a-z][a-z0-9-]*)\.md$", "", &name) else {
                continue;
            };
            let phase = re::group(&name, &m, 1).map(str::to_owned);
            load(
                &phases_dir.join(&name),
                posix_join(&["phases", &name]),
                RuleScope::Phase,
                phase,
            )?;
        }
    }

    rules.sort_by(|a, b| match a.scope.cmp(&b.scope) {
        Ordering::Equal => locale_compare(&a.path, &b.path),
        other => other,
    });
    Ok(rules)
}

/// The rules a stage runs under: every org, team and project file, plus the
/// phase file matching the stage's phase. Strictly additive: nothing a
/// narrower layer says removes a broader layer's file.
#[must_use]
pub fn resolve_for_phase<'a>(phase: &str, rules: &'a [RuleFile]) -> Vec<&'a RuleFile> {
    rules
        .iter()
        .filter(|rule| rule.scope != RuleScope::Phase || rule.phase.as_deref() == Some(phase))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_dates() {
        assert!(is_calendar_date("2026-02-28"));
        assert!(is_calendar_date("2024-02-29"));
        assert!(!is_calendar_date("2026-02-29"));
        assert!(!is_calendar_date("2026-13-01"));
        assert!(!is_calendar_date("2026-1-01"));
        assert!(!is_calendar_date("2026-04-31"));
    }

    #[test]
    fn empty_status_line_is_present_and_invalid() {
        let parsed = parse_frontmatter("---\nstatus:\npairing: focus-linter\n---\n");
        // `status:` with nothing after it reads the next line as its value.
        assert_eq!(parsed.status.as_deref(), Some("pairing: focus-linter"));
        assert!(validate_frontmatter(&parsed, "f").is_err());
        assert_eq!(
            parse_frontmatter("no frontmatter"),
            RuleFrontmatter::default()
        );
    }
}

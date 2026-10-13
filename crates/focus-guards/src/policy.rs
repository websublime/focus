//! Guard Policy: how far the guards stand aside for a piece of work.
//!
//! One setting, three values. `strict`: an input that changed after the
//! person approved or confirmed something is asked about once, and the
//! authority fences hold against work nobody directed. `relaxed`: a changed
//! input is recorded and told to the person in one line, and the work
//! continues; the plan-approval and review-freeze fences stand aside.
//! `off` also lowers the state-transition and reviewer-scope fences.
//!
//! The value is the intent's own state line when it has one, else the
//! scope's default; a memory layer that declares `strict` wins over both
//! and cannot be flipped from chat, and a memory `relaxed` or `off` (the
//! narrowest layer that declares one) replaces a value that came from the
//! scope or is not set.
//!
//! The old name of the setting, Change Control, is read for one release: the
//! state field `Change Control` and the memory heading `## Change Control`
//! still resolve. Every writer emits the new name.
//!
//! Mirrors `resolveGuardPolicy`, `parseGuardPolicyStateLine`,
//! `changeControlSourceLabel`, `formatGuardPolicy`,
//! `scopeDefinitionGuardPolicy` and `setGuardPolicyLine` in `aidlc-lib.ts`.

use std::path::Path;

pub use focus_graph::guard_memory::Declaration;
pub use focus_graph::scopes::GuardPolicy;
use focus_js::text::trim;
use focus_js::{Result, bail, re};
use focus_state::fields::{get_field, set_field};

/// The state field carrying the policy line.
pub const GUARD_POLICY_FIELD: &str = "Guard Policy";
/// The retired state field, read for one release and never written.
pub const CHANGE_CONTROL_FIELD: &str = "Change Control";

/// The intent's own policy line, `<value> (<source>)`, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateLine {
    pub value: GuardPolicy,
    /// Where the line says the value came from: `you`, `command`, `not set`,
    /// a scope (`scope classic`) or a memory file (`project.md`).
    pub source: String,
}

/// `parseGuardPolicyStateLine`: the line's value and source. `None` for an
/// absent, empty or malformed line.
#[must_use]
pub fn parse_state_line(raw: Option<&str>) -> Option<StateLine> {
    let raw = raw?;
    if raw.is_empty() {
        return None;
    }
    let text = trim(raw);
    let found = re::find(r"^(strict|relaxed|off)\b(?:\s*\((.*)\))?\s*$", "i", text)?;
    let value = GuardPolicy::parse(re::group(text, &found, 1)?)?;
    let label = trim(re::group(text, &found, 2).unwrap_or_default());
    Some(StateLine {
        value,
        source: source_from_label(label),
    })
}

/// `changeControlSourceFromLabel`: the source a line's label stands for.
fn source_from_label(label: &str) -> String {
    if label == "set by you" {
        return "you".to_owned();
    }
    if label == "set by a command" {
        return "command".to_owned();
    }
    if let Some(found) = re::find(r"^from\s+(.+)$", "", label) {
        return trim(re::group(label, &found, 1).unwrap_or_default()).to_owned();
    }
    if label.is_empty() {
        "you".to_owned()
    } else {
        label.to_owned()
    }
}

/// `changeControlSourceLabel`: the label rendered after a value: `from scope
/// classic`, `from project.md`, `set by you`, `set by a command`, `not set`.
#[must_use]
pub fn source_label(source: &str) -> String {
    match source {
        "not set" => source.to_owned(),
        "command" => "set by a command".to_owned(),
        "you" => "set by you".to_owned(),
        other => format!("from {other}"),
    }
}

/// `formatGuardPolicy`: the full state-line value, `off (from scope classic)`.
#[must_use]
pub fn format_guard_policy(value: GuardPolicy, source: &str) -> String {
    format!("{} ({})", value.as_str(), source_label(source))
}

/// `scopeDefinitionGuardPolicy`: a scope's default, what its file declares,
/// else `off`; a scope not defined at all stays `strict`.
#[must_use]
pub fn scope_default(declared: Option<Option<GuardPolicy>>) -> GuardPolicy {
    match declared {
        None => GuardPolicy::Strict,
        Some(declared) => declared.unwrap_or(GuardPolicy::Off),
    }
}

/// The resolved policy of a workflow and everything it was resolved from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub value: GuardPolicy,
    /// Where the value came from, worded for a person: `scope classic`,
    /// `project.md`, `you`, `command` or `not set`.
    pub source: String,
    pub scope_default: GuardPolicy,
    /// The intent's own valid state line, when it carries one.
    pub intent: Option<StateLine>,
    /// The value before memory is applied: the line, else strict.
    pub state_value: GuardPolicy,
    /// The field text exactly as stored, invalid text included.
    pub raw_state_value: Option<String>,
    /// The field the line was read from, current or retired.
    pub state_field: Option<&'static str>,
    /// Both stored lines when they disagree or either cannot be parsed.
    pub conflict: Option<(String, String)>,
    /// The first memory layer declaring strict, when one does.
    pub memory_strict: Option<Declaration>,
}

impl Resolution {
    /// Where the value came from, as a label: `from scope classic`.
    #[must_use]
    pub fn source_label(&self) -> String {
        source_label(&self.source)
    }
}

/// `resolveGuardPolicy`, on what it reads: the state text, the scope's
/// default (see [`scope_default`]) and the memory declarations, org then
/// team then project, which `declarations` reads once the state line has
/// been judged (a malformed line is refused before a memory file is). The
/// refusal names `state_path`, and `tolerate_invalid_state` turns it into
/// `strict`. The refusal names the entry skill as `/focus`, which it is in
/// every harness Focus has (upstream says `$aidlc` under Codex).
///
/// # Errors
///
/// A lone policy line that is not one of the three values, unless
/// tolerated; a memory file that cannot be read or declares a mode that is
/// not one of the three.
pub fn resolve(
    state: &str,
    state_path: &Path,
    scope_default: GuardPolicy,
    declarations: impl FnOnce() -> Result<Vec<Declaration>>,
    tolerate_invalid_state: bool,
) -> Result<Resolution> {
    let raw_guard_policy = get_field(state, GUARD_POLICY_FIELD);
    let raw_change_control = get_field(state, CHANGE_CONTROL_FIELD);
    let state_field = if raw_guard_policy.is_some() {
        Some(GUARD_POLICY_FIELD)
    } else if raw_change_control.is_some() {
        Some(CHANGE_CONTROL_FIELD)
    } else {
        None
    };
    let raw_state_value = raw_guard_policy.or(raw_change_control);
    let intent = parse_state_line(raw_state_value);
    let both = raw_guard_policy.zip(raw_change_control);
    let retired = both.and_then(|(_, retired)| parse_state_line(Some(retired)));
    let conflict = both.filter(|_| match (&intent, &retired) {
        (Some(intent), Some(retired)) => intent.value != retired.value,
        _ => true,
    });
    if let Some(raw) = raw_state_value
        && intent.is_none()
        && conflict.is_none()
        && !tolerate_invalid_state
    {
        bail!(
            "Invalid Guard Policy \"{raw}\" in {} (field: {}). Expected one of: strict, relaxed, off. Run /focus --guard-policy strict, relaxed, or off to repair it.",
            state_path.display(),
            state_field.unwrap_or_default()
        );
    }
    let conflict = conflict.map(|(a, b)| (a.to_owned(), b.to_owned()));
    let declarations = declarations()?;
    let (state_value, state_source) = match (&conflict, &intent) {
        (Some(_), _) => (GuardPolicy::Strict, "conflicting state lines".to_owned()),
        (None, Some(intent)) => (intent.value, intent.source.clone()),
        (None, None) => (GuardPolicy::Strict, "not set".to_owned()),
    };
    let base = |value, source: String, memory_strict| Resolution {
        value,
        source,
        scope_default,
        intent: intent.clone(),
        state_value,
        raw_state_value: raw_state_value.map(str::to_owned),
        state_field,
        conflict: conflict.clone(),
        memory_strict,
    };
    if let Some(strict) = declarations
        .iter()
        .find(|declaration| declaration.value == GuardPolicy::Strict)
    {
        return Ok(base(
            GuardPolicy::Strict,
            format!("{}.md", strict.layer),
            Some(strict.clone()),
        ));
    }
    // A memory layer's relaxed or off (the narrowest layer that declares one)
    // replaces a value that came from the scope, or none at all; the person's
    // own switch and a strict lock still win.
    if let Some(layered) = declarations.last()
        && conflict.is_none()
        && intent
            .as_ref()
            .is_none_or(|intent| intent.source.starts_with("scope "))
    {
        return Ok(base(layered.value, format!("{}.md", layered.layer), None));
    }
    Ok(base(state_value, state_source, None))
}

/// `setGuardPolicyLine`: `content` with the policy line `line` under its
/// current name. A retired line is renamed in place; with neither, the line
/// is inserted after the first of `Review Override`, `Test Strategy` and
/// `Scope`, or at the end. Every write removes any retired line left.
#[must_use]
pub fn set_guard_policy_line(content: &str, line: &str) -> String {
    let mut content = content.to_owned();
    if get_field(&content, GUARD_POLICY_FIELD).is_none() {
        let retired = format!(r"^(\s*(?:[-*]\s*)?\*\*){CHANGE_CONTROL_FIELD}(\*\*:)");
        if re::test(&retired, "m", &content) {
            content = re::replace(
                &retired,
                "m",
                &content,
                &format!("$1{GUARD_POLICY_FIELD}$2"),
            );
        } else {
            content = insert_after_anchors(
                &content,
                &["Review Override", "Test Strategy", "Scope"],
                GUARD_POLICY_FIELD,
            );
        }
    }
    content = re::replace(
        &format!(r"^[ \t]*(?:[-\*][ \t]*)?\*\*{CHANGE_CONTROL_FIELD}\*\*:[^\r\n]*(?:\r?\n|$)"),
        "gm",
        &content,
        "",
    );
    set_field(&content, GUARD_POLICY_FIELD, line)
}

/// An empty `- **field**:` bullet after the first of `anchors` that is in
/// `content`, or at its end.
pub(crate) fn insert_after_anchors(content: &str, anchors: &[&str], field: &str) -> String {
    for anchor in anchors {
        let line = format!(r"^(- \*\*{}\*\*:[^\n]*)$", re::escape(anchor));
        let inserted = re::replace(&line, "m", content, &format!("$1\n- **{field}**:"));
        if inserted != content {
            return inserted;
        }
    }
    format!("{}\n- **{field}**:\n", focus_js::text::trim_end(content))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaration(layer: &'static str, value: GuardPolicy) -> Declaration {
        Declaration {
            layer,
            path: Path::new("/p/focus/spaces/default/memory").join(format!("{layer}.md")),
            heading: "## Guard Policy",
            value,
        }
    }

    fn resolved(state: &str, scope: GuardPolicy, memory: &[Declaration]) -> Resolution {
        resolve(
            state,
            Path::new("/p/focus-state.md"),
            scope,
            || Ok(memory.to_vec()),
            false,
        )
        .unwrap()
    }

    #[test]
    fn the_line_names_its_value_and_source() {
        let line = |raw: &str| parse_state_line(Some(raw)).unwrap();
        assert_eq!(line("off (from scope classic)").source, "scope classic");
        assert_eq!(line("Relaxed (set by you)").source, "you");
        assert_eq!(line("strict (set by a command)").source, "command");
        assert_eq!(line("strict").source, "you");
        assert_eq!(line("off (project.md)").source, "project.md");
        assert_eq!(line("off ()").source, "you");
        assert_eq!(line("off (you)").source, "you");
        assert_eq!(line("off (from  team.md )").source, "team.md");
        for raw in ["", "offx", "none", "strict (", "off relaxed"] {
            assert_eq!(parse_state_line(Some(raw)), None, "{raw:?}");
        }
        assert_eq!(parse_state_line(None), None);
        assert_eq!(
            format_guard_policy(GuardPolicy::Off, "scope classic"),
            "off (from scope classic)"
        );
        assert_eq!(
            format_guard_policy(GuardPolicy::Strict, "not set"),
            "strict (not set)"
        );
        assert_eq!(
            format_guard_policy(GuardPolicy::Strict, "command"),
            "strict (set by a command)"
        );
        assert_eq!(
            format_guard_policy(GuardPolicy::Relaxed, "you"),
            "relaxed (set by you)"
        );
        assert_eq!(scope_default(None), GuardPolicy::Strict);
        assert_eq!(scope_default(Some(None)), GuardPolicy::Off);
        assert_eq!(
            scope_default(Some(Some(GuardPolicy::Strict))),
            GuardPolicy::Strict
        );
    }

    #[test]
    fn the_intents_line_wins_over_the_scope_and_memory_over_both() {
        let none = resolved("- **Scope**: feature\n", GuardPolicy::Off, &[]);
        assert_eq!(
            (none.value, none.source.as_str()),
            (GuardPolicy::Strict, "not set")
        );
        assert_eq!(none.scope_default, GuardPolicy::Off);
        let own = resolved(
            "- **Guard Policy**: relaxed (set by you)\n",
            GuardPolicy::Off,
            &[],
        );
        assert_eq!(
            (own.value, own.source.as_str()),
            (GuardPolicy::Relaxed, "you")
        );
        assert_eq!(own.state_field, Some(GUARD_POLICY_FIELD));
        let retired = resolved(
            "- **Change Control**: off (from scope x)\n",
            GuardPolicy::Off,
            &[],
        );
        assert_eq!(retired.state_field, Some(CHANGE_CONTROL_FIELD));
        assert_eq!(retired.source, "scope x");

        // Memory strict wins over everything and names its layer.
        let pinned = resolved(
            "- **Guard Policy**: off (set by you)\n",
            GuardPolicy::Off,
            &[
                declaration("org", GuardPolicy::Off),
                declaration("team", GuardPolicy::Strict),
            ],
        );
        assert_eq!(
            (pinned.value, pinned.source.as_str()),
            (GuardPolicy::Strict, "team.md")
        );
        assert_eq!(pinned.memory_strict.as_ref().unwrap().layer, "team");
        // The first layer declaring strict is the one named.
        let twice = resolved(
            "",
            GuardPolicy::Off,
            &[
                declaration("org", GuardPolicy::Strict),
                declaration("project", GuardPolicy::Strict),
            ],
        );
        assert_eq!(twice.source, "org.md");
        // A memory relaxed or off replaces a scope value or none, not a person's.
        let layered = resolved(
            "- **Guard Policy**: off (from scope classic)\n",
            GuardPolicy::Off,
            &[
                declaration("org", GuardPolicy::Off),
                declaration("project", GuardPolicy::Relaxed),
            ],
        );
        assert_eq!(
            (layered.value, layered.source.as_str()),
            (GuardPolicy::Relaxed, "project.md")
        );
        let kept = resolved(
            "- **Guard Policy**: off (set by you)\n",
            GuardPolicy::Off,
            &[declaration("project", GuardPolicy::Relaxed)],
        );
        assert_eq!(
            (kept.value, kept.source.as_str()),
            (GuardPolicy::Off, "you")
        );
    }

    #[test]
    fn two_lines_that_disagree_resolve_to_strict() {
        let both = resolved(
            "- **Guard Policy**: off (set by you)\n- **Change Control**: relaxed (set by you)\n",
            GuardPolicy::Off,
            &[],
        );
        assert_eq!(both.value, GuardPolicy::Strict);
        assert_eq!(both.source, "conflicting state lines");
        assert_eq!(
            both.conflict,
            Some((
                "off (set by you)".to_owned(),
                "relaxed (set by you)".to_owned()
            ))
        );
        let agree = resolved(
            "- **Guard Policy**: off (set by you)\n- **Change Control**: off\n",
            GuardPolicy::Off,
            &[],
        );
        assert_eq!(agree.value, GuardPolicy::Off);
        assert_eq!(agree.conflict, None);
        // A retired line that cannot be read beside a good one is a conflict too.
        let half = resolved(
            "- **Guard Policy**: off (set by you)\n- **Change Control**: maybe\n",
            GuardPolicy::Off,
            &[],
        );
        assert_eq!(half.value, GuardPolicy::Strict);
        assert_eq!(
            half.conflict.as_ref().map(|(_, retired)| retired.as_str()),
            Some("maybe")
        );
        // Memory relaxed does not settle a conflict.
        let still = resolved(
            "- **Guard Policy**: off\n- **Change Control**: strict\n",
            GuardPolicy::Off,
            &[declaration("org", GuardPolicy::Relaxed)],
        );
        assert_eq!(still.value, GuardPolicy::Strict);
        assert!(still.conflict.is_some());
    }

    #[test]
    fn a_malformed_lone_line_is_refused_unless_tolerated() {
        let state = "- **Guard Policy**: sometimes\n";
        let refused = resolve(
            state,
            Path::new("/p/s.md"),
            GuardPolicy::Off,
            || Ok(Vec::new()),
            false,
        )
        .unwrap_err();
        assert_eq!(
            refused.message(),
            "Invalid Guard Policy \"sometimes\" in /p/s.md (field: Guard Policy). Expected one of: strict, relaxed, off. Run /focus --guard-policy strict, relaxed, or off to repair it."
        );
        let tolerated = resolve(
            state,
            Path::new("/p/s.md"),
            GuardPolicy::Off,
            || Ok(Vec::new()),
            true,
        )
        .unwrap();
        assert_eq!(
            (tolerated.value, tolerated.source.as_str()),
            (GuardPolicy::Strict, "not set")
        );
        assert_eq!(tolerated.raw_state_value.as_deref(), Some("sometimes"));
        assert_eq!(tolerated.intent, None);
    }

    #[test]
    fn the_policy_line_is_written_under_its_current_name() {
        assert_eq!(
            set_guard_policy_line(
                "- **Scope**: feature\n- **Change Control**: off\n",
                "strict (set by you)"
            ),
            "- **Scope**: feature\n- **Guard Policy**: strict (set by you)\n"
        );
        assert_eq!(
            set_guard_policy_line("- **Scope**: feature\n\n## Notes\n", "off (set by you)"),
            "- **Scope**: feature\n- **Guard Policy**: off (set by you)\n\n## Notes\n"
        );
        assert_eq!(
            set_guard_policy_line("- **Nothing**: x\n\n", "off (set by you)"),
            "- **Nothing**: x\n- **Guard Policy**: off (set by you)\n"
        );
        // Both lines: the current one is set, the retired one removed.
        assert_eq!(
            set_guard_policy_line(
                "- **Guard Policy**: off\n- **Change Control**: off\n- **A**: 1\n",
                "relaxed (set by you)"
            ),
            "- **Guard Policy**: relaxed (set by you)\n- **A**: 1\n"
        );
    }
}

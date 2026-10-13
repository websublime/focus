//! The bulleted fields of the state file.
//!
//! ```text
//! - **Scope**: feature
//! - **Current Stage**: code-generation
//! - **Completed**: 7
//! ```
//!
//! A field is read and written on its own line: the first `- **Name**:`
//! bullet, with what follows the colon as its value. Writing keeps the rest
//! of the file byte for byte, and a field that is not there is, depending on
//! the writer, left alone, refused, or inserted at the end of a section.
//!
//! What a value may hold is the caller's affair, with one exception upstream
//! has and this port keeps: a value is spliced in through JavaScript's
//! `String.prototype.replace`, so `$` patterns in it (`$1`, `$&`, `$$`) are
//! expanded as that function expands them. Upstream's own writers quote the
//! `$` of a value that may carry one.
//!
//! Mirrors `getField`, `setField`, `setFieldStrict`, `setOrInsertField`,
//! `removeField`, `setPhaseProgress`, `appendUnderHeading`, `parseRefsList`,
//! `emitRefsList`, `appendSlug` and `removeSlug` in `aidlc-lib.ts`.

use focus_js::order::cmp_utf16;
use focus_js::text::trim;
use focus_js::{Result, bail, re};

pub use focus_audit::read::bullet_field as get_field;

fn field_line(field: &str) -> String {
    format!(r"^(- \*\*{}\*\*:)[ \t]*.*$", re::escape(field))
}

/// `setField`: the first `- **field**:` bullet with `value` as its value.
/// Content without the field is unchanged.
#[must_use]
pub fn set_field(content: &str, field: &str, value: &str) -> String {
    let line = field_line(field);
    if !re::test(&line, "m", content) {
        return content.to_owned();
    }
    re::replace(&line, "m", content, &format!("$1 {value}"))
}

/// `setFieldStrict`: as [`set_field`], refusing content without the field.
///
/// # Errors
///
/// The field is not in `content`.
pub fn set_field_strict(content: &str, field: &str, value: &str) -> Result<String> {
    let line = field_line(field);
    if !re::test(&line, "m", content) {
        bail!(
            "Field not found in state file: \"{field}\". Cannot update — refusing to silently no-op."
        );
    }
    Ok(re::replace(&line, "m", content, &format!("$1 {value}")))
}

/// `setOrInsertField`: as [`set_field`], or, when the field is not there, a
/// new bullet at the end of the `heading` section.
///
/// # Errors
///
/// The field is not in `content` and neither is the heading.
pub fn set_or_insert_field(
    content: &str,
    heading: &str,
    field: &str,
    value: &str,
) -> Result<String> {
    let line = field_line(field);
    if re::test(&line, "m", content) {
        return Ok(re::replace(&line, "m", content, &format!("$1 {value}")));
    }
    append_under_heading(content, heading, &format!("- **{field}**: {value}\n"))
}

/// `removeField`: content without the first `- **field**:` bullet line, its
/// line ending included. Content without the field is unchanged.
#[must_use]
pub fn remove_field(content: &str, field: &str) -> String {
    let line = format!(r"^- \*\*{}\*\*:[ \t]*.*(?:\r?\n)?", re::escape(field));
    re::replace(&line, "m", content, "")
}

/// `setPhaseProgress`: the `- **Ideation**: Active` row of the Phase
/// Progress section, for the phase `ideation`. A row that is not there is
/// left alone: the section is for reading, and must never fail a transition.
#[must_use]
pub fn set_phase_progress(content: &str, phase: &str, status: &str) -> String {
    let mut chars = phase.chars();
    let label = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    set_field(content, &label, status)
}

/// `appendUnderHeading`: `new_content` inserted at the end of the `heading`
/// section, before the next `## ` heading or at the end of the file. When a
/// heading follows and `new_content` ends with one line break, a blank line
/// is put between them.
///
/// # Errors
///
/// `heading` is not a line of `content`.
pub fn append_under_heading(content: &str, heading: &str, new_content: &str) -> Result<String> {
    let line = format!(r"^{}[ \t]*$", re::escape(heading));
    let Some(found) = re::find(&line, "m", content) else {
        bail!("appendUnderHeading: heading not found: {heading}");
    };
    // Upstream steps over the heading's line break before looking for the
    // next heading; a `^` that matches after a line break finds the same one.
    let body_start = found.range.end;
    let next = re::find(r"^## [^\n]*$", "m", &content[body_start..]);
    let insert_at = next
        .as_ref()
        .map_or(content.len(), |next| body_start + next.range.start);
    let separator =
        if next.is_some() && new_content.ends_with('\n') && !new_content.ends_with("\n\n") {
            "\n"
        } else {
            ""
        };
    Ok(format!(
        "{}{new_content}{separator}{}",
        &content[..insert_at],
        &content[insert_at..]
    ))
}

/// `parseRefsList`: the slugs of a list-shaped value, `[a, b]`, `a, b` or
/// the placeholder `[empty list]`.
#[must_use]
pub fn parse_refs_list(value: &str) -> Vec<String> {
    let trimmed = trim(value);
    if trimmed.is_empty() || trimmed == "[empty list]" {
        return Vec::new();
    }
    let inner = if trimmed.starts_with('[') && trimmed.ends_with(']') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };
    inner
        .split(',')
        .map(trim)
        .filter(|slug| !slug.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `emitRefsList`: `[a, b]` with the slugs in JavaScript's sort order, or
/// `[empty list]`.
#[must_use]
pub fn emit_refs_list(slugs: &[String]) -> String {
    if slugs.is_empty() {
        return "[empty list]".to_owned();
    }
    let mut sorted = slugs.to_vec();
    sorted.sort_by(|a, b| cmp_utf16(a, b));
    format!("[{}]", sorted.join(", "))
}

/// `appendSlug`: the list with `slug` added.
///
/// # Errors
///
/// `slug` is already in the list.
pub fn append_slug(current: &str, slug: &str) -> Result<String> {
    let mut list = parse_refs_list(current);
    if list.iter().any(|known| known == slug) {
        bail!("slug already present in refs list: \"{slug}\"");
    }
    list.push(slug.to_owned());
    Ok(emit_refs_list(&list))
}

/// `removeSlug`: the list with `slug` taken out.
///
/// # Errors
///
/// `slug` is not in the list.
pub fn remove_slug(current: &str, slug: &str) -> Result<String> {
    let list = parse_refs_list(current);
    if !list.iter().any(|known| known == slug) {
        bail!("slug not present in refs list: \"{slug}\"");
    }
    let kept: Vec<String> = list.into_iter().filter(|known| known != slug).collect();
    Ok(emit_refs_list(&kept))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "# State\n\n## Workflow\n- **Scope**: feature\n- **Completed**: 2\n\n## Runtime State\n- **Parked**: no\n\n## Notes\n";

    #[test]
    fn a_field_is_read_and_written_on_its_own_line() {
        assert_eq!(get_field(STATE, "Scope"), Some("feature"));
        assert_eq!(
            get_field("# S\r\n\r\n- **Scope**: classic\r\n- **A**: 1\n", "Scope"),
            Some("classic")
        );
        assert_eq!(get_field(STATE, "Nope"), None);
        let written = set_field(STATE, "Scope", "bugfix");
        assert_eq!(get_field(&written, "Scope"), Some("bugfix"));
        assert_eq!(written.len(), STATE.len() - 1);
        // An empty value is a bullet with nothing after the colon, and reads
        // as empty, not as the next line.
        let emptied = set_field(STATE, "Scope", "");
        assert!(emptied.contains("- **Scope**: \n- **Completed**"));
        assert_eq!(get_field(&emptied, "Scope"), Some(""));
        // And writing over an empty value does not eat the next line.
        assert_eq!(
            set_field(&emptied, "Scope", "mvp"),
            STATE.replace("- **Scope**: feature", "- **Scope**: mvp")
        );
        // Dollar patterns in a value are expanded as JavaScript expands them.
        assert!(set_field(STATE, "Scope", "$1x$$").contains("- **Scope**: - **Scope**:x$\n"));
        assert_eq!(set_field(STATE, "Nope", "x"), STATE);
        assert!(set_field_strict(STATE, "Nope", "x").is_err());
    }

    #[test]
    fn a_field_is_inserted_at_the_end_of_its_section_or_removed_with_its_line() {
        let inserted =
            set_or_insert_field(STATE, "## Runtime State", "Parked At Stage", "x").unwrap();
        // After the section's trailing blank line, and a blank line before
        // the next heading: that is where upstream puts it.
        assert!(inserted.contains("- **Parked**: no\n\n- **Parked At Stage**: x\n\n## Notes\n"));
        let last = set_or_insert_field(STATE, "## Notes", "Note", "y").unwrap();
        assert!(last.ends_with("## Notes\n- **Note**: y\n"));
        assert!(set_or_insert_field(STATE, "## Missing", "Note", "y").is_err());
        // One blank line before the next heading, and none added to content
        // that already ends with one or ends without a line break.
        let section = "## A\n- x\n## B\n";
        assert_eq!(
            append_under_heading(section, "## A", "- y\n").unwrap(),
            "## A\n- x\n- y\n\n## B\n"
        );
        assert_eq!(
            append_under_heading(section, "## A", "- y\n\n").unwrap(),
            "## A\n- x\n- y\n\n## B\n"
        );
        assert_eq!(
            append_under_heading(section, "## A", "- y").unwrap(),
            "## A\n- x\n- y## B\n"
        );
        assert_eq!(
            append_under_heading(section, "## B", "- y\n").unwrap(),
            "## A\n- x\n## B\n- y\n"
        );
        assert_eq!(
            set_or_insert_field(STATE, "## Missing", "Scope", "mvp").unwrap(),
            set_field(STATE, "Scope", "mvp")
        );
        assert_eq!(
            remove_field(STATE, "Completed"),
            STATE.replace("- **Completed**: 2\n", "")
        );
        assert_eq!(remove_field(STATE, "Nope"), STATE);
        assert_eq!(
            remove_field("- **A**: 1\r\n- **B**: 2\r\n", "A"),
            "- **B**: 2\r\n"
        );
    }

    #[test]
    fn phase_progress_rows_are_fields_named_after_the_phase() {
        let state = "## Phase Progress\n- **Ideation**: Pending\n- **Inception**: Pending\n";
        assert_eq!(
            set_phase_progress(state, "inception", "Active"),
            "## Phase Progress\n- **Ideation**: Pending\n- **Inception**: Active\n"
        );
        assert_eq!(set_phase_progress(state, "operation", "Active"), state);
        assert_eq!(set_phase_progress(state, "", "Active"), state);
    }

    #[test]
    fn refs_lists_round_trip_in_sorted_order() {
        assert_eq!(parse_refs_list("[empty list]"), Vec::<String>::new());
        assert_eq!(parse_refs_list(""), Vec::<String>::new());
        assert_eq!(parse_refs_list(" [b, a,, c ] "), ["b", "a", "c"]);
        assert_eq!(parse_refs_list("b, a"), ["b", "a"]);
        assert_eq!(emit_refs_list(&[]), "[empty list]");
        assert_eq!(append_slug("[b]", "a").unwrap(), "[a, b]");
        assert_eq!(append_slug("[b]", "\u{1F600}").unwrap(), "[b, \u{1F600}]");
        // In UTF-16 order: a character past the basic plane sorts before U+E000.
        assert_eq!(
            append_slug("[b, \u{e000}]", "\u{1F600}").unwrap(),
            "[b, \u{1F600}, \u{e000}]"
        );
        assert!(append_slug("[b]", "b").is_err());
        assert_eq!(remove_slug("[a, b]", "a").unwrap(), "[b]");
        assert_eq!(remove_slug("[a]", "a").unwrap(), "[empty list]");
        assert!(remove_slug("[a]", "b").is_err());
    }
}

//! Stage Progress: one checkbox line per stage.
//!
//! ```text
//! - [x] requirements-analysis — EXECUTE
//! - [-] code-generation — EXECUTE
//! - [S] nfr-design — SKIP: not needed for a bugfix
//! ```
//!
//! The mark in the box is where the stage stands; what follows the dash is
//! the action the approved plan gave it, which the mark does not change.
//!
//! Mirrors `parseCheckboxes` and `parseStateStageSuffixes` in `aidlc-lib.ts`
//! (L29731, L37631), and `completedStateOverlay` in `aidlc-runtime.ts` (L934).

use focus_audit::read::bullet_field;
use focus_js::re;
use focus_js::text::trim;

/// Where a stage stands, by the mark in its box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckboxState {
    /// `[ ]`
    Pending,
    /// `[-]`
    InProgress,
    /// `[?]`
    AwaitingApproval,
    /// `[R]`
    Revising,
    /// `[x]`
    Completed,
    /// `[S]`
    Skipped,
}

impl CheckboxState {
    /// The six states, in the order upstream lists them.
    pub const ALL: [CheckboxState; 6] = [
        Self::Pending,
        Self::InProgress,
        Self::AwaitingApproval,
        Self::Revising,
        Self::Completed,
        Self::Skipped,
    ];

    /// The state named `name`, as `isCheckboxState` admits it: exactly one
    /// of the six names.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.name() == name)
    }

    /// The box as it is written: `[x]`, `[ ]` and so on (`CHECKBOX_MAP`).
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[-]",
            Self::AwaitingApproval => "[?]",
            Self::Revising => "[R]",
            Self::Completed => "[x]",
            Self::Skipped => "[S]",
        }
    }

    /// The name upstream gives the state.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in-progress",
            Self::AwaitingApproval => "awaiting-approval",
            Self::Revising => "revising",
            Self::Completed => "completed",
            Self::Skipped => "skipped",
        }
    }
}

/// One checkbox line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkbox {
    pub slug: String,
    pub state: CheckboxState,
    /// What follows the dash, trimmed: `EXECUTE`, or `SKIP: reason`.
    pub suffix: String,
}

/// Every checkbox line of a state file, in order. A stage listed twice is
/// answered twice.
#[must_use]
pub fn parse_checkboxes(content: &str) -> Vec<Checkbox> {
    re::compile(r"^- \[([ xSR?-])\] (\S+)\s*—\s*(.*)$", "m")
        .find_iter(content)
        .map(|found| {
            let part = |index| re::group(content, &found, index).unwrap_or_default();
            let state = match part(1) {
                "-" => CheckboxState::InProgress,
                "?" => CheckboxState::AwaitingApproval,
                "R" => CheckboxState::Revising,
                "x" => CheckboxState::Completed,
                "S" => CheckboxState::Skipped,
                _ => CheckboxState::Pending,
            };
            Checkbox {
                slug: part(2).to_owned(),
                state,
                suffix: trim(part(3)).to_owned(),
            }
        })
        .collect()
}

/// `setCheckbox`: the first checkbox line of `slug` with its box set to
/// `state`, whatever it was. Content with no such line is unchanged.
#[must_use]
pub fn set_checkbox(content: &str, slug: &str, state: CheckboxState) -> String {
    let line = format!(r"^(- )\[[ xSR?-]\]( {} —)", re::escape(slug));
    re::replace(&line, "m", content, &format!("$1{}$2", state.marker()))
}

/// `setStageSuffix`: the first checkbox line of `slug` with its action set
/// to `action` (`EXECUTE` or `SKIP`), its box untouched. Content with no
/// such line is unchanged.
#[must_use]
pub fn set_stage_suffix(content: &str, slug: &str, action: &str) -> String {
    let line = format!(
        r"^(- \[[ xSR?-]\] {}\s*—\s*)(EXECUTE|SKIP)\b",
        re::escape(slug)
    );
    re::replace(&line, "m", content, &format!("$1{action}"))
}

/// `countCheckboxes`: how many checkbox lines are in `state`.
#[must_use]
pub fn count_checkboxes(content: &str, state: CheckboxState) -> usize {
    parse_checkboxes(content)
        .iter()
        .filter(|line| line.state == state)
        .count()
}

/// The action the approved plan gave each stage, `EXECUTE` or `SKIP`, in the
/// order stages are first listed. A stage listed twice has its last action.
#[must_use]
pub fn parse_stage_suffixes(content: &str) -> Vec<(String, &'static str)> {
    let mut actions: Vec<(String, &'static str)> = Vec::new();
    for found in
        re::compile(r"^- \[[ xSR?-]\] (\S+)\s*—\s*(EXECUTE|SKIP)\b", "m").find_iter(content)
    {
        let slug = re::group(content, &found, 1).unwrap_or_default();
        let action = if re::group(content, &found, 2) == Some("EXECUTE") {
            "EXECUTE"
        } else {
            "SKIP"
        };
        match actions.iter_mut().find(|(known, _)| known == slug) {
            Some(known) => known.1 = action,
            None => actions.push((slug.to_owned(), action)),
        }
    }
    actions
}

/// What a complete workflow's state file says of the stages its plan
/// executes, in the order it first lists them: `approved` for a ticked one,
/// `pending` for any other that was not skipped. `None` while the workflow
/// is not complete (its `Status` field is not `Completed`).
///
/// The runtime summary lays this over the runtime graph: once a workflow is
/// complete, its checkboxes have the last word.
#[must_use]
pub fn completed_overlay(state: &str) -> Option<Vec<(String, &'static str)>> {
    if bullet_field(state, "Status") != Some("Completed") {
        return None;
    }
    let actions = parse_stage_suffixes(state);
    let mut outcomes: Vec<(String, &'static str)> = Vec::new();
    for line in parse_checkboxes(state) {
        let executes = actions
            .iter()
            .any(|(slug, action)| *slug == line.slug && *action == "EXECUTE");
        let outcome = match line.state {
            _ if !executes => continue,
            CheckboxState::Completed => "approved",
            CheckboxState::Skipped => continue,
            _ => "pending",
        };
        match outcomes.iter_mut().find(|(slug, _)| *slug == line.slug) {
            Some(known) => known.1 = outcome,
            None => outcomes.push((line.slug, outcome)),
        }
    }
    Some(outcomes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = "# State\n- **Status**: In Progress\n\n## Stage Progress\n- [x] intent-capture — EXECUTE\n- [-] code-generation —EXECUTE: now\n- [S] nfr-design  —  SKIP: not needed  \n- [?] build-and-test — EXECUTED\n- [ ] code-generation — SKIP\n  - [x] indented — EXECUTE\n- [X] capital — EXECUTE\n- [x] no-dash - EXECUTE\n";

    #[test]
    fn checkbox_lines_are_read_in_order() {
        let read: Vec<(String, &str, String)> = parse_checkboxes(STATE)
            .into_iter()
            .map(|line| (line.slug, line.state.name(), line.suffix))
            .collect();
        let expected = [
            ("intent-capture", "completed", "EXECUTE"),
            ("code-generation", "in-progress", "EXECUTE: now"),
            ("nfr-design", "skipped", "SKIP: not needed"),
            ("build-and-test", "awaiting-approval", "EXECUTED"),
            ("code-generation", "pending", "SKIP"),
        ];
        assert_eq!(read.len(), expected.len());
        for (line, (slug, state, suffix)) in read.iter().zip(expected) {
            assert_eq!(
                (line.0.as_str(), line.1, line.2.as_str()),
                (slug, state, suffix)
            );
        }
    }

    #[test]
    fn a_checkbox_is_set_on_its_own_line_only() {
        let state =
            "- [ ] aXb \u{2014} EXECUTE\n- [ ] a.b \u{2014} EXECUTE\n- [x] a.b \u{2014} SKIP\n";
        assert_eq!(
            set_checkbox(state, "aXb", CheckboxState::Completed),
            "- [x] aXb \u{2014} EXECUTE\n- [ ] a.b \u{2014} EXECUTE\n- [x] a.b \u{2014} SKIP\n"
        );
        // The first line of a slug, whatever its state, and nothing for a
        // slug that is not there; the name is matched as written, so `.`
        // is a dot.
        assert_eq!(
            set_checkbox(state, "a.b", CheckboxState::Skipped),
            "- [ ] aXb \u{2014} EXECUTE\n- [S] a.b \u{2014} EXECUTE\n- [x] a.b \u{2014} SKIP\n"
        );
        assert_eq!(set_checkbox(state, "a", CheckboxState::Skipped), state);
        assert_eq!(
            set_stage_suffix(state, "a.b", "SKIP"),
            "- [ ] aXb \u{2014} EXECUTE\n- [ ] a.b \u{2014} SKIP\n- [x] a.b \u{2014} SKIP\n"
        );
        assert_eq!(set_stage_suffix(state, "a.bx", "SKIP"), state);
        assert_eq!(count_checkboxes(state, CheckboxState::Pending), 2);
        assert_eq!(
            CheckboxState::parse("in-progress"),
            Some(CheckboxState::InProgress)
        );
        assert_eq!(CheckboxState::parse("In-Progress"), None);
    }

    #[test]
    fn the_plan_action_is_the_word_after_the_dash() {
        // `EXECUTED` is not `EXECUTE`: the action is a whole word.
        assert_eq!(
            parse_stage_suffixes(STATE),
            [
                ("intent-capture".to_owned(), "EXECUTE"),
                ("code-generation".to_owned(), "SKIP"),
                ("nfr-design".to_owned(), "SKIP"),
            ]
        );
    }

    #[test]
    fn a_complete_workflows_checkboxes_have_the_last_word() {
        let state = "- **Status**: Completed\n\n## Stage Progress\n- [x] intent-capture \u{2014} EXECUTE\n- [ ] code-generation \u{2014} EXECUTE\n- [S] build-and-test \u{2014} EXECUTE\n- [x] nine \u{2014} SKIP: no\n- [-] made-up \u{2014} EXECUTE\n- [x] made-up \u{2014} EXECUTE\n";
        // Stages the plan executes, as first listed: a ticked one approved,
        // any other that was not skipped pending. A later line of the same
        // stage has the last word and keeps the first one's place.
        assert_eq!(
            completed_overlay(state).unwrap(),
            [
                ("intent-capture".to_owned(), "approved"),
                ("code-generation".to_owned(), "pending"),
                ("made-up".to_owned(), "approved"),
            ]
        );
        assert_eq!(
            completed_overlay(&state.replace("Completed", "In Progress")),
            None
        );
        assert_eq!(
            completed_overlay(&state.replace("Completed", "completed")),
            None
        );
        assert_eq!(completed_overlay("# State\n"), None);
    }
}

//! The state file a new piece of work starts with, built from its plan.
//!
//! Creation completes every initialization stage and hands off to the first
//! stage past them: the file says so (`Phase Progress`, `Stage Progress`,
//! `Current Status`), and the ledger's rows say the same (see
//! [`crate::create`]).
//!
//! Mirrors the state-init half of `handleIntentCreateStateBuild` in
//! `aidlc-utility.ts` (L8244), `authoritativeProjectDescription` and
//! `hasUnsafeSingleLineCharacter` in `aidlc-lib.ts`.

use focus_graph::runtime::{ScopeDefinition, Stage};
use focus_guards::policy::source_label;
use focus_js::re;
use focus_js::text::trim;
use focus_workspace::scan::Scan;
use serde_json::Value;

use crate::plan::{
    Action, PHASES, PLAN_FIELD, Plan, action_of, composed_plan_label, first_post_init_stage,
    grid_action, next_in_scope_stage,
};

/// The state file's version, as creation stamps it.
pub const CURRENT_STATE_VERSION: &str = "8";
/// Where the exact description is kept beside the state file.
pub const PROJECT_DESCRIPTION_FILE: &str = "project-description.json";
/// The state field that says who decided the project type.
pub const PROJECT_TYPE_SOURCE_FIELD: &str = "Project Type Source";
pub const PROJECT_TYPE_SOURCE_SCAN: &str = "workspace scan";
pub const PROJECT_TYPE_SOURCE_PERSON: &str = "you";
/// The Stages to Skip entry that marks Reverse Engineering as skipped only
/// because the work is a new project.
pub const GREENFIELD_RE_SKIP_LABEL: &str = "(reverse-engineering \u{2014} greenfield)";

/// The ceremonies, in the order the state lists them: the key, its field
/// and its flag.
pub const CEREMONIES: [(&str, &str, &str); 5] = [
    ("sensors", "Sensors", "--sensors"),
    ("learnings", "Learnings", "--learnings"),
    (
        "summary_confirmation",
        "Summary Confirmation",
        "--summary-confirmation",
    ),
    ("plan_approval", "Plan Approval", "--plan-approval"),
    ("collaborators", "Collaborators", "--collaborators"),
];

/// `VALID_DEPTHS` and `VALID_TEST_STRATEGIES`: the word as typed, lowercased,
/// to the word as stored.
#[must_use]
pub fn level_word(raw: &str) -> Option<&'static str> {
    match raw.to_lowercase().as_str() {
        "minimal" => Some("Minimal"),
        "standard" => Some("Standard"),
        "comprehensive" => Some("Comprehensive"),
        _ => None,
    }
}

/// `declaredProjectType`: the type a person declared, as the state keeps it.
#[must_use]
pub fn declared_project_type(raw: Option<&str>) -> Option<&'static str> {
    match raw.map(|raw| trim(raw).to_lowercase()).as_deref() {
        Some("greenfield") => Some("Greenfield"),
        Some("brownfield") => Some("Brownfield"),
        _ => None,
    }
}

/// `scopeCeremonyDefault`: what the scope's file says of a ceremony; on
/// when it says nothing.
#[must_use]
pub fn scope_ceremony_default(definition: &ScopeDefinition, key: &str) -> &'static str {
    if definition.meta.ceremony_setting(key) == "off" {
        "off"
    } else {
        "on"
    }
}

/// `formatCeremony`: `on (from scope feature)`.
#[must_use]
pub fn format_ceremony(value: &str, source: &str) -> String {
    format!("{value} ({})", source_label(source))
}

/// A request split into the person's own words and a pasted document, as
/// `authoritativeProjectDescription` splits it. Only what the state file
/// needs is kept: the words, and whether a document was pasted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    pub description: String,
    pub pasted_document: bool,
}

const ONLY_DOCUMENT_REQUEST: &str = "Build what the pasted document describes.";

/// `authoritativeProjectDescription`. The document runs from the first
/// `<document>` to the last `</document>`; a marker with no partner makes
/// the rest of the message on its side the document; a message that is only
/// a document asks to build what it describes.
#[must_use]
pub fn authoritative_description(raw: &str) -> Description {
    let open = "<document>";
    let close = "</document>";
    let first = raw.find(open);
    let last = raw.rfind(close);
    if first.is_none() && last.is_none() {
        return Description {
            description: trim(raw).to_owned(),
            pasted_document: false,
        };
    }
    let unbalanced = first.is_some() && raw.matches(open).count() > raw.matches(close).count();
    let opened = first.is_some_and(|first| unbalanced || last.is_none_or(|last| last > first));
    let closed = !unbalanced && last.is_some_and(|last| first.is_none_or(|first| last > first));
    let start = if opened { first.unwrap_or(0) } else { 0 };
    let end = if closed {
        last.unwrap_or(0) + close.len()
    } else {
        raw.len()
    };
    let directions = trim(&format!("{}{}", &raw[..start], &raw[end..])).to_owned();
    Description {
        description: if directions.is_empty() {
            ONLY_DOCUMENT_REQUEST.to_owned()
        } else {
            directions
        },
        pasted_document: true,
    }
}

/// `hasUnsafeSingleLineCharacter`: a control character, DEL, or a line or
/// paragraph separator.
#[must_use]
pub fn has_unsafe_single_line_character(value: &str) -> bool {
    value.chars().any(is_unsafe_single_line)
}

fn is_unsafe_single_line(ch: char) -> bool {
    let code = u32::from(ch);
    code <= 0x1f || code == 0x7f || code == 0x2028 || code == 0x2029
}

/// The description as the state's `Project` line shows it: a pasted
/// document's words or `[Pasted document provided]`; a text with a
/// character a line cannot hold has each replaced by a space, runs of
/// spaces folded, and `[Project description]` when nothing is left.
#[must_use]
pub fn project_line(raw: &str) -> String {
    let authority = authoritative_description(raw);
    let preview = if authority.pasted_document {
        if authority.description.is_empty() {
            "[Pasted document provided]".to_owned()
        } else {
            authority.description
        }
    } else {
        raw.to_owned()
    };
    if !has_unsafe_single_line_character(&preview) {
        return preview;
    }
    let spaced: String = preview
        .chars()
        .map(|ch| if is_unsafe_single_line(ch) { ' ' } else { ch })
        .collect();
    let folded = trim(&re::replace(" {2,}", "g", &spaced, " ")).to_owned();
    if folded.is_empty() {
        "[Project description]".to_owned()
    } else {
        folded
    }
}

/// What the state file is built from.
pub struct Inputs<'a> {
    pub definition: &'a ScopeDefinition,
    pub graph: &'a [Stage],
    /// The plan this workflow runs: the scope's grid, or the plan composed
    /// for it (`Some` when composed, with the plan's name).
    pub plan: &'a Plan,
    pub composed: Option<Option<&'a str>>,
    /// The project type: the person's word, or the scan's.
    pub declared_type: Option<&'static str>,
    pub scan: &'a Scan,
    pub depth: Option<&'a str>,
    pub test_strategy: Option<&'a str>,
    /// The stored review override, when one was asked: `""` clears it.
    pub review_override: Option<String>,
    /// The Guard Policy line, already formatted.
    pub guard_policy: &'a str,
    /// Each ceremony asked for with the creation, and whether the person
    /// set it: `(key, value, set_by_person)`.
    pub ceremonies: &'a [(&'static str, &'static str, bool)],
    pub description: &'a str,
    pub request: Option<&'a str>,
    pub timestamp: &'a str,
}

/// The state file, and what the ledger and the summary say of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub content: String,
    pub project_type: &'static str,
    pub project_type_source: &'static str,
    pub plan: Plan,
    pub execute_numbers: Vec<String>,
    pub skip_entries: Vec<String>,
    pub depth: String,
    pub test_strategy: String,
    pub first_post_init: String,
    pub first_phase: String,
    pub first_agent: String,
    pub total_in_scope: usize,
    pub completed_init: usize,
    /// The note printed when an incremental scope scans Greenfield.
    pub greenfield_note: bool,
}

/// Build the state file.
#[must_use]
pub fn build(inputs: &Inputs) -> Built {
    let definition = inputs.definition;
    let graph = inputs.graph;
    let scope = &definition.meta.name;
    let project_type = inputs.declared_type.unwrap_or(inputs.scan.project_type);
    let project_type_source = if inputs.declared_type.is_some() {
        PROJECT_TYPE_SOURCE_PERSON
    } else {
        PROJECT_TYPE_SOURCE_SCAN
    };
    let depth = inputs
        .depth
        .and_then(level_word)
        .map_or_else(|| definition.meta.depth.clone(), str::to_owned);
    let test_strategy = inputs
        .test_strategy
        .and_then(level_word)
        .map(str::to_owned)
        .or_else(|| definition.meta.test_strategy.clone())
        .unwrap_or_else(|| depth.clone());

    // The stages to execute and to skip, by number.
    let mut execute_numbers: Vec<String> = Vec::new();
    let mut skip_entries: Vec<String> = Vec::new();
    for stage in graph {
        if action_of(inputs.plan, &stage.slug) == "EXECUTE" {
            execute_numbers.push(stage.number.clone());
        } else {
            skip_entries.push(format!("{} ({})", stage.number, stage.slug));
        }
    }

    // A new project has nothing to reverse-engineer.
    let mut plan: Plan = inputs.plan.clone();
    let mut greenfield_note = false;
    let reverse = graph
        .iter()
        .find(|stage| stage.slug == "reverse-engineering");
    if project_type.to_lowercase() == "greenfield" {
        if action_of(&plan, "reverse-engineering") == "EXECUTE" {
            if let Some(known) = plan
                .iter_mut()
                .find(|(slug, _)| slug == "reverse-engineering")
            {
                known.1 = "SKIP";
            }
            if let Some(reverse) = reverse {
                execute_numbers.retain(|number| *number != reverse.number);
                skip_entries.push(format!("{} {GREENFIELD_RE_SKIP_LABEL}", reverse.number));
            }
            greenfield_note = inputs.declared_type.is_none() && definition.meta.existing_code;
        } else if inputs.composed.is_some()
            && grid_action(definition, "reverse-engineering") == "EXECUTE"
        {
            // A plan composed for a new project leaves out the Reverse
            // Engineering its scope runs; marked as the new-project skip, so
            // saying later that it is existing code puts it back.
            if let Some(reverse) = reverse {
                let plain = format!("{} ({})", reverse.number, reverse.slug);
                if let Some(entry) = skip_entries.iter_mut().find(|entry| **entry == plain) {
                    *entry = format!("{} {GREENFIELD_RE_SKIP_LABEL}", reverse.number);
                }
            }
        }
    }

    // The action a stage line shows: the adjusted plan's, else the grid's.
    let line_action = |slug: &str| -> Action {
        if plan.iter().any(|(known, _)| known == slug) {
            action_of(&plan, slug)
        } else {
            grid_action(definition, slug)
        }
    };

    let first_post_init = first_post_init_stage(&plan, graph);
    let mut stage_progress = String::new();
    for (phase, header) in PHASES.iter().zip([
        "INITIALIZATION PHASE",
        "IDEATION PHASE",
        "INCEPTION PHASE",
        "CONSTRUCTION PHASE",
        "OPERATION PHASE",
    ]) {
        stage_progress.push_str(&format!("\n### {header}\n"));
        if *phase == "construction" {
            stage_progress.push_str("Per unit: [TBD]\n");
        }
        for stage in graph.iter().filter(|stage| stage.phase == *phase) {
            let marker = if *phase == "initialization" {
                "[x]"
            } else {
                "[ ]"
            };
            let suffix = if line_action(&stage.slug) == "EXECUTE" {
                "EXECUTE"
            } else {
                "SKIP"
            };
            stage_progress.push_str(&format!("- {marker} {} \u{2014} {suffix}\n", stage.slug));
        }
    }
    // `String.prototype.replace` with a string pattern: the first match.
    stage_progress = stage_progress.replacen(
        &format!("- [ ] {first_post_init}"),
        &format!("- [-] {first_post_init}"),
        1,
    );

    let total_in_scope = execute_numbers.len();
    let completed_init = graph
        .iter()
        .filter(|stage| stage.phase == "initialization")
        .count();
    let first_entry = graph.iter().find(|stage| stage.slug == first_post_init);
    let first_phase =
        first_entry.map_or_else(|| "IDEATION".to_owned(), |stage| stage.phase.to_uppercase());
    let first_agent = first_entry
        .and_then(|stage| stage.raw.get("lead_agent").and_then(Value::as_str))
        .map_or_else(|| "focus-product-agent".to_owned(), str::to_owned);
    let next_after_first =
        next_in_scope_stage(&first_post_init, definition, graph, Some(&stage_progress));
    let next_stage = next_after_first.map_or("none", |stage| stage.slug.as_str());

    let project_desc = project_line(inputs.description);

    let phase_status = |phase: &str| -> &'static str {
        if phase == "initialization" {
            return "Verified";
        }
        if first_entry.is_some_and(|stage| stage.phase == phase) {
            return "Active";
        }
        let runs = graph
            .iter()
            .any(|stage| stage.phase == phase && line_action(&stage.slug) == "EXECUTE");
        if runs { "Pending" } else { "Skipped" }
    };

    let checkpoints = line_action("units-generation") == "EXECUTE"
        && graph.iter().any(|stage| {
            stage.phase == "construction"
                && stage.for_each.as_deref() == Some("unit-of-work")
                && stage.raw.get("workspace_requires") == Some(&Value::Bool(true))
                && line_action(&stage.slug) == "EXECUTE"
        });
    let checkpoint_defaults = if checkpoints {
        "- **Construction Checkpoints**: enabled\n- **Construction Iteration**: unit-major\n- **Construction Execution**: serial\n"
    } else {
        ""
    };

    let ceremony_lines: Vec<String> = CEREMONIES
        .iter()
        .map(|(key, field, _)| {
            let asked = inputs.ceremonies.iter().find(|(k, _, _)| k == key);
            let value = asked.map_or_else(
                || scope_ceremony_default(definition, key),
                |(_, value, _)| value,
            );
            let source = match asked {
                None => format!("scope {scope}"),
                Some((_, _, true)) => "you".to_owned(),
                Some((_, _, false)) => "command".to_owned(),
            };
            format!("- **{field}**: {}", format_ceremony(value, &source))
        })
        .collect();

    let plan_line = match inputs.composed {
        Some(name) => format!("- **{PLAN_FIELD}**: {}\n", composed_plan_label(name)),
        None => String::new(),
    };
    let request_line = inputs
        .request
        .map(|request| format!("- **Question Id**: {request}\n"))
        .unwrap_or_default();
    let review = inputs.review_override.clone().unwrap_or_default();
    let ts = inputs.timestamp;
    let content = format!(
        "# Focus State Tracking\n\n## Project Information\n- **Project**: {project_desc}\n- **Project Description Source**: {PROJECT_DESCRIPTION_FILE}\n- **Project Type**: {project_type}\n- **{PROJECT_TYPE_SOURCE_FIELD}**: {project_type_source}\n- **Scope**: {scope}\n{plan_line}- **Start Date**: {ts}\n{request_line}- **State Version**: {CURRENT_STATE_VERSION}\n- **Active Agent**: {first_agent}\n- **Worktree Path**:\n- **Bolt Refs**:\n- **Practices Affirmed Timestamp**:\n\n## Scope Configuration\n- **Stages to Execute**: {}\n- **Stages to Skip**: {}\n- **Depth**: {depth}\n- **Test Strategy**: {test_strategy}\n- **Review Override**: {review}\n- **Guard Policy**: {}\n{}\n\n## Workspace State\n- **Project Root**: .\n- **Languages**: {}\n- **Frameworks**: {}\n- **Build System**: {}\n\n## Execution Plan Summary\n- **Total Stages**: {total_in_scope}\n- **Completed**: {completed_init}\n- **In Progress**: {first_post_init}\n\n## Runtime State\n- **Revision Count**: 0\n{checkpoint_defaults}\n## Phase Progress\n<!-- Status values: Pending, Active, Verified, Skipped -->\n\n- **Initialization**: {}\n- **Ideation**: {}\n- **Inception**: {}\n- **Construction**: {}\n- **Operation**: {}\n\n## Stage Progress\n<!-- Checkbox states: [ ] not started, [-] in progress, [?] awaiting approval (gate open), [R] revising (user rejected gate), [x] completed, [S] skipped via --stage/--phase jump -->\n{stage_progress}\n## Current Status\n- **Lifecycle Phase**: {first_phase}\n- **Current Stage**: {first_post_init}\n- **Next Stage**: {next_stage}\n- **Status**: Running\n- **Last Updated**: {ts}\n\n## Session Resume Point\n- **Last Completed Stage**: state-init\n- **Next Action**: Execute {first_post_init}\n- **Pending Artifacts**: none\n",
        execute_numbers.join(", "),
        if skip_entries.is_empty() {
            "none".to_owned()
        } else {
            skip_entries.join(", ")
        },
        inputs.guard_policy,
        ceremony_lines.join("\n"),
        inputs.scan.languages,
        inputs.scan.frameworks,
        inputs.scan.build_system,
        phase_status("initialization"),
        phase_status("ideation"),
        phase_status("inception"),
        phase_status("construction"),
        phase_status("operation"),
    );

    Built {
        content,
        project_type,
        project_type_source,
        plan,
        execute_numbers,
        skip_entries,
        depth,
        test_strategy,
        first_post_init,
        first_phase,
        first_agent,
        total_in_scope,
        completed_init,
        greenfield_note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_line_holds_one_line() {
        assert_eq!(project_line("Add login"), "Add login");
        assert_eq!(
            project_line("line one\nline two\ttab  spaced \u{2028} sep"),
            "line one line two tab spaced sep"
        );
        assert_eq!(project_line("\u{1}\u{2}"), "[Project description]");
        assert_eq!(project_line("  padded  "), "  padded  ");
        // A pasted document: the person's words around it, or a stand-in.
        assert_eq!(
            project_line("Build this <document>a spec\nmore</document> please"),
            "Build this  please"
        );
        assert_eq!(
            project_line("<document>only a document</document>"),
            ONLY_DOCUMENT_REQUEST
        );
        assert_eq!(project_line("<document>no closing"), ONLY_DOCUMENT_REQUEST);
        assert_eq!(project_line("after </document> only"), "only");
        assert_eq!(
            project_line("</document> before <document>"),
            ONLY_DOCUMENT_REQUEST
        );
        // More openings than closings: the document runs to the end.
        assert_eq!(project_line("x <document>a</document> y <document>b"), "x");
    }

    #[test]
    fn levels_and_project_types_are_matched_by_word() {
        assert_eq!(level_word("MINIMAL"), Some("Minimal"));
        assert_eq!(level_word("Standard"), Some("Standard"));
        assert_eq!(level_word("deep"), None);
        assert_eq!(level_word("constructor"), None);
        assert_eq!(
            declared_project_type(Some(" Greenfield ")),
            Some("Greenfield")
        );
        assert_eq!(declared_project_type(Some("green")), None);
        assert_eq!(declared_project_type(None), None);
        assert_eq!(
            format_ceremony("on", "scope feature"),
            "on (from scope feature)"
        );
        assert_eq!(format_ceremony("off", "you"), "off (set by you)");
    }
}

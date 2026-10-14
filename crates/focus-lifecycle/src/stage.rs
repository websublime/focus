//! What a transition reads off a compiled stage beyond its typed fields.
//!
//! The compiled graph carries more than `focus-graph` types: the stage's
//! name, its lead agent, its mode, its reviewer and review class, whether it
//! produces source code, and which artifacts each unit kind owes. They are
//! read off the node as upstream reads them, and a node that lacks one reads
//! as upstream's `undefined` does.

use focus_graph::runtime::{Stage, truthy};
use serde_json::{Map, Value};

/// `stage.name`; the slug when the node carries no name.
#[must_use]
pub fn name(stage: &Stage) -> &str {
    text(stage, "name").unwrap_or(&stage.slug)
}

/// `stage.lead_agent`; empty when the node carries none.
#[must_use]
pub fn lead_agent(stage: &Stage) -> &str {
    text(stage, "lead_agent").unwrap_or("")
}

/// `stage.mode`: `inline`, `subagent`, `mob` or `pipeline`.
#[must_use]
pub fn mode(stage: &Stage) -> Option<&str> {
    text(stage, "mode")
}

/// `stage.reviewer`: the agent that reviews the stage, when one is declared.
#[must_use]
pub fn reviewer(stage: &Stage) -> Option<&str> {
    text(stage, "reviewer").filter(|value| !value.is_empty())
}

/// `stage.review_class`: `adversarial` or `advisory`, when declared.
#[must_use]
pub fn review_class(stage: &Stage) -> Option<&str> {
    text(stage, "review_class")
}

/// `stage.workspace_requires`: the stage writes source code.
#[must_use]
pub fn workspace_requires(stage: &Stage) -> bool {
    stage.raw.get("workspace_requires").is_some_and(truthy)
}

/// `stage.summary_confirmation`: `required` or `if-present`, when declared.
#[must_use]
pub fn summary_confirmation(stage: &Stage) -> Option<&str> {
    text(stage, "summary_confirmation")
}

/// `stage.produces_kinds`: which artifacts each unit kind owes, when the
/// stage declares the map.
#[must_use]
pub fn produces_kinds(stage: &Stage) -> Option<&Map<String, Value>> {
    stage.raw.get("produces_kinds").and_then(Value::as_object)
}

/// `isPerUnitStage`: the stage runs once per unit of work.
#[must_use]
pub fn is_per_unit(stage: &Stage) -> bool {
    stage.for_each.as_deref() == Some("unit-of-work")
}

fn text<'a>(stage: &'a Stage, key: &str) -> Option<&'a str> {
    stage.raw.get(key).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fields_are_read_off_the_node() {
        let node = json!({
            "slug": "code-generation", "phase": "construction", "name": "Code Generation",
            "lead_agent": "focus-build-agent", "mode": "subagent", "reviewer": "focus-architecture-reviewer-agent",
            "review_class": "adversarial", "workspace_requires": true, "for_each": "unit-of-work",
            "produces_kinds": {"code-summary": ["service"]}
        });
        let stage = &focus_graph::runtime::load_from_value(&json!([node]))[0];
        assert_eq!(name(stage), "Code Generation");
        assert_eq!(lead_agent(stage), "focus-build-agent");
        assert_eq!(mode(stage), Some("subagent"));
        assert_eq!(reviewer(stage), Some("focus-architecture-reviewer-agent"));
        assert_eq!(review_class(stage), Some("adversarial"));
        assert!(workspace_requires(stage));
        assert_eq!(summary_confirmation(stage), None);
        assert!(is_per_unit(stage));
        assert!(produces_kinds(stage).is_some_and(|map| map.contains_key("code-summary")));
        let bare =
            &focus_graph::runtime::load_from_value(&json!([{"slug": "x", "phase": "ideation"}]))[0];
        assert_eq!(name(bare), "x");
        assert_eq!(lead_agent(bare), "");
        assert_eq!(reviewer(bare), None);
        assert!(!workspace_requires(bare));
        assert!(!is_per_unit(bare));
    }
}

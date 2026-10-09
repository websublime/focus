//! Behavioural notes as tests. Each expectation here was checked against the
//! upstream function under Bun; the broad coverage lives in the oracle corpus
//! replayed by `focus-graph`.

use serde_json::json;

use super::*;

#[test]
fn frontmatter_block_needs_both_fences() {
    assert_eq!(frontmatter_block("---\na: 1\n---\nbody"), Some("a: 1"));
    assert_eq!(frontmatter_block("---\r\na: 1\r\n---\r\n"), Some("a: 1"));
    assert_eq!(frontmatter_block("\n---\na: 1\n---"), None);
    assert_eq!(frontmatter_block("---\na: 1\n"), None);
    // The closing fence needs a line of its own after at least one newline.
    assert_eq!(frontmatter_block("---\n---\n"), None);
    assert_eq!(frontmatter_block("---\n\n---\n"), Some(""));
}

#[test]
fn scalar_field_strips_quotes_and_block_markers() {
    let fm = "name: \"Quoted\"\nsingle: 'x'\nfolded: >\n  text\nplain:   spaced out   \nlone: \"";
    assert_eq!(scalar_field(fm, "name"), "Quoted");
    assert_eq!(scalar_field(fm, "single"), "x");
    assert_eq!(scalar_field(fm, "folded"), "");
    assert_eq!(scalar_field(fm, "plain"), "spaced out");
    assert_eq!(scalar_field(fm, "lone"), "");
    assert_eq!(scalar_field(fm, "absent"), "");
}

#[test]
fn scalar_field_reads_past_an_empty_line_value() {
    // `\s*` after the colon spans the newline, so the next line is captured.
    assert_eq!(
        scalar_field("condition:\nlead_agent: x", "condition"),
        "lead_agent: x"
    );
    assert_eq!(scalar_field("condition:   \n", "condition"), "");
}

#[test]
fn list_field_reads_block_and_flow_forms() {
    assert_eq!(
        list_field("produces:\n  - a\n  - \"b\"\nnext: 1", "produces"),
        ["a", "b"]
    );
    assert_eq!(
        list_field("produces: [a, 'b', c] # why", "produces"),
        ["a", "b", "c"]
    );
    assert_eq!(list_field("produces: []", "produces"), Vec::<String>::new());
    assert_eq!(
        list_field("produces:\n  -a\n", "produces"),
        Vec::<String>::new()
    );
    assert_eq!(list_field("other: 1", "produces"), Vec::<String>::new());
    // The block stops at the first line that is not an indented `- ` item.
    assert_eq!(list_field("produces:\n  - a\n\n  - b\n", "produces"), ["a"]);
}

#[test]
fn inline_list_rejects_unbalanced_input() {
    assert_eq!(parse_inline_list("[a, b"), Vec::<String>::new());
    assert_eq!(parse_inline_list("[a, \"b]"), Vec::<String>::new());
    assert_eq!(parse_inline_list("[a] trailing"), Vec::<String>::new());
    assert_eq!(parse_inline_list("[a, , b,]"), ["a", "b"]);
    assert_eq!(parse_inline_list("[\"a,b\", c]"), ["a,b", "c"]);
}

#[test]
fn object_list_coerces_bare_booleans_only() {
    let fm = "consumes:\n  - artifact: a\n    required: true\n  - artifact: \"b\"\n    required: \"false\"\n    conditional_on: brownfield";
    assert_eq!(
        object_list_field(fm, "consumes").unwrap(),
        vec![
            json!({ "artifact": "a", "required": true }),
            json!({ "artifact": "b", "required": "false", "conditional_on": "brownfield" }),
        ]
    );
    assert_eq!(
        object_list_field("consumes: []", "consumes").unwrap(),
        Vec::<Value>::new()
    );
}

#[test]
fn object_list_refuses_a_blank_line_between_items() {
    let fm =
        "consumes:\n  - artifact: a\n    required: true\n\n  - artifact: b\n    required: false\n";
    assert_eq!(
        object_list_field(fm, "consumes").unwrap_err().message(),
        "Blank line not allowed inside consumes[] block — list items must be consecutive"
    );
}

#[test]
fn map_of_lists_requires_inline_values() {
    let fm = "produces_kinds:\n  frontend-components: [ui]\n  api-spec: [service, library]\n";
    assert_eq!(
        Value::Object(map_of_lists_field(fm, "produces_kinds").unwrap()),
        json!({ "frontend-components": ["ui"], "api-spec": ["service", "library"] })
    );
    assert_eq!(
        map_of_lists_field("produces_kinds:\n  a: ui\n", "produces_kinds")
            .unwrap_err()
            .message(),
        "produces_kinds.a must be an inline list (e.g. [service, ui]), got: ui"
    );
}

#[test]
fn ars_block_is_assembled_in_canonical_order() {
    let fm = "ars:\n  role: \"core\"\n  cost: 3 # mid\n  targets: [iae, ve]\n  extra: x\n";
    assert_eq!(
        focus_js::json::stringify(&ars_field(fm).unwrap()),
        r#"{"targets":["iae","ve"],"cost":3,"role":"core","extra":"x"}"#
    );
    assert_eq!(ars_field("ars: nope").unwrap(), json!("nope"));
    assert_eq!(
        ars_field("ars:\n  cost: ~\n  targets: [a,,b]\n").unwrap(),
        json!({ "targets": "[a,,b]", "cost": null })
    );
}

#[test]
fn yaml_comment_needs_leading_whitespace_and_respects_quotes() {
    assert_eq!(strip_yaml_comment("value # note"), "value");
    assert_eq!(
        strip_yaml_comment("value#not-a-comment"),
        "value#not-a-comment"
    );
    assert_eq!(strip_yaml_comment("\"a # b\" # c"), "\"a # b\"");
    assert_eq!(strip_yaml_comment("# all"), "");
}

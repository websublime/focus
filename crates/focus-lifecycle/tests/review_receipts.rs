//! What the review receipts refuse by name: the two readings that arrive
//! with units (a unit's own source binding, and the claims of a team).
//! Upstream answers each; Focus says which reading it does not have yet
//! instead of answering without it. Everything else is compared with
//! upstream in the oracle corpus (`review_receipts`).

use std::path::Path;

use focus_harness::Harness;
use focus_lifecycle::oracle::run_case;
use focus_lifecycle::workflow::Workflow;
use focus_workspace::Layout;
use focus_workspace::records::{Invoker, Selection};
use serde_json::{Value, json};

fn install() -> Workflow {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    Workflow::open(
        Layout::new(fixtures.join("upstream/installed/claude"), Harness::Claude),
        Selection {
            space: "default".to_owned(),
            intent: None,
            session_id: None,
            binding: None,
        },
        Invoker::default(),
    )
    .unwrap()
}

fn row(at: u32, event: &str, fields: &[(&str, &str)]) -> String {
    let lines: String = fields
        .iter()
        .map(|(name, value)| format!("**{name}**: {value}\n"))
        .collect();
    format!("\n## Row\n**Timestamp**: 2026-01-05T10:00:{at:02}Z\n**Event**: {event}\n{lines}---\n")
}

/// A piece of work with two units, `api` and `web`, at a stage that runs
/// for each: its ledger, the state, and whether the stage writes code.
fn case(slug: &str, phase: &str, artifact: &str, state_more: &str, rows: &[String]) -> Value {
    let record = "intents/rec-1a2b3c4d";
    let state = format!(
        "# AI-DLC State\n\n## Workflow\n- **Scope**: feature\n{state_more}\n## Stage Progress\n- [x] units-generation — EXECUTE\n"
    );
    json!({
        "kind": "review_receipts",
        "slug": slug,
        "state": state,
        "files": {
            "intents/active-intent": "rec-1a2b3c4d\n",
            format!("{record}/{phase}/api/{slug}/{artifact}.md"): "# Of api\n",
            format!("{record}/{phase}/web/{slug}/{artifact}.md"): "# Of web\n",
            format!("{record}/inception/units-generation/unit-of-work-dependency.md"):
                "# Dependencies\n\n```yaml\nunits:\n  - name: api\n    depends_on: []\n  - name: web\n    depends_on: []\n```\n",
        },
        "records": {},
        "source": { "src/a.ts": "a\n" },
        "after": { "files": {}, "source": {} },
        "shards": { "laptop-abc123.md": format!("# Focus Audit Log\n{}", rows.concat()) },
        "units": ["api", "web"],
        "patch": { "reviewer": "architecture-reviewer-agent", "produces": [artifact], "optional_produces": [] },
        "class": null,
        "env": {},
        "keep": false,
    })
}

const REVIEWER: (&str, &str) = ("Reviewer", "architecture-reviewer-agent");

fn opened(slug: &str) -> Vec<String> {
    vec![
        row(1, "WORKFLOW_STARTED", &[("Scope", "feature")]),
        row(2, "STAGE_STARTED", &[("Stage", slug)]),
    ]
}

/// Not ported yet: a unit's verdict that recorded the unit's own source is
/// weighed by upstream against the unit's manifest and the listing its
/// reviewer saw. Units arrive with a later slice, and the answer is
/// refused by name until then.
#[test]
fn a_unit_verdict_bound_to_the_units_own_source_is_refused_by_name() {
    let bound = format!("sha256:{}", "a".repeat(64));
    let asked = [
        ("Stage", "code-generation"),
        REVIEWER,
        ("Iteration", "1"),
        ("Unit", "api"),
        ("Artifact Fingerprint", "{ART:api}"),
        ("Unit Source Fingerprint", bound.as_str()),
    ];
    let mut answered = asked.to_vec();
    answered.push(("Verdict", "READY"));
    let mut rows = opened("code-generation");
    rows.push(row(3, "REVIEW_REQUESTED", &asked));
    rows.push(row(4, "REVIEW_COMPLETED", &answered));
    let mut asked_of = case(
        "code-generation",
        "construction",
        "code-generation-plan",
        "",
        &rows,
    );
    asked_of["patch"]["workspace_requires"] = json!(true);
    assert_eq!(
        run_case(&install(), &asked_of)["error"],
        "The review of unit \"api\" in \"code-generation\" is bound to the unit's own source, and Focus does not read that yet (units arrive with a later slice)."
    );
    // The same verdict with the check of the source's freshness switched
    // off is weighed like any other: nothing of the unit's source is read.
    asked_of["env"] = json!({ "FOCUS_SKIP_SOURCE_FRESHNESS": "1" });
    let weighed = run_case(&install(), &asked_of);
    assert_eq!(
        weighed["units"]["verdicts"],
        json!([["api", "READY"]]),
        "{weighed}"
    );
}

/// Not ported yet: a request of a unit, in a stage that writes code, binds
/// the unit's own source, and whether it still describes it is the same
/// reading.
#[test]
fn a_units_pending_request_in_a_stage_that_writes_code_is_refused_by_name() {
    let mut rows = opened("code-generation");
    rows.push(row(
        3,
        "REVIEW_REQUESTED",
        &[
            ("Stage", "code-generation"),
            REVIEWER,
            ("Iteration", "1"),
            ("Unit", "web"),
            ("Artifact Fingerprint", "{ART:web}"),
        ],
    ));
    let mut asked_of = case(
        "code-generation",
        "construction",
        "code-generation-plan",
        "",
        &rows,
    );
    asked_of["patch"]["workspace_requires"] = json!(true);
    assert_eq!(
        run_case(&install(), &asked_of)["error"],
        "A review request of unit \"web\" in \"code-generation\" binds the unit's own source, and Focus does not read that yet (units arrive with a later slice)."
    );
}

/// Not ported yet: while units are owned by a team, whether a row that
/// names a unit belongs to the current claim is read from the team's
/// claims, which are deferred with team ownership.
#[test]
fn a_row_of_a_unit_owned_by_a_team_is_refused_by_name() {
    let mut rows = opened("functional-design");
    rows.push(row(
        3,
        "GATE_REJECTED",
        &[("Stage", "functional-design"), ("Unit", "api")],
    ));
    let asked_of = case(
        "functional-design",
        "construction",
        "functional-spec",
        "- **Unit Ownership**: team\n",
        &rows,
    );
    assert_eq!(
        run_case(&install(), &asked_of)["error"],
        "A review row names a unit while units are owned by a team, and Focus does not read team claims yet: whether the row belongs to the current claim cannot be told."
    );
    // A row with no unit is no claim's.
    let mut rows = opened("functional-design");
    rows.push(row(3, "SESSION_STARTED", &[]));
    let asked_of = case(
        "functional-design",
        "construction",
        "functional-spec",
        "- **Unit Ownership**: team\n",
        &rows,
    );
    let weighed = run_case(&install(), &asked_of);
    assert!(weighed.get("error").is_none(), "{weighed}");
}

//! What the attempt a guard reads refuses by name: the count of a unit's
//! reviews while units are owned by a team. Upstream leaves out the rows
//! of an earlier claim of the unit; Focus does not read team claims yet,
//! and says so instead of counting without them. Everything else is
//! compared with upstream in the oracle corpus (`guard_attempt`).

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

/// The attempt of `unit` (or the stage's) at `functional-design`, in a
/// piece of work with two units whose ledger has `rows` after the stage's
/// start.
fn asked(unit: Option<&str>, ownership: &str, rows: &[String]) -> Value {
    let record = "intents/rec-1a2b3c4d";
    let state = format!(
        "# AI-DLC State\n\n## Workflow\n- **Scope**: feature\n{ownership}\n## Stage Progress\n- [x] units-generation — EXECUTE\n"
    );
    let opened = [
        row(1, "WORKFLOW_STARTED", &[("Scope", "feature")]),
        row(2, "STAGE_STARTED", &[("Stage", "functional-design")]),
    ];
    json!({
        "kind": "guard_attempt",
        "slug": "functional-design",
        "state": state,
        "files": {
            "intents/active-intent": "rec-1a2b3c4d\n",
            format!("{record}/construction/api/functional-design/functional-spec.md"): "# Of api\n",
            format!("{record}/construction/web/functional-design/functional-spec.md"): "# Of web\n",
            format!("{record}/inception/units-generation/unit-of-work-dependency.md"):
                "# Dependencies\n\n```yaml\nunits:\n  - name: api\n    depends_on: []\n  - name: web\n    depends_on: []\n```\n",
        },
        "records": {},
        "source": {},
        "after": { "files": {}, "source": {} },
        "shards": { "laptop-abc123.md": format!("# Focus Audit Log\n{}{}", opened.concat(), rows.concat()) },
        "units": ["api", "web"],
        "patch": { "reviewer": "architecture-reviewer-agent", "produces": ["functional-spec"], "optional_produces": [] },
        "class": null,
        "env": {},
        "keep": false,
        "unit": unit,
        "weighed": null,
        "window": null,
        "summary": null,
        "budget": null,
        "status": "ask",
        "counted": null,
        "require": null,
        "single": false,
    })
}

const TEAM: &str = "- **Unit Ownership**: team\n";
const REFUSED: &str = "The review attempt of unit \"api\" is counted while units are owned by a team, and Focus does not read team claims yet: whether a row of the unit belongs to its current claim cannot be told.";

/// Not ported yet: while units are owned by a team, the count of a unit's
/// reviews weighs each row of the unit against the unit's current claim
/// (`reviewAttemptEventMatchesCurrentClaim`), and team claims are deferred
/// with team ownership.
#[test]
fn a_units_attempt_under_a_team_is_refused_by_name_when_a_row_is_the_units() {
    // The start of the unit's own bolt names it with no `Unit` field.
    let bolt = [row(
        3,
        "BOLT_STARTED",
        &[("Bolt names", "api"), ("Bolt slug", "api")],
    )];
    let refused = run_case(&install(), &asked(Some("api"), TEAM, &bolt));
    assert_eq!(refused["error"], REFUSED, "{refused}");

    // The same ledger counts for the other unit, for the stage, and for
    // the unit itself once units are not a team's: no row there has to be
    // weighed against a claim.
    for (unit, ownership) in [(Some("web"), TEAM), (None, TEAM), (Some("api"), "")] {
        let counted = run_case(&install(), &asked(unit, ownership, &bolt));
        assert!(counted.get("error").is_none(), "{unit:?}: {counted}");
        assert_eq!(counted["attempt"]["recovery"], "available", "{counted}");
    }

    // A bolt of another slug is not the unit's own, nor is one of the
    // unit's slug that names another unit; and a row that is not one the
    // count reads is nobody's.
    let others = [
        row(
            3,
            "BOLT_STARTED",
            &[("Bolt names", "api"), ("Bolt slug", "another")],
        ),
        row(
            4,
            "BOLT_STARTED",
            &[("Bolt names", "web"), ("Bolt slug", "api")],
        ),
        row(5, "SESSION_STARTED", &[]),
    ];
    let counted = run_case(&install(), &asked(Some("api"), TEAM, &others));
    assert!(counted.get("error").is_none(), "{counted}");
}

/// A row that names the unit in its `Unit` field is refused one reader
/// earlier, by the receipts, in their own words: either way the attempt is
/// not said without the claim.
#[test]
fn a_row_naming_the_unit_under_a_team_is_refused_by_the_receipts_first() {
    let rejected = [row(
        3,
        "GATE_REJECTED",
        &[("Stage", "functional-design"), ("Unit", "api")],
    )];
    let refused = run_case(&install(), &asked(Some("api"), TEAM, &rejected));
    assert_eq!(
        refused["error"],
        "A review row names a unit while units are owned by a team, and Focus does not read team claims yet: whether the row belongs to the current claim cannot be told.",
        "{refused}"
    );
}

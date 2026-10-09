//! The compile, end to end.
//!
//! The golden tests compare whole files with what upstream's packager emitted.
//! The behavioural tests each change one thing in an installed project. Their
//! expectations were confirmed against upstream's compile on the same change
//! (`tools/oracle/differential.py`), except for the tests marked as deliberate
//! deviations, which pin behaviour that is Focus's own.

mod common;

use common::Project;
use focus_graph::{Sources, compile};
use focus_harness::Harness;
use serde_json::Value;

fn stage<'a>(graph: &'a Value, slug: &str) -> &'a Value {
    graph
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["slug"] == slug)
        .unwrap_or_else(|| panic!("no stage {slug}"))
}

/// A named way to break a project, and how the refusal must end.
type Breakage = (&'static str, fn(&Project), &'static str);

fn compile_error(project: &Project) -> String {
    compile(&project.sources())
        .expect_err("compile should refuse this tree")
        .to_string()
}

#[test]
fn a_source_checkout_compiles_to_upstreams_bytes_for_every_harness() {
    for harness in Harness::ALL {
        let compiled = compile(&Sources::from_core(&common::core(), harness)).unwrap();
        assert_eq!(
            compiled.stage_graph_json,
            common::golden(harness, "stage-graph.json"),
            "{harness} stage graph"
        );
        assert_eq!(
            compiled.scope_grid_json,
            common::golden(harness, "scope-grid.json"),
            "{harness} scope grid"
        );
        assert_eq!(compiled.stages.len(), 33);
        assert!(compiled.advisories.is_empty());
    }
}

#[test]
fn harnesses_differ_only_in_where_sensors_live() {
    let claude = common::golden(Harness::Claude, "stage-graph.json");
    let copilot = common::golden(Harness::Copilot, "stage-graph.json");
    assert_ne!(claude, copilot);
    assert_eq!(
        claude.replace(".claude/sensors/", ".focus/sensors/"),
        copilot
    );
}

#[test]
fn an_installed_tree_recompiles_to_the_same_bytes() {
    let project = Project::installed("recompile");
    let compiled = compile(&project.sources()).unwrap();
    assert_eq!(
        compiled.stage_graph_json,
        common::golden(Harness::Claude, "stage-graph.json")
    );
    assert_eq!(
        compiled.scope_grid_json,
        common::golden(Harness::Claude, "scope-grid.json")
    );
}

#[test]
fn a_new_stage_takes_the_next_number_and_installed_numbers_hold() {
    let project = Project::installed("new-stage");
    project.add_stage(
        "inception",
        "zz-extra",
        "requires_stage:\n  - delivery-planning\nscopes:\n  - feature\n",
    );
    let compiled = compile(&project.sources()).unwrap();
    let graph: Value = serde_json::from_str(&compiled.stage_graph_json).unwrap();
    let before: Value =
        serde_json::from_str(&common::golden(Harness::Claude, "stage-graph.json")).unwrap();

    assert_eq!(stage(&graph, "zz-extra")["number"], "2.10");
    assert_eq!(stage(&graph, "zz-extra")["name"], "Zz Extra");
    for old in before.as_array().unwrap() {
        assert_eq!(
            stage(&graph, old["slug"].as_str().unwrap()),
            old,
            "an installed stage changed"
        );
    }
    // 2.10 sorts after 2.9: numbers order by their parts, not as text.
    let slugs: Vec<&str> = graph
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["slug"].as_str().unwrap())
        .collect();
    let at = |slug: &str| slugs.iter().position(|s| *s == slug).unwrap();
    assert_eq!(at("zz-extra"), at("delivery-planning") + 1);

    let grid: Value = serde_json::from_str(&compiled.scope_grid_json).unwrap();
    assert_eq!(grid["feature"]["stages"]["zz-extra"], "EXECUTE");
    assert_eq!(grid["bugfix"]["stages"]["zz-extra"], "SKIP");
}

#[test]
fn new_stages_are_numbered_by_their_edges_then_hint_then_slug() {
    let project = Project::installed("seed-order");
    // File order (aa, bb, mm, zz) contradicts the flow: aa needs zz.
    project.add_stage("operation", "aa-last", "requires_stage:\n  - zz-first\n");
    project.add_stage("operation", "zz-first", "");
    project.add_stage("operation", "mm-hinted", "number: 4.2\n");
    project.add_stage("operation", "bb-hinted", "number: 4.9\n");
    let graph: Value =
        serde_json::from_str(&compile(&project.sources()).unwrap().stage_graph_json).unwrap();

    // Hinted stages go first, in hint order; the hint's own value (4.2, 4.9)
    // is never used as the number.
    assert_eq!(stage(&graph, "mm-hinted")["number"], "4.8");
    assert_eq!(stage(&graph, "bb-hinted")["number"], "4.9");
    assert_eq!(stage(&graph, "zz-first")["number"], "4.10");
    assert_eq!(stage(&graph, "aa-last")["number"], "4.11");
}

#[test]
fn a_repeated_dependency_is_one_edge() {
    let project = Project::installed("dup-edge");
    project.add_stage(
        "inception",
        "dup-edge",
        "requires_stage:\n  - delivery-planning\n  - delivery-planning\n",
    );
    let graph: Value =
        serde_json::from_str(&compile(&project.sources()).unwrap().stage_graph_json).unwrap();
    assert_eq!(
        stage(&graph, "dup-edge")["requires_stage"],
        serde_json::json!(["delivery-planning"])
    );
}

#[test]
fn scope_names_that_are_integers_come_first_as_in_a_js_object() {
    let project = Project::installed("int-scopes");
    project.add_stage(
        "ideation",
        "extra-scope",
        "scopes:\n  - 2024\n  - brand-new\n  - 7\n",
    );
    let compiled = compile(&project.sources()).unwrap();
    let grid: Value = serde_json::from_str(&compiled.scope_grid_json).unwrap();
    let scopes: Vec<&str> = grid
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .take(4)
        .collect();
    assert_eq!(scopes, ["7", "2024", "brand-new", "bugfix"]);
}

#[test]
fn a_grid_column_without_a_scope_file_is_dropped() {
    let project = Project::installed("orphan-column");
    project.edit(
        ".claude/tools/data/scope-grid.json",
        "{\n",
        "{\n  \"ghost\": { \"stages\": {} },\n",
    );
    let compiled = compile(&project.sources()).unwrap();
    assert_eq!(
        compiled.scope_grid_json,
        common::golden(Harness::Claude, "scope-grid.json")
    );
}

#[test]
fn a_per_unit_build_stage_that_is_not_a_subagent_gets_an_advisory() {
    let project = Project::installed("advisory");
    project.edit(
        ".claude/focus-common/stages/construction/code-generation.md",
        "mode: subagent",
        "mode: inline",
    );
    let compiled = compile(&project.sources()).unwrap();
    assert_eq!(
        compiled.advisories,
        [
            "[advisory] stage \"code-generation\" is the per-unit build stage (for_each: unit-of-work + workspace_requires) but mode is \"inline\", not \"subagent\" — the autonomous Construction swarm will NOT fire for it; units build serially."
        ]
    );
}

#[test]
fn authoring_errors_refuse_the_compile_and_name_the_fix() {
    let cases: [Breakage; 9] = [
        (
            "filename",
            |p| {
                p.add_stage("inception", "right-name", "");
                let from = p
                    .harness_dir()
                    .join("focus-common/stages/inception/right-name.md");
                std::fs::rename(&from, from.with_file_name("wrong-name.md")).unwrap();
            },
            "wrong-name.md: stage filename stem \"wrong-name\" does not match frontmatter slug \"right-name\". Rename the file or fix the slug.",
        ),
        (
            "sensor",
            |p| {
                p.add_stage(
                    "inception",
                    "needs-sensor",
                    "sensors:\n  - no-such-sensor\n",
                )
            },
            "Stage \"needs-sensor\" imports unknown sensor id \"no-such-sensor\". Known ids: claim-sources, linter, required-sections, traceability, type-check, upstream-coverage",
        ),
        (
            "agent",
            |p| {
                p.add_stage(
                    "inception",
                    "needs-agent",
                    "reviewer: focus-nobody-agent\nreview_artifact: thing\nproduces:\n  - thing\n",
                )
            },
            "needs-agent.md: schema validation failed: reviewer \"focus-nobody-agent\" has no matching .claude/agents/*.md",
        ),
        (
            "dependency",
            |p| {
                p.add_stage(
                    "inception",
                    "dangling",
                    "requires_stage:\n  - no-such-stage\n",
                )
            },
            "Unknown requires_stage: \"no-such-stage\" on stage \"dangling\". Every requires_stage entry must reference a known stage slug.",
        ),
        (
            "forward-edge",
            |p| {
                p.edit(
                    ".claude/focus-common/stages/ideation/intent-capture.md",
                    "requires_stage: []",
                    "requires_stage:\n  - feedback-optimization",
                )
            },
            "Compile invariant violated: stage \"intent-capture\" (1.1) requires \"feedback-optimization\" (4.7) — dependency must be lower-numbered. Fix: either renumber in stage-graph.json to match the dependency direction, or remove the offending requires_stage edge.",
        ),
        (
            "cycle",
            |p| {
                p.add_stage("operation", "loop-a", "requires_stage:\n  - loop-b\n");
                p.add_stage("operation", "loop-b", "requires_stage:\n  - loop-a\n");
                p.add_stage("operation", "loop-c", "requires_stage:\n  - loop-a\n");
            },
            "Cannot seed stage numbers for phase \"operation\": requires_stage cycle among new stages (stuck: loop-a, loop-b, loop-c). Break the cycle.",
        ),
        (
            "keys",
            |p| {
                p.add_stage(
                    "inception",
                    "reserved",
                    "timeout: 30\nbundle: x\nmystery: y\n",
                )
            },
            "reserved.md: schema validation failed: timeout is reserved (sensor binding); not active yet; bundle: was renamed; write plugin: for ownership; unknown key: mystery",
        ),
        (
            "scope",
            |p| {
                p.edit(
                    ".claude/scopes/focus-bugfix.md",
                    "skeleton: off",
                    "skeleton: maybe",
                )
            },
            "focus-bugfix.md has invalid skeleton value \"maybe\". Expected \"on\" or \"off\".",
        ),
        (
            "rule",
            |p| {
                p.write(
                    "focus/spaces/default/memory/project.md",
                    "---\nstale_after: 2026-02-30\n---\nbody\n",
                )
            },
            "project.md: stale_after must be a real calendar date in YYYY-MM-DD format; got \"2026-02-30\"",
        ),
    ];
    for (name, break_it, expected) in cases {
        let project = Project::installed(&format!("error-{name}"));
        break_it(&project);
        let message = compile_error(&project);
        assert!(message.ends_with(expected), "{name}: got {message}");
    }
}

#[test]
fn a_consumed_artifact_needs_exactly_one_producer() {
    let project = Project::installed("two-producers");
    // `traceability` is produced by several stages and consumed by none: legal.
    compile(&project.sources()).unwrap();
    project.add_stage(
        "inception",
        "second-producer",
        "produces:\n  - requirements\n",
    );
    let message = compile_error(&project);
    assert!(
        message.starts_with("Duplicate producers for consumed artifact \"requirements\" in "),
        "{message}"
    );
    assert!(
        message.contains("requirements-analysis.md (stage \"requirements-analysis\"), "),
        "{message}"
    );
    assert!(message.contains("second-producer.md (stage \"second-producer\") — consumed by stage \"contract-design\" in "), "{message}");
    assert!(
        message.ends_with("Rename one produced artifact or update the consumer."),
        "{message}"
    );
}

/// Deliberate deviation. Upstream names the two files in whichever order the
/// filesystem listed their phase directories; here the earlier phase is always
/// the one "already declared".
#[test]
fn a_duplicate_slug_names_the_earlier_phase_as_the_first_declaration() {
    let project = Project::installed("dup-slug");
    project.add_stage("operation", "twin", "");
    project.add_stage("inception", "twin", "");
    project.edit(
        ".claude/focus-common/stages/inception/twin.md",
        "phase: inception",
        "phase: operation",
    );
    let message = compile_error(&project);
    let (in_file, declared_in) = message.split_once(" — already declared in ").unwrap();
    // Paths print with the platform's separator.
    let file = |phase: &str| {
        std::path::Path::new(phase)
            .join("twin.md")
            .display()
            .to_string()
    };
    assert!(
        in_file.starts_with("Duplicate stage slug \"twin\" in ")
            && in_file.ends_with(&file("operation")),
        "{message}"
    );
    assert!(
        declared_in.ends_with(&format!("{}. Rename one of them.", file("inception"))),
        "{message}"
    );
}

const PLUGIN_STAGES: [(&str, &str, &str); 2] = [
    (
        "inception",
        "acme-lint",
        "plugin: acme\nrequires_stage:\n  - requirements-analysis\nscopes:\n  - feature\n  - acme-quick\nproduces:\n  - lint-report\n",
    ),
    (
        "construction",
        "acme-ship",
        "plugin: acme\nrequires_stage:\n  - acme-lint\nscopes:\n  - acme-quick\nconsumes:\n  - artifact: lint-report\n    required: true\n",
    ),
];

/// A project with a two-stage plugin and the plugin's own scope installed.
fn with_plugin(name: &str, selection: Option<&[&str]>) -> Project {
    let project = Project::installed(name);
    for (phase, slug, extra) in PLUGIN_STAGES {
        // `consumes:` is in `extra` for the second stage; drop the default.
        project.add_stage(phase, slug, extra);
        if extra.contains("consumes:") {
            project.edit(
                &format!(".claude/focus-common/stages/{phase}/{slug}.md"),
                "consumes: []\n",
                "",
            );
        }
    }
    project.write(
        ".claude/scopes/acme-quick.md",
        "---\nname: acme-quick\nplugin: acme\ndepth: Minimal\ndescription: d\n---\n",
    );
    if let Some(selection) = selection {
        let manifest: Value =
            serde_json::from_str(&project.read(".claude/tools/data/harness.json")).unwrap();
        let mut manifest = manifest.as_object().unwrap().clone();
        manifest.insert("plugins".to_owned(), serde_json::json!(selection));
        project.write(
            ".claude/tools/data/harness.json",
            &Value::Object(manifest).to_string(),
        );
    }
    project
}

// The parity of everything below with upstream is established by the
// recorded differential (crates/focus-cli/tests/recorded.rs replays it). These
// tests state the behaviour in the library's own terms.

#[test]
fn without_a_selection_every_plugin_is_enabled() {
    let project = with_plugin("plugins-none", None);
    let compiled = compile(&project.sources()).unwrap();
    assert!(compiled.stages.iter().all(|s| s.enabled));
    assert!(!compiled.stage_graph_json.contains("\"enabled\""));
    let grid: Value = serde_json::from_str(&compiled.scope_grid_json).unwrap();
    assert_eq!(grid["acme-quick"]["stages"]["acme-ship"], "EXECUTE");
    assert_eq!(grid["feature"]["stages"]["acme-lint"], "EXECUTE");
}

#[test]
fn an_unselected_plugin_keeps_its_stages_in_the_graph_but_out_of_the_grid() {
    let project = with_plugin("plugins-core", Some(&["focus"]));
    let compiled = compile(&project.sources()).unwrap();
    let graph: Value = serde_json::from_str(&compiled.stage_graph_json).unwrap();
    // Disabled, not removed: the stage keeps its number for when the plugin
    // is selected again.
    let lint = stage(&graph, "acme-lint");
    assert_eq!(lint["enabled"], false);
    assert_eq!(lint["plugin"], "acme");
    assert!(
        stage(&graph, "requirements-analysis")
            .get("enabled")
            .is_none()
    );
    let keys: Vec<&String> = lint.as_object().unwrap().keys().collect();
    assert_eq!(keys[..5], ["slug", "number", "name", "plugin", "enabled"]);

    let grid: Value = serde_json::from_str(&compiled.scope_grid_json).unwrap();
    assert!(
        grid.get("acme-quick").is_none(),
        "a disabled plugin's scope has no column"
    );
    assert!(grid["feature"]["stages"].get("acme-lint").is_none());
    assert_eq!(grid.as_object().unwrap().len(), 11);
}

#[test]
fn a_plugin_only_install_keeps_initialization_and_the_plugins_scopes() {
    let project = with_plugin("plugins-only", Some(&["acme"]));
    let compiled = compile(&project.sources()).unwrap();
    for stage in &compiled.stages {
        let expected =
            stage.frontmatter.phase == "initialization" || stage.plugin_owner() == "acme";
        assert_eq!(stage.enabled, expected, "{}", stage.slug());
    }
    let grid: Value = serde_json::from_str(&compiled.scope_grid_json).unwrap();
    let scopes: Vec<&String> = grid.as_object().unwrap().keys().collect();
    assert_eq!(scopes, ["acme-quick"]);
    let cells = grid["acme-quick"]["stages"].as_object().unwrap();
    assert_eq!(cells["state-init"], "EXECUTE");
    assert_eq!(
        cells.len(),
        5,
        "three initialization stages and the plugin's two"
    );
}

#[test]
fn a_selection_that_starves_an_enabled_stage_is_refused() {
    let project = with_plugin("plugins-closure", Some(&["focus"]));
    project.add_stage(
        "operation",
        "needs-lint",
        "consumes:\n  - artifact: lint-report\n    required: true\n",
    );
    project.edit(
        ".claude/focus-common/stages/operation/needs-lint.md",
        "consumes: []\n",
        "",
    );
    assert_eq!(
        compile_error(&project),
        "Plugin selection closure failed: enabled stage \"needs-lint\" consumes required artifact \"lint-report\", but its only producer(s) are disabled: acme-lint (acme). Enable plugin(s) acme or disable the consuming stage."
    );
}

#[test]
fn a_malformed_selection_is_refused() {
    let project = Project::installed("plugins-bad");
    project.write(
        ".claude/tools/data/harness.json",
        "{\"name\":\"claude\",\"plugins\":\"acme\"}",
    );
    assert!(
        compile_error(&project)
            .ends_with("harness.json field \"plugins\" must be an array of non-empty strings.")
    );
}

const SENTINEL_BEGIN: &str = focus_graph::composed::GRID_BEGIN;
const SENTINEL_END: &str = focus_graph::composed::GRID_END;

fn identity(name: &str) -> String {
    format!(
        "---\nname: {name}\ndepth: Minimal\ndescription: composed\n---\n\n# {name}\n\nA tailored plan.\n"
    )
}

fn record(name: &str) -> String {
    format!(
        "{}\n{SENTINEL_BEGIN}\n\n```json\n{{\n  \"stages\": {{\n    \"state-init\": \"EXECUTE\",\n    \"code-generation\": \"EXECUTE\"\n  }}\n}}\n```\n\n{SENTINEL_END}\n",
        identity(name)
    )
}

fn compile_in_place(project: &Project) -> focus_graph::Result<focus_graph::Compiled> {
    let layout = project.layout();
    let lock = focus_workspace::WorkspaceLock::acquire(&layout, "test")?;
    focus_graph::write::write_compiled(&layout, &lock, |_| {})
}

#[test]
fn a_composed_scope_survives_a_recompile_and_a_reinstall() {
    let project = Project::installed("composed-record");
    project.write("focus/scopes/spike.md", &record("spike"));

    // The harness tree knows nothing of the scope yet: the record restores
    // its identity file and its grid column.
    let compiled = compile_in_place(&project).unwrap();
    assert_eq!(compiled.composed.record_names, ["spike"]);
    assert_eq!(
        project.read(".claude/scopes/focus-spike.md"),
        identity("spike")
    );
    let grid: Value =
        serde_json::from_str(&project.read(".claude/tools/data/scope-grid.json")).unwrap();
    assert_eq!(
        grid["spike"]["stages"],
        serde_json::json!({"state-init": "EXECUTE", "code-generation": "EXECUTE"})
    );
    assert_eq!(grid.as_object().unwrap().len(), 12);

    // Compiling again changes nothing.
    let before = (
        project.read(".claude/tools/data/scope-grid.json"),
        project.read(".claude/tools/data/stage-graph.json"),
        project.read("focus/scopes/spike.md"),
    );
    compile_in_place(&project).unwrap();
    assert_eq!(project.read(".claude/tools/data/scope-grid.json"), before.0);
    assert_eq!(
        project.read(".claude/tools/data/stage-graph.json"),
        before.1
    );
    assert_eq!(project.read("focus/scopes/spike.md"), before.2);

    // A hand edit to the identity file is kept; the column still comes from
    // the record.
    project.edit(".claude/scopes/focus-spike.md", "composed", "hand edited");
    compile_in_place(&project).unwrap();
    assert!(
        project
            .read(".claude/scopes/focus-spike.md")
            .contains("hand edited")
    );
}

/// Deliberate deviation. Upstream leaves the identity file it restored in
/// place when the compile then fails; with no grid column behind it, the
/// scope reads as one that skips every stage.
#[test]
fn a_refused_compile_takes_back_the_scope_files_it_restored() {
    let project = Project::installed("composed-rollback");
    project.write("focus/scopes/spike.md", &record("spike"));
    project.add_stage(
        "inception",
        "dangling",
        "requires_stage:\n  - no-such-stage\n",
    );
    let grid = project.read(".claude/tools/data/scope-grid.json");

    let message = compile_in_place(&project).unwrap_err().to_string();
    assert!(message.contains("Unknown requires_stage"), "{message}");
    assert!(!project.exists(".claude/scopes/focus-spike.md"));
    assert_eq!(project.read(".claude/tools/data/scope-grid.json"), grid);

    // Once the tree compiles, the same record restores the scope.
    std::fs::remove_file(
        project
            .harness_dir()
            .join("focus-common/stages/inception/dangling.md"),
    )
    .unwrap();
    compile_in_place(&project).unwrap();
    assert_eq!(
        project.read(".claude/scopes/focus-spike.md"),
        identity("spike")
    );
}

/// Deliberate deviation. Upstream checks only the data directory, and renames
/// over a linked file. Here a link at either target refuses the compile
/// before anything is written, so the two files never come from two compiles.
#[cfg(unix)]
#[test]
fn a_linked_target_refuses_the_compile_before_anything_is_written() {
    let project = Project::installed("linked-target");
    project.write("focus/scopes/spike.md", &record("spike"));
    project.edit(
        ".claude/focus-common/stages/construction/code-generation.md",
        "mode: subagent",
        "mode: inline",
    );
    let graph = project.read(".claude/tools/data/stage-graph.json");
    let data = project.harness_dir().join("tools/data");
    std::fs::rename(data.join("scope-grid.json"), data.join("elsewhere.json")).unwrap();
    std::os::unix::fs::symlink(data.join("elsewhere.json"), data.join("scope-grid.json")).unwrap();

    let layout = project.layout();
    let lock = focus_workspace::WorkspaceLock::acquire(&layout, "test").unwrap();
    let mut advisories = Vec::new();
    let refused = focus_graph::write::write_compiled(&layout, &lock, |compiled| {
        advisories.clone_from(&compiled.advisories);
    })
    .unwrap_err();

    assert!(
        refused
            .to_string()
            .contains("holds a link, so Focus changed nothing there"),
        "{refused}"
    );
    // The stale graph is still the one on disk, the restored scope file is
    // gone again, and the compile's advisory was reported before the refusal.
    assert_eq!(project.read(".claude/tools/data/stage-graph.json"), graph);
    assert!(!project.exists(".claude/scopes/focus-spike.md"));
    assert_eq!(advisories.len(), 1);
}

#[test]
fn a_scope_composed_before_records_existed_gets_one() {
    let project = Project::installed("composed-legacy");
    project.write(".claude/scopes/focus-legacy.md", &identity("legacy"));
    project.edit(
        ".claude/tools/data/scope-grid.json",
        "{\n",
        "{\n  \"legacy\": { \"stages\": { \"state-init\": \"EXECUTE\" } },\n",
    );
    let compiled = compile_in_place(&project).unwrap();
    assert_eq!(compiled.composed.grid_only_names, ["legacy"]);
    let written = project.read("focus/scopes/legacy.md");
    assert!(written.starts_with(&identity("legacy")));
    assert!(written.contains("\"state-init\": \"EXECUTE\""));

    // From now on the record is the source: the next compile finds it there.
    let again = compile_in_place(&project).unwrap();
    assert_eq!(again.composed.record_names, ["legacy"]);
    assert!(again.composed.grid_only_names.is_empty());
    assert_eq!(project.read("focus/scopes/legacy.md"), written);
}

#[test]
fn a_composed_scope_is_kept_under_a_plugin_selection() {
    let project = with_plugin("composed-selected", Some(&["focus"]));
    project.write("focus/scopes/spike.md", &record("spike"));
    let compiled = compile_in_place(&project).unwrap();
    let grid: Value = serde_json::from_str(&compiled.scope_grid_json).unwrap();
    assert!(grid.get("spike").is_some());
    assert!(grid.get("acme-quick").is_none());
}

#[test]
fn a_failed_compile_writes_nothing() {
    let project = Project::installed("compile-fails");
    let before = project.read(".claude/tools/data/stage-graph.json");
    project.add_stage(
        "inception",
        "dangling",
        "requires_stage:\n  - no-such-stage\n",
    );
    assert!(compile_in_place(&project).is_err());
    assert_eq!(project.read(".claude/tools/data/stage-graph.json"), before);
}

/// Deliberate deviation. Upstream back-fills a record for a grid column whose
/// `stages` is not an object (`null`, a list), writing that value into it;
/// the record then fails to parse, and so does every later compile. Focus
/// writes no record.
#[test]
fn a_column_without_stages_is_not_given_a_record() {
    for (name, stages) in [("null", "null"), ("list", "[\"state-init\"]")] {
        let project = Project::installed(&format!("composed-{name}"));
        project.write(".claude/scopes/focus-legacy.md", &identity("legacy"));
        project.edit(
            ".claude/tools/data/scope-grid.json",
            "{\n",
            &format!("{{\n  \"legacy\": {{ \"stages\": {stages} }},\n"),
        );
        compile_in_place(&project).unwrap();
        assert!(!project.exists("focus/scopes/legacy.md"), "{name}");
        compile_in_place(&project).unwrap();
    }
}

/// Deliberate deviation. Upstream reads a harness manifest it cannot parse as
/// "no plugin selection" and compiles, which silently enables every plugin an
/// install had deselected. Focus refuses to write until the manifest is
/// restored.
#[test]
fn an_unreadable_manifest_stops_an_in_place_compile() {
    let project = Project::installed("manifest-broken");
    project.write(".claude/tools/data/harness.json", "{ not json");
    let message = compile_in_place(&project).unwrap_err().message().to_owned();
    assert!(
        message.ends_with(
            "harness.json is not the Claude Code harness manifest: it must be a JSON object whose \"name\" is \"claude\". Restore it, then compile again."
        ),
        "{message}"
    );

    let project = Project::installed("manifest-missing");
    std::fs::remove_file(project.root().join(".claude/tools/data/harness.json")).unwrap();
    let message = compile_in_place(&project).unwrap_err().message().to_owned();
    assert!(
        message.starts_with("compile requires an installed project harness at "),
        "{message}"
    );
}

/// Deliberate deviation. Upstream accepts any pinned number: one that does not
/// parse compares as `NaN`, the stage order becomes whatever the sort makes of
/// it, and the compile succeeds. Here the installed graph is refused.
#[test]
fn a_pinned_number_that_does_not_parse_is_refused() {
    let project = Project::installed("bad-pin");
    project.edit(
        ".claude/tools/data/stage-graph.json",
        "\"number\": \"2.3\"",
        "\"number\": \"two.three\"",
    );
    let message = compile_error(&project);
    assert!(
        message.ends_with(
            "pins stage \"requirements-analysis\" to \"two.three\", which is not <phase>.<index>"
        ),
        "{message}"
    );
}

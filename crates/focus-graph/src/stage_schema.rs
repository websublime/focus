//! Stage frontmatter: reading it and checking it.
//!
//! Mirrors `parseStageFrontmatter` (`aidlc-lib.ts` L36736) and
//! `validateStageFrontmatter` (`aidlc-stage-schema.ts`). The split is kept:
//! [`parse`] is a pure reader that passes unknown keys through, [`validate`]
//! is a pure checker that reports every problem it finds, in upstream's order
//! and wording, and only then yields the typed [`StageFrontmatter`].
//!
//! Two upstream behaviours are deliberately not reproduced, because both are
//! accidents of JavaScript's prototype chain rather than rules anyone wrote:
//!
//! - A frontmatter key named `__proto__` is silently dropped upstream (the
//!   assignment `obj["__proto__"] = "text"` is a no-op). Here it is an ordinary
//!   unknown key and is reported.
//! - A key named `constructor` is reported upstream as "reserved" with the
//!   source text of the `Object` function as the reason (`"constructor" in
//!   RESERVED_KEYS` is true through the prototype). Here it is an ordinary
//!   unknown key.

use focus_frontmatter as fm;
use focus_js::text::{describe, split_lines, trim};
use focus_js::{Result, bail, re};
use serde_json::{Map, Number, Value};

use crate::vocabulary::{UNIT_KINDS, artifact_filename};

/// The five lifecycle phases, in spine order. A stage's number prefix is its
/// phase's index here.
pub const PHASES: [&str; 5] = [
    "initialization",
    "ideation",
    "inception",
    "construction",
    "operation",
];
pub const VALID_EXECUTIONS: [&str; 2] = ["ALWAYS", "CONDITIONAL"];
pub const VALID_MODES: [&str; 5] = ["inline", "subagent", "pipeline", "mob", "agent-team"];
/// Topologies that are meaningless without collaborators.
pub const ENSEMBLE_MODES: [&str; 2] = ["pipeline", "mob"];
pub const VALID_CONDITIONAL_ON: [&str; 2] = ["brownfield", "greenfield"];
/// The conductor itself, named as `lead_agent` on the bootstrap stages. It has
/// no agent file by design, so the agent cross-check exempts it.
pub const RESERVED_AGENT_SLUG: &str = "orchestrator";
/// Keys held back for subsystems that do not exist yet, with the subsystem.
pub const RESERVED_KEYS: [(&str, &str); 4] = [
    ("on_failure", "loop driver"),
    ("blocks_on", "construction worktrees"),
    ("timeout", "sensor binding"),
    ("retry", "loop driver"),
];
pub const WHEN_PREDICATE_KEYS: [&str; 1] = ["producer-in-plan"];
pub const ARS_COMPONENT_KEYS: [&str; 5] = ["iae", "csu", "ve", "r", "ua"];
pub const ARS_ROLES: [&str; 4] = ["initialization", "core", "phase-gate", "structural"];
pub const ARS_PROJECT_TYPE_KEYS: [&str; 2] = ["brownfield", "greenfield"];
pub const ARS_COST_MIN: i64 = 1;
pub const ARS_COST_MAX: i64 = 5;
const ARS_KEYS: [&str; 4] = ["targets", "cost", "role", "project_types"];

const REQUIRED_FIELDS: [&str; 12] = [
    "slug",
    "phase",
    "execution",
    "condition",
    "lead_agent",
    "support_agents",
    "mode",
    "produces",
    "consumes",
    "requires_stage",
    "inputs",
    "outputs",
];
const OPTIONAL_FIELDS: [&str; 17] = [
    "number",
    "name",
    "plugin",
    "for_each",
    "workspace_requires",
    "optional_produces",
    "produces_kinds",
    "sensors",
    "scopes",
    "reviewer",
    "review_artifact",
    "reviewer_max_iterations",
    "review_class",
    "summary_confirmation",
    "when",
    "ars",
    "required_sections",
];

/// Kebab-case identifier: stage slugs and artifact names.
pub const SLUG_RE: &str = r"^[a-z][a-z0-9-]*$";
const NUMBER_RE: &str = r"^\d+\.\d+$";

/// One `consumes:` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Consume {
    pub artifact: String,
    pub required: bool,
    pub conditional_on: Option<String>,
}

/// A stage's frontmatter after it has passed [`validate`].
///
/// Enumerated fields stay strings: the compiled graph carries them verbatim,
/// and [`validate`] has already confined each to its allowed set.
#[derive(Debug, Clone, PartialEq)]
pub struct StageFrontmatter {
    pub slug: String,
    /// Authored ordering hint; the engine assigns every compiled number.
    pub number: Option<String>,
    pub name: Option<String>,
    pub plugin: Option<String>,
    pub phase: String,
    pub execution: String,
    pub condition: String,
    pub lead_agent: String,
    pub support_agents: Vec<String>,
    pub mode: String,
    pub for_each: Option<String>,
    pub workspace_requires: Option<bool>,
    pub produces: Vec<String>,
    pub optional_produces: Option<Vec<String>>,
    /// Artifact name to the unit kinds it applies to, in authored order.
    pub produces_kinds: Option<Map<String, Value>>,
    pub consumes: Vec<Consume>,
    pub requires_stage: Vec<String>,
    pub sensors: Option<Vec<String>>,
    pub scopes: Option<Vec<String>>,
    pub reviewer: Option<String>,
    pub review_artifact: Option<String>,
    pub reviewer_max_iterations: Option<Number>,
    pub review_class: Option<String>,
    pub summary_confirmation: Option<String>,
    /// The composer screening prior, compiled into the node verbatim.
    pub ars: Option<Value>,
    pub inputs: String,
    pub outputs: String,
}

/// Read a stage file's frontmatter into a loosely typed object.
///
/// No validation happens here. Unknown top-level keys are passed through so
/// [`validate`] can name them.
///
/// # Errors
///
/// The file has no frontmatter block, or a nested block (`consumes`,
/// `produces_kinds`, `ars`) is malformed.
pub fn parse(raw: &str) -> Result<Map<String, Value>> {
    let Some(block) = fm::frontmatter_block(raw) else {
        bail!("Stage file missing YAML frontmatter (---...---)");
    };

    let mut top_level: Vec<&str> = Vec::new();
    for line in split_lines(block) {
        if let Some(m) = re::find(r"^([a-z_][a-z0-9_]*)\s*:", "", line) {
            let key = re::group(line, &m, 1).unwrap_or("");
            if !top_level.contains(&key) {
                top_level.push(key);
            }
        }
    }
    let has = |key: &str| top_level.contains(&key);

    const ARRAY_KEYS: [&str; 5] = [
        "support_agents",
        "produces",
        "requires_stage",
        "sensors",
        "scopes",
    ];
    // Keys with their own reader below; the scalar loop would stamp them "".
    const STRUCTURED_KEYS: [&str; 6] = [
        "consumes",
        "when",
        "produces_kinds",
        "ars",
        "optional_produces",
        "required_sections",
    ];

    let mut obj = Map::new();
    for key in &top_level {
        if ARRAY_KEYS.contains(key) || STRUCTURED_KEYS.contains(key) {
            continue;
        }
        obj.insert(
            (*key).to_owned(),
            Value::String(fm::scalar_field(block, key)),
        );
    }

    // Required list fields are present even when empty; the validator rejects
    // an absent required field.
    for key in ARRAY_KEYS {
        obj.insert(key.to_owned(), string_list(fm::list_field(block, key)));
    }
    obj.insert(
        "consumes".to_owned(),
        Value::Array(fm::object_list_field(block, "consumes")?),
    );

    // Optional fields stay absent unless authored, so a stage that does not
    // use them compiles to the same bytes as before they existed.
    if has("optional_produces") {
        obj.insert(
            "optional_produces".to_owned(),
            string_list(fm::list_field(block, "optional_produces")),
        );
    }
    if has("produces_kinds") {
        obj.insert(
            "produces_kinds".to_owned(),
            Value::Object(fm::map_of_lists_field(block, "produces_kinds")?),
        );
    }
    if has("required_sections") {
        obj.insert(
            "required_sections".to_owned(),
            string_list(fm::list_field(block, "required_sections")),
        );
    }

    // The one numeric scalar: an integer literal becomes a number; anything
    // else stays a string for the validator to reject.
    if let Some(Value::String(raw)) = obj.get("reviewer_max_iterations")
        && re::test(r"^-?\d+$", "", raw)
    {
        let number = fm::js_number(raw);
        obj.insert("reviewer_max_iterations".to_owned(), number);
    }
    // The one boolean scalar, same discipline.
    if let Some(Value::String(raw)) = obj.get("workspace_requires")
        && (raw == "true" || raw == "false")
    {
        let flag = Value::Bool(raw == "true");
        obj.insert("workspace_requires".to_owned(), flag);
    }

    if has("when") {
        obj.insert("when".to_owned(), when_field(block));
    }
    if has("ars") {
        obj.insert("ars".to_owned(), fm::ars_field(block)?);
    }
    Ok(obj)
}

/// The `when:` single-key predicate map, in block or inline form; a value in
/// neither form is kept as the raw scalar so the validator rejects its shape.
fn when_field(block: &str) -> Value {
    let single = |key: &str, value: &str| {
        let mut map = Map::new();
        map.insert(key.to_owned(), Value::String(value.to_owned()));
        Value::Object(map)
    };
    if let Some(m) = re::find(
        r"^when:\s*\n\s+([a-z][a-z0-9-]*)\s*:\s*(.+?)\s*$",
        "m",
        block,
    ) {
        return single(
            re::group(block, &m, 1).unwrap_or(""),
            re::group(block, &m, 2).unwrap_or(""),
        );
    }
    if let Some(m) = re::find(
        r"^when:\s*\{\s*([a-z][a-z0-9-]*)\s*:\s*([^}]+?)\s*\}\s*$",
        "m",
        block,
    ) {
        return single(
            re::group(block, &m, 1).unwrap_or(""),
            trim(re::group(block, &m, 2).unwrap_or("")),
        );
    }
    Value::String(fm::scalar_field(block, "when"))
}

/// What [`validate`] can cross-check against.
#[derive(Debug, Clone, Copy, Default)]
pub struct ValidationContext<'a> {
    /// Known agent slugs. When set, `lead_agent`, every `support_agents` entry
    /// and `reviewer` must be one of them (or the reserved orchestrator).
    pub agents: Option<&'a [String]>,
}

/// Check a parsed frontmatter object against the stage schema.
///
/// # Errors
///
/// Every violation found, in upstream's order and wording. Collecting all of
/// them, instead of stopping at the first, is part of the contract: an author
/// fixes a stage file in one pass.
pub fn validate(
    obj: &Value,
    ctx: ValidationContext<'_>,
) -> std::result::Result<StageFrontmatter, Vec<String>> {
    let Value::Object(o) = obj else {
        return Err(vec![format!("expected object, got {}", describe(obj))]);
    };
    let mut errors: Vec<String> = Vec::new();
    let reserved = |key: &str| {
        RESERVED_KEYS
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, why)| *why)
    };

    for key in o.keys() {
        if let Some(why) = reserved(key) {
            errors.push(format!("{key} is reserved ({why}); not active yet"));
        }
    }
    for key in o.keys() {
        // `bundle` was the ownership key before the rename; name the fix.
        if key == "bundle" {
            errors.push("bundle: was renamed; write plugin: for ownership".to_owned());
            continue;
        }
        let known =
            REQUIRED_FIELDS.contains(&key.as_str()) || OPTIONAL_FIELDS.contains(&key.as_str());
        if !known && reserved(key).is_none() {
            errors.push(format!("unknown key: {key}"));
        }
    }
    for field in REQUIRED_FIELDS {
        if !o.contains_key(field) {
            errors.push(format!("missing required field: {field}"));
        }
    }

    check_string(o, "slug", &mut errors);
    check_pattern(o, "slug", SLUG_RE, "kebab-case", &mut errors);
    check_string(o, "number", &mut errors);
    check_pattern(
        o,
        "number",
        NUMBER_RE,
        "<phase-prefix>.<index>",
        &mut errors,
    );
    check_string(o, "name", &mut errors);
    check_string(o, "plugin", &mut errors);
    check_string(o, "phase", &mut errors);
    check_enum(o, "phase", &PHASES, &mut errors);
    check_string(o, "execution", &mut errors);
    check_enum(o, "execution", &VALID_EXECUTIONS, &mut errors);
    check_string(o, "condition", &mut errors);
    check_string(o, "lead_agent", &mut errors);
    check_string_array(o, "support_agents", &mut errors);
    check_string(o, "mode", &mut errors);
    check_enum(o, "mode", &VALID_MODES, &mut errors);

    let mode = o.get("mode").and_then(Value::as_str);
    let support_agents = o.get("support_agents").and_then(Value::as_array);
    if let Some(mode) = mode.filter(|m| ENSEMBLE_MODES.contains(m))
        && support_agents.is_none_or(Vec::is_empty)
    {
        errors.push(format!(
            "mode \"{mode}\" requires a non-empty support_agents"
        ));
    }
    if let (Some("pipeline"), Some(lead), Some(support)) = (
        mode,
        o.get("lead_agent").and_then(Value::as_str),
        support_agents,
    ) {
        let mut seen = vec![lead];
        for agent in support.iter().filter_map(Value::as_str) {
            if seen.contains(&agent) {
                errors.push(format!(
                    "mode \"pipeline\" requires unique lead/support chain entries; duplicate agent \"{agent}\""
                ));
                break;
            }
            seen.push(agent);
        }
    }

    if let Some(v) = o.get("for_each").filter(|v| !v.is_string()) {
        errors.push(format!("for_each must be string, got {}", describe(v)));
    }
    if let Some(v) = o.get("workspace_requires").filter(|v| !v.is_boolean()) {
        errors.push(format!(
            "workspace_requires must be boolean, got {}",
            describe(v)
        ));
    }

    check_string(o, "reviewer", &mut errors);
    check_string(o, "review_artifact", &mut errors);
    if let Some(v) = o.get("reviewer_max_iterations")
        && !is_integer_at_least(v, 1)
    {
        errors.push(format!(
            "reviewer_max_iterations must be a positive integer, got {}",
            describe(v)
        ));
    }
    if let Some(v) = o.get("summary_confirmation")
        && !matches!(v.as_str(), Some("required" | "if-present"))
    {
        errors.push(format!(
            "summary_confirmation must be one of required, if-present, got {}",
            describe(v)
        ));
    }

    let reviewer_declared = o.contains_key("reviewer");
    let review_artifact_declared = o.contains_key("review_artifact");
    if o.contains_key("reviewer_max_iterations") && !reviewer_declared {
        errors.push("reviewer_max_iterations requires a reviewer".to_owned());
    }
    if reviewer_declared && !review_artifact_declared {
        errors.push("reviewer requires review_artifact".to_owned());
    }
    if review_artifact_declared && !reviewer_declared {
        errors.push("review_artifact requires a reviewer".to_owned());
    }
    if let Some(v) = o.get("review_class") {
        if !matches!(v.as_str(), Some("adversarial" | "advisory")) {
            errors.push("review_class must be \"adversarial\" or \"advisory\"".to_owned());
        }
        if !reviewer_declared {
            errors.push("review_class requires a reviewer".to_owned());
        }
    }

    if o.contains_key("required_sections") {
        check_string_array(o, "required_sections", &mut errors);
        check_items(o, "required_sections", &mut errors, |s| trim(s).is_empty());
    }

    if let Some(when) = o.get("when") {
        check_when(when, &mut errors);
    }
    if let Some(ars) = o.get("ars") {
        check_ars(ars, &mut errors);
    }

    check_string_array(o, "produces", &mut errors);
    if let Some(optional) = o.get("optional_produces") {
        check_string_array(o, "optional_produces", &mut errors);
        for (i, name) in strings_with_index(optional) {
            if !re::test(SLUG_RE, "", name) {
                errors.push(format!(
                    "optional_produces[{i}] must be kebab-case, got \"{name}\""
                ));
            }
        }
    }
    if let Some(kinds) = o.get("produces_kinds") {
        check_produces_kinds(o, kinds, &mut errors);
    }
    if let Some(review_artifact) = o.get("review_artifact").and_then(Value::as_str) {
        check_review_artifact(o, review_artifact, &mut errors);
    }
    if let Some(consumes) = o.get("consumes") {
        check_consumes(consumes, &mut errors);
    }
    check_string_array(o, "requires_stage", &mut errors);
    for field in ["sensors", "scopes"] {
        if o.contains_key(field) {
            check_string_array(o, field, &mut errors);
            check_items(o, field, &mut errors, str::is_empty);
        }
    }
    check_string(o, "inputs", &mut errors);
    check_string(o, "outputs", &mut errors);

    if let Some(agents) = ctx.agents {
        let unknown = |slug: &str| slug != RESERVED_AGENT_SLUG && !agents.iter().any(|a| a == slug);
        if let Some(lead) = o
            .get("lead_agent")
            .and_then(Value::as_str)
            .filter(|a| unknown(a))
        {
            errors.push(format!(
                "lead_agent \"{lead}\" has no matching .claude/agents/*.md"
            ));
        }
        if let Some(support) = o.get("support_agents") {
            for (i, agent) in strings_with_index(support) {
                if unknown(agent) {
                    errors.push(format!(
                        "support_agents[{i}] \"{agent}\" has no matching .claude/agents/*.md"
                    ));
                }
            }
        }
        if let Some(reviewer) = o
            .get("reviewer")
            .and_then(Value::as_str)
            .filter(|a| unknown(a))
        {
            errors.push(format!(
                "reviewer \"{reviewer}\" has no matching .claude/agents/*.md"
            ));
        }
    }

    if errors.is_empty() {
        Ok(typed(o))
    } else {
        Err(errors)
    }
}

fn check_when(when: &Value, errors: &mut Vec<String>) {
    let Value::Object(map) = when else {
        errors.push(format!("when must be object, got {}", describe(when)));
        return;
    };
    if map.len() != 1 {
        errors.push(format!(
            "when must have exactly one predicate key, got {}",
            map.len()
        ));
    }
    for (key, value) in map {
        if !WHEN_PREDICATE_KEYS.contains(&key.as_str()) {
            errors.push(format!(
                "when has unknown predicate \"{key}\"; allowed: {}",
                WHEN_PREDICATE_KEYS.join(" | ")
            ));
        } else if value.as_str().is_none_or(|s| trim(s).is_empty()) {
            errors.push(format!("when.{key} must be a non-empty artifact slug"));
        }
    }
}

fn check_ars(ars: &Value, errors: &mut Vec<String>) {
    let Value::Object(map) = ars else {
        errors.push(format!("ars must be object, got {}", describe(ars)));
        return;
    };
    for key in map.keys() {
        if !ARS_KEYS.contains(&key.as_str()) {
            errors.push(format!(
                "ars has unknown key \"{key}\"; allowed: targets | cost | role | project_types"
            ));
        }
    }
    match map.get("targets") {
        None => errors.push("ars.targets is required (an empty list is allowed)".to_owned()),
        Some(targets) => {
            check_symbol_list(targets, "ars.targets", &ARS_COMPONENT_KEYS, false, errors)
        }
    }
    match map.get("cost") {
        None => errors.push(
            "ars.cost is required (null marks a stage that is not numerically screenable)"
                .to_owned(),
        ),
        Some(Value::Null) => {}
        Some(cost) => {
            let in_range = is_integer_at_least(cost, ARS_COST_MIN)
                && !is_integer_at_least(cost, ARS_COST_MAX + 1);
            if !in_range {
                errors.push(format!(
                    "ars.cost must be null or an integer {ARS_COST_MIN}..{ARS_COST_MAX}, got {}",
                    describe(cost)
                ));
            }
        }
    }
    if let Some(role) = map.get("role")
        && !role.as_str().is_some_and(|r| ARS_ROLES.contains(&r))
    {
        errors.push(format!(
            "ars.role must be one of {}, got {}",
            ARS_ROLES.join(" | "),
            describe(role)
        ));
    }
    if let Some(project_types) = map.get("project_types") {
        check_symbol_list(
            project_types,
            "ars.project_types",
            &ARS_PROJECT_TYPE_KEYS,
            true,
            errors,
        );
    }
}

/// A list of symbols from `allowed`, each at most once; every bad item is
/// reported under its own index.
fn check_symbol_list(
    value: &Value,
    field: &str,
    allowed: &[&str],
    non_empty: bool,
    errors: &mut Vec<String>,
) {
    let items = match value.as_array() {
        Some(items) if !(non_empty && items.is_empty()) => items,
        _ => {
            let qualifier = if non_empty { "non-empty " } else { "" };
            errors.push(format!(
                "{field} must be a {qualifier}list, got {}",
                describe(value)
            ));
            return;
        }
    };
    let mut seen: Vec<&str> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item.as_str().filter(|s| allowed.contains(s)) {
            None => errors.push(format!(
                "{field}[{i}] must be one of {}, got {}",
                allowed.join(" | "),
                describe(item)
            )),
            Some(symbol) if seen.contains(&symbol) => {
                errors.push(format!("{field}[{i}] repeats \"{symbol}\""))
            }
            Some(symbol) => seen.push(symbol),
        }
    }
}

/// Every `produces_kinds` key must name a declared output, and every value
/// must be a non-empty list of unit kinds. An orphan key is an error: a
/// silently ignored entry would switch the pruning off without a trace.
fn check_produces_kinds(o: &Map<String, Value>, kinds: &Value, errors: &mut Vec<String>) {
    let Value::Object(map) = kinds else {
        errors.push(format!(
            "produces_kinds must be object, got {}",
            describe(kinds)
        ));
        return;
    };
    let declared: Vec<&str> = ["produces", "optional_produces"]
        .into_iter()
        .filter_map(|field| o.get(field))
        .flat_map(|list| strings_with_index(list).map(|(_, s)| s))
        .collect();
    for (name, value) in map {
        if !re::test(SLUG_RE, "", name) {
            errors.push(format!("produces_kinds key \"{name}\" is not kebab-case"));
        } else if !declared.contains(&name.as_str()) {
            errors.push(format!("produces_kinds key \"{name}\" is not in produces"));
        }
        match value.as_array().filter(|list| !list.is_empty()) {
            None => errors.push(format!(
                "produces_kinds.{name} must be a non-empty list of unit kinds"
            )),
            Some(list) => {
                for kind in list {
                    if !kind.as_str().is_some_and(|k| UNIT_KINDS.contains(&k)) {
                        let shown = kind.as_str().unwrap_or_else(|| describe(kind));
                        errors.push(format!(
                            "produces_kinds.{name} lists unknown kind \"{shown}\""
                        ));
                    }
                }
            }
        }
    }
}

/// The reviewed artifact must be a required Markdown output, and for a
/// per-unit stage it must exist for every unit kind the stage applies to.
fn check_review_artifact(o: &Map<String, Value>, review_artifact: &str, errors: &mut Vec<String>) {
    let required: Vec<&str> = o
        .get("produces")
        .map(|list| strings_with_index(list).map(|(_, s)| s).collect())
        .unwrap_or_default();
    if !required.contains(&review_artifact) {
        errors.push(format!(
            "review_artifact \"{review_artifact}\" must name a required produces entry"
        ));
    }
    if !artifact_filename(review_artifact).ends_with(".md") {
        errors.push(format!(
            "review_artifact \"{review_artifact}\" must resolve to a Markdown artifact"
        ));
    }

    let per_unit = o.get("for_each").and_then(Value::as_str) == Some("unit-of-work");
    let Some(kinds_map) = o
        .get("produces_kinds")
        .and_then(Value::as_object)
        .filter(|_| per_unit)
    else {
        return;
    };
    // An artifact with no entry in the map applies to every kind.
    let kinds_of = |name: &str| -> Vec<&str> {
        match kinds_map.get(name).and_then(Value::as_array) {
            Some(list) => list.iter().filter_map(Value::as_str).collect(),
            None => UNIT_KINDS.to_vec(),
        }
    };
    let mut applicable: Vec<&str> = Vec::new();
    for name in &required {
        for kind in kinds_of(name) {
            if !applicable.contains(&kind) {
                applicable.push(kind);
            }
        }
    }
    let target = kinds_of(review_artifact);
    let missing: Vec<&str> = applicable
        .into_iter()
        .filter(|kind| !target.contains(kind))
        .collect();
    if !missing.is_empty() {
        errors.push(format!(
            "review_artifact \"{review_artifact}\" is pruned for applicable unit kinds: {}",
            missing.join(", ")
        ));
    }
}

fn check_consumes(consumes: &Value, errors: &mut Vec<String>) {
    let Value::Array(entries) = consumes else {
        errors.push(format!(
            "consumes must be array, got {}",
            describe(consumes)
        ));
        return;
    };
    for (i, entry) in entries.iter().enumerate() {
        let Value::Object(e) = entry else {
            errors.push(format!(
                "consumes[{i}] must be object, got {}",
                describe(entry)
            ));
            continue;
        };
        match e.get("artifact") {
            None => errors.push(format!("consumes[{i}].artifact missing")),
            Some(Value::String(artifact)) => {
                if !re::test(SLUG_RE, "", artifact) {
                    errors.push(format!(
                        "consumes[{i}].artifact must be kebab-case, got \"{artifact}\""
                    ));
                }
            }
            Some(other) => errors.push(format!(
                "consumes[{i}].artifact must be string, got {}",
                describe(other)
            )),
        }
        match e.get("required") {
            None => errors.push(format!("consumes[{i}].required missing")),
            Some(Value::Bool(_)) => {}
            Some(other) => errors.push(format!(
                "consumes[{i}].required must be boolean, got {}",
                describe(other)
            )),
        }
        match e.get("conditional_on") {
            None => {}
            Some(Value::String(on)) => {
                if !VALID_CONDITIONAL_ON.contains(&on.as_str()) {
                    errors.push(format!(
                        "consumes[{i}].conditional_on must be one of {}, got \"{on}\"",
                        VALID_CONDITIONAL_ON.join(" | ")
                    ));
                }
            }
            Some(other) => {
                errors.push(format!(
                    "consumes[{i}].conditional_on must be string, got {}",
                    describe(other)
                ));
            }
        }
    }
}

fn check_string(o: &Map<String, Value>, field: &str, errors: &mut Vec<String>) {
    if let Some(v) = o.get(field).filter(|v| !v.is_string()) {
        errors.push(format!("{field} must be string, got {}", describe(v)));
    }
}

fn check_string_array(o: &Map<String, Value>, field: &str, errors: &mut Vec<String>) {
    let Some(v) = o.get(field) else { return };
    let Value::Array(items) = v else {
        errors.push(format!("{field} must be array, got {}", describe(v)));
        return;
    };
    for (i, item) in items.iter().enumerate() {
        if !item.is_string() {
            errors.push(format!(
                "{field}[{i}] must be string, got {}",
                describe(item)
            ));
        }
    }
}

/// Report `<field>[i] must be non-empty` for each string item `is_blank` flags.
fn check_items(
    o: &Map<String, Value>,
    field: &str,
    errors: &mut Vec<String>,
    is_blank: impl Fn(&str) -> bool,
) {
    let Some(list) = o.get(field) else { return };
    for (i, item) in strings_with_index(list) {
        if is_blank(item) {
            errors.push(format!("{field}[{i}] must be non-empty"));
        }
    }
}

fn check_enum(o: &Map<String, Value>, field: &str, allowed: &[&str], errors: &mut Vec<String>) {
    // A non-string was already reported by check_string.
    if let Some(v) = o.get(field).and_then(Value::as_str)
        && !allowed.contains(&v)
    {
        errors.push(format!(
            "{field} must be one of {}, got \"{v}\"",
            allowed.join(" | ")
        ));
    }
}

fn check_pattern(
    o: &Map<String, Value>,
    field: &str,
    pattern: &str,
    shape: &str,
    errors: &mut Vec<String>,
) {
    if let Some(v) = o.get(field).and_then(Value::as_str)
        && !re::test(pattern, "", v)
    {
        errors.push(format!("{field} must be {shape}, got \"{v}\""));
    }
}

/// `typeof v === "number" && Number.isInteger(v) && v >= min`.
fn is_integer_at_least(v: &Value, min: i64) -> bool {
    match v {
        Value::Number(n) => match (n.as_i64(), n.as_u64(), n.as_f64()) {
            (Some(i), _, _) => i >= min,
            (None, Some(_), _) => true,
            #[allow(clippy::cast_precision_loss)]
            (None, None, Some(f)) => f.fract() == 0.0 && f >= min as f64,
            (None, None, None) => false,
        },
        _ => false,
    }
}

/// The string items of a list value, each with its index in the list.
fn strings_with_index(list: &Value) -> impl Iterator<Item = (usize, &str)> {
    list.as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, item)| item.as_str().map(|s| (i, s)))
}

fn string_list(items: Vec<String>) -> Value {
    Value::Array(items.into_iter().map(Value::String).collect())
}

/// Lift a validated object into the typed form. Every access below is backed
/// by a check in [`validate`]; this is the single trust boundary.
fn typed(o: &Map<String, Value>) -> StageFrontmatter {
    let string = |field: &str| o.get(field).and_then(Value::as_str).map(str::to_owned);
    let required = |field: &str| string(field).unwrap_or_default();
    let list = |field: &str| -> Option<Vec<String>> {
        o.get(field).and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
    };
    let consumes = o
        .get("consumes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|e| Consume {
            artifact: e
                .get("artifact")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            required: e
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or_default(),
            conditional_on: e
                .get("conditional_on")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
        .collect();
    StageFrontmatter {
        slug: required("slug"),
        number: string("number"),
        name: string("name"),
        plugin: string("plugin"),
        phase: required("phase"),
        execution: required("execution"),
        condition: required("condition"),
        lead_agent: required("lead_agent"),
        support_agents: list("support_agents").unwrap_or_default(),
        mode: required("mode"),
        for_each: string("for_each"),
        workspace_requires: o.get("workspace_requires").and_then(Value::as_bool),
        produces: list("produces").unwrap_or_default(),
        optional_produces: list("optional_produces"),
        produces_kinds: o.get("produces_kinds").and_then(Value::as_object).cloned(),
        consumes,
        requires_stage: list("requires_stage").unwrap_or_default(),
        sensors: list("sensors"),
        scopes: list("scopes"),
        reviewer: string("reviewer"),
        review_artifact: string("review_artifact"),
        reviewer_max_iterations: o
            .get("reviewer_max_iterations")
            .and_then(Value::as_number)
            .cloned(),
        review_class: string("review_class"),
        summary_confirmation: string("summary_confirmation"),
        ars: o.get("ars").cloned(),
        inputs: required("inputs"),
        outputs: required("outputs"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = "slug: s\nphase: inception\nexecution: ALWAYS\ncondition: c\nlead_agent: orchestrator\nsupport_agents: []\nmode: inline\nproduces: []\nconsumes: []\nrequires_stage: []\ninputs: i\noutputs: o";

    fn errors_for(extra: &str) -> Vec<String> {
        let parsed = parse(&format!("---\n{MINIMAL}\n{extra}\n---\n")).unwrap();
        validate(&Value::Object(parsed), ValidationContext::default())
            .err()
            .unwrap_or_default()
    }

    #[test]
    fn a_minimal_stage_is_valid_and_typed() {
        let parsed = parse(&format!("---\n{MINIMAL}\n---\n")).unwrap();
        let stage = validate(&Value::Object(parsed), ValidationContext::default()).unwrap();
        assert_eq!(stage.slug, "s");
        assert_eq!(stage.phase, "inception");
        assert!(stage.support_agents.is_empty() && stage.sensors.as_deref() == Some(&[][..]));
        assert_eq!(stage.optional_produces, None);
    }

    #[test]
    fn every_violation_is_reported_in_one_pass() {
        // Checked against upstream's validator on the same document.
        let doc = format!(
            "---\n{}\nreviewer_max_iterations: 0\nreview_class: gentle\n---\n",
            MINIMAL.replace("mode: inline", "mode: mob")
        );
        let errors = validate(
            &Value::Object(parse(&doc).unwrap()),
            ValidationContext::default(),
        )
        .unwrap_err();
        assert_eq!(
            errors,
            [
                "mode \"mob\" requires a non-empty support_agents",
                "reviewer_max_iterations must be a positive integer, got number",
                "reviewer_max_iterations requires a reviewer",
                "review_class must be \"adversarial\" or \"advisory\"",
                "review_class requires a reviewer",
            ]
        );
    }

    #[test]
    fn a_repeated_key_reads_its_first_occurrence() {
        // The scalar reader takes the first `mode:` line, so the later one
        // changes nothing and the document stays valid.
        assert_eq!(errors_for("mode: mob"), Vec::<String>::new());
    }

    /// Deliberate deviation: upstream drops a `__proto__` key without a word
    /// and accepts the file.
    #[test]
    fn a_proto_key_is_an_unknown_key() {
        assert_eq!(errors_for("__proto__: x"), ["unknown key: __proto__"]);
    }

    /// Deliberate deviation: upstream reports `constructor` as reserved, with
    /// the source text of JavaScript's `Object` function as the reason.
    #[test]
    fn a_constructor_key_is_an_unknown_key() {
        assert_eq!(errors_for("constructor: x"), ["unknown key: constructor"]);
    }

    #[test]
    fn non_objects_are_refused_by_shape() {
        let refuse = |v: Value| validate(&v, ValidationContext::default()).unwrap_err();
        assert_eq!(refuse(Value::Null), ["expected object, got null"]);
        assert_eq!(
            refuse(serde_json::json!([])),
            ["expected object, got array"]
        );
        assert_eq!(
            refuse(serde_json::json!("x")),
            ["expected object, got string"]
        );
    }
}

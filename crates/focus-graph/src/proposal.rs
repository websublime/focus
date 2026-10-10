//! Validating a composer proposal: a grid plus the settings a tailored plan
//! would run with.
//!
//! The composer agent proposes; this code decides whether the proposal can
//! reach a person, and returns every number and route the approval gate
//! shows, so the gate never displays something a model computed. Mirrors the
//! `validate-grid` handler and its helpers in `aidlc-graph.ts` (L1777–1965,
//! L3636) and `aidlc-lib.ts` (`planChangesBetween`, `scopeSettingsOffList`).

use focus_js::text::trim;
use focus_js::{Result, json, num};
use serde_json::{Map, Value};

use crate::query::{GridOptions, Nearest, validate_grid};
use crate::runtime::{ScopeDefinition, Stage, column};
use crate::scopes::{CEREMONY_KEYS, GuardPolicy, ScopeMetadata};

/// The settings a proposal carries beside its grid, in the order they are
/// echoed. The names are the scope-file keys, so approved values are copied
/// into a scope file unchanged.
pub const SCOPE_SETTING_KEYS: [&str; 6] = [
    "sensors",
    "learnings",
    "summary_confirmation",
    "plan_approval",
    "collaborators",
    "review_cap",
];

/// The stock scopes a code-findings report may run on.
pub const REPORT_FIX_SCOPES: [&str; 2] = ["bugfix", "security-patch"];

const DEPTHS: [&str; 3] = ["minimal", "standard", "comprehensive"];

/// What the command line asked for.
#[derive(Debug, Clone, Default)]
pub struct ProposalRequest<'a> {
    pub strict: bool,
    pub project_type: Option<&'a str>,
    /// The proposal comes from a code-findings report.
    pub report: bool,
    /// `--matched <stock>`: the proposal claims to be that stock scope.
    pub matched: Option<&'a str>,
    /// `--custom`: the proposal is a plan of its own.
    pub custom: bool,
    /// `--keywords`: keywords the new scope would claim.
    pub keywords: Option<Vec<String>>,
    /// `--guard-policy <value>`; a `guardPolicy` member is read otherwise.
    pub guard_policy: Option<&'a str>,
}

/// What the validation reads from the install.
pub struct ProposalWorld<'a> {
    pub graph: &'a [Stage],
    pub grid: &'a Map<String, Value>,
    /// Enabled scopes with their columns.
    pub mapping: &'a [ScopeDefinition],
    /// The first memory layer that pins Guard Policy to strict, as the
    /// refusal sentence. Called only for a relaxed or off proposal.
    pub memory_strict: &'a dyn Fn() -> Result<Option<String>>,
    /// Whether a kill switch forces a ceremony off on this machine, and the
    /// name of the switch. Fails when the settings that could record one
    /// cannot be read.
    pub kill_switch: &'a dyn Fn(&str) -> Result<Option<String>>,
}

/// `String(value)` for a JSON value.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => num::to_string(n.as_f64().unwrap_or(f64::NAN)),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// The grid inside a proposal: the document itself, or its `stages` member
/// when it has one (the shape of a scope-grid entry). `None` when what is
/// left is not an object. Every action is read as text, so a wrong type is
/// reported as an invalid action instead of crashing the validation.
#[must_use]
pub fn grid_of(proposal: &Value) -> Option<Map<String, Value>> {
    let raw = match proposal.get("stages") {
        Some(stages @ (Value::Object(_) | Value::Array(_))) if proposal.is_object() => stages,
        _ => proposal,
    };
    Some(
        raw.as_object()?
            .iter()
            .map(|(slug, action)| (slug.clone(), Value::String(js_string(action))))
            .collect(),
    )
}

fn metadata<'a>(mapping: &'a [ScopeDefinition], scope: &str) -> Option<&'a ScopeMetadata> {
    mapping
        .iter()
        .map(|def| &def.meta)
        .find(|meta| meta.name == scope)
}

/// A scope's settings as the runtime resolves them: a missing ceremony line
/// is on, a missing review cap is adversarial.
fn scope_settings_of(mapping: &[ScopeDefinition], scope: &str) -> Option<Map<String, Value>> {
    let meta = metadata(mapping, scope)?;
    let mut out: Map<String, Value> = CEREMONY_KEYS
        .iter()
        .map(|key| ((*key).to_owned(), Value::from(meta.ceremony_setting(key))))
        .collect();
    out.insert(
        "review_cap".to_owned(),
        Value::from(meta.review_cap.as_deref().unwrap_or("adversarial")),
    );
    Some(out)
}

/// A scope's Guard Policy default: what its file declares, else off; strict
/// for a scope with no file here. The name is matched trimmed and lowercased,
/// as upstream matches it.
fn guard_policy_default(mapping: &[ScopeDefinition], scope: &str) -> GuardPolicy {
    let wanted = trim(scope).to_lowercase();
    mapping
        .iter()
        .find(|def| def.meta.name == wanted)
        .map_or(GuardPolicy::Strict, ScopeDefinition::guard_policy)
}

/// Check a proposal's `scopeSettings`. Every key is required, so the gate row
/// and the scope file name the same values, and each value must be the exact
/// word the scope loader accepts.
#[must_use]
pub fn validate_scope_settings(raw: &Value) -> (Option<Map<String, Value>>, Vec<String>) {
    let expected = SCOPE_SETTING_KEYS.join(", ");
    let Value::Object(entries) = raw else {
        return (
            None,
            vec![format!(
                "Scope settings must be an object naming {expected}."
            )],
        );
    };
    let mut errors = Vec::new();
    for key in entries.keys() {
        if !SCOPE_SETTING_KEYS.contains(&key.as_str()) {
            errors.push(format!(
                "Scope settings name unknown key \"{key}\" (expected {expected})."
            ));
        }
    }
    let missing: Vec<&str> = SCOPE_SETTING_KEYS
        .into_iter()
        .filter(|key| !entries.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        errors.push(format!(
            "Scope settings are missing {}. Name every setting.",
            missing.join(", ")
        ));
    }
    for key in SCOPE_SETTING_KEYS {
        let Some(value) = entries.get(key) else {
            continue;
        };
        let allowed: &[&str] = if key == "review_cap" {
            &["adversarial", "advisory", "none"]
        } else {
            &["on", "off"]
        };
        if !value.as_str().is_some_and(|word| allowed.contains(&word)) {
            errors.push(format!(
                "Scope setting {key} must be one of: {} (got {}).",
                allowed.join(", "),
                json::stringify(value)
            ));
        }
    }
    if !errors.is_empty() {
        return (None, errors);
    }
    // Rebuilt in key order, so the echo reads the same whatever order the
    // proposal used.
    let settings = SCOPE_SETTING_KEYS
        .into_iter()
        .filter_map(|key| entries.get(key).map(|v| (key.to_owned(), v.clone())))
        .collect();
    (Some(settings), errors)
}

/// The ceremonies a set of settings switches off, as the labels a plan's
/// summary shows.
fn settings_off_list(settings: &Map<String, Value>) -> Vec<String> {
    let is = |key: &str, word: &str| settings.get(key).is_some_and(|v| v == word);
    [
        (is("review_cap", "none"), "reviewers"),
        (is("sensors", "off"), "sensors"),
        (is("learnings", "off"), "learnings ritual"),
        (is("summary_confirmation", "off"), "summary confirmation"),
        (is("plan_approval", "off"), "plan approval"),
        (is("collaborators", "off"), "collaborators"),
    ]
    .into_iter()
    .filter(|(off, _)| *off)
    .map(|(_, label)| label.to_owned())
    .collect()
}

/// Plan approval off, where the scope the plan runs on asks for it, would let
/// a command the agent runs lower the person's own approval. Only the person
/// turns it off.
fn plan_approval_lowering_error(
    mapping: &[ScopeDefinition],
    scope: &str,
    settings: Option<&Map<String, Value>>,
) -> Option<String> {
    let proposes_off = settings.is_some_and(|s| s.get("plan_approval").is_some_and(|v| v == "off"));
    let stock_on = scope_settings_of(mapping, scope)
        .is_some_and(|s| s.get("plan_approval").is_some_and(|v| v == "on"));
    (proposes_off && stock_on).then(|| {
        format!(
            "Stock scope \"{scope}\" asks the person to approve each code plan, but the proposal shows plan_approval off. Show on: only the person turns plan approval off, and their own words at the gate are recorded and applied when the work is created."
        )
    })
}

/// The settings a proposal changes from the stock scope it runs on, applied
/// to this piece of work when it is created.
fn creation_settings_for(
    mapping: &[ScopeDefinition],
    stock_scope: &str,
    settings: &Map<String, Value>,
) -> Map<String, Value> {
    let Some(stock) = scope_settings_of(mapping, stock_scope) else {
        return Map::new();
    };
    let mut changes = Map::new();
    for key in CEREMONY_KEYS {
        if settings.get(key) != stock.get(key)
            && let Some(value) = settings.get(key)
        {
            changes.insert(key.to_owned(), value.clone());
        }
    }
    if settings.get("review_cap") != stock.get("review_cap")
        && let Some(value) = settings.get("review_cap")
    {
        changes.insert("review".to_owned(), value.clone());
    }
    changes
}

/// The errors of a proposal that names its routing (`--matched` or
/// `--custom`). Either route needs the settings and a Guard Policy, so the
/// gate never shows a row nobody checked. A matched proposal must be its
/// stock scope's grid exactly, and may not show a Guard Policy below that
/// scope's default.
fn route_errors(
    world: &ProposalWorld<'_>,
    matched: Option<&str>,
    has_settings: bool,
    has_guard_policy: bool,
    guard_policy: Option<GuardPolicy>,
    nearest: &[Nearest],
    settings: Option<&Map<String, Value>>,
) -> Vec<String> {
    let route = if matched.is_some() {
        "matched"
    } else {
        "custom"
    };
    let mut errors = Vec::new();
    if !has_settings {
        errors.push(format!(
            "A {route} proposal must carry scopeSettings ({}).",
            SCOPE_SETTING_KEYS.join(", ")
        ));
    }
    if !has_guard_policy {
        errors.push(format!(
            "A {route} proposal must carry a Guard Policy (--guard-policy or a guardPolicy member)."
        ));
    }
    let Some(matched) = matched else {
        return errors;
    };
    let Some(entry) = nearest.iter().find(|candidate| candidate.scope == matched) else {
        errors.push(format!(
            "--matched names \"{matched}\", which is not a stock scope."
        ));
        return errors;
    };
    if entry.diff > 0 {
        errors.push(format!(
            "A matched proposal carries stock scope \"{matched}\"'s grid verbatim; this grid differs on {}. Adopt the stock grid, or propose it as custom.",
            entry.differs.join(", ")
        ));
    }
    if let Some(policy) = guard_policy {
        let stock = guard_policy_default(world.mapping, matched);
        if !policy.at_least(stock) {
            errors.push(format!(
                "Stock scope \"{matched}\" defaults Guard Policy to {stock}, but the proposal shows {policy}. Show {stock}{}, or propose it as custom.",
                if stock == GuardPolicy::Strict {
                    ""
                } else {
                    " (or a stricter value, which creation applies)"
                },
                stock = stock.as_str(),
                policy = policy.as_str(),
            ));
        }
    }
    errors.extend(plan_approval_lowering_error(
        world.mapping,
        matched,
        settings,
    ));
    errors
}

/// The stock scope a custom plan runs on when the person approves it without
/// saving it as a scope, and the stage changes that turn that scope's grid
/// into the plan.
///
/// It is the nearest stock scope whose Guard Policy default is the plan's or
/// lower (so creating the work never lowers anything) and that adds nothing
/// the gate does not show: no walking-skeleton checkpoint, no test strategy
/// other than the plan's depth. On a new project, a scope meant for existing
/// code is passed over when another qualifies.
fn custom_plan_base(
    world: &ProposalWorld<'_>,
    proposal: &Map<String, Value>,
    guard_policy: GuardPolicy,
    nearest: &[Nearest],
    depth: &str,
    project_type: Option<&str>,
) -> std::result::Result<(String, Map<String, Value>), String> {
    let skipped_init: Vec<&str> = world
        .graph
        .iter()
        .filter(|stage| {
            stage.phase == "initialization"
                && !proposal.get(&stage.slug).is_some_and(|a| a == "EXECUTE")
        })
        .map(|stage| stage.slug.as_str())
        .collect();
    if !skipped_init.is_empty() {
        return Err(format!(
            "A plan cannot skip initialization stages ({}); they always run.",
            skipped_init.join(", ")
        ));
    }
    let adds_nothing = |scope: &str| {
        metadata(world.mapping, scope).is_none_or(|meta| {
            !meta.skeleton
                && meta
                    .test_strategy
                    .as_ref()
                    .is_none_or(|strategy| strategy.to_lowercase() == depth.to_lowercase())
        })
    };
    let qualifies = |scope: &str| {
        guard_policy.at_least(guard_policy_default(world.mapping, scope)) && adds_nothing(scope)
    };
    let fits_new_work = |scope: &str| {
        project_type != Some("greenfield")
            || !metadata(world.mapping, scope).is_some_and(|meta| meta.existing_code)
    };
    let base = nearest
        .iter()
        .find(|c| qualifies(&c.scope) && fits_new_work(&c.scope))
        .or_else(|| nearest.iter().find(|c| qualifies(&c.scope)));
    let Some(base) = base else {
        return Err(format!(
            "No stock scope here defaults Guard Policy to {} or lower without a walking skeleton or a test strategy other than the plan's depth, so a plan for this piece of work cannot carry it. Propose strict, or a value at or above such a stock scope's default.",
            guard_policy.as_str()
        ));
    };

    let base_cells = column(world.grid, &base.scope);
    let executes = |cells: Option<&Map<String, Value>>, slug: &str| {
        cells
            .and_then(|cells| cells.get(slug))
            .is_some_and(|action| action == "EXECUTE")
    };
    let (mut skip, mut add) = (Vec::new(), Vec::new());
    for stage in world.graph {
        let from = executes(base_cells, &stage.slug);
        let to = executes(Some(proposal), &stage.slug);
        if from && !to {
            skip.push(Value::from(stage.slug.clone()));
        }
        if !from && to {
            add.push(Value::from(stage.slug.clone()));
        }
    }
    let mut changes = Map::new();
    changes.insert("skip".to_owned(), Value::Array(skip));
    changes.insert("add".to_owned(), Value::Array(add));
    Ok((base.scope.clone(), changes))
}

/// Keywords a new scope would claim that an existing scope already claims.
/// Scope inference takes the first alphabetical keyword match, so a shared
/// keyword would shadow the incumbent on every future cold start; it is an
/// error naming the scope that holds it. Compared case-insensitively.
fn keyword_collisions(mapping: &[ScopeDefinition], granted: &[String]) -> Vec<String> {
    let mut errors = Vec::new();
    for keyword in granted {
        let mut holders: Vec<&str> = mapping
            .iter()
            .filter(|def| {
                def.meta
                    .keywords
                    .iter()
                    .any(|k| k.to_lowercase() == keyword.to_lowercase())
            })
            .map(|def| def.meta.name.as_str())
            .collect();
        holders.sort_by(|a, b| focus_js::order::cmp_utf16(a, b));
        if !holders.is_empty() {
            errors.push(format!(
                "Keyword \"{keyword}\" is already claimed by scope{} [{}] - granting it would shadow that scope in keyword inference. Pick a keyword no existing scope claims.",
                if holders.len() > 1 { "s" } else { "" },
                holders.join(", ")
            ));
        }
    }
    errors
}

fn strings(items: &[String]) -> Value {
    Value::Array(items.iter().cloned().map(Value::String).collect())
}

/// Validate `proposal` (the whole document; its grid is `grid`) and return
/// the result document and whether it is valid.
///
/// # Errors
///
/// Only when a memory file cannot be judged (see [`crate::guard_memory`]) or
/// declares an invalid mode. Every problem with the proposal itself is in the
/// result's `errors`.
pub fn validate(
    proposal: &Value,
    grid: &Map<String, Value>,
    request: &ProposalRequest<'_>,
    world: &ProposalWorld<'_>,
) -> Result<(Value, bool)> {
    let checked = validate_grid(
        grid,
        GridOptions {
            project_type: request.project_type,
            strict: request.strict,
            label: None,
        },
        world.graph,
        world.grid,
    );
    let mut errors = checked.errors;
    let mut advisories = checked.advisories;
    let mut summary = checked.summary;
    let nearest_all = checked.nearest_stock;

    // A code-findings report is a fix: its match, and a custom plan's base,
    // come only from the fix scopes, never from a lighter scope whose grid
    // happens to sit nearer.
    let nearest: Vec<Nearest> = if request.report {
        nearest_all
            .iter()
            .filter(|row| REPORT_FIX_SCOPES.contains(&row.scope.as_str()))
            .cloned()
            .collect()
    } else {
        nearest_all.clone()
    };
    if let Some(matched) = request.matched.filter(|_| request.report)
        && !REPORT_FIX_SCOPES.contains(&matched)
    {
        errors.push(format!(
                "A code-findings report runs on {}, so it cannot be matched to \"{matched}\". Adopt the nearest of those, or propose it as custom.",
                REPORT_FIX_SCOPES.join(" or ")
            ));
    }
    if let Some(keywords) = &request.keywords {
        errors.extend(keyword_collisions(world.mapping, keywords));
    }

    // The Guard Policy rides with the grid: a flag, or a member beside
    // `stages` (`changeControl` is the retired spelling).
    let member = |key: &str| proposal.get(key).and_then(Value::as_str);
    let policy_raw = request
        .guard_policy
        .or_else(|| member("guardPolicy"))
        .or_else(|| member("changeControl"));
    let mut guard_policy: Option<GuardPolicy> = None;
    if let Some(raw) = policy_raw {
        match GuardPolicy::parse(raw) {
            None => errors.push(format!(
                "Guard Policy must be one of: strict, relaxed, off (got \"{raw}\")."
            )),
            Some(policy) => {
                guard_policy = Some(policy);
                if policy != GuardPolicy::Strict {
                    errors.extend((world.memory_strict)()?);
                }
            }
        }
    }

    let settings_raw = proposal.get("scopeSettings");
    let mut settings: Option<Map<String, Value>> = None;
    if let Some(raw) = settings_raw {
        let (parsed, setting_errors) = validate_scope_settings(raw);
        errors.extend(setting_errors);
        if let Some(parsed) = parsed {
            summary.off = settings_off_list(&parsed);
            for key in CEREMONY_KEYS {
                if parsed.get(key).is_some_and(|v| v == "on")
                    && let Some(switch) = (world.kill_switch)(key)?
                {
                    advisories.push(format!(
                            "{key} is on in these settings, but {switch} forces it off on this machine; the scope still stores on, and the ceremony runs once that switch is cleared."
                        ));
                }
            }
            settings = Some(parsed);
        }
    }

    // What a custom plan starts from: classic's policy and settings, echoed
    // on every run that is not --matched.
    let custom_start = if request.matched.is_none() {
        scope_settings_of(world.mapping, "classic").map(|classic| {
            let mut start = Map::new();
            start.insert(
                "guard_policy".to_owned(),
                Value::from(guard_policy_default(world.mapping, "classic").as_str()),
            );
            start.insert("scope_settings".to_owned(), Value::Object(classic));
            Value::Object(start)
        })
    } else {
        None
    };

    let mut routing: Vec<(&str, Value)> = Vec::new();
    if request.matched.is_some() || request.custom {
        let route = route_errors(
            world,
            request.matched,
            settings_raw.is_some(),
            policy_raw.is_some(),
            guard_policy,
            &nearest_all,
            settings.as_ref(),
        );
        errors.extend(route.iter().cloned());

        // A custom plan names its own depth: its base is picked by distance
        // and Guard Policy, so the base's depth may not be the one it needs.
        let depth_word = member("depth").map(|d| trim(d).to_lowercase());
        let plan_depth = depth_word
            .as_deref()
            .and_then(|word| DEPTHS.into_iter().find(|d| *d == word));
        if request.matched.is_none() && plan_depth.is_none() {
            errors.push(
                "A custom proposal must carry its depth: a depth member of minimal, standard, or comprehensive."
                    .to_owned(),
            );
        }
        let base = match (request.matched, guard_policy, plan_depth) {
            (None, Some(policy), Some(depth)) if route.is_empty() => Some(custom_plan_base(
                world,
                grid,
                policy,
                &nearest,
                depth,
                request.project_type,
            )),
            _ => None,
        };
        match &base {
            Some(Err(error)) => errors.push(error.clone()),
            Some(Ok((scope, _))) => {
                errors.extend(plan_approval_lowering_error(
                    world.mapping,
                    scope,
                    settings.as_ref(),
                ));
            }
            None => {}
        }
        if let (true, true, Some(settings)) = (errors.is_empty(), route.is_empty(), &settings) {
            if let Some(matched) = request.matched {
                routing.push(("routing", Value::from("matched")));
                routing.push(("matched_scope", Value::from(matched)));
                routing.push((
                    "creation_settings",
                    Value::Object(creation_settings_for(world.mapping, matched, settings)),
                ));
            } else {
                routing.push(("routing", Value::from("custom")));
                if let Some(Ok((scope, changes))) = &base {
                    routing.push(("base_scope", Value::from(scope.clone())));
                    routing.push(("plan_changes", Value::Object(changes.clone())));
                    routing.push((
                        "creation_settings",
                        Value::Object(creation_settings_for(world.mapping, scope, settings)),
                    ));
                    let base_depth = metadata(world.mapping, scope)
                        .map(|meta| meta.depth.to_lowercase())
                        .unwrap_or_default();
                    if let Some(depth) = plan_depth.filter(|depth| *depth != base_depth) {
                        routing.push(("creation_depth", Value::from(depth)));
                    }
                }
            }
        }
    }

    let valid = errors.is_empty();
    let mut out = Map::new();
    out.insert("valid".to_owned(), Value::Bool(valid));
    out.insert("errors".to_owned(), strings(&errors));
    out.insert("advisories".to_owned(), strings(&advisories));
    out.insert("summary".to_owned(), summary.to_json());
    out.insert(
        "nearest_stock".to_owned(),
        Value::Array(nearest.iter().map(Nearest::to_json).collect()),
    );
    if let Some(policy) = guard_policy {
        out.insert("change_control".to_owned(), Value::from(policy.as_str()));
        out.insert("guard_policy".to_owned(), Value::from(policy.as_str()));
    }
    if let Some(settings) = settings {
        out.insert("scope_settings".to_owned(), Value::Object(settings));
    }
    if let Some(start) = custom_start {
        out.insert("custom_start".to_owned(), start);
    }
    for (key, value) in routing {
        out.insert(key.to_owned(), value);
    }
    Ok((Value::Object(out), valid))
}

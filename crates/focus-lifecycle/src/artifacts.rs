//! Where a stage's artifacts are, and whether its work is there.
//!
//! An ordinary stage writes under `<record>/<phase>/<slug>/`; the code
//! knowledge base stage writes under the space's `codekb/<repo>/`; a stage
//! that runs per unit of work writes under `<record>/construction/<unit>/
//! <slug>/`, one directory per unit the Bolt DAG names. A code-producing
//! stage (`workspace_requires`) must also leave source work outside the
//! workspace tree, which git can tell from a brownfield baseline where a
//! bare listing cannot.
//!
//! Mirrors `producesDirsForStage`, `producesArtifactsExist`,
//! `workspaceHasSourceFile`, `gitHasSourceWork`, `dirHasSourceWork` and
//! `workspaceHasWork` in `aidlc-state.ts`, `resolveArtifactInstances` in
//! `aidlc-artifact-resolution.ts`, and `resolveBoltDag`, `codekbRepoName`,
//! `codekbDir`, `usesStageLevelPerUnitArtifacts` and `filterProducesByKind`
//! in `aidlc-lib.ts`.

use std::path::{Path, PathBuf};
use std::process::Command;

use focus_audit::units::parse_bolt_dag;
use focus_graph::runtime::Stage;
use focus_graph::vocabulary::artifact_filename;
use focus_intent::create::is_valid_repo_name;
use focus_js::order::{cmp_utf16, locale_compare};
use focus_js::text::trim;
use focus_js::{Result, bail, json};
use focus_state::fields::get_field;
use focus_state::progress::{CheckboxState, parse_checkboxes, parse_stage_suffixes};
use focus_workspace::names::WORKSPACE_DIR;
use focus_workspace::records::read_intent_registry;
use serde_json::{Map, Value};

use crate::registry::intent_repos;
use crate::stage;
use crate::workflow::Workflow;

/// `KNOWN_CODEKB_STAGES`: the stages whose artifacts live in the code
/// knowledge base.
pub const KNOWN_CODEKB_STAGES: [&str; 1] = ["reverse-engineering"];

/// `HARNESS_DOC_DIRS`: the top-level directories that are not source code:
/// the workspace, the harness directories, the engine's and git's.
const HARNESS_DOC_DIRS: [&str; 8] = [
    WORKSPACE_DIR,
    ".claude",
    ".kiro",
    ".codex",
    ".opencode",
    ".focus",
    ".cursor",
    ".git",
];

// --- The unit DAG -------------------------------------------------------------

/// `BoltDagResolution`: the units of the piece of work, from the runtime
/// graph's cache when it still matches the authored dependency artifact,
/// else from that artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dag {
    /// No dependency artifact and no cache: a workflow with no units.
    None,
    /// The artifact exists but cannot be read: the unit set is unknowable.
    Malformed { reason: String, detail: String },
    Ok {
        batches: Vec<Vec<String>>,
        units: Vec<String>,
        /// `(unit, kind)` for the units that declare a kind; `None` when
        /// none does.
        unit_kinds: Option<Vec<(String, String)>>,
    },
}

impl Dag {
    fn kind_of(&self, unit: &str) -> Option<&str> {
        match self {
            Self::Ok {
                unit_kinds: Some(kinds),
                ..
            } => kinds
                .iter()
                .find(|(name, _)| name == unit)
                .map(|(_, kind)| kind.as_str()),
            _ => None,
        }
    }
}

fn valid_unit_name(name: &str) -> bool {
    focus_audit::units::validate_unit_name(name).is_none()
}

/// The runtime graph's cached DAG, when it holds a well-formed one.
fn cached_dag(graph_path: &Path) -> Option<Dag> {
    let text = std::fs::read_to_string(graph_path).ok()?;
    let graph = json::parse(&text).ok()?;
    let bolt_dag = graph.get("bolt_dag")?;
    let batches: Vec<Vec<String>> = bolt_dag
        .get("batches")?
        .as_array()?
        .iter()
        .map(|batch| {
            batch
                .as_array()?
                .iter()
                .map(|unit| {
                    unit.as_str()
                        .filter(|name| valid_unit_name(name))
                        .map(str::to_owned)
                })
                .collect::<Option<Vec<String>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    let units: Vec<String> = batches.iter().flatten().cloned().collect();
    if units.is_empty() {
        return None;
    }
    let mut seen = units.clone();
    seen.sort();
    seen.dedup();
    if seen.len() != units.len() {
        return None;
    }
    let mut unit_kinds: Vec<(String, String)> = Vec::new();
    if let Some(declared) = bolt_dag.get("units").and_then(Value::as_array) {
        for unit in declared {
            if let (Some(name), Some(kind)) = (
                unit.get("name").and_then(Value::as_str),
                unit.get("kind").and_then(Value::as_str),
            ) {
                match unit_kinds.iter_mut().find(|(known, _)| known == name) {
                    Some(entry) => entry.1 = kind.to_owned(),
                    None => unit_kinds.push((name.to_owned(), kind.to_owned())),
                }
            }
        }
    }
    Some(Dag::Ok {
        batches,
        units,
        unit_kinds: if unit_kinds.is_empty() {
            None
        } else {
            Some(unit_kinds)
        },
    })
}

/// `boltDagMatches`: the same batches and the same kinds.
fn dag_matches(a: &Dag, b: &Dag) -> bool {
    match (a, b) {
        (
            Dag::Ok {
                batches: left,
                unit_kinds: left_kinds,
                ..
            },
            Dag::Ok {
                batches: right,
                unit_kinds: right_kinds,
                ..
            },
        ) => {
            if left != right {
                return false;
            }
            let left_kinds = left_kinds.clone().unwrap_or_default();
            let right_kinds = right_kinds.clone().unwrap_or_default();
            left_kinds.len() == right_kinds.len()
                && left_kinds.iter().all(|(unit, kind)| {
                    right_kinds
                        .iter()
                        .any(|(other, other_kind)| other == unit && other_kind == kind)
                })
        }
        _ => false,
    }
}

/// `resolveBoltDag` for the selected record.
#[must_use]
pub fn resolve_bolt_dag(wf: &Workflow) -> Dag {
    let root = wf.selection.docs_root(&wf.layout);
    let cached = cached_dag(&root.join("runtime-graph.json"));
    let dependency = root
        .join("inception")
        .join("units-generation")
        .join("unit-of-work-dependency.md");
    if !dependency.exists() {
        return cached.unwrap_or(Dag::None);
    }
    let body = match std::fs::read(&dependency) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(err) => {
            return Dag::Malformed {
                reason: "unreadable".to_owned(),
                detail: err.to_string(),
            };
        }
    };
    let authored = match parse_bolt_dag(&body) {
        Ok(dag) => {
            let unit_kinds: Vec<(String, String)> = dag
                .units
                .iter()
                .filter_map(|unit| unit.kind.clone().map(|kind| (unit.name.clone(), kind)))
                .collect();
            Dag::Ok {
                units: dag.batches.iter().flatten().cloned().collect(),
                batches: dag.batches,
                unit_kinds: if unit_kinds.is_empty() {
                    None
                } else {
                    Some(unit_kinds)
                },
            }
        }
        Err(unreadable) => {
            return Dag::Malformed {
                reason: unreadable.reason.to_owned(),
                detail: unreadable.detail,
            };
        }
    };
    match cached {
        Some(cached) if dag_matches(&cached, &authored) => cached,
        _ => authored,
    }
}

/// `filterProducesByKind`: the artifacts of `names` a unit of `kind` owes.
/// Everything when the stage maps no kinds or the unit has none; an
/// artifact the map does not name applies to every kind.
#[must_use]
pub fn filter_produces_by_kind<'a>(
    produces_kinds: Option<&Map<String, Value>>,
    names: &'a [String],
    kind: Option<&str>,
) -> Vec<&'a String> {
    let (Some(map), Some(kind)) = (produces_kinds, kind) else {
        return names.iter().collect();
    };
    names
        .iter()
        .filter(|name| match map.get(name.as_str()) {
            None => true,
            Some(kinds) => kinds
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|k| k.as_str() == Some(kind))),
        })
        .collect()
}

// --- Plan actions -------------------------------------------------------------

/// `effectivePlanAction`: the state's suffix for the stage, else the scope's
/// grid action.
#[must_use]
pub fn effective_plan_action(
    wf: &Workflow,
    slug: &str,
    scope: &str,
    state: &str,
) -> Option<&'static str> {
    if let Some((_, action)) = parse_stage_suffixes(state)
        .into_iter()
        .find(|(known, _)| known == slug)
    {
        return Some(action);
    }
    wf.install
        .definition(scope)
        .and_then(|definition| definition.stages.get(slug))
        .and_then(Value::as_str)
        .map(|action| {
            if action == "EXECUTE" {
                "EXECUTE"
            } else {
                "SKIP"
            }
        })
}

/// `usesStageLevelPerUnitArtifacts`: per-unit stages write at stage level
/// when Units Generation is not planned and has not run.
#[must_use]
pub fn uses_stage_level_per_unit_artifacts(wf: &Workflow, scope: &str, state: &str) -> bool {
    if effective_plan_action(wf, "units-generation", scope, state) == Some("EXECUTE") {
        return false;
    }
    !parse_checkboxes(state)
        .iter()
        .any(|line| line.slug == "units-generation" && line.state == CheckboxState::Completed)
}

// --- The code knowledge base --------------------------------------------------

fn git_rev_parse_single(project: &Path, flag: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", flag])
        .current_dir(project)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = trim(&String::from_utf8_lossy(&out.stdout)).to_owned();
    if text.is_empty() { None } else { Some(text) }
}

/// `mainCheckoutRepoName`: in a linked worktree, the main checkout's name.
fn main_checkout_repo_name(project: &Path) -> Option<String> {
    let git_dir = git_rev_parse_single(project, "--git-dir")?;
    let common = git_rev_parse_single(project, "--git-common-dir")?;
    let git_dir = std::fs::canonicalize(project.join(git_dir)).ok()?;
    let common = std::fs::canonicalize(project.join(common)).ok()?;
    if git_dir == common {
        return None;
    }
    let name = common.parent()?.file_name()?.to_string_lossy().into_owned();
    if name.is_empty() { None } else { Some(name) }
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `movedFolderStoreName`: a folder that was moved keeps the one store in
/// the space no intent's repos claim.
fn moved_folder_store_name(wf: &Workflow, name: &str) -> Option<String> {
    let root = wf.layout.space_dir(&wf.selection.space).join("codekb");
    let intent = wf.selection.intent.as_deref()?;
    if root.join(name).exists() {
        return None;
    }
    let rows = read_intent_registry(&wf.layout, &wf.selection.space);
    let well_formed = rows
        .iter()
        .all(|row| row.is_object() && row.get("repos").is_none_or(Value::is_array));
    if !well_formed
        || !rows
            .iter()
            .any(|row| focus_workspace::records::row_names_record(row, intent))
    {
        return None;
    }
    let claimed: Vec<&str> = rows
        .iter()
        .filter_map(|row| row.get("repos").and_then(Value::as_array))
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let entries = std::fs::read_dir(&root).ok()?;
    let stores: Vec<String> = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|store| is_valid_repo_name(store) && !claimed.contains(&store.as_str()))
        .collect();
    if stores.len() == 1 {
        stores.into_iter().next()
    } else {
        None
    }
}

/// `codekbRepoName`: the one recorded repo; the project's own name with
/// two or more; else the main checkout's name (a linked worktree) or the
/// folder's, keeping a moved folder's store.
///
/// # Errors
///
/// The row's `repos` is not a list of names.
pub fn codekb_repo_name(wf: &Workflow) -> Result<String> {
    let repos = intent_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    if repos.len() == 1 {
        return Ok(repos[0].clone());
    }
    if repos.len() > 1 {
        return Ok(basename(&wf.layout.project));
    }
    let name =
        main_checkout_repo_name(&wf.layout.project).unwrap_or_else(|| basename(&wf.layout.project));
    Ok(moved_folder_store_name(wf, &name).unwrap_or(name))
}

/// `codekbDir`: the store of `repo` in the active space.
#[must_use]
pub fn codekb_dir(wf: &Workflow, repo: &str) -> PathBuf {
    wf.layout
        .space_dir(&wf.selection.space)
        .join("codekb")
        .join(repo)
}

fn codekb_repos(wf: &Workflow) -> Result<Vec<String>> {
    let selected = intent_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    let mut repos: Vec<String> = if selected.is_empty() {
        vec![codekb_repo_name(wf)?]
    } else {
        selected
    };
    repos.retain(|repo| !repo.is_empty());
    repos.sort_by(|a, b| cmp_utf16(a, b));
    repos.dedup();
    Ok(repos)
}

// --- Where a stage writes -----------------------------------------------------

/// `producesDirsForStage`: the directories a stage's artifacts may be in.
///
/// # Errors
///
/// The state file cannot be read, for a per-unit stage.
pub fn produces_dirs_for_stage(wf: &Workflow, stage: &Stage) -> Result<Vec<PathBuf>> {
    if KNOWN_CODEKB_STAGES.contains(&stage.slug.as_str()) {
        let repos = intent_repos(
            &wf.layout,
            &wf.selection.space,
            wf.selection.intent.as_deref(),
        )?;
        let resolved = if repos.is_empty() {
            vec![codekb_repo_name(wf)?]
        } else {
            repos
        };
        return Ok(resolved.iter().map(|repo| codekb_dir(wf, repo)).collect());
    }
    let Some(record) = wf.record_dir() else {
        return Ok(Vec::new());
    };
    if stage::has_unit_marker(stage) {
        let state = wf.read_state()?;
        let scope = get_field(&state, "Scope").unwrap_or("");
        if uses_stage_level_per_unit_artifacts(wf, scope, &state) {
            return Ok(vec![record.join("construction").join(&stage.slug)]);
        }
        let construction = record.join("construction");
        if !construction.exists() {
            return Ok(Vec::new());
        }
        let mut dirs = Vec::new();
        for entry in std::fs::read_dir(&construction).map_err(|err| {
            focus_js::EngineError::new(format!("{}: {err}", construction.display()))
        })? {
            let entry = entry.map_err(|err| focus_js::EngineError::new(err.to_string()))?;
            let dir = construction.join(entry.file_name()).join(&stage.slug);
            if dir.exists() {
                dirs.push(dir);
            }
        }
        return Ok(dirs);
    }
    Ok(vec![record.join(&stage.phase).join(&stage.slug)])
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// `producesArtifactsExist`: at least one declared artifact exists under a
/// resolved directory (every one, in every store, for the code knowledge
/// base); a stage that declares none passes; so does a per-unit stage whose
/// kinds map leaves every unit of the DAG owing nothing.
///
/// # Errors
///
/// The state file cannot be read, for a per-unit stage.
pub fn produces_artifacts_exist(wf: &Workflow, stage: &Stage) -> Result<bool> {
    if stage.produces.is_empty() {
        return Ok(true);
    }
    if stage::has_unit_marker(stage)
        && let Some(kinds) = stage::produces_kinds(stage)
    {
        let state = wf.read_state()?;
        let scope = get_field(&state, "Scope").unwrap_or("");
        if !uses_stage_level_per_unit_artifacts(wf, scope, &state) {
            let dag = resolve_bolt_dag(wf);
            if let Dag::Ok {
                units,
                unit_kinds: Some(_),
                ..
            } = &dag
                && units.iter().all(|unit| {
                    filter_produces_by_kind(Some(kinds), &stage.produces, dag.kind_of(unit))
                        .is_empty()
                })
            {
                return Ok(true);
            }
        }
    }
    let dirs = produces_dirs_for_stage(wf, stage)?;
    if KNOWN_CODEKB_STAGES.contains(&stage.slug.as_str()) {
        return Ok(!dirs.is_empty()
            && dirs.iter().all(|dir| {
                stage
                    .produces
                    .iter()
                    .all(|name| is_regular_file(&dir.join(artifact_filename(name))))
            }));
    }
    Ok(dirs.iter().any(|dir| {
        stage
            .produces
            .iter()
            .any(|name| is_regular_file(&dir.join(artifact_filename(name))))
    }))
}

// --- Source work --------------------------------------------------------------

fn dir_has_file(dir: &Path) -> std::io::Result<bool> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let meta = std::fs::metadata(&path)?;
        if meta.is_file() {
            return Ok(true);
        }
        if meta.is_dir() && dir_has_file(&path)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `workspaceHasSourceFile`: a file at the root, or anywhere under a
/// top-level directory that is not a harness or workspace one.
#[must_use]
pub fn workspace_has_source_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.filter_map(std::result::Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if HARNESS_DOC_DIRS.contains(&name.as_str()) {
            continue;
        }
        let path = dir.join(&name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if meta.is_file() {
            return true;
        }
        if meta.is_dir() && dir_has_file(&path).unwrap_or(false) {
            return true;
        }
    }
    false
}

/// `isNonDocPath`: a git-reported path whose first segment is not a
/// harness or workspace directory.
fn is_non_doc_path(path: &str) -> bool {
    let rel = trim(path);
    let rel = rel.strip_prefix('"').unwrap_or(rel);
    let rel = rel.strip_suffix('"').unwrap_or(rel);
    if rel.is_empty() {
        return false;
    }
    let first = rel.split('/').next().unwrap_or(rel);
    !HARNESS_DOC_DIRS.contains(&first)
}

/// `git`: run git in `dir`; `None` on any failure.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn is_git_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--is-inside-work-tree"]).is_some_and(|out| trim(&out) == "true")
}

/// `gitHasSourceWork`: an uncommitted or untracked non-doc change, or a
/// non-doc path in the last commit. `None` when git cannot answer (no
/// parent commit to diff), so the caller falls back to the listing.
#[must_use]
pub fn git_has_source_work(dir: &Path) -> Option<bool> {
    let porcelain = git(dir, &["status", "--porcelain"])?;
    for line in porcelain.split('\n') {
        if trim(line).is_empty() {
            continue;
        }
        let path_part: String = line.chars().skip(3).collect();
        let candidate = match path_part.split_once(" -> ") {
            Some((_, renamed)) => renamed.to_owned(),
            None => path_part,
        };
        if is_non_doc_path(&candidate) {
            return Some(true);
        }
    }
    let last_commit = git(dir, &["diff", "--name-only", "HEAD~1", "HEAD"])?;
    Some(last_commit.split('\n').any(is_non_doc_path))
}

/// `dirHasSourceWork`: git's answer in a repository, the listing's elsewhere.
fn dir_has_source_work(dir: &Path) -> bool {
    if is_git_repo(dir)
        && let Some(verdict) = git_has_source_work(dir)
    {
        return verdict;
    }
    workspace_has_source_file(dir)
}

/// `workspaceHasWork`: the project, or one of the record's repositories,
/// holds source work. Git is asked only in a folder with a real `.git`
/// entry; any other folder gets the listing alone.
///
/// # Errors
///
/// The row's `repos` is not a list of names.
pub fn workspace_has_work(wf: &Workflow) -> Result<bool> {
    if dir_has_source_work(&wf.layout.project) {
        return Ok(true);
    }
    let repos = intent_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    Ok(repos.iter().any(|repo| {
        if !is_valid_repo_name(repo) {
            return false;
        }
        let dir = wf.layout.project.join(repo);
        if dir.join(".git").exists() {
            dir_has_source_work(&dir)
        } else {
            workspace_has_source_file(&dir)
        }
    }))
}

// --- Artifact instances -------------------------------------------------------

/// `ArtifactInstance`: one concrete file an artifact resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub artifact: String,
    pub producer: String,
    pub absolute_path: PathBuf,
    /// Relative to the project, with forward slashes.
    pub relative_path: String,
    pub unit: Option<String>,
    pub unit_kind: Option<String>,
}

fn relative_to_project(project: &Path, path: &Path) -> String {
    path.strip_prefix(project)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `safeStageUnitDirectories`: the units whose directory holds the stage's,
/// for a record with no DAG.
fn stage_unit_directories(record: &Path, slug: &str) -> Vec<(String, Option<String>)> {
    let construction = record.join("construction");
    let Ok(entries) = std::fs::read_dir(&construction) else {
        return Vec::new();
    };
    let mut units: Vec<(String, Option<String>)> = entries
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| construction.join(name).join(slug).is_dir())
        .map(|name| (name, None))
        .collect();
    units.sort_by(|a, b| locale_compare(&a.0, &b.0));
    units
}

/// `resolveArtifactRuntimeUnits`: the DAG's units with their kinds; the
/// directory listing when there is no DAG.
///
/// # Errors
///
/// The DAG is malformed, so the unit set is unknowable.
fn runtime_units(
    wf: &Workflow,
    record: &Path,
    slug: &str,
) -> Result<Vec<(String, Option<String>)>> {
    match resolve_bolt_dag(wf) {
        Dag::Malformed { reason, detail } => bail!(
            "Cannot resolve per-unit artifacts for \"{slug}\": unit DAG is {reason} ({detail})."
        ),
        Dag::Ok { units, .. } => {
            let dag = resolve_bolt_dag(wf);
            let mut named: Vec<(String, Option<String>)> = Vec::new();
            for unit in units {
                if unit.is_empty() {
                    continue;
                }
                let kind = dag.kind_of(&unit).map(str::to_owned);
                if let Some((_, known)) = named.iter().find(|(name, _)| *name == unit) {
                    if *known != kind {
                        bail!(
                            "Conflicting runtime kinds for unit \"{unit}\": \"{}\" and \"{}\".",
                            known.clone().unwrap_or_default(),
                            kind.unwrap_or_default()
                        );
                    }
                    continue;
                }
                named.push((unit, kind));
            }
            named.sort_by(|a, b| locale_compare(&a.0, &b.0));
            Ok(named)
        }
        Dag::None => Ok(stage_unit_directories(record, slug)),
    }
}

/// `resolveArtifactInstances`: the files `artifact` of `owner` resolves to
/// for this record and state.
///
/// # Errors
///
/// No record is selected; a per-unit stage with no `Scope`, with no
/// effective action for Units Generation, or with a malformed DAG.
pub fn resolve_artifact_instances(
    wf: &Workflow,
    artifact: &str,
    owner: &Stage,
    state: &str,
) -> Result<Vec<Instance>> {
    let filename = artifact_filename(artifact);
    let project = &wf.layout.project;
    let instance = |absolute: PathBuf, unit: Option<String>, kind: Option<String>| Instance {
        artifact: artifact.to_owned(),
        producer: owner.slug.clone(),
        relative_path: relative_to_project(project, &absolute),
        absolute_path: absolute,
        unit,
        unit_kind: kind,
    };
    if KNOWN_CODEKB_STAGES.contains(&owner.slug.as_str()) {
        return Ok(codekb_repos(wf)?
            .iter()
            .map(|repo| instance(codekb_dir(wf, repo).join(&filename), None, None))
            .collect());
    }
    let Some(record) = wf.record_dir() else {
        bail!(
            "Cannot resolve artifact \"{artifact}\" for stage \"{}\": no active intent record is available.",
            owner.slug
        );
    };
    if stage::is_per_unit(owner) {
        let scope = trim(get_field(state, "Scope").unwrap_or(""));
        if scope.is_empty() {
            bail!(
                "Cannot resolve artifacts for per-unit stage \"{}\": workflow Scope is missing.",
                owner.slug
            );
        }
        if effective_plan_action(wf, "units-generation", scope, state).is_none() {
            bail!(
                "Cannot resolve artifacts for per-unit stage \"{}\": Units Generation has no effective action for scope \"{scope}\".",
                owner.slug
            );
        }
        if uses_stage_level_per_unit_artifacts(wf, scope, state) {
            return Ok(vec![instance(
                record.join(&owner.phase).join(&owner.slug).join(&filename),
                None,
                None,
            )]);
        }
        let mut instances: Vec<Instance> = runtime_units(wf, &record, &owner.slug)?
            .into_iter()
            .filter(|(_, kind)| {
                filter_produces_by_kind(
                    stage::produces_kinds(owner),
                    std::slice::from_ref(&artifact.to_owned()),
                    kind.as_deref(),
                )
                .len()
                    == 1
            })
            .map(|(unit, kind)| {
                instance(
                    record
                        .join("construction")
                        .join(&unit)
                        .join(&owner.slug)
                        .join(&filename),
                    Some(unit),
                    kind,
                )
            })
            .collect();
        instances.sort_by(|a, b| locale_compare(&a.relative_path, &b.relative_path));
        return Ok(instances);
    }
    Ok(vec![instance(
        record.join(&owner.phase).join(&owner.slug).join(&filename),
        None,
        None,
    )])
}

// --- Review artifact entries --------------------------------------------------

/// `ReviewArtifactEntry`, as far as a jump names it: the artifact's logical
/// path, and its file when one place holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewEntry {
    pub logical_path: String,
    pub path: Option<PathBuf>,
}

/// The entry's path as the workspace names it (`workspaceArtifactPath`):
/// relative to the project with forward slashes, or the logical path.
#[must_use]
pub fn workspace_artifact_path(project: &Path, entry: &ReviewEntry) -> String {
    match &entry.path {
        Some(path) => relative_to_project(project, path),
        None => entry.logical_path.clone(),
    }
}

/// `reviewArtifactEntries(pd, stage)`: where each declared artifact of the
/// stage, required or optional, is or would be. `None` when no record is
/// selected.
///
/// # Errors
///
/// The row's `repos` is not a list of names, for a stage of the code
/// knowledge base.
pub fn review_artifact_entries(
    wf: &Workflow,
    stage: &Stage,
    state: Option<&str>,
) -> Result<Option<Vec<ReviewEntry>>> {
    // Read where upstream reads it, for a stage of the code knowledge
    // base alone. The one caller, a jump, has read the same row for its
    // baseline by then, so a row that is refused never gets here.
    let listed = if KNOWN_CODEKB_STAGES.contains(&stage.slug.as_str()) {
        intent_repos(
            &wf.layout,
            &wf.selection.space,
            wf.selection.intent.as_deref(),
        )?
    } else {
        Vec::new()
    };
    Ok(review_entries(wf, stage, state, listed))
}

fn review_entries(
    wf: &Workflow,
    stage: &Stage,
    state: Option<&str>,
    listed: Vec<String>,
) -> Option<Vec<ReviewEntry>> {
    let kinds = stage::produces_kinds(stage);
    let artifacts_for_kind = |kind: Option<&str>| -> Vec<String> {
        filter_produces_by_kind(kinds, &stage.produces, kind)
            .into_iter()
            .chain(filter_produces_by_kind(
                kinds,
                &stage.optional_produces,
                kind,
            ))
            .cloned()
            .collect()
    };
    let all = artifacts_for_kind(None);
    let entry = |logical: String, path: Option<PathBuf>| ReviewEntry {
        logical_path: logical,
        path,
    };
    if KNOWN_CODEKB_STAGES.contains(&stage.slug.as_str()) {
        let root = wf.layout.space_dir(&wf.selection.space).join("codekb");
        let mut repos = listed;
        if repos.is_empty()
            && let Ok(entries) = std::fs::read_dir(&root)
        {
            repos = entries
                .filter_map(std::result::Result::ok)
                .filter(|entry| root.join(entry.file_name()).is_dir())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect();
        }
        if repos.is_empty() {
            return Some(
                all.iter()
                    .map(|name| entry(format!("codekb/*/{}", artifact_filename(name)), None))
                    .collect(),
            );
        }
        return Some(
            repos
                .iter()
                .flat_map(|repo| {
                    all.iter().map(move |name| {
                        let file = artifact_filename(name);
                        entry(
                            format!("codekb/{repo}/{file}"),
                            Some(codekb_dir(wf, repo).join(file)),
                        )
                    })
                })
                .collect(),
        );
    }
    let record = wf.record_dir()?;
    let stage_level = || -> Vec<ReviewEntry> {
        all.iter()
            .map(|name| {
                let file = artifact_filename(name);
                entry(
                    format!("{}/{}/{file}", stage.phase, stage.slug),
                    Some(record.join(&stage.phase).join(&stage.slug).join(file)),
                )
            })
            .collect()
    };
    if !stage::has_unit_marker(stage) {
        return Some(stage_level());
    }
    let stage_level_present = all.iter().any(|name| {
        record
            .join(&stage.phase)
            .join(&stage.slug)
            .join(artifact_filename(name))
            .exists()
    });
    let discovered: Vec<String> = stage_unit_directories(&record, &stage.slug)
        .into_iter()
        .map(|(unit, _)| unit)
        .collect();
    let state_text = match state {
        Some(text) => Some(text.to_owned()),
        None => wf.read_state().ok(),
    };
    if let Some(text) = &state_text
        && uses_stage_level_per_unit_artifacts(wf, get_field(text, "Scope").unwrap_or(""), text)
        && (stage_level_present || discovered.is_empty())
    {
        return Some(stage_level());
    }
    let dag = resolve_bolt_dag(wf);
    let units: Vec<String> = match &dag {
        Dag::Ok { units, .. } => units.clone(),
        _ => {
            if stage_level_present {
                return Some(stage_level());
            }
            discovered
        }
    };
    if units.is_empty() {
        return Some(
            all.iter()
                .map(|name| {
                    entry(
                        format!("construction/*/{}/{}", stage.slug, artifact_filename(name)),
                        None,
                    )
                })
                .collect(),
        );
    }
    let mut entries = Vec::new();
    for unit in &units {
        for name in artifacts_for_kind(dag.kind_of(unit)) {
            let file = artifact_filename(&name);
            entries.push(entry(
                format!("construction/{unit}/{}/{file}", stage.slug),
                Some(
                    record
                        .join("construction")
                        .join(unit)
                        .join(&stage.slug)
                        .join(file),
                ),
            ));
        }
    }
    Some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_are_filtered_by_kind() {
        let map: Map<String, Value> =
            json::parse(r#"{"code-summary":["service"],"ui-spec":["ui"]}"#)
                .unwrap()
                .as_object()
                .cloned()
                .unwrap();
        let names: Vec<String> = ["plan", "code-summary", "ui-spec"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let kept = |kind: Option<&str>, with: bool| -> Vec<String> {
            filter_produces_by_kind(if with { Some(&map) } else { None }, &names, kind)
                .into_iter()
                .cloned()
                .collect()
        };
        assert_eq!(kept(Some("service"), true), ["plan", "code-summary"]);
        assert_eq!(kept(Some("packaging"), true), ["plan"]);
        assert_eq!(kept(None, true), names);
        assert_eq!(kept(Some("service"), false), names);
    }

    #[test]
    fn source_work_is_told_from_planning_docs() {
        let root = std::env::temp_dir().join(format!("focus-source-work-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("focus/spaces/default")).unwrap();
        std::fs::create_dir_all(root.join(".claude/tools")).unwrap();
        std::fs::write(root.join("focus/spaces/default/x.md"), "x").unwrap();
        std::fs::write(root.join(".claude/tools/t.ts"), "x").unwrap();
        assert!(!workspace_has_source_file(&root));
        std::fs::create_dir_all(root.join("src/deep/er")).unwrap();
        assert!(!workspace_has_source_file(&root));
        std::fs::write(root.join("src/deep/er/main.rs"), "fn main() {}").unwrap();
        assert!(workspace_has_source_file(&root));
        assert!(is_non_doc_path("src/main.rs"));
        assert!(is_non_doc_path("\"odd name.rs\""));
        assert!(!is_non_doc_path("focus/spaces/default/x.md"));
        assert!(!is_non_doc_path(" "));
        assert!(!is_git_repo(&root));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cached_dag_is_read_when_well_formed() {
        let root = std::env::temp_dir().join(format!("focus-cached-dag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("runtime-graph.json");
        std::fs::write(&path, r#"{"bolt_dag":{"batches":[["auth","store"],["api"]],"units":[{"name":"auth","kind":"service"},{"name":"api"}]}}"#).unwrap();
        let dag = cached_dag(&path).unwrap();
        assert_eq!(
            dag,
            Dag::Ok {
                batches: vec![
                    vec!["auth".to_owned(), "store".to_owned()],
                    vec!["api".to_owned()]
                ],
                units: vec!["auth".to_owned(), "store".to_owned(), "api".to_owned()],
                unit_kinds: Some(vec![("auth".to_owned(), "service".to_owned())]),
            }
        );
        assert_eq!(dag.kind_of("auth"), Some("service"));
        assert_eq!(dag.kind_of("api"), None);
        std::fs::write(&path, r#"{"bolt_dag":{"batches":[["auth","auth"]]}}"#).unwrap();
        assert!(cached_dag(&path).is_none());
        std::fs::write(&path, r#"{"bolt_dag":{"batches":[["bad name"]]}}"#).unwrap();
        assert!(cached_dag(&path).is_none());
        std::fs::write(&path, "{}").unwrap();
        assert!(cached_dag(&path).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}

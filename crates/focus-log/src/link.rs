//! `log link`: a link of a pipeline stage returned.
//!
//! A stage in `pipeline` mode hands its work down a chain, the lead agent
//! and then each supporting agent, once per repository when the piece of
//! work spans several. The conductor records each link when it returns:
//! `focus engine log link --stage <slug> --link <agent> [--repo <repo>]
//! [--artifact <path>] [--single]`. The receipt is written only for the
//! link the chain is waiting for: one the stage declares, not recorded yet
//! in this attempt, with the link before it recorded. Order, duplicates and
//! the attempt are read under the audit lock, so two conductors cannot both
//! advance one chain (`handleLink`, `aidlc-log.ts` L2 537).
//!
//! The lead's link of `reverse-engineering` hands a file to the links after
//! it, the developer's scan, and its receipt binds to that file: where it
//! is, what it holds and when it was written. It has to be the stage's
//! handoff for the repository, written in this attempt and rewritten since
//! any earlier receipt of the same link.
//!
//! # What a link does not read
//!
//! Upstream's reader of the receipts (`pipelineLinkEvidence`) also works
//! out which links the stage still owes and which repositories a row that
//! kept the stage's work answers for (`currentPipelineReuseEvidence`, and
//! for a stage run by itself whether the knowledge base's store is still
//! current). A link reads the receipts and nothing else of it: the rest is
//! what closes the stage, and comes with the verb that does (slice 3).
//!
//! # Tests that change no answer
//!
//! - The ceremony's setting is `on` or `off` and nothing else, so asking
//!   for `off` and asking for anything but `on` are one question.
//! - Whether a receipt's file is still current is asked of the lead's
//!   receipts alone, and the chain asks it of no other: naming the lead
//!   there says who the question is about, and decides nothing.

use std::path::Path;

use focus_audit::read::block_field;
use focus_audit::shards::ShardEvent;
use focus_evidence::pipeline;
use focus_graph::runtime::Stage;
use focus_js::path::{self as jspath, Flavour};
use focus_js::{EngineError, Result, bail, date, num};
use focus_lifecycle::workflow::Workflow;
use focus_lifecycle::{admission, artifacts, gates, registry, stage};
use focus_state::fields::get_field;
use serde_json::{Value, json};

use crate::flags;

/// `stage.support_agents`.
fn declared_supports(node: &Stage) -> Vec<String> {
    node.raw
        .get("support_agents")
        .and_then(Value::as_array)
        .map(|agents| {
            agents
                .iter()
                .filter_map(|agent| agent.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// `singleStageAttemptScope`: the scope the stage's own run recorded on its
/// start, the newest one.
fn single_stage_scope(events: &[ShardEvent], slug: &str) -> Option<String> {
    let workflow = format!("single-stage:{slug}");
    events
        .iter()
        .rfind(|row| {
            row.event == "STAGE_STARTED"
                && block_field(&row.block, "Stage") == Some(slug)
                && block_field(&row.block, "Workflow") == Some(workflow.as_str())
        })
        .and_then(|row| block_field(&row.block, "Scope").map(str::to_owned))
}

/// `effectiveSupportAgentsForProject`: the collaborators the stage gets in
/// this run. None when the `collaborators` ceremony is off (the kill
/// switch, the run's own setting, the scope's default); the declared ones
/// when the state cannot be read. A stage run by itself reads the scope its
/// attempt recorded and no state.
fn effective_supports(
    wf: &Workflow,
    node: &Stage,
    events: &[ShardEvent],
    single_run: bool,
) -> Vec<String> {
    let declared = declared_supports(node);
    if declared.is_empty() {
        return declared;
    }
    let (scope, state) = if single_run {
        (single_stage_scope(events, &node.slug), None)
    } else {
        match wf.read_state() {
            Ok(state) => (get_field(&state, "Scope").map(str::to_owned), Some(state)),
            Err(_) => return declared,
        }
    };
    let setting = admission::ceremony_setting_of(
        wf,
        scope.as_deref(),
        state.as_deref(),
        "collaborators",
        "Collaborators",
        "DISABLE_COLLABORATORS",
    );
    if setting == "off" {
        Vec::new()
    } else {
        declared
    }
}

/// `handleLink`.
///
/// # Errors
///
/// What upstream refuses, in its words.
pub fn link(open: &dyn Fn() -> Result<Workflow>, args: &[String]) -> Result<Value> {
    let (_, flags) = flags::parse(args)?;
    let Some(slug) = flags.get("stage").filter(|slug| !slug.is_empty()) else {
        bail!("Missing --stage <slug>");
    };
    let Some(link) = flags.get("link").filter(|link| !link.is_empty()) else {
        bail!("Missing --link <agent>");
    };
    if flags.get("intent").is_some_and(|value| !value.is_empty())
        || flags.get("space").is_some_and(|value| !value.is_empty())
    {
        bail!(
            "The link command does not accept --intent/--space selectors. Switch to the target workspace first."
        );
    }
    let wf = open()?;
    if wf.selection.intent.is_none() {
        bail!("Cannot resolve the active intent for pipeline link logging.");
    }
    let single_run = flags.get("single") == Some("true");
    let held = wf.lock("log link")?;
    let events = pipeline::ordered(wf.events());
    let Some(node) = wf
        .install
        .stage(slug)
        .filter(|node| stage::mode(node) == Some("pipeline"))
    else {
        bail!("Cannot record pipeline link: stage \"{slug}\" is not mode: pipeline.");
    };
    let mut links = vec![stage::lead_agent(node).to_owned()];
    links.extend(effective_supports(&wf, node, &events, single_run));
    let Some(index) = links.iter().position(|declared| declared == link) else {
        bail!(
            "Cannot record pipeline link for \"{slug}\": \"{link}\" is not in its declared lead/support chain ({}).",
            links.join(", ")
        );
    };

    let repos = registry::intent_repos(
        &wf.layout,
        &wf.selection.space,
        wf.selection.intent.as_deref(),
    )?;
    let state = wf.read_state().unwrap_or_default();
    let accepts_changes = gates::guard_policy_accepts_changes(&wf, &state);
    let record = wf.record_dir();
    let raw = pipeline::current_receipts(
        &events,
        slug,
        single_run,
        accepts_changes,
        &wf.install.graph,
    );
    let is_lead_scan = slug == "reverse-engineering";
    let lead = stage::lead_agent(node);
    let receipts = pipeline::chain(&raw, &links, &repos, &|receipt| {
        !is_lead_scan
            || receipt.link != lead
            || pipeline::artifact_is_current(
                &wf.layout.project,
                record.as_deref(),
                receipt,
                accepts_changes,
            )
    });
    // With no registered repository the project root is the one
    // repository, under the name `codekb-path` prints. The knowledge base's
    // commands take that name as `--repo`, so a receipt reads it as the
    // root too.
    let root_name = if repos.is_empty() {
        Some(artifacts::codekb_repo_name(&wf)?)
    } else {
        None
    };
    let repo = flags
        .get("repo")
        .filter(|repo| Some(*repo) != root_name.as_deref());
    if !repos.is_empty() {
        let Some(repo) = repo.filter(|repo| !repo.is_empty()) else {
            bail!(
                "Cannot record pipeline link for \"{slug}\": this intent records repository identity; pass --repo <repo>."
            );
        };
        if !repos.iter().any(|registered| registered == repo) {
            bail!(
                "Cannot record pipeline link for \"{slug}\": repo \"{repo}\" is not registered for this intent ({}).",
                repos.join(", ")
            );
        }
    } else if repo.is_some_and(|repo| !repo.is_empty()) {
        bail!(
            "Cannot record pipeline link for \"{slug}\": this intent has no registered repo identity; omit --repo."
        );
    }
    let repo = repo.filter(|repo| !repo.is_empty());
    let of_repo = |name: &str| match repo {
        Some(repo) => format!(" {name} repo \"{repo}\""),
        None => String::new(),
    };
    let done = |agent: &str| {
        receipts
            .iter()
            .any(|receipt| receipt.link == agent && receipt.repo.as_deref() == repo)
    };
    if done(link) {
        bail!(
            "Cannot record pipeline link for \"{slug}\": link \"{link}\"{} already completed this attempt.",
            of_repo("for")
        );
    }
    if index > 0 && !done(&links[index - 1]) {
        bail!(
            "Cannot record pipeline link for \"{slug}\": \"{link}\" is out of order; position {}/{} requires current-attempt receipt for \"{}\"{}.",
            index + 1,
            links.len(),
            links[index - 1],
            of_repo("in")
        );
    }

    let mut fields: Vec<(&str, String)> = vec![
        ("Stage", slug.to_owned()),
        ("Link", link.to_owned()),
        ("Position", format!("{}/{}", index + 1, links.len())),
    ];
    if is_lead_scan && link == lead {
        let handoff = lead_scan(
            &wf,
            &Scan {
                artifact: flags.get("artifact").filter(|path| !path.is_empty()),
                repo,
                root_name: root_name.as_deref(),
                record: record.as_deref(),
                started_at: &pipeline::attempt_started_at(
                    &events,
                    slug,
                    single_run,
                    &wf.install.graph,
                ),
                previous: pipeline::latest_artifact_mtime(&events, slug, link, repo, single_run),
            },
        )?;
        fields.extend(handoff);
    }
    if let Some(repo) = repo {
        fields.push(("Repo", repo.to_owned()));
    }
    if single_run {
        fields.push(("Workflow", format!("single-stage:{slug}")));
    }
    // Upstream's own sentence wraps nothing here: the ledger's words are the error.
    wf.emit_plain(&held, "PIPELINE_LINK_COMPLETED", &fields)?;
    drop(held);

    let mut out = serde_json::Map::new();
    out.insert("emitted".to_owned(), json!("PIPELINE_LINK_COMPLETED"));
    out.insert("stage".to_owned(), json!(slug));
    out.insert("link".to_owned(), json!(link));
    if let Some(repo) = repo {
        out.insert("repo".to_owned(), json!(repo));
    }
    if single_run {
        out.insert("single".to_owned(), json!(true));
    }
    Ok(Value::Object(out))
}

/// What the receipt of the developer's scan is asked about.
struct Scan<'a> {
    artifact: Option<&'a str>,
    repo: Option<&'a str>,
    root_name: Option<&'a str>,
    record: Option<&'a Path>,
    /// When the stage's attempt started; empty when the ledger has no start.
    started_at: &'a str,
    /// The newest time an earlier receipt of this link recorded.
    previous: Option<f64>,
}

/// The fields the lead's link of `reverse-engineering` binds its handoff
/// with: the file named has to be the stage's developer scan for the
/// repository, a regular file reached through no link, written in this
/// attempt and rewritten since any earlier receipt of the link.
fn lead_scan(wf: &Workflow, scan: &Scan) -> Result<Vec<(&'static str, String)>> {
    let Some(named) = scan.artifact else {
        bail!(
            "Cannot record reverse-engineering developer link: pass --artifact \"<record>/inception/reverse-engineering/developer-scan[-<repo>].md\"."
        );
    };
    let Some(record) = scan.record else {
        bail!(
            "Cannot record reverse-engineering developer link: active intent record is unavailable."
        );
    };
    let host = Flavour::host();
    let project = wf.layout.project.to_string_lossy().into_owned();
    let folder = record.join("inception").join("reverse-engineering");
    let at = |name: String| folder.join(name).to_string_lossy().into_owned();
    // The root's handoff may also carry the root's name, as a registered
    // repository's does.
    let accepted = match scan.repo {
        Some(repo) => vec![at(format!("developer-scan-{repo}.md"))],
        None => vec![
            at("developer-scan.md".to_owned()),
            at(format!(
                "developer-scan-{}.md",
                scan.root_name.unwrap_or_default()
            )),
        ],
    };
    let from_project =
        |path: &str| jspath::relative(host, &project, &project, path).replace('\\', "/");
    let artifact = jspath::resolve(host, &project, &[&project, named]);
    if !accepted.contains(&artifact) {
        bail!(
            "Cannot record reverse-engineering developer link: --artifact must resolve to {}.",
            from_project(&accepted[0])
        );
    }
    // Asked by name, as upstream asks (`existsSync`): it picks the sentence
    // of the refusal and nothing else. The file is read below as every file
    // of the record that decides something.
    if !Path::new(&artifact).exists() {
        bail!(
            "Cannot record reverse-engineering developer link: handoff file does not exist: {named}."
        );
    }
    let relative = jspath::relative(host, &project, &project, &artifact);
    let handoff = pipeline::read_handoff(&wf.layout.project, &relative).map_err(|why| {
        EngineError::new(format!(
            "Cannot record reverse-engineering developer link: handoff file must be a regular file with no symlink path components ({why})."
        ))
    })?;
    // A start that is no instant compares with nothing, as upstream's does.
    #[allow(clippy::cast_precision_loss)]
    let before_attempt = scan.started_at.is_empty()
        || date::parse_instant(scan.started_at)
            .is_some_and(|start| handoff.mtime_ms < start as f64);
    if before_attempt {
        bail!(
            "Cannot record reverse-engineering developer link: {} was not written in the current stage attempt.",
            from_project(&artifact)
        );
    }
    if scan
        .previous
        .is_some_and(|previous| handoff.mtime_ms <= previous)
    {
        bail!(
            "Cannot record reverse-engineering developer link: {} was not rewritten after its prior pipeline receipt.",
            from_project(&artifact)
        );
    }
    Ok(vec![
        ("Artifact Path", from_project(&artifact)),
        ("Artifact SHA256", pipeline::digest(&handoff.bytes)),
        ("Artifact Mtime Ms", num::to_string(handoff.mtime_ms)),
    ])
}

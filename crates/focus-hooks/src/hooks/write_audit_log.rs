//! PostToolUse: emit `ARTIFACT_CREATED` or `ARTIFACT_UPDATED` when a file
//! under the active record, or under the space's code knowledge base, is
//! written or edited. Mirrors `aidlc-write-audit-log.ts`.

use std::path::Path;
use std::time::SystemTime;

use focus_audit::time::iso_timestamp;
use focus_js::path::{Flavour, is_absolute, join};
use focus_js::re;
use focus_workspace::Layout;
use serde_json::{Value, json};

use super::{append, dropped, is_refusal, open, refused};
use crate::engine::refuse_summary_authorization;
use crate::health::{debug, hooks_health_dir, write_status_file};
use crate::{Outcome, context, payload};

/// `normalizeDriveLetter`: a lower-case drive letter upper-cased.
#[must_use]
pub fn normalize_drive_letter(path: &str) -> String {
    if re::test(r"^[a-z]:(?:[\\/]|$)", "", path) {
        let mut out = String::new();
        out.push_str(&path[..1].to_uppercase());
        out.push_str(&path[1..]);
        out
    } else {
        path.to_owned()
    }
}

fn slashed(path: &Path) -> String {
    normalize_drive_letter(&path.to_string_lossy().replace('\\', "/"))
        .trim_end_matches('/')
        .to_owned()
}

pub fn run(input: &str, layout: &Layout) -> Outcome {
    let parsed = payload::parse(input);
    let session = parsed
        .as_ref()
        .and_then(|map| map.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    // A write in a conversation that has not joined the selected workflow
    // is not that workflow's artifact.
    let workflow = match context::enter(layout, session.as_deref()) {
        Ok(workflow) => workflow,
        Err(err) => return refused(&err),
    };
    if workflow.stands_outside() {
        return Outcome::exit(0);
    }
    let selection = workflow.selection.as_ref().expect("a selection inside");
    let hook = "write-audit-log";
    debug(
        layout,
        selection,
        hook,
        "invoked",
        &[("projectDir", json!(layout.project.to_string_lossy()))],
    );
    write_status_file(
        &hooks_health_dir(layout, selection),
        "write-audit-log.last",
        &iso_timestamp(SystemTime::now()),
        false,
    );
    let Some(parsed) = parsed else {
        debug(
            layout,
            selection,
            hook,
            "exit: stdin parse failed",
            &[("input", json!(input.chars().take(200).collect::<String>()))],
        );
        return Outcome::exit(0);
    };
    let tool = payload::text(Some(&parsed), "tool_name").to_owned();
    let tool_input = parsed.get("tool_input").and_then(Value::as_object);
    let raw_file = match payload::text_field(tool_input, "file_path", hook) {
        Ok(text) => text,
        Err(err) => return refused(&err),
    };
    if raw_file.is_empty() {
        return Outcome::exit(0);
    }
    let flavour = Flavour::host();
    let project = layout.project.to_string_lossy().into_owned();
    let file = if is_absolute(flavour, raw_file) {
        raw_file.to_owned()
    } else {
        join(flavour, &[&project, raw_file])
    };
    // Forward-slash form with the drive letter normalized, for every match
    // below; the File field carries the same spelling.
    let file_norm = normalize_drive_letter(&file.replace('\\', "/"));
    // Only writes under the active intent's record tree, or the space's
    // codekb tree, are logged.
    let record_root = slashed(&selection.docs_root(layout));
    let under_record =
        file_norm == record_root || file_norm.starts_with(&format!("{record_root}/"));
    let codekb_root = slashed(&layout.space_dir(&selection.space).join("codekb"));
    let under_codekb = file_norm.starts_with(&format!("{codekb_root}/"));
    debug(
        layout,
        selection,
        hook,
        "path-gate",
        &[
            ("tool", json!(tool)),
            ("file", json!(file_norm)),
            ("recordRoot", json!(record_root)),
            ("underRecord", json!(under_record)),
            ("codekbRoot", json!(codekb_root)),
            ("underCodekb", json!(under_codekb)),
        ],
    );
    if !under_record && !under_codekb {
        debug(
            layout,
            selection,
            hook,
            "exit: not under record or codekb root",
            &[],
        );
        return Outcome::exit(0);
    }
    // A write to an audit shard itself is not logged (no recursion).
    if file.ends_with("/audit.md")
        || file.ends_with("\\audit.md")
        || re::test(r"[/\\]audit[/\\][^/\\]+\.md$", "", &file)
    {
        debug(
            layout,
            selection,
            hook,
            "exit: write to audit shard (recursion guard)",
            &[],
        );
        return Outcome::exit(0);
    }
    let wf = match open(layout, selection, &workflow.invoker) {
        Ok(wf) => wf,
        Err(err) => return refused(&err),
    };
    // The audit trail is not created here: the orchestrator creates it at
    // workflow start.
    let shard = match wf.ledger().audit_file(&wf.selector(), &wf.invoker) {
        Ok(shard) => Some(shard),
        Err(err) if is_refusal(&err) => return refused(&err),
        Err(_) => None,
    };
    if !shard.is_some_and(|shard| shard.exists()) {
        debug(layout, selection, hook, "exit: audit file missing", &[]);
        return Outcome::exit(0);
    }
    // The context breadcrumb: the path relative to the record root, or
    // `codekb > <repo> > <name>` for a codekb write.
    let context = if under_record && file_norm.len() > record_root.len() {
        file_norm[record_root.len() + 1..].replace('/', " > ")
    } else if under_codekb {
        format!(
            "codekb > {}",
            file_norm[codekb_root.len() + 1..].replace('/', " > ")
        )
    } else {
        match file
            .find("focus-docs/")
            .or_else(|| file.find("focus-docs\\"))
        {
            Some(at) => file[at + "focus-docs/".len()..].replace(['/', '\\'], " > "),
            None => file.clone(),
        }
    };
    // CREATE vs UPDATE: an Edit is always an update; a Write created the
    // file when its modification time still matches its creation time.
    let event = if tool == "Edit" {
        "ARTIFACT_UPDATED"
    } else {
        let is_new = match std::fs::metadata(&file) {
            Ok(meta) => {
                let millis = |time: std::io::Result<SystemTime>| {
                    time.ok()
                        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map_or(0.0, |elapsed| elapsed.as_secs_f64() * 1000.0)
                };
                (millis(meta.modified()) - millis(meta.created())).abs() < 10.0
            }
            // A stat failure reads as a new file (safer than an update).
            Err(_) => true,
        };
        if is_new {
            "ARTIFACT_CREATED"
        } else {
            "ARTIFACT_UPDATED"
        }
    };
    // A write under the record descends from the summary confirmation that
    // is the active authorization for its stage (M5).
    if under_record
        && file_norm.len() > record_root.len()
        && let Err(err) = refuse_summary_authorization(layout, selection)
    {
        return refused(&err);
    }
    let fields = [
        ("Tool", tool.clone()),
        ("File", file_norm.clone()),
        ("Context", context.clone()),
    ];
    match append(&wf, event, &fields) {
        Ok(()) => {
            debug(
                layout,
                selection,
                hook,
                "emitted",
                &[
                    ("eventType", json!(event)),
                    ("file", json!(file_norm)),
                    ("context", json!(context)),
                ],
            );
        }
        Err(err) => {
            debug(
                layout,
                selection,
                hook,
                "exit: emit threw",
                &[
                    ("eventType", json!(event)),
                    ("error", json!(err.to_string())),
                ],
            );
            if let Some(outcome) = dropped(layout, selection, hook, &err) {
                return outcome;
            }
        }
    }
    Outcome::exit(0)
}

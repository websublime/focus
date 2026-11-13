//! An answer's text in a file (`readAnswerTextFile` in `aidlc-lib.ts`).
//! Text that holds shell characters arrives in a file the agent wrote with
//! its file tool, so no shell reads it on the way. The file is read once
//! and removed: it is a message in transit, not a record.

use std::path::Path;

use focus_js::path::{self as jspath, Flavour};
use focus_js::text::trim;
use focus_js::{EngineError, Result, bail, re};
use focus_lifecycle::workflow::Workflow;
use focus_workspace::names::ENGINE_DIR;
use focus_workspace::{files, links, observer};

/// `ANSWER_TEXT_MAX_BYTES`.
const MAX_BYTES: u64 = 64 * 1024;

/// `ANSWER_TEXT_DIR`: the folder of a record where the agent writes a
/// person's words.
fn folder() -> String {
    format!("{ENGINE_DIR}/answer-text")
}

/// A record-relative name inside the folder, with no step up or in place.
fn in_folder(name: &str) -> bool {
    let folder = folder();
    !name.split('/').any(|part| part == ".." || part == ".")
        && name.starts_with(&format!("{folder}/"))
        && name.len() > folder.len() + 1
}

/// `readAnswerTextFile`: the text of one file the agent wrote, which is
/// then removed. The file is named relative to the record, or from the
/// project or in full when that names the same file, and has to be a plain
/// file inside the record's answer-text folder, reached through no link.
///
/// # Errors
///
/// No piece of work is selected; the name is not one inside the folder;
/// the file is a link or behind one, is not a regular file with one name,
/// is over the size, or is empty; the process is an observer.
pub fn read(wf: &Workflow, file: &str) -> Result<String> {
    let Some(root) = wf.record_dir() else {
        bail!("An answer text file needs an active piece of work.");
    };
    let host = Flavour::host();
    let shown = |path: &Path| path.to_string_lossy().into_owned();
    let given = file.replace('\\', "/");
    // Named from the project or in full, it is the same file: read it
    // under its name in the record, which still has to be in the folder.
    let project = shown(&wf.layout.project);
    let resolved = jspath::resolve(host, &project, &[&project, &given]);
    let from_record = jspath::relative(host, &project, &shown(&root), &resolved).replace('\\', "/");
    let relative = if !jspath::is_absolute(host, file) && in_folder(&given) {
        given
    } else if !jspath::is_absolute(host, &from_record) && in_folder(&from_record) {
        from_record
    } else {
        let folder = folder();
        bail!(
            "An answer text file must be inside {folder}/ in the work's record, named relative to the record (for example {folder}/answer.txt)."
        );
    };
    let anchor = std::fs::canonicalize(&root)
        .map_err(|err| EngineError::new(format!("{}: {err}", root.display())))?;
    links::refuse_link_in_chain(&anchor, Path::new(&relative))?;
    let path = anchor.join(&relative);
    let bytes = files::read_regular_within(&path, "answer text file", Some(MAX_BYTES))?;
    let text = re::replace(r"\r?\n$", "", &String::from_utf8_lossy(&bytes), "");
    // `removeRecordFileNoFollow`.
    observer::refuse_write("removeRecordFileNoFollow")?;
    links::refuse_link_in_chain(&anchor, Path::new(&relative))?;
    if let Err(err) = std::fs::remove_file(&path)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        bail!("{}: {err}", path.display());
    }
    if trim(&text).is_empty() {
        bail!("The answer text file {relative} is empty.");
    }
    Ok(text)
}

//! Writing the compiled files into an installed harness tree.

use focus_js::{EngineError, Result, bail};
use focus_workspace::atomic::write_atomic;
use focus_workspace::links::refuse_link_on_the_way;
use focus_workspace::manifest::announced_name;
use focus_workspace::{Layout, WorkspaceLock, observer};

use crate::compile::{Compiled, Sources, compile};
use crate::composed;

/// Compile `layout`'s harness tree and write `stage-graph.json` and
/// `scope-grid.json` in place, reconciling composed scopes on the way:
///
/// 1. restore the identity file of any composed scope whose record survives
///    but whose projection was lost (a reinstall replaced the harness tree);
/// 2. compile, and write both files atomically;
/// 3. give a record to any composed scope that has only its harness copy.
///
/// The caller holds the workspace lock around all three, so two compiles never
/// interleave. Both files come from one compile, so they never disagree.
/// `on_compiled` runs once the compile has succeeded and before anything is
/// written, which is when a caller reports the compile's advisories: a write
/// that fails afterwards must not swallow them.
///
/// Nothing is left behind when the compile is refused. Readers take no lock,
/// and a restored identity file without its grid column reads as a scope that
/// skips every stage, so the files step 1 restored are removed again when the
/// compile fails or a write target sits behind a link. Both targets are
/// checked for links before either is written.
///
/// Mirrors `writeCompiledGraphLocked` in `aidlc-graph.ts` (L851), which
/// leaves the restored files in place on a failed compile and checks only the
/// data directory for links.
///
/// # Errors
///
/// No harness is installed at `layout`, the compile fails or a write target
/// sits behind a link (nothing is changed then), or a write fails.
pub fn write_compiled(
    layout: &Layout,
    _lock: &WorkspaceLock,
    on_compiled: impl FnOnce(&Compiled),
) -> Result<Compiled> {
    require_installed(layout)?;
    let records_dir = layout.composed_scopes_dir();
    let scopes_dir = layout.scopes_dir();
    let restored = composed::materialize_identities(&layout.project, &records_dir, &scopes_dir)?;
    let undo_restore = || {
        for path in &restored {
            let _ = std::fs::remove_file(path);
        }
    };

    let targets = [layout.stage_graph(), layout.scope_grid()];
    let compiled = match compile(&Sources::from_layout(layout)) {
        Ok(compiled) => compiled,
        Err(err) => {
            undo_restore();
            return Err(err);
        }
    };
    on_compiled(&compiled);
    if let Err(err) = targets
        .iter()
        .try_for_each(|path| refuse_link_on_the_way(&layout.project, path))
    {
        undo_restore();
        return Err(err);
    }
    for (path, text) in targets
        .iter()
        .zip([&compiled.stage_graph_json, &compiled.scope_grid_json])
    {
        observer::refuse_write(observer::WRITE_FILE_ATOMIC)?;
        write_atomic(path, text.as_bytes())
            .map_err(|err| EngineError::new(format!("{}: {err}", path.display())))?;
    }

    composed::backfill_records(
        &layout.project,
        &records_dir,
        &scopes_dir,
        &compiled.composed.grid_only_names,
        &compiled.scope_grid_json,
    )?;
    Ok(compiled)
}

/// Fail unless `layout`'s harness is installed in its project. A caller
/// checks this before taking the workspace lock, so that running the command
/// in a directory that is not a project creates nothing there.
///
/// # Errors
///
/// There is no harness manifest, or it does not announce this harness.
pub fn require_installed(layout: &Layout) -> Result<()> {
    let manifest = layout.manifest();
    if !manifest.exists() {
        bail!(
            "compile requires an installed project harness at {}",
            layout.harness_root().display()
        );
    }
    // Upstream compiles on with an unreadable manifest, as if it selected
    // nothing. That would silently enable every plugin, so it is refused.
    if announced_name(&manifest).as_deref() != Some(layout.harness.name()) {
        bail!(
            "{} is not the {} harness manifest: it must be a JSON object whose \"name\" is \"{}\". Restore it, then compile again.",
            manifest.display(),
            layout.harness.product_name(),
            layout.harness.name()
        );
    }
    Ok(())
}

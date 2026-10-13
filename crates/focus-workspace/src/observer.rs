//! The observer barrier.
//!
//! Some engine invocations exist only to learn something: the Stop hook asks
//! the engine what it would do next, and `unit start` asks which unit it
//! would route. Each marks itself in the environment, and from then on the
//! process is an observer: it must change no byte of the project. The
//! engine's durable write primitives each start by asking this module, so an
//! observer that reaches one stops there instead of minting, as a side effect
//! of looking, the very thing it came to read.
//!
//! Mirrors `refuseEngineObserverWrite`, `isReadOnlyEngineProbe` and
//! `EngineModeViolationError` in `aidlc-lib.ts` (L7995, L26659, L7982).

use focus_js::{Result, bail};

use crate::env;

/// Marks an invocation made by the Stop hook to read the current directive.
pub const STOP_HOOK_PROBE: &str = "STOP_HOOK_PROBE";
/// Marks the route check `unit start` runs to learn which unit is next.
pub const ROUTE_CHECK: &str = "ROUTE_CHECK";

/// The labels upstream gives the write primitives Focus has. A refusal names
/// one, and the name is kept so the sentence reads the same from both
/// engines: `write_atomic` is upstream's `writeFileAtomic`, and the one place
/// an audit shard is appended to is its `appendAuditBlockAtPath`.
pub const WRITE_FILE_ATOMIC: &str = "writeFileAtomic";
pub const APPEND_AUDIT_BLOCK: &str = "appendAuditBlockAtPath";
pub const WRITE_STATE_FILE: &str = "writeStateFile";

/// The variable that makes this process an observer, when one does.
#[must_use]
pub fn observing() -> Option<&'static str> {
    [STOP_HOOK_PROBE, ROUTE_CHECK]
        .into_iter()
        .find(|name| env::is_on(name))
}

/// Stop an observer at a durable write. Call it first in the primitive,
/// before anything is created.
///
/// # Errors
///
/// The process is an observer.
pub fn refuse_write(primitive: &str) -> Result<()> {
    if let Some(marker) = observing() {
        bail!(
            "An engine observer ({}) reached the durable write primitive \"{primitive}\". Observers read the current directive and must change no project bytes. This is an engine defect, not a workflow problem.",
            env::full_name(marker)
        );
    }
    Ok(())
}

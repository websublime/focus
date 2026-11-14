//! What a review request binds its review to.
//!
//! A `REVIEW_REQUESTED` row carries what the review's completion is checked
//! against: the fingerprint of the artifacts the reviewer was sent to, the
//! request's own id, the fingerprints of the source, and the cause of a
//! recovery. A row from before review records carries the boundary of the
//! review appendix in the artifact instead of an id. A row with a field that
//! is not what it should be binds nothing: the request is then one no
//! completion answers (`reviewRequestBindingFromBlock`,
//! `reviewRequestBindingIsModern` and `reviewRequestReplaces`,
//! `aidlc-lib.ts` L16 315 to L16 457 and L19 082).

use focus_audit::read::block_field;
use focus_js::re;

/// `LegacyReviewAppendixBinding`: the fields a request written under the
/// retired appendix protocol carried. Read so that a review requested before
/// review records can still be completed; never written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAppendix {
    pub artifact: String,
    pub offset: u64,
    pub prior_digest: Option<String>,
    pub prior_length: Option<u64>,
    pub challenge: Option<String>,
    /// Whether the artifact already had a review section when the review
    /// was requested.
    pub prior_appendix: bool,
}

/// `ReviewRequestBinding`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestBinding {
    pub artifact_fingerprint: String,
    pub request_id: Option<String>,
    pub legacy_appendix: Option<LegacyAppendix>,
    pub source_fingerprint: Option<String>,
    pub unit_source_fingerprint: Option<String>,
    /// `artifact`, `source` or `artifact+source`.
    pub recovery_cause: Option<String>,
}

const FINGERPRINT: &str = "^sha256:[0-9a-f]{64}$";
const APPENDIX_DIGEST: &str = "^(?:none|sha256:[0-9a-f]{64})$";
const REQUEST_ID: &str = "^review:[0-9a-f]{32}$";
const SOURCE_FINGERPRINT: &str = "^(?:[0-9a-f]{40}|[0-9a-f]{64}|unbindable)$";
const UNIT_SOURCE_FINGERPRINT: &str = "^(?:sha256:[0-9a-f]{64}|unbindable)$";

/// A count written in digits that a JavaScript number holds exactly
/// (`/^[0-9]+$/`, then `Number.isSafeInteger`): `None` for anything else.
fn safe_count(raw: &str) -> Option<u64> {
    if !re::test("^[0-9]+$", "", raw) {
        return None;
    }
    let digits = raw.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    // 2^53 - 1 has sixteen digits: a longer number is past it.
    if digits.len() > 16 {
        return None;
    }
    digits
        .parse::<u64>()
        .ok()
        .filter(|count| *count <= 9_007_199_254_740_991)
}

/// `reviewRequestBindingFromBlock`: what the request row `block` binds, or
/// `None` when a field of it is not what it should be.
#[must_use]
pub fn request_binding(block: &str) -> Option<RequestBinding> {
    let field = |name: &str| block_field(block, name);
    let allowed =
        |value: Option<&str>, pattern: &str| value.is_none_or(|value| re::test(pattern, "", value));

    let artifact_fingerprint = field("Artifact Fingerprint")?;
    if !re::test(FINGERPRINT, "", artifact_fingerprint) {
        return None;
    }
    let request_id = field("Request Id");
    if !allowed(request_id, REQUEST_ID) {
        return None;
    }

    let prior_digest = field("Review Appendix Prior Digest");
    let raw_prior_length = field("Review Appendix Prior Length");
    let challenge = field("Review Challenge");
    let legacy_appendix = match (
        field("Review Appendix Artifact"),
        field("Review Appendix Offset"),
    ) {
        (Some(artifact), Some(raw_offset)) => {
            let offset = safe_count(raw_offset)?;
            if !allowed(prior_digest, APPENDIX_DIGEST) {
                return None;
            }
            let prior_length = match raw_prior_length {
                None => None,
                Some(raw) => {
                    let length = safe_count(raw)?;
                    // A length goes with a digest, and no appendix has none.
                    if prior_digest.is_none_or(|digest| (digest == "none") != (length == 0)) {
                        return None;
                    }
                    Some(length)
                }
            };
            // A challenge answers an appendix that was there.
            if challenge.is_some_and(|challenge| {
                !re::test(REQUEST_ID, "", challenge)
                    || prior_length.is_none_or(|length| length == 0)
            }) {
                return None;
            }
            Some(LegacyAppendix {
                artifact: artifact.to_owned(),
                offset,
                prior_digest: prior_digest.map(str::to_owned),
                prior_length,
                challenge: challenge.map(str::to_owned),
                prior_appendix: match prior_length {
                    Some(length) => length > 0,
                    None => prior_digest.is_some_and(|digest| digest != "none"),
                },
            })
        }
        (None, None) => {
            // Appendix evidence without the boundary it qualifies.
            if prior_digest.is_some() || raw_prior_length.is_some() || challenge.is_some() {
                return None;
            }
            None
        }
        _ => return None,
    };

    let source_fingerprint = field("Source Fingerprint");
    if !allowed(source_fingerprint, SOURCE_FINGERPRINT) {
        return None;
    }
    let unit_source_fingerprint = field("Unit Source Fingerprint");
    if !allowed(unit_source_fingerprint, UNIT_SOURCE_FINGERPRINT) {
        return None;
    }
    let recovery_cause = field("Recovery Cause");
    if recovery_cause
        .is_some_and(|cause| !matches!(cause, "artifact" | "source" | "artifact+source"))
    {
        return None;
    }
    Some(RequestBinding {
        artifact_fingerprint: artifact_fingerprint.to_owned(),
        request_id: request_id.map(str::to_owned),
        legacy_appendix,
        source_fingerprint: source_fingerprint.map(str::to_owned),
        unit_source_fingerprint: unit_source_fingerprint.map(str::to_owned),
        recovery_cause: recovery_cause.map(str::to_owned),
    })
}

/// `reviewRequestBindingIsModern`: whether a completion can be held to the
/// binding. A request of the record era carries its id; one of the appendix
/// era pinned the appendix's boundary and, where an appendix was already
/// there, a challenge. A stage that writes the workspace needs the source
/// fingerprint either way.
#[must_use]
pub fn binding_is_modern(binding: &RequestBinding, workspace_requires: bool) -> bool {
    if workspace_requires && binding.source_fingerprint.is_none() {
        return false;
    }
    if binding.request_id.is_some() {
        return true;
    }
    binding.legacy_appendix.as_ref().is_some_and(|legacy| {
        legacy.prior_length.is_none_or(|length| length == 0) || legacy.challenge.is_some()
    })
}

/// `reviewRequestReplaces`: whether the request row `block` replaces the
/// request pending at its pass. It does when it names that request's id
/// (`none` for one recorded before request ids). `pending` is `None` when no
/// request is pending, and holds `None` when the pending row bound nothing.
#[must_use]
pub fn request_replaces(block: &str, pending: Option<Option<&RequestBinding>>) -> bool {
    let (Some(named), Some(Some(binding))) = (block_field(block, "Replaces Request Id"), pending)
    else {
        return false;
    };
    named == binding.request_id.as_deref().unwrap_or("none")
}

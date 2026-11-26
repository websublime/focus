//! A unit's own source: the paths its manifest claims, the listing those
//! claims take out of the workspace's, and the fingerprint a review of the
//! unit is bound to.
//!
//! A unit of work says what it writes in `source-manifest.json`, one claim
//! per write: a file by its path, or a folder by a path that ends in `/`.
//! Its source is then the part of the workspace's listing its claims
//! cover, and the fingerprint binds that part to the manifest's own bytes,
//! so neither can change under a review without the review seeing it.
//!
//! Here are the parts that read no file: the path of a claim as it is
//! written and kept, what a claim covers, the unit's listing, its
//! fingerprint, and the text a snapshot of it holds. The manifest's reader
//! (which asks git what is ignored and follows links) and the snapshots
//! come with the slices that need them.
//!
//! Mirrors `normalizeManifestSourcePath`, `sourceClaimCovers`,
//! `restrictSourceListing`, `serializeUnitSourceListing`,
//! `unitSourceFingerprint` and `parseUnitSourceListing` in `aidlc-lib.ts`.

use std::collections::HashSet;

use super::listing::{self, Listing};
use super::raw;

/// `SourceClaimModel`: the path keys a manifest claims exactly, and the
/// keys of the folders it claims whole (each ends in `/`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaimModel {
    pub claims: HashSet<String>,
    pub prefixes: Vec<String>,
}

/// A claim's path as the manifest keeps it, and whether it claims a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimPath {
    pub path: String,
    pub prefix: bool,
}

/// Whether the path starts like a Windows drive (`C:/`).
fn starts_with_drive(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'/'
}

/// `normalizeManifestSourcePath`: a claim's path with its empty and `.`
/// segments gone. A claim is a literal path below the repository's root,
/// written with `/`; a trailing `/` claims the folder.
///
/// # Errors
///
/// What is wrong with the path, in upstream's words.
pub fn normalize_manifest_path(path: &str) -> Result<ClaimPath, String> {
    if path.is_empty() {
        return Err("writes[].path must be non-empty".to_owned());
    }
    if path.contains('\0') {
        return Err("writes[].path cannot contain a NUL byte".to_owned());
    }
    if path.contains('\\') {
        return Err("writes[].path must use POSIX '/' separators, not backslashes".to_owned());
    }
    if path.starts_with('/') || starts_with_drive(path) {
        return Err("writes[].path must be relative, not absolute".to_owned());
    }
    // Brackets and braces are plain characters of a file's name; only `*`
    // and `?`, which read as a pattern, are refused.
    if path.contains(['*', '?']) {
        return Err("writes[].path names one file or folder literally (a trailing / claims a folder); * and ? are not accepted".to_owned());
    }
    if path.split('/').any(|segment| segment == "..") {
        return Err("writes[].path cannot contain '..' segments".to_owned());
    }
    let prefix = path.ends_with('/');
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    if segments.is_empty() {
        return Err("writes[].path must name a path below the repository root".to_owned());
    }
    Ok(ClaimPath {
        path: format!("{}{}", segments.join("/"), if prefix { "/" } else { "" }),
        prefix,
    })
}

/// `sourceClaimCovers`: whether a path key is claimed, exactly or by a
/// folder's claim.
#[must_use]
pub fn claim_covers(key: &str, model: &ClaimModel) -> bool {
    model.claims.contains(key) || model.prefixes.iter().any(|prefix| key.starts_with(prefix))
}

/// `restrictSourceListing`: the part of a listing the claims cover, in the
/// listing's order.
#[must_use]
pub fn restrict(listing: &Listing, model: &ClaimModel) -> Listing {
    listing
        .iter()
        .filter(|(key, _)| claim_covers(key, model))
        .collect()
}

/// Sixty-four lowercase hex digits.
fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// `serializeUnitSourceListing`: a first row binding the manifest's bytes,
/// then the listing the claims cover.
///
/// # Errors
///
/// A manifest hash that is not one, or a listing that cannot be written.
pub fn serialize(
    listing: &Listing,
    model: &ClaimModel,
    manifest_sha256: &str,
) -> Result<String, String> {
    if !is_sha256(manifest_sha256) {
        return Err("Invalid source-manifest sha256".to_owned());
    }
    Ok(format!(
        "manifest\t{manifest_sha256}\t-\n{}",
        listing::serialize(&restrict(listing, model))?
    ))
}

/// `unitSourceFingerprint`: the value a row binds a unit's source by: the
/// manifest's bytes and every path it claims now.
///
/// The same listing over raw bytes (the files' and the manifest's), as it
/// was recorded before line endings were read as LF, is remembered as this
/// one ([`super::raw`]).
///
/// # Errors
///
/// As [`serialize`].
pub fn fingerprint(
    listing: &Listing,
    model: &ClaimModel,
    manifest_sha256: &str,
) -> Result<String, String> {
    let fingerprint = format!(
        "sha256:{}",
        listing::sha256(&serialize(listing, model, manifest_sha256)?)
    );
    let raw_manifest = raw::raw_form(manifest_sha256);
    let mut raw_listing: Option<Listing> = None;
    for (key, entry) in listing.iter() {
        if let Some(raw) = raw::raw_form(entry) {
            raw_listing
                .get_or_insert_with(|| listing.clone())
                .set(key, raw);
        }
    }
    if raw_manifest.is_some() || raw_listing.is_some() {
        let raw = serialize(
            raw_listing.as_ref().unwrap_or(listing),
            model,
            raw_manifest.as_deref().unwrap_or(manifest_sha256),
        )?;
        raw::remember(&format!("sha256:{}", listing::sha256(&raw)), &fingerprint);
    }
    Ok(fingerprint)
}

/// What a unit's snapshot holds: the manifest's hash and the listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub manifest_sha256: String,
    pub listing: Listing,
}

/// `parseUnitSourceListing`: the text [`serialize`] writes, read back;
/// `None` when a row is malformed.
///
/// # Errors
///
/// As [`listing::parse`]: a path with a NUL in it.
pub fn parse(serialized: &str) -> Result<Option<Parsed>, String> {
    let Some((header, rest)) = serialized.split_once('\n') else {
        return Ok(None);
    };
    let Some(sha) = header
        .strip_prefix("manifest\t")
        .and_then(|rest| rest.strip_suffix("\t-"))
        .filter(|sha| is_sha256(sha))
    else {
        return Ok(None);
    };
    Ok(listing::parse(rest)?.map(|listing| Parsed {
        manifest_sha256: sha.to_owned(),
        listing,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(claims: &[&str], prefixes: &[&str]) -> ClaimModel {
        ClaimModel {
            claims: claims.iter().map(|claim| (*claim).to_owned()).collect(),
            prefixes: prefixes.iter().map(|prefix| (*prefix).to_owned()).collect(),
        }
    }

    #[test]
    fn a_claims_path_is_literal_and_below_the_root() {
        let kept =
            |path: &str| normalize_manifest_path(path).map(|claim| (claim.path, claim.prefix));
        assert_eq!(kept("src//a.ts"), Ok(("src/a.ts".to_owned(), false)));
        assert_eq!(kept("./src/./lib/"), Ok(("src/lib/".to_owned(), true)));
        assert_eq!(
            kept("app/[id]/{a}.tsx"),
            Ok(("app/[id]/{a}.tsx".to_owned(), false))
        );
        assert!(kept("").is_err() && kept("/a").is_err() && kept("C:/a").is_err());
        assert!(kept("a\\b").is_err() && kept("a/../b").is_err() && kept("a*").is_err());
        assert_eq!(
            kept("./"),
            Err("writes[].path must name a path below the repository root".to_owned())
        );
    }

    #[test]
    fn a_fingerprint_binds_the_manifest_and_what_it_claims() {
        let oid = "a".repeat(40);
        let listing: Listing = [
            ("\0src/a.ts", format!("100644 {oid}")),
            ("\0src/lib/b.ts", format!("100644 {oid}")),
            ("\0other.ts", format!("100644 {oid}")),
        ]
        .into_iter()
        .collect();
        let claimed = model(&["\0src/a.ts"], &["\0src/lib/"]);
        let sha = "b".repeat(64);
        let text = serialize(&listing, &claimed, &sha).unwrap();
        assert_eq!(
            text,
            format!(
                "manifest\t{sha}\t-\n\tsrc/a.ts\t100644\t{oid}\n\tsrc/lib/b.ts\t100644\t{oid}\n"
            )
        );
        let parsed = parse(&text).unwrap().unwrap();
        assert_eq!(parsed.manifest_sha256, sha);
        assert_eq!(parsed.listing, restrict(&listing, &claimed));
        let bound = fingerprint(&listing, &claimed, &sha).unwrap();
        assert_ne!(
            bound,
            fingerprint(&listing, &claimed, &"c".repeat(64)).unwrap()
        );
        assert_ne!(
            bound,
            fingerprint(&listing, &model(&["\0src/a.ts"], &[]), &sha).unwrap()
        );
        assert_eq!(
            fingerprint(&listing, &claimed, "nope"),
            Err("Invalid source-manifest sha256".to_owned())
        );
        assert_eq!(parse("manifest\tnope\t-\n"), Ok(None));
        assert_eq!(parse(&text[..text.len() - 1]), Ok(None));
    }
}

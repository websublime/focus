//! Sensor manifests: deterministic checks a stage can import.
//!
//! A manifest (`sensors/focus-<id>.md`) says what the sensor is; a stage pulls
//! it in by listing the id under `sensors:`. The compile resolves each import
//! and snapshots the manifest's dispatch fields into the graph, so the hook
//! that fires sensors never re-reads manifests mid-workflow. Mirrors
//! `aidlc-sensor-schema.ts` and `loadSensors` / `resolveSensorsForStage` in
//! `aidlc-graph.ts`.
//!
//! Not ported yet: the manifest's `input_schema` key set, which only the
//! sensor dispatcher reads.

use std::path::Path;

use focus_frontmatter as fm;
use focus_js::order::cmp_utf16;
use focus_js::text::parse_int;
use focus_js::{Result, bail, re};

use crate::fsx::{posix_join, read_dir_sorted, read_text};

/// A sensor manifest as authored. Fields the schema requires are still
/// `Option` here: [`parse_manifest`] only reads, [`validate_manifest`] decides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SensorManifest {
    pub id: Option<String>,
    pub kind: Option<String>,
    pub command: Option<String>,
    pub default_severity: Option<String>,
    pub description: Option<String>,
    pub category: Option<String>,
    /// `write` unless the manifest says otherwise.
    pub fire_on: String,
    /// Path-shape filter the firing hook applies; absent means any output.
    pub matches: Option<String>,
    pub timeout_seconds: Option<i64>,
}

/// A manifest with its id and harness-relative display path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SensorFile {
    pub id: String,
    /// e.g. `.claude/sensors/focus-linter.md`.
    pub path: String,
    pub manifest: SensorManifest,
}

/// `parseSensorManifest`.
///
/// # Errors
///
/// The file has no frontmatter block.
pub fn parse_manifest(raw: &str) -> Result<SensorManifest> {
    let Some(block) = fm::frontmatter_block(fm::strip_bom(raw)) else {
        return Err(fm::missing_frontmatter("Sensor manifest"));
    };
    let field = |key: &str| Some(fm::scalar_field(block, key)).filter(|v| !v.is_empty());
    #[allow(clippy::cast_possible_truncation)]
    let timeout_seconds = field("timeout_seconds")
        .and_then(|t| parse_int(&t))
        .map(|n| n as i64);
    Ok(SensorManifest {
        id: field("id"),
        kind: field("kind"),
        command: field("command"),
        default_severity: field("default_severity"),
        description: field("description"),
        category: field("category"),
        fire_on: field("fire_on").unwrap_or_else(|| "write".to_owned()),
        matches: field("matches"),
        timeout_seconds,
    })
}

/// `validateSensorManifest`: `filename_id` is the id the file name promises.
///
/// # Errors
///
/// The first violation, prefixed with `file`.
pub fn validate_manifest(manifest: &SensorManifest, file: &str, filename_id: &str) -> Result<()> {
    let required = [
        ("id", &manifest.id),
        ("kind", &manifest.kind),
        ("command", &manifest.command),
        ("default_severity", &manifest.default_severity),
        ("description", &manifest.description),
    ];
    for (field, value) in required {
        if value.is_none() {
            bail!("{file}: missing required field: {field}");
        }
    }
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    let (id, kind, severity) = (
        text(&manifest.id),
        text(&manifest.kind),
        text(&manifest.default_severity),
    );

    if id.is_empty() {
        bail!("{file}: id must be a non-empty string");
    }
    if id != filename_id {
        bail!(
            "{file}: id \"{id}\" must match filename stem \"{filename_id}\" (file should be focus-{id}.md)"
        );
    }
    if kind != "deterministic" {
        bail!(
            "{file}: kind must be \"deterministic\" (got \"{kind}\"); other kinds reserved for future releases"
        );
    }
    if text(&manifest.command).is_empty() {
        bail!("{file}: command must be a non-empty string");
    }
    if !matches!(severity.as_str(), "advisory" | "blocking") {
        bail!(
            "{file}: default_severity must be one of: \"advisory\", \"blocking\" (got \"{severity}\")"
        );
    }
    if !matches!(manifest.fire_on.as_str(), "write" | "gate") {
        bail!(
            "{file}: fire_on must be one of: \"write\", \"gate\" (got \"{}\")",
            manifest.fire_on
        );
    }
    if text(&manifest.description).is_empty() {
        bail!("{file}: description must be a non-empty string");
    }
    if manifest.matches.as_deref() == Some("") {
        bail!("{file}: matches must be a non-empty string when present");
    }
    Ok(())
}

/// Load every manifest in `dir`, in file-name order. A missing directory reads
/// as no sensors. `harness_dir` (e.g. `.claude`) prefixes each display path.
///
/// # Errors
///
/// A malformed or invalid manifest, or two manifests declaring the same id.
pub fn load_sensors(dir: &Path, harness_dir: &str) -> Result<Vec<SensorFile>> {
    let mut out: Vec<SensorFile> = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for name in read_dir_sorted(dir)? {
        // Files without the `focus-` prefix are not manifests.
        let Some(m) = re::find(r"^focus-([a-z][a-z0-9-]*)\.md$", "", &name) else {
            continue;
        };
        let filename_id = re::group(&name, &m, 1).unwrap_or("");
        let path = dir.join(&name);
        let shown = path.display().to_string();
        let manifest = parse_manifest(&read_text(&path)?).map_err(|err| err.prefixed(&shown))?;

        // Checked before validation so two files claiming one id report the
        // duplicate, not the second file's id/filename mismatch.
        if let Some(previous) = manifest
            .id
            .as_ref()
            .and_then(|id| out.iter().find(|s| &s.id == id))
        {
            bail!(
                "{shown}: duplicate sensor id \"{}\" — also declared in {}. Rename one of them.",
                previous.id,
                previous.path
            );
        }
        validate_manifest(&manifest, &shown, filename_id)?;
        out.push(SensorFile {
            id: manifest.id.clone().unwrap_or_default(),
            path: posix_join(&[harness_dir, "sensors", &name]),
            manifest,
        });
    }
    Ok(out)
}

/// Resolve a stage's `sensors:` imports, in declared order.
///
/// # Errors
///
/// An import names no known manifest. Authoring errors fail the compile
/// instead of surfacing as a sensor that silently never fires.
pub fn resolve_for_stage<'a>(
    slug: &str,
    imports: &[String],
    registry: &'a [SensorFile],
) -> Result<Vec<&'a SensorFile>> {
    let mut out = Vec::with_capacity(imports.len());
    for id in imports {
        let Some(sensor) = registry.iter().find(|s| &s.id == id) else {
            let mut known: Vec<&str> = registry.iter().map(|s| s.id.as_str()).collect();
            known.sort_by(|a, b| cmp_utf16(a, b));
            let known = if known.is_empty() {
                "(none)".to_owned()
            } else {
                known.join(", ")
            };
            bail!("Stage \"{slug}\" imports unknown sensor id \"{id}\". Known ids: {known}");
        };
        out.push(sensor);
    }
    Ok(out)
}

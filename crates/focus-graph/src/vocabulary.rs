//! Names shared across the graph: artifact files and unit kinds.

/// What a unit of work is. Drives per-unit pruning of a stage's outputs
/// (`produces_kinds`). Mirrors `UNIT_KINDS` in `aidlc-lib.ts`.
pub const UNIT_KINDS: [&str; 5] = ["service", "spec", "ui", "packaging", "library"];

/// The file an artifact name resolves to. Every artifact is `<name>.md` except
/// the few whose wire name and filename differ. Mirrors `artifactFilename` in
/// `aidlc-artifact-vocabulary.ts`; guards, directives, sensors and receipts
/// must all name the same physical file, so this is the one place that maps
/// the exceptions.
#[must_use]
pub fn artifact_filename(name: &str) -> String {
    match name {
        "build-test-results" | "load-test-results" => "test-results.md".to_owned(),
        "traceability" => "traceability.json".to_owned(),
        other => format!("{other}.md"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exceptions_and_default() {
        assert_eq!(artifact_filename("requirements"), "requirements.md");
        assert_eq!(artifact_filename("build-test-results"), "test-results.md");
        assert_eq!(artifact_filename("traceability"), "traceability.json");
    }
}

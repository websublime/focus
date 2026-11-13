//! Names shared across the graph: artifact files, unit kinds, and the
//! section a heading names.

use focus_js::re;
use focus_js::text::trim;

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

/// `headingKey`: the section a contract heading names, read past a leading
/// emoji decoration. A run of emoji, joined or not, and the white space
/// after it is decoration, so an information sign before `Sources` still
/// names the `Sources` section; a symbol shown as text (a copyright sign
/// with no variation selector) is not. Which sequences are emoji is the
/// regex engine's table, compared with upstream's runtime by the oracle
/// corpus (kind `heading_key`), which holds every one of them.
#[must_use]
pub fn heading_key(heading: &str) -> String {
    let named = re::replace(
        r"^\p{RGI_Emoji}(?:\u200D?\p{RGI_Emoji})*[ \t]+",
        "v",
        heading,
        "",
    );
    trim(&named).to_owned()
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

    #[test]
    fn a_leading_emoji_is_decoration_and_a_text_symbol_is_not() {
        for (heading, key) in [
            ("Sources", "Sources"),
            ("  Sources  ", "Sources"),
            ("\u{2139}\u{fe0f} Sources", "Sources"),
            ("\u{2139} Sources", "\u{2139} Sources"),
            ("\u{1f600}\u{1f600}\tQ1. x", "Q1. x"),
            ("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} Q2", "Q2"),
            ("\u{1f600}\u{200d}\u{1f600} Q3", "Q3"),
            ("\u{1f600}Q4", "\u{1f600}Q4"),
            ("\u{a9} Q5", "\u{a9} Q5"),
            ("x \u{1f600} Q6", "x \u{1f600} Q6"),
            ("\u{1f600} ", ""),
        ] {
            assert_eq!(heading_key(heading), key, "{heading:?}");
        }
    }
}

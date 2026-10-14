//! The deterministic workspace scan: is there code here, and in what.
//!
//! Creating an intent classifies the project as Greenfield or Brownfield and
//! records its languages, frameworks and build system, from rules over the
//! files that are there: no guess is made from their contents beyond a few
//! manifests. The scan reads the person's own project files by name, as
//! upstream does: they are not workspace data, and nothing the engine
//! decides rests on one of them being what its path says.
//!
//! Mirrors the scanner in `aidlc-utility.ts` (`detectWorkspace`, L7376,
//! `scanSignals`, `countFilesByLang`, `detectFrameworks`,
//! `detectBuildSystem`, `parseGitmodules`, `scanSubmodules`) and
//! `aidlcRootIntegrations` in `aidlc-lib.ts` (L3750). What depends on the
//! order a directory is read in (which language is listed first when two
//! have the same count) follows that order, as upstream does; the nested
//! walk sorts its entries, as upstream does.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use focus_js::order::cmp_utf16;
use focus_js::re;
use focus_js::text::trim;

use crate::project::installed_harnesses;

/// What `.gitmodules` declares, and whether the submodule was fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submodule {
    pub name: String,
    /// As written in `.gitmodules`: relative, with no `..` segment.
    pub path: String,
    /// Empty when the entry names none.
    pub url: String,
    /// `<project>/<path>/.git` exists.
    pub initialized: bool,
}

/// What the scan found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scan {
    /// `Greenfield` or `Brownfield`.
    pub project_type: &'static str,
    /// `TypeScript, JavaScript`, or `Unknown`.
    pub languages: String,
    /// `React, Vite`, or `Unknown`.
    pub frameworks: String,
    /// `npm (package.json)`, or `Unknown`.
    pub build_system: String,
    /// The directories below the root that decided a Brownfield verdict,
    /// with every git repository the walk passed that held no such hit,
    /// comma-joined and relative to the root; only when there was a hit.
    pub nested_root: Option<String>,
    pub submodules: Vec<Submodule>,
}

/// The git command that fetches the submodules a scan found unfetched.
pub const SUBMODULE_INIT_REMEDY: &str = "git submodule update --init --recursive";

/// Submodule paths for a warning: at most five, then `(+N more)`.
#[must_use]
pub fn enumerate_submodule_paths(entries: &[Submodule]) -> String {
    let paths: Vec<&str> = entries.iter().map(|entry| entry.path.as_str()).collect();
    if paths.len() <= 5 {
        return paths.join(", ");
    }
    format!("{} (+{} more)", paths[..5].join(", "), paths.len() - 5)
}

const LANG_BY_EXT: [(&str, &str); 19] = [
    (".ts", "TypeScript"),
    (".tsx", "TypeScript"),
    (".js", "JavaScript"),
    (".jsx", "JavaScript"),
    (".mjs", "JavaScript"),
    (".cjs", "JavaScript"),
    (".py", "Python"),
    (".java", "Java"),
    (".kt", "Kotlin"),
    (".go", "Go"),
    (".rs", "Rust"),
    (".rb", "Ruby"),
    (".cs", "C#"),
    (".cpp", "C++"),
    (".c", "C"),
    (".h", "C"),
    (".hpp", "C++"),
    (".swift", "Swift"),
    (".php", "PHP"),
];

/// Directories the root scan counts to depth 6, in this order; the top-level
/// sweep never enters them, so no file is counted twice.
const SOURCE_DIRS: [&str; 6] = ["src", "app", "lib", "pages", "components", "tests"];

/// Names never counted or entered, at any depth: harness trees, the
/// workspace of an older layout, dependencies and build output.
const EXCLUDE: [&str; 14] = [
    ".claude",
    ".kiro",
    ".codex",
    ".opencode",
    ".focus",
    ".cursor",
    "focus-docs",
    "node_modules",
    ".git",
    "dist",
    "build",
    ".next",
    "target",
    "vendor",
];

/// Manifests that mark a directory as an application, not scaffolding.
const SOURCE_MANIFESTS: [&str; 10] = [
    "requirements.txt",
    "pyproject.toml",
    "setup.py",
    "Cargo.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "composer.json",
    "Gemfile",
];

/// Directory names the nested walk never descends into, compared
/// lowercased: they commonly hold samples and boilerplate, not the
/// project's own source.
const NESTED_EXCLUDE: [&str; 14] = [
    "focus",
    "docs",
    "doc",
    "examples",
    "example",
    "samples",
    "sample",
    "demos",
    "demo",
    "reference",
    "testdata",
    "fixtures",
    "templates",
    "scripts",
];

const NESTED_MAX_DEPTH: usize = 3;

fn skip_nested_dir(entry: &str) -> bool {
    let lower = entry.to_lowercase();
    entry.starts_with('.')
        || EXCLUDE.iter().any(|name| name.to_lowercase() == lower)
        || NESTED_EXCLUDE.contains(&lower.as_str())
        || SOURCE_DIRS.contains(&entry)
}

/// Language counts in the order a language was first seen.
type Counts = Vec<(&'static str, usize)>;

fn add_count(counts: &mut Counts, lang: &'static str, n: usize) {
    match counts.iter_mut().find(|(known, _)| *known == lang) {
        Some(known) => known.1 += n,
        None => counts.push((lang, n)),
    }
}

/// The names of a directory, in the order the OS lists them; nothing when
/// it cannot be read.
fn names(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Files Focus wrote whole into a directory it is installed in: each
/// installed harness's projection descriptor lists them as whole-file root
/// integrations. They are the engine's own files, never the project's code,
/// so the language count skips them. A missing or unreadable descriptor
/// claims nothing.
fn engine_whole_files(dir: &Path) -> HashSet<PathBuf> {
    let mut found = HashSet::new();
    for harness in installed_harnesses(dir) {
        let descriptor = dir
            .join(harness.dir())
            .join("tools")
            .join("data")
            .join("focus-projection.json");
        let Ok(bytes) = std::fs::read(&descriptor) else {
            continue;
        };
        let Ok(parsed) = focus_js::json::parse(&String::from_utf8_lossy(&bytes)) else {
            continue;
        };
        let Some(integrations) = parsed.get("rootIntegrations").and_then(Value::as_array) else {
            continue;
        };
        for integration in integrations {
            let Some(path) = integration.get("path").and_then(Value::as_str) else {
                continue;
            };
            let Some(path) = normalize_generation_path(path) else {
                continue;
            };
            // A descriptor stores an extended policy under `extendedPolicy`
            // and names it `whole-file`; read back, the extended one is the
            // policy, so such a file is not a whole file here.
            let extended = integration
                .get("extendedPolicy")
                .and_then(Value::as_str)
                .filter(|policy| ["jsonc-settings", "json-entries"].contains(policy));
            let policy = extended.or_else(|| integration.get("policy").and_then(Value::as_str));
            if path != "." && policy == Some("whole-file") {
                found.insert(dir.join(path));
            }
        }
    }
    found
}

/// `normalizeGenerationPath`: a relative, portable path with no `..`, as
/// `a/b`; `.` for the root; nothing for a path that escapes or globs.
fn normalize_generation_path(path: &str) -> Option<String> {
    let portable = trim(path).replace('\\', "/");
    if portable.is_empty()
        || portable.starts_with('/')
        || re::test(r"^[A-Za-z]:/", "", &portable)
        || portable.contains(['*', '?', '[', ']'])
    {
        return None;
    }
    let segments: Vec<&str> = portable
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect();
    if segments.contains(&"..") {
        return None;
    }
    Some(if segments.is_empty() {
        ".".to_owned()
    } else {
        segments.join("/")
    })
}

/// `countFilesByLang`: count the files under `dir` by language, `max_depth`
/// levels down, never through a link. `skip_dirs` are not entered at this
/// level only; `skip_files` are never counted.
fn count_files_by_lang(
    dir: &Path,
    counts: &mut Counts,
    max_depth: i32,
    skip_dirs: Option<&[&str]>,
    skip_files: &HashSet<PathBuf>,
) {
    if max_depth < 0 {
        return;
    }
    for entry in names(dir) {
        if EXCLUDE.contains(&entry.as_str()) {
            continue;
        }
        let full = dir.join(&entry);
        // The link's own metadata: a link is neither a directory nor a file
        // here, so it is never followed.
        let Ok(meta) = std::fs::symlink_metadata(&full) else {
            continue;
        };
        if meta.is_dir() {
            if skip_dirs.is_some_and(|dirs| dirs.contains(&entry.as_str())) {
                continue;
            }
            count_files_by_lang(&full, counts, max_depth - 1, None, skip_files);
        } else if meta.is_file() {
            if skip_files.contains(&full) {
                continue;
            }
            let Some(dot) = entry.rfind('.').filter(|dot| *dot > 0) else {
                continue;
            };
            let ext = entry[dot..].to_lowercase();
            if let Some((_, lang)) = LANG_BY_EXT.iter().find(|(known, _)| *known == ext) {
                add_count(counts, lang, 1);
            }
        }
    }
}

/// The text of a file under `dir`, as `readFileSync(…, "utf-8")` reads it.
fn text_of(dir: &Path, name: &str) -> Option<String> {
    std::fs::read(dir.join(name))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// `package.json` parsed, when it is an object.
fn package_json(dir: &Path) -> Option<Value> {
    let parsed = focus_js::json::parse(&text_of(dir, "package.json")?).ok()?;
    parsed.is_object().then_some(parsed)
}

/// JavaScript's truth of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `Object.keys(value).length` of a JSON value.
fn key_count(value: &Value) -> usize {
    match value {
        Value::Object(map) => map.len(),
        Value::Array(items) => items.len(),
        Value::String(s) => s.encode_utf16().count(),
        _ => 0,
    }
}

fn detect_frameworks(top: &HashSet<String>, dir: &Path) -> Vec<&'static str> {
    let has = |name: &str| top.contains(name);
    let any = |names: &[&str]| names.iter().any(|name| has(name));
    let mut found = Vec::new();
    if any(&[
        "next.config.js",
        "next.config.ts",
        "next.config.mjs",
        "next.config.cjs",
    ]) {
        found.push("Next.js");
    }
    if any(&["vite.config.js", "vite.config.ts", "vite.config.mjs"]) {
        found.push("Vite");
    }
    if has("angular.json") {
        found.push("Angular");
    }
    if any(&["nuxt.config.js", "nuxt.config.ts"]) {
        found.push("Nuxt");
    }
    if has("remix.config.js") {
        found.push("Remix");
    }
    if has("gatsby-config.js") {
        found.push("Gatsby");
    }
    if any(&["astro.config.mjs", "astro.config.js", "astro.config.ts"]) {
        found.push("Astro");
    }
    if has("svelte.config.js") {
        found.push("Svelte");
    }
    if has("nest-cli.json") {
        found.push("NestJS");
    }
    // React surfaces through package.json: `{...dependencies,
    // ...peerDependencies}.react`, so a peer entry has the last word.
    if has("package.json")
        && package_json(dir).is_some_and(|package| {
            ["peerDependencies", "dependencies"]
                .iter()
                .find_map(|key| package.get(key)?.as_object()?.get("react"))
                .is_some_and(truthy)
        })
    {
        found.push("React");
    }
    if has("manage.py") {
        found.push("Django");
    }
    if has("Gemfile")
        && text_of(dir, "Gemfile")
            .is_some_and(|gemfile| re::test(r"^[^#]*\brails\b", "m", &gemfile))
    {
        found.push("Rails");
    }
    if has("pom.xml") && text_of(dir, "pom.xml").is_some_and(|pom| pom.contains("spring-boot")) {
        found.push("Spring Boot");
    }
    found
}

fn detect_build_system(top: &HashSet<String>, dir: &Path) -> String {
    let has = |name: &str| top.contains(name);
    let system = if has("package.json") {
        if has("pnpm-lock.yaml") {
            "pnpm (package.json)"
        } else if has("yarn.lock") {
            "yarn (package.json)"
        } else if has("bun.lockb") || has("bun.lock") {
            "bun (package.json)"
        } else {
            "npm (package.json)"
        }
    } else if has("pyproject.toml") {
        match text_of(dir, "pyproject.toml") {
            Some(text) if text.contains("[tool.poetry]") => "poetry (pyproject.toml)",
            Some(text) if text.contains("[tool.uv]") => "uv (pyproject.toml)",
            Some(text) if text.contains("[tool.hatch]") => "hatch (pyproject.toml)",
            _ => "python (pyproject.toml)",
        }
    } else if has("requirements.txt") {
        "pip (requirements.txt)"
    } else if has("setup.py") {
        "setuptools (setup.py)"
    } else if has("Cargo.toml") {
        "cargo (Cargo.toml)"
    } else if has("go.mod") {
        "go modules (go.mod)"
    } else if has("pom.xml") {
        "maven (pom.xml)"
    } else if has("build.gradle") || has("build.gradle.kts") {
        "gradle (build.gradle)"
    } else if has("composer.json") {
        "composer (composer.json)"
    } else if has("Gemfile") {
        "bundler (Gemfile)"
    } else {
        "Unknown"
    };
    system.to_owned()
}

/// `hasNonDevDeps`: `package.json` declares a runtime dependency. Peer
/// dependencies are what a consumer provides, not what this project needs.
fn has_non_dev_deps(dir: &Path) -> bool {
    package_json(dir)
        .and_then(|package| package.get("dependencies").cloned())
        .is_some_and(|deps| key_count(&deps) > 0)
}

/// The signals of one directory, for the root and for each directory the
/// nested walk visits.
struct Signals {
    brownfield: bool,
    counts: Counts,
    frameworks: Vec<&'static str>,
    build_system: String,
}

/// `scanSignals`. `file_scan_depth` is how deep the top-level sweep counts
/// (0: the files directly under `dir`); a present source directory is
/// counted to depth 6 by its own pass.
fn scan_signals(dir: &Path, file_scan_depth: i32) -> Signals {
    let top: HashSet<String> = names(dir)
        .into_iter()
        .filter(|name| !EXCLUDE.contains(&name.as_str()))
        .collect();
    let mut counts = Counts::new();
    let whole = engine_whole_files(dir);
    count_files_by_lang(
        dir,
        &mut counts,
        file_scan_depth,
        Some(&SOURCE_DIRS),
        &whole,
    );
    for name in SOURCE_DIRS {
        if top.contains(name) {
            count_files_by_lang(&dir.join(name), &mut counts, 6, None, &whole);
        }
    }
    let frameworks = detect_frameworks(&top, dir);
    let build_system = detect_build_system(&top, dir);
    let brownfield = !counts.is_empty()
        || !frameworks.is_empty()
        || (top.contains("package.json") && has_non_dev_deps(dir))
        || SOURCE_MANIFESTS.iter().any(|name| top.contains(*name))
        || SOURCE_DIRS.iter().any(|name| top.contains(*name));
    Signals {
        brownfield,
        counts,
        frameworks,
        build_system,
    }
}

/// `parseGitmodules`: the entries of a `.gitmodules`, line by line and
/// tolerant: malformed content yields whatever parses, never an error. An
/// entry with no path is dropped, as is a path that is absolute or escapes
/// through a `..` segment.
#[must_use]
pub fn parse_gitmodules(content: &str) -> Vec<(String, String, String)> {
    let mut entries = Vec::new();
    let mut current: Option<(String, String, String)> = None;
    let finish = |current: &mut Option<(String, String, String)>, entries: &mut Vec<_>| {
        let Some(entry) = current.take() else {
            return;
        };
        let path = entry.1.as_str();
        let unsafe_path = path.is_empty()
            || path.starts_with('/')
            || re::test(r"^[A-Za-z]:[\\/]", "", path)
            || path.split(['/', '\\']).any(|segment| segment == "..");
        if !unsafe_path {
            entries.push(entry);
        }
    };
    let header = re::compile(r#"^\[submodule\s+"(.+)"\]$"#, "");
    for raw_line in content.split('\n') {
        let line = trim(raw_line.strip_suffix('\r').unwrap_or(raw_line));
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            finish(&mut current, &mut entries);
            current = header
                .find(line)
                .and_then(|found| re::group(line, &found, 1))
                .map(|name| (name.to_owned(), String::new(), String::new()));
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        let Some(eq) = line.find('=') else {
            continue;
        };
        let key = trim(&line[..eq]);
        let value = trim(&line[eq + 1..]);
        if key == "path" {
            entry.1 = value.to_owned();
        } else if key == "url" {
            entry.2 = value.to_owned();
        }
    }
    finish(&mut current, &mut entries);
    entries
}

/// `scanSubmodules`: the root `.gitmodules`, each entry probed for its
/// `.git`. A missing or unreadable file declares none.
fn scan_submodules(project: &Path) -> Vec<Submodule> {
    let Some(content) = text_of(project, ".gitmodules") else {
        return Vec::new();
    };
    parse_gitmodules(&content)
        .into_iter()
        .map(|(name, path, url)| Submodule {
            initialized: project.join(&path).join(".git").exists(),
            name,
            path,
            url,
        })
        .collect()
}

fn is_git_repo_dir(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// `detectWorkspace`: the scan of a project.
#[must_use]
pub fn detect(project: &Path) -> Scan {
    let root = scan_signals(project, 0);
    let mut counts = root.counts;
    let mut frameworks = root.frameworks;
    let mut build_system = root.build_system;
    let mut brownfield = root.brownfield;
    let mut nested_hits: Vec<String> = Vec::new();
    let mut nested_roots: Vec<String> = Vec::new();

    // The nested fallback, only when the root itself shows no code: every
    // container directory, sorted, three levels down at most. A directory
    // that scans Brownfield is counted once and not entered; a git
    // repository with nothing in it is still named as a root, so one repo
    // of a multi-repo folder is never reported as the whole project.
    if !brownfield {
        walk_containers(
            project,
            &[],
            0,
            &mut Aggregate {
                counts: &mut counts,
                frameworks: &mut frameworks,
                build_system: &mut build_system,
                brownfield: &mut brownfield,
                nested_hits: &mut nested_hits,
                nested_roots: &mut nested_roots,
            },
        );
    }

    // The primary language is the most counted; a secondary one has at
    // least a fifth of its count.
    let mut sorted = counts;
    sorted.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    let languages = match sorted.first() {
        None => "Unknown".to_owned(),
        Some((primary, primary_count)) => {
            let threshold = (primary_count / 5).max(1);
            let mut listed = vec![*primary];
            listed.extend(
                sorted[1..]
                    .iter()
                    .filter(|(_, count)| *count >= threshold)
                    .map(|(lang, _)| *lang),
            );
            listed.join(", ")
        }
    };

    // A `.gitmodules` with a valid entry declares code, even unfetched: a
    // root signal, folded in after the nested fallback so that nested
    // attribution still runs when submodules are the only signal.
    let submodules = scan_submodules(project);
    if !submodules.is_empty() {
        brownfield = true;
    }

    Scan {
        project_type: if brownfield {
            "Brownfield"
        } else {
            "Greenfield"
        },
        languages,
        frameworks: if frameworks.is_empty() {
            "Unknown".to_owned()
        } else {
            frameworks.join(", ")
        },
        build_system,
        nested_root: (!nested_hits.is_empty()).then(|| nested_roots.join(", ")),
        submodules,
    }
}

struct Aggregate<'a> {
    counts: &'a mut Counts,
    frameworks: &'a mut Vec<&'static str>,
    build_system: &'a mut String,
    brownfield: &'a mut bool,
    nested_hits: &'a mut Vec<String>,
    nested_roots: &'a mut Vec<String>,
}

/// Whether a hit was found at or below `parent`.
fn walk_containers(
    parent: &Path,
    parent_parts: &[String],
    parent_depth: usize,
    into: &mut Aggregate,
) -> bool {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return false;
    };
    let mut entries: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort_by(|a, b| cmp_utf16(a, b));

    let mut found = false;
    for entry in entries {
        if skip_nested_dir(&entry) {
            continue;
        }
        let full = parent.join(&entry);
        // The link's own metadata, so a linked directory is not entered.
        let Ok(meta) = std::fs::symlink_metadata(&full) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let mut parts = parent_parts.to_vec();
        parts.push(entry);
        let depth = parent_depth + 1;
        let sub = scan_signals(&full, 0);
        if sub.brownfield {
            *into.brownfield = true;
            found = true;
            into.nested_hits.push(parts.join("/"));
            into.nested_roots.push(parts.join("/"));
            for (lang, n) in sub.counts {
                add_count(into.counts, lang, n);
            }
            for framework in sub.frameworks {
                if !into.frameworks.contains(&framework) {
                    into.frameworks.push(framework);
                }
            }
            if *into.build_system == "Unknown" {
                *into.build_system = sub.build_system;
            }
            continue;
        }
        if depth < NESTED_MAX_DEPTH && walk_containers(&full, &parts, depth, into) {
            found = true;
        } else if is_git_repo_dir(&full) {
            into.nested_roots.push(parts.join("/"));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("focus-scan-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        fn dir(&self, rel: &str) {
            std::fs::create_dir_all(self.0.join(rel)).unwrap();
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn an_empty_folder_is_greenfield() {
        let tree = Tree::new("empty");
        let scan = detect(&tree.0);
        assert_eq!(scan.project_type, "Greenfield");
        assert_eq!(scan.languages, "Unknown");
        assert_eq!(scan.frameworks, "Unknown");
        assert_eq!(scan.build_system, "Unknown");
        assert_eq!(scan.nested_root, None);
        assert!(scan.submodules.is_empty());
        // A folder that is not there scans the same way.
        assert_eq!(detect(&tree.0.join("nope")), scan);
    }

    #[test]
    fn the_root_sweep_and_the_source_dirs_count_files() {
        let tree = Tree::new("root");
        for name in [
            "a.ts",
            "b.TS",
            "c.tsx",
            "c.js",
            "README.md",
            ".env.py",
            "noext",
        ] {
            tree.write(name, "");
        }
        // Not entered by the sweep: only `src` and its kin are, and those
        // seven levels down (`src` itself is the first).
        tree.write("other/deep.py", "");
        tree.write("src/a/b/c/d/e/f/g.py", "");
        tree.write("src/a/b/c/d/e/f/g/h.py", "");
        tree.write("src/node_modules/x.py", "");
        tree.write("src/one.rs", "");
        let scan = detect(&tree.0);
        assert_eq!(scan.project_type, "Brownfield");
        // TypeScript 3, Python 2 (`.env.py` has an extension), JavaScript 1
        // and Rust 1 in the order they were seen: a fifth of 3 is 0, so the
        // threshold is 1 and every language is listed.
        assert_eq!(scan.languages, "TypeScript, Python, JavaScript, Rust");
        assert_eq!(scan.build_system, "Unknown");
    }

    #[test]
    fn secondary_languages_need_a_fifth_of_the_primary() {
        let tree = Tree::new("fifth");
        for n in 0..10 {
            tree.write(&format!("src/{n}.go"), "");
        }
        tree.write("src/a.rs", "");
        tree.write("src/b.rs", "");
        tree.write("src/c.py", "");
        assert_eq!(detect(&tree.0).languages, "Go, Rust");
    }

    #[test]
    fn frameworks_and_build_systems_are_read_off_manifests() {
        let tree = Tree::new("fw");
        tree.write(
            "package.json",
            r#"{"dependencies":{"left-pad":"1"},"peerDependencies":{"react":""}}"#,
        );
        tree.write("vite.config.ts", "");
        tree.write("yarn.lock", "");
        tree.write("Gemfile", "# rails\ngem 'rails'\n");
        tree.write("pom.xml", "<x>spring-boot</x>");
        tree.write("manage.py", "");
        let scan = detect(&tree.0);
        // A peer entry has the last word on React, and "" is not truthy.
        assert_eq!(scan.frameworks, "Vite, Django, Rails, Spring Boot");
        assert_eq!(scan.build_system, "yarn (package.json)");
        assert_eq!(scan.project_type, "Brownfield");

        tree.write("package.json", r#"{"dependencies":{"react":"18"}}"#);
        assert_eq!(
            detect(&tree.0).frameworks,
            "Vite, React, Django, Rails, Spring Boot"
        );
        tree.write("Gemfile", "gem 'x' # rails\nrailsy\n");
        assert_eq!(
            detect(&tree.0).frameworks,
            "Vite, React, Django, Spring Boot"
        );
    }

    #[test]
    fn a_dependency_alone_is_code_and_a_peer_dependency_is_not() {
        let tree = Tree::new("deps");
        tree.write("package.json", r#"{"peerDependencies":{"react":"18"}}"#);
        let scan = detect(&tree.0);
        assert_eq!(scan.frameworks, "React");
        tree.write(
            "package.json",
            r#"{"devDependencies":{"x":"1"},"peerDependencies":{"y":"1"}}"#,
        );
        assert_eq!(detect(&tree.0).project_type, "Greenfield");
        assert_eq!(detect(&tree.0).build_system, "npm (package.json)");
        tree.write("package.json", r#"{"dependencies":"abc"}"#);
        assert_eq!(detect(&tree.0).project_type, "Brownfield");
        tree.write("package.json", r#"{"dependencies":[]}"#);
        assert_eq!(detect(&tree.0).project_type, "Greenfield");
        tree.write("package.json", "[1]");
        assert_eq!(detect(&tree.0).project_type, "Greenfield");
    }

    #[test]
    fn python_build_systems_are_told_apart() {
        let tree = Tree::new("py");
        tree.write("pyproject.toml", "[tool.uv]\n");
        assert_eq!(detect(&tree.0).build_system, "uv (pyproject.toml)");
        tree.write("pyproject.toml", "[tool.hatch]\n[tool.poetry]\n");
        assert_eq!(detect(&tree.0).build_system, "poetry (pyproject.toml)");
        tree.write("pyproject.toml", "[project]\n");
        assert_eq!(detect(&tree.0).build_system, "python (pyproject.toml)");
        assert_eq!(detect(&tree.0).project_type, "Brownfield");
    }

    #[test]
    fn nested_projects_are_found_below_an_empty_root() {
        let tree = Tree::new("nested");
        tree.write("README.md", "");
        tree.write("services/api/go.mod", "");
        tree.write("services/api/main.go", "");
        tree.write("services/api/handlers/h.go", "");
        tree.write("services/web/index.html", "");
        tree.dir("services/web/.git");
        tree.write("examples/x.py", "");
        tree.write("a/b/c/d/deep.py", "");
        tree.write("Docs/x.py", "");
        let scan = detect(&tree.0);
        assert_eq!(scan.project_type, "Brownfield");
        // The hit's own sweep counts its top-level files only.
        assert_eq!(scan.languages, "Go");
        assert_eq!(scan.build_system, "go modules (go.mod)");
        assert_eq!(
            scan.nested_root.as_deref(),
            Some("services/api, services/web")
        );
    }

    #[test]
    fn a_git_repo_with_nothing_in_it_is_not_a_hit() {
        let tree = Tree::new("repo");
        tree.dir("web/.git");
        let scan = detect(&tree.0);
        assert_eq!(scan.project_type, "Greenfield");
        assert_eq!(scan.nested_root, None);
    }

    #[test]
    fn gitmodules_declare_code_and_are_probed() {
        let tree = Tree::new("sub");
        tree.write(
            ".gitmodules",
            "[submodule \"libs/one\"]\n\tpath = libs/one\n\turl = https://x/one.git\n; comment\n[submodule \"two\"]\n  path=/abs\n[submodule \"three\"]\n path = a/../b\n[other]\npath = x\n[submodule \"four\"]\nurl = u\n[submodule \"five\"]\npath = five\n",
        );
        tree.dir("libs/one/.git");
        let scan = detect(&tree.0);
        assert_eq!(scan.project_type, "Brownfield");
        assert_eq!(scan.languages, "Unknown");
        assert_eq!(
            scan.submodules,
            [
                Submodule {
                    name: "libs/one".into(),
                    path: "libs/one".into(),
                    url: "https://x/one.git".into(),
                    initialized: true
                },
                Submodule {
                    name: "five".into(),
                    path: "five".into(),
                    url: String::new(),
                    initialized: false
                },
            ]
        );
        assert_eq!(parse_gitmodules("garbage\n= x\n[submodule]\npath=y\n"), []);
        assert_eq!(
            enumerate_submodule_paths(&scan.submodules),
            "libs/one, five"
        );
        let many: Vec<Submodule> = (0..7)
            .map(|n| Submodule {
                name: n.to_string(),
                path: n.to_string(),
                url: String::new(),
                initialized: false,
            })
            .collect();
        assert_eq!(enumerate_submodule_paths(&many), "0, 1, 2, 3, 4 (+2 more)");
        assert_eq!(
            enumerate_submodule_paths(&many[..6]),
            "0, 1, 2, 3, 4 (+1 more)"
        );
        assert_eq!(enumerate_submodule_paths(&many[..5]), "0, 1, 2, 3, 4");
    }

    #[test]
    fn files_the_engine_wrote_whole_are_not_code() {
        let tree = Tree::new("whole");
        tree.write(".cursor/tools/data/harness.json", r#"{"name":"cursor"}"#);
        tree.write(
            ".cursor/tools/data/focus-projection.json",
            r#"{"rootIntegrations":[{"path":"install.ts","policy":"whole-file"},{"path":"a.ts","policy":"whole-file","extendedPolicy":"json-entries"},{"path":"./sub/../b.ts","policy":"whole-file"},{"path":".","policy":"whole-file"}]}"#,
        );
        tree.write("install.ts", "");
        assert_eq!(detect(&tree.0).project_type, "Greenfield");
        tree.write("a.ts", "");
        assert_eq!(detect(&tree.0).languages, "TypeScript");
        assert_eq!(
            normalize_generation_path(" .\\x\\.\\y "),
            Some("x/y".to_owned())
        );
        assert_eq!(normalize_generation_path("./"), Some(".".to_owned()));
        assert_eq!(normalize_generation_path("C:/x"), None);
        assert_eq!(normalize_generation_path("a*"), None);
    }

    #[cfg(unix)]
    #[test]
    fn links_are_not_followed() {
        let tree = Tree::new("links");
        tree.write("real/x.py", "");
        std::os::unix::fs::symlink(tree.0.join("real/x.py"), tree.0.join("y.py")).unwrap();
        std::os::unix::fs::symlink(tree.0.join("real"), tree.0.join("src")).unwrap();
        std::os::unix::fs::symlink(tree.0.join("real"), tree.0.join("linked")).unwrap();
        // A link named `src` is an app source dir, and is read through, as
        // upstream reads it; a linked file is never counted.
        let scan = detect(&tree.0);
        assert_eq!(scan.project_type, "Brownfield");
        assert_eq!(scan.languages, "Python");
        assert_eq!(scan.nested_root, None);
        std::fs::remove_file(tree.0.join("src")).unwrap();
        // With the root empty, the walk enters `real` and not the link to it.
        let scan = detect(&tree.0);
        assert_eq!(scan.languages, "Python");
        assert_eq!(scan.nested_root.as_deref(), Some("real"));
    }
}

//! Agent personas: one `<slug>.md` per agent.
//!
//! Mirrors `loadAgents` and its helpers in `aidlc-lib.ts` (L36566–36652).
//! The compile needs only the roster of slugs, to reject a stage that names an
//! agent with no file.

use std::path::Path;

use focus_frontmatter as fm;
use focus_js::order::locale_compare;
use focus_js::{Result, bail, re};

use crate::fsx::{read_dir_sorted, read_text};

/// What the engine reads from an agent file's frontmatter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMetadata {
    pub slug: String,
    pub display_name: String,
    pub examples: Vec<String>,
}

/// Frontmatter keys only one of the engine's own personas declares.
const FOCUS_AGENT_KEYS: [&str; 4] = ["display_name", "examples", "tier", "plugin"];

/// Why a file in the agents directory counts as a Focus persona, or `None`
/// when it is someone else's agent file and must be left alone. The harness's
/// agents directory is shared with the project's own agents.
#[must_use]
pub fn agent_claim(path: &Path) -> Option<String> {
    if file_name(path).starts_with("focus-") {
        return Some("its name starts with focus-".to_owned());
    }
    let body = read_text(path).ok()?;
    let block = fm::frontmatter_block(fm::strip_bom(&body))?;
    FOCUS_AGENT_KEYS
        .into_iter()
        .find(|key| re::test(&format!("^{key}:"), "m", block))
        .map(|key| format!("it declares `{key}:`"))
}

/// Load every Focus persona in `dir`, sorted by slug.
///
/// # Errors
///
/// The directory is unreadable, a persona lacks frontmatter or a required
/// field, or two personas share a slug.
pub fn load_agents(dir: &Path) -> Result<Vec<AgentMetadata>> {
    let mut agents: Vec<AgentMetadata> = Vec::new();
    let mut files: Vec<(String, String)> = Vec::new();
    for name in read_dir_sorted(dir)? {
        if !name.ends_with(".md") || name == "focus.md" {
            continue;
        }
        let path = dir.join(&name);
        if agent_claim(&path).is_none() {
            continue;
        }
        let agent = parse_agent(&path)?;
        let shown = path.display().to_string();
        if let Some((_, previous)) = files.iter().find(|(slug, _)| *slug == agent.slug) {
            bail!(
                "Duplicate agent slug \"{}\" in {shown}: already declared in {previous}. Rename one of them.",
                agent.slug
            );
        }
        files.push((agent.slug.clone(), shown));
        agents.push(agent);
    }
    agents.sort_by(|a, b| locale_compare(&a.slug, &b.slug));
    Ok(agents)
}

fn parse_agent(path: &Path) -> Result<AgentMetadata> {
    let body = read_text(path)?;
    let shown = path.display();
    // Explain the classification only when it is not obvious from the name.
    let because = if file_name(path).starts_with("focus-") {
        String::new()
    } else {
        agent_claim(path)
            .map(|claim| format!(" (treated as a Focus persona because {claim})"))
            .unwrap_or_default()
    };
    let Some(block) = fm::frontmatter_block(&body) else {
        bail!("Agent file missing frontmatter: {shown}{because}");
    };
    let slug = fm::scalar_field(block, "name");
    let display_name = fm::scalar_field(block, "display_name");
    let missing: Vec<&str> = [("name", &slug), ("display_name", &display_name)]
        .into_iter()
        .filter(|(_, value)| value.is_empty())
        .map(|(field, _)| field)
        .collect();
    if !missing.is_empty() {
        bail!(
            "Agent file {shown} missing required frontmatter: {}{because}",
            missing.join(", ")
        );
    }
    Ok(AgentMetadata {
        slug,
        display_name,
        examples: fm::list_field(block, "examples"),
    })
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

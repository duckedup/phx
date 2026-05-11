use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSource {
    User,
    Project,
}

#[derive(Debug, Clone)]
pub struct AgentDefinition {
    pub name: String,
    pub description: String,
    pub tools: Vec<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub system_prompt: String,
    pub source: AgentSource,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone, Default)]
struct AgentFrontmatter {
    name: Option<String>,
    description: Option<String>,
    tools: Option<String>,
    model: Option<String>,
    provider: Option<String>,
}

fn strip_quotes(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(s)
}

fn parse_frontmatter(content: &str) -> (AgentFrontmatter, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (AgentFrontmatter::default(), content.to_string());
    }

    let after_first = &trimmed[3..];
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    let Some(end_idx) = after_first.find("\n---") else {
        return (AgentFrontmatter::default(), content.to_string());
    };

    let frontmatter = &after_first[..end_idx];
    let body_start = end_idx + 4;
    let body = after_first[body_start..]
        .strip_prefix('\n')
        .unwrap_or(&after_first[body_start..]);

    let mut meta = AgentFrontmatter::default();

    for line in frontmatter.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = strip_quotes(value.trim());
        match key {
            "name" => meta.name = Some(value.to_string()),
            "description" => meta.description = Some(value.to_string()),
            "tools" => meta.tools = Some(value.to_string()),
            "model" => meta.model = Some(value.to_string()),
            "provider" => meta.provider = Some(value.to_string()),
            _ => {}
        }
    }

    (meta, body.to_string())
}

fn parse_tools_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .flat_map(|chunk| chunk.split_whitespace())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn parse_agent_file(
    content: &str,
    file_stem: &str,
    source: AgentSource,
    file_path: PathBuf,
) -> Option<AgentDefinition> {
    let (meta, body) = parse_frontmatter(content);

    let description = match &meta.description {
        Some(d) if !d.is_empty() => d.clone(),
        _ => {
            tracing::warn!(
                "agent '{}' skipped: missing description in frontmatter",
                file_stem
            );
            return None;
        }
    };

    let name = meta.name.unwrap_or_else(|| file_stem.to_string());
    let tools = meta.tools.map(|t| parse_tools_list(&t)).unwrap_or_default();

    Some(AgentDefinition {
        name,
        description,
        tools,
        model: meta.model,
        provider: meta.provider,
        system_prompt: body,
        source,
        file_path,
    })
}

fn scan_agents_dir(dir: &Path, source: AgentSource) -> Vec<AgentDefinition> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut agents = vec![];
    for entry in entries.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext != Some("md") {
            continue;
        }
        let file_stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(def) = parse_agent_file(&content, &file_stem, source.clone(), path) {
            agents.push(def);
        }
    }
    agents.sort_by_key(|a| a.name.to_lowercase());
    agents
}

pub fn discover_agents(project_dir: Option<&Path>, user_home: &Path) -> Vec<AgentDefinition> {
    let mut seen = HashSet::new();
    let mut all = Vec::new();

    let mut add = |agents: Vec<AgentDefinition>| {
        for agent in agents {
            if !seen.insert(agent.name.clone()) {
                tracing::warn!("agent '{}' shadowed by higher-priority source", agent.name);
            } else {
                all.push(agent);
            }
        }
    };

    if let Some(p) = project_dir {
        add(scan_agents_dir(
            &p.join(".phoenix/agents"),
            AgentSource::Project,
        ));
    }

    add(scan_agents_dir(
        &user_home.join(".phoenix/agents"),
        AgentSource::User,
    ));

    all.sort_by_key(|a| a.name.to_lowercase());
    all
}

pub fn find_agent<'a>(agents: &'a [AgentDefinition], name: &str) -> Option<&'a AgentDefinition> {
    let lower = name.to_lowercase();
    agents.iter().find(|a| a.name.to_lowercase() == lower)
}

pub fn build_agent_catalog(agents: &[AgentDefinition]) -> String {
    if agents.is_empty() {
        return String::new();
    }

    let mut catalog = String::from(
        "The following custom agents are available. Spawn them using spawn_agent \
         with the \"agent\" parameter set to the agent name.\n\n\
         <custom_agents>\n",
    );

    for agent in agents {
        catalog.push_str("  <agent>\n");
        catalog.push_str(&format!("    <name>{}</name>\n", agent.name));
        catalog.push_str(&format!(
            "    <description>{}</description>\n",
            agent.description
        ));
        if !agent.tools.is_empty() {
            catalog.push_str(&format!("    <tools>{}</tools>\n", agent.tools.join(", ")));
        }
        if let Some(model) = &agent.model {
            catalog.push_str(&format!("    <model>{model}</model>\n"));
        }
        catalog.push_str("  </agent>\n");
    }

    catalog.push_str("</custom_agents>");
    catalog
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parse_frontmatter_full() {
        let content = "\
---
name: code-reviewer
description: Reviews code for correctness
tools: read, bash, edit
model: claude-sonnet-4-6
provider: claude
---

You are a senior code reviewer.";
        let (meta, body) = parse_frontmatter(content);
        assert_eq!(meta.name.as_deref(), Some("code-reviewer"));
        assert_eq!(
            meta.description.as_deref(),
            Some("Reviews code for correctness")
        );
        assert_eq!(meta.tools.as_deref(), Some("read, bash, edit"));
        assert_eq!(meta.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(meta.provider.as_deref(), Some("claude"));
        assert!(body.contains("You are a senior"));
    }

    #[test]
    fn parse_frontmatter_no_frontmatter() {
        let content = "# Just markdown\nNo frontmatter.";
        let (meta, body) = parse_frontmatter(content);
        assert!(meta.name.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn parse_frontmatter_missing_closing() {
        let content = "---\nname: broken\ndescription: No closing\n";
        let (meta, body) = parse_frontmatter(content);
        assert!(meta.name.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn parse_frontmatter_quoted_values() {
        let content = "---\nname: 'quoted'\ndescription: \"double quoted\"\n---\nbody";
        let (meta, _body) = parse_frontmatter(content);
        assert_eq!(meta.name.as_deref(), Some("quoted"));
        assert_eq!(meta.description.as_deref(), Some("double quoted"));
    }

    #[test]
    fn parse_tools_list_comma_separated() {
        let tools = parse_tools_list("Read, Bash, Edit");
        assert_eq!(tools, vec!["read", "bash", "edit"]);
    }

    #[test]
    fn parse_tools_list_space_separated() {
        let tools = parse_tools_list("Read Bash Edit");
        assert_eq!(tools, vec!["read", "bash", "edit"]);
    }

    #[test]
    fn parse_tools_list_mixed() {
        let tools = parse_tools_list("Read, Bash  Edit");
        assert_eq!(tools, vec!["read", "bash", "edit"]);
    }

    #[test]
    fn parse_tools_list_empty() {
        let tools = parse_tools_list("");
        assert!(tools.is_empty());
    }

    #[test]
    fn parse_agent_file_full() {
        let content = "\
---
name: reviewer
description: Code review agent
tools: read, bash
model: claude-sonnet-4-6
---

Review the code.";
        let def = parse_agent_file(
            content,
            "reviewer",
            AgentSource::User,
            PathBuf::from("/test/reviewer.md"),
        )
        .unwrap();
        assert_eq!(def.name, "reviewer");
        assert_eq!(def.description, "Code review agent");
        assert_eq!(def.tools, vec!["read", "bash"]);
        assert_eq!(def.model.as_deref(), Some("claude-sonnet-4-6"));
        assert!(def.system_prompt.contains("Review the code."));
    }

    #[test]
    fn parse_agent_file_name_defaults_to_stem() {
        let content = "---\ndescription: A test agent\n---\nDo things.";
        let def = parse_agent_file(
            content,
            "my-agent",
            AgentSource::Project,
            PathBuf::from("/test/my-agent.md"),
        )
        .unwrap();
        assert_eq!(def.name, "my-agent");
    }

    #[test]
    fn parse_agent_file_skips_missing_description() {
        let content = "---\nname: no-desc\n---\nDo things.";
        let def = parse_agent_file(
            content,
            "no-desc",
            AgentSource::User,
            PathBuf::from("/test/no-desc.md"),
        );
        assert!(def.is_none());
    }

    #[test]
    fn parse_agent_file_empty_tools() {
        let content = "---\ndescription: No tools specified\n---\nDo things.";
        let def = parse_agent_file(
            content,
            "bare",
            AgentSource::User,
            PathBuf::from("/test/bare.md"),
        )
        .unwrap();
        assert!(def.tools.is_empty());
    }

    #[test]
    fn discover_finds_agents() {
        let home = tempdir().unwrap();
        let agents_dir = home.path().join(".phoenix/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Reviews code\ntools: read\n---\nReview.",
        )
        .unwrap();

        let agents = discover_agents(None, home.path());
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "reviewer");
        assert_eq!(agents[0].source, AgentSource::User);
    }

    #[test]
    fn discover_project_overrides_user() {
        let home = tempdir().unwrap();
        let project = tempdir().unwrap();

        let user_dir = home.path().join(".phoenix/agents");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(
            user_dir.join("shared.md"),
            "---\nname: shared\ndescription: User version\n---\nUser prompt.",
        )
        .unwrap();

        let proj_dir = project.path().join(".phoenix/agents");
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(
            proj_dir.join("shared.md"),
            "---\nname: shared\ndescription: Project version\n---\nProject prompt.",
        )
        .unwrap();

        let agents = discover_agents(Some(project.path()), home.path());
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].description, "Project version");
        assert_eq!(agents[0].source, AgentSource::Project);
    }

    #[test]
    fn discover_skips_non_md_files() {
        let home = tempdir().unwrap();
        let agents_dir = home.path().join(".phoenix/agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(agents_dir.join("notes.txt"), "not an agent").unwrap();
        std::fs::write(
            agents_dir.join("valid.md"),
            "---\ndescription: Valid agent\n---\nDo things.",
        )
        .unwrap();

        let agents = discover_agents(None, home.path());
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "valid");
    }

    #[test]
    fn discover_empty_when_no_dir() {
        let home = tempdir().unwrap();
        let agents = discover_agents(None, home.path());
        assert!(agents.is_empty());
    }

    #[test]
    fn find_agent_case_insensitive() {
        let agents = vec![AgentDefinition {
            name: "Code-Reviewer".to_string(),
            description: "Reviews code".to_string(),
            tools: vec![],
            model: None,
            provider: None,
            system_prompt: String::new(),
            source: AgentSource::User,
            file_path: PathBuf::from("/test.md"),
        }];
        assert!(find_agent(&agents, "code-reviewer").is_some());
        assert!(find_agent(&agents, "CODE-REVIEWER").is_some());
        assert!(find_agent(&agents, "nonexistent").is_none());
    }

    #[test]
    fn catalog_empty_when_no_agents() {
        assert!(build_agent_catalog(&[]).is_empty());
    }

    #[test]
    fn catalog_contains_agent_info() {
        let agents = vec![AgentDefinition {
            name: "reviewer".to_string(),
            description: "Reviews code".to_string(),
            tools: vec!["read".to_string(), "bash".to_string()],
            model: Some("claude-sonnet-4-6".to_string()),
            provider: None,
            system_prompt: String::new(),
            source: AgentSource::User,
            file_path: PathBuf::from("/test.md"),
        }];
        let catalog = build_agent_catalog(&agents);
        assert!(catalog.contains("<custom_agents>"));
        assert!(catalog.contains("<name>reviewer</name>"));
        assert!(catalog.contains("<description>Reviews code</description>"));
        assert!(catalog.contains("<tools>read, bash</tools>"));
        assert!(catalog.contains("<model>claude-sonnet-4-6</model>"));
        assert!(catalog.contains("</custom_agents>"));
    }

    #[test]
    fn catalog_omits_empty_tools_and_model() {
        let agents = vec![AgentDefinition {
            name: "simple".to_string(),
            description: "Simple agent".to_string(),
            tools: vec![],
            model: None,
            provider: None,
            system_prompt: String::new(),
            source: AgentSource::User,
            file_path: PathBuf::from("/test.md"),
        }];
        let catalog = build_agent_catalog(&agents);
        assert!(!catalog.contains("<tools>"));
        assert!(!catalog.contains("<model>"));
    }
}

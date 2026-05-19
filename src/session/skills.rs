use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillSource {
    User,
    Project,
    Explicit,
}

#[derive(Debug, Clone, Default)]
pub struct SkillMetadata {
    pub name: Option<String>,
    pub description: Option<String>,
    pub compatibility: Option<String>,
    pub license: Option<String>,
    pub model: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub allowed_tools: Option<String>,
    pub is_tool: bool,
    pub hooks: Vec<String>,
    pub can_block: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    pub skill_md: PathBuf,
    pub source: SkillSource,
    pub compatibility: Option<String>,
    pub license: Option<String>,
    pub model: Option<String>,
    pub metadata: BTreeMap<String, String>,
    pub allowed_tools: Option<String>,
    pub is_tool: bool,
    pub hooks: Vec<String>,
    pub can_block: Vec<String>,
}

fn find_skill_md(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.eq_ignore_ascii_case("skill.md") && entry.path().is_file() {
            return Some(entry.path());
        }
    }
    None
}

fn strip_quotes(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(s)
}

fn validate_name(name: &str, dir_name: &str) {
    if name.len() > 64 {
        tracing::warn!(
            "skill '{}': name exceeds 64 characters ({})",
            name,
            name.len()
        );
    }
    if name != dir_name {
        tracing::warn!(
            "skill '{}': name does not match directory name '{}'",
            name,
            dir_name
        );
    }
    if name.starts_with('-') || name.ends_with('-') {
        tracing::warn!("skill '{}': name must not start or end with a hyphen", name);
    }
    if name.contains("--") {
        tracing::warn!(
            "skill '{}': name must not contain consecutive hyphens",
            name
        );
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        tracing::warn!(
            "skill '{}': name should only contain lowercase letters, digits, and hyphens",
            name
        );
    }
}

pub fn parse_skill_frontmatter(content: &str) -> (SkillMetadata, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (SkillMetadata::default(), content.to_string());
    }

    let after_first = &trimmed[3..];
    let after_first = after_first.strip_prefix('\n').unwrap_or(after_first);

    let Some(end_idx) = after_first.find("\n---") else {
        return (SkillMetadata::default(), content.to_string());
    };

    let frontmatter = &after_first[..end_idx];
    let body_start = end_idx + 4; // "\n---"
    let body = after_first[body_start..]
        .strip_prefix('\n')
        .unwrap_or(&after_first[body_start..]);

    let mut meta = SkillMetadata::default();
    let mut in_metadata = false;

    for line in frontmatter.lines() {
        let trimmed_line = line.trim();

        if in_metadata {
            let is_indented = line.starts_with(' ') || line.starts_with('\t');
            if is_indented {
                if let Some((k, v)) = trimmed_line.split_once(':') {
                    meta.metadata
                        .insert(k.trim().to_string(), strip_quotes(v.trim()).to_string());
                }
                continue;
            } else {
                in_metadata = false;
            }
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = strip_quotes(value.trim());
        match key {
            "name" => meta.name = Some(value.to_string()),
            "description" => meta.description = Some(value.to_string()),
            "compatibility" => meta.compatibility = Some(value.to_string()),
            "license" => meta.license = Some(value.to_string()),
            "model" => meta.model = Some(value.to_string()),
            "allowed-tools" => meta.allowed_tools = Some(value.to_string()),
            "isTool" | "is-tool" | "is_tool" => {
                meta.is_tool = value.eq_ignore_ascii_case("true");
            }
            "hooks" => {
                meta.hooks = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "can-block" | "can_block" | "canBlock" => {
                meta.can_block = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            "metadata" => {
                in_metadata = true;
            }
            _ => {}
        }
    }

    (meta, body.to_string())
}

fn scan_skills_dir(dir: &Path, source: SkillSource) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut skills = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let dir = path;
        if let Some(skill_md) = find_skill_md(&dir) {
            let content = std::fs::read_to_string(&skill_md).unwrap_or_default();
            let (meta, _body) = parse_skill_frontmatter(&content);

            let description = match &meta.description {
                Some(d) if !d.is_empty() => d.clone(),
                _ => {
                    tracing::info!(
                        "skill '{}' skipped: missing description in frontmatter (see agentskills.io)",
                        dir_name
                    );
                    continue;
                }
            };

            let name = meta.name.clone().unwrap_or_else(|| dir_name.clone());
            if meta.name.is_some() {
                validate_name(&name, &dir_name);
            }

            skills.push(Skill {
                name,
                description,
                dir,
                skill_md,
                source: source.clone(),
                compatibility: meta.compatibility,
                license: meta.license,
                model: meta.model,
                metadata: meta.metadata,
                allowed_tools: meta.allowed_tools,
                is_tool: meta.is_tool,
                hooks: meta.hooks,
                can_block: meta.can_block,
            });
        }
    }
    skills.sort_by_key(|a| a.name.to_lowercase());
    skills
}

pub fn discover_in(base: &Path, source: SkillSource) -> Vec<Skill> {
    scan_skills_dir(&base.join("skills"), source)
}

fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~") {
        home.join(rest)
    } else {
        path.to_path_buf()
    }
}

pub fn discover_layered(
    project_dir: Option<&Path>,
    user_home: &Path,
    custom_dirs: &[PathBuf],
) -> Vec<Skill> {
    let mut seen = HashSet::new();
    let mut all = Vec::new();

    let mut add = |skills: Vec<Skill>| {
        for skill in skills {
            if !seen.insert(skill.name.clone()) {
                tracing::warn!("skill '{}' shadowed by higher-priority source", skill.name);
            } else {
                all.push(skill);
            }
        }
    };

    if let Some(p) = project_dir {
        add(scan_skills_dir(
            &p.join(".phx/skills"),
            SkillSource::Project,
        ));
        add(scan_skills_dir(
            &p.join(".agents/skills"),
            SkillSource::Project,
        ));
    }

    add(scan_skills_dir(
        &user_home.join(".phx/skills"),
        SkillSource::User,
    ));
    add(scan_skills_dir(
        &user_home.join(".agents/skills"),
        SkillSource::User,
    ));

    for dir in custom_dirs {
        let expanded = expand_tilde(dir, user_home);
        add(scan_skills_dir(&expanded, SkillSource::Explicit));
    }

    all.sort_by_key(|a| a.name.to_lowercase());
    all
}

// ---------------------------------------------------------------------------
// Catalog disclosure (Tier 1)
// ---------------------------------------------------------------------------

pub fn build_skill_catalog(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut catalog = String::from(
        "The following skills provide specialized instructions for specific tasks.\n\
         When a task matches a skill's description, use your file-read tool to load\n\
         the SKILL.md at the listed location before proceeding.\n\
         When a skill references relative paths, resolve them against the skill's\n\
         directory (the parent of SKILL.md) and use absolute paths in tool calls.\n\n\
         <available_skills>\n",
    );

    for skill in skills.iter().filter(|s| !s.is_tool) {
        catalog.push_str("  <skill>\n");
        catalog.push_str(&format!("    <name>{}</name>\n", skill.name));
        catalog.push_str(&format!(
            "    <description>{}</description>\n",
            skill.description
        ));
        catalog.push_str(&format!(
            "    <location>{}</location>\n",
            skill.skill_md.display()
        ));
        catalog.push_str("  </skill>\n");
    }

    catalog.push_str("</available_skills>");
    catalog
}

// ---------------------------------------------------------------------------
// Structured activation (Tier 2)
// ---------------------------------------------------------------------------

fn list_skill_resources(skill_dir: &Path) -> Vec<String> {
    let mut resources = Vec::new();
    let subdirs = ["scripts", "references", "assets"];

    for subdir in &subdirs {
        let dir = skill_dir.join(subdir);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    resources.push(format!(
                        "{}/{}",
                        subdir,
                        entry.file_name().to_string_lossy()
                    ));
                }
            }
        }
    }

    resources.sort();
    resources
}

pub fn load_skill_body(skill: &Skill) -> anyhow::Result<String> {
    let content = std::fs::read_to_string(&skill.skill_md)?;
    let (_meta, body) = parse_skill_frontmatter(&content);

    let resources = list_skill_resources(&skill.dir);

    let mut output = format!(
        "<skill_content name=\"{}\">\n{}\n\nSkill directory: {}\nRelative paths in this skill are relative to the skill directory.",
        skill.name,
        body,
        skill.dir.display()
    );

    if !resources.is_empty() {
        output.push_str("\n\n<skill_resources>\n");
        for r in &resources {
            output.push_str(&format!("  <file>{r}</file>\n"));
        }
        output.push_str("</skill_resources>");
    }

    output.push_str("\n</skill_content>");

    Ok(output)
}

pub fn load_skill_prompt(skill: &Skill) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(&skill.skill_md)?)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn skill_with_desc(dir: &Path, name: &str, desc: &str) {
        let skills_dir = dir.join(name);
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(
            skills_dir.join("skill.md"),
            format!("---\nname: {name}\ndescription: {desc}\n---\n# {name}"),
        )
        .unwrap();
    }

    #[test]
    fn empty_when_no_skills_dir() {
        let dir = tempdir().unwrap();
        let skills = discover_in(dir.path(), SkillSource::User);
        assert!(skills.is_empty());
    }

    #[test]
    fn finds_sorted_skills() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        skill_with_desc(&skills_dir, "beta", "Beta skill");
        skill_with_desc(&skills_dir, "alpha", "Alpha skill");

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "alpha");
        assert_eq!(skills[1].name, "beta");
    }

    #[test]
    fn skips_dirs_without_skill_md() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        skill_with_desc(&skills_dir, "valid", "A valid skill");
        std::fs::create_dir_all(skills_dir.join("empty")).unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "valid");
    }

    #[test]
    fn skips_skills_without_description() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("no-desc")).unwrap();
        std::fs::write(
            skills_dir.join("no-desc/skill.md"),
            "---\nname: no-desc\n---\n# No desc",
        )
        .unwrap();

        skill_with_desc(&skills_dir, "has-desc", "I have a description");

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "has-desc");
    }

    #[test]
    fn skips_skills_without_frontmatter() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("plain")).unwrap();
        std::fs::write(skills_dir.join("plain/skill.md"), "# Just markdown").unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        assert!(skills.is_empty());
    }

    #[test]
    fn case_insensitive_skill_md() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("upper")).unwrap();
        std::fs::write(
            skills_dir.join("upper/SKILL.MD"),
            "---\nname: upper\ndescription: Upper case file\n---\n# Upper",
        )
        .unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 1);
    }

    #[test]
    fn layered_discovery() {
        let home = tempdir().unwrap();
        let project = tempdir().unwrap();
        skill_with_desc(&home.path().join(".phx/skills"), "a", "Skill A");
        skill_with_desc(&project.path().join(".phx/skills"), "b", "Skill B");

        let skills = discover_layered(Some(project.path()), home.path(), &[]);
        assert_eq!(skills.len(), 2);
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn load_skill_prompt_reads_content() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        skill_with_desc(&skills_dir, "test", "Test skill");

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 1);
        let content = load_skill_prompt(&skills[0]).unwrap();
        assert!(content.contains("test"));
    }

    // --- Frontmatter parsing tests ---

    #[test]
    fn parse_frontmatter_full() {
        let content = "---\nname: my-skill\ndescription: Do cool things\ncompatibility: \"Requires Python 3.8+\"\nlicense: MIT\n---\n# Instructions\nHello";
        let (meta, body) = parse_skill_frontmatter(content);
        assert_eq!(meta.name.as_deref(), Some("my-skill"));
        assert_eq!(meta.description.as_deref(), Some("Do cool things"));
        assert_eq!(meta.compatibility.as_deref(), Some("Requires Python 3.8+"));
        assert_eq!(meta.license.as_deref(), Some("MIT"));
        assert!(body.starts_with("# Instructions"));
    }

    #[test]
    fn parse_frontmatter_no_frontmatter() {
        let content = "# Just markdown\nNo frontmatter here.";
        let (meta, body) = parse_skill_frontmatter(content);
        assert!(meta.name.is_none());
        assert!(meta.description.is_none());
        assert_eq!(body, content);
    }

    #[test]
    fn parse_frontmatter_single_quotes() {
        let content = "---\nname: 'quoted-name'\ndescription: 'A description'\n---\nbody";
        let (meta, _body) = parse_skill_frontmatter(content);
        assert_eq!(meta.name.as_deref(), Some("quoted-name"));
        assert_eq!(meta.description.as_deref(), Some("A description"));
    }

    #[test]
    fn parse_frontmatter_missing_description() {
        let content = "---\nname: no-desc\n---\nbody";
        let (meta, _body) = parse_skill_frontmatter(content);
        assert_eq!(meta.name.as_deref(), Some("no-desc"));
        assert!(meta.description.is_none());
    }

    #[test]
    fn parse_frontmatter_malformed() {
        let content = "---\ngarbage content without colons\n---\nbody";
        let (meta, body) = parse_skill_frontmatter(content);
        assert!(meta.name.is_none());
        assert_eq!(body, "body");
    }

    #[test]
    fn parse_frontmatter_metadata_block() {
        let content = "---\nname: meta-skill\ndescription: Has metadata\nmetadata:\n  author: example-org\n  version: \"1.0\"\n---\nbody";
        let (meta, _body) = parse_skill_frontmatter(content);
        assert_eq!(
            meta.metadata.get("author").map(|s| s.as_str()),
            Some("example-org")
        );
        assert_eq!(
            meta.metadata.get("version").map(|s| s.as_str()),
            Some("1.0")
        );
    }

    #[test]
    fn parse_frontmatter_allowed_tools() {
        let content = "---\nname: tools-skill\ndescription: Has tools\nallowed-tools: Bash(git:*) Read\n---\nbody";
        let (meta, _body) = parse_skill_frontmatter(content);
        assert_eq!(meta.allowed_tools.as_deref(), Some("Bash(git:*) Read"));
    }

    #[test]
    fn parse_frontmatter_model() {
        let content = "---\nname: fancy\ndescription: Uses a specific model\nmodel: claude-opus-4-7\n---\nbody";
        let (meta, _body) = parse_skill_frontmatter(content);
        assert_eq!(meta.model.as_deref(), Some("claude-opus-4-7"));
    }

    #[test]
    fn parse_frontmatter_model_quoted() {
        let content = "---\nname: fancy\ndescription: Quoted model\nmodel: \"gpt-4.1\"\n---\nbody";
        let (meta, _body) = parse_skill_frontmatter(content);
        assert_eq!(meta.model.as_deref(), Some("gpt-4.1"));
    }

    #[test]
    fn discovered_skill_carries_model() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("modeled")).unwrap();
        std::fs::write(
            skills_dir.join("modeled/skill.md"),
            "---\nname: modeled\ndescription: Has model\nmodel: claude-sonnet-4-6\n---\n# Body",
        )
        .unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].model.as_deref(), Some("claude-sonnet-4-6"));
    }

    // --- Discovery tests ---

    #[test]
    fn discover_agents_dir() {
        let home = tempdir().unwrap();
        skill_with_desc(
            &home.path().join(".agents/skills"),
            "cross-client",
            "Cross-client skill",
        );

        let skills = discover_layered(None, home.path(), &[]);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "cross-client");
        assert_eq!(skills[0].description, "Cross-client skill");
    }

    #[test]
    fn discover_custom_dirs() {
        let home = tempdir().unwrap();
        let custom = tempdir().unwrap();
        std::fs::create_dir_all(custom.path().join("my-skill")).unwrap();
        std::fs::write(
            custom.path().join("my-skill/skill.md"),
            "---\nname: my-skill\ndescription: From custom dir\n---\n# Custom",
        )
        .unwrap();

        let skills = discover_layered(None, home.path(), &[custom.path().to_path_buf()]);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert_eq!(skills[0].source, SkillSource::Explicit);
    }

    #[test]
    fn project_overrides_user() {
        let home = tempdir().unwrap();
        let project = tempdir().unwrap();

        skill_with_desc(&home.path().join(".phx/skills"), "shared", "User version");
        skill_with_desc(
            &project.path().join(".phx/skills"),
            "shared",
            "Project version",
        );

        let skills = discover_layered(Some(project.path()), home.path(), &[]);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "Project version");
        assert_eq!(skills[0].source, SkillSource::Project);
    }

    #[test]
    fn phx_takes_precedence_over_agents_same_scope() {
        let home = tempdir().unwrap();

        skill_with_desc(&home.path().join(".phx/skills"), "dupe", "From phx");
        skill_with_desc(&home.path().join(".agents/skills"), "dupe", "From agents");

        let skills = discover_layered(None, home.path(), &[]);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "From phx");
    }

    #[test]
    fn tilde_expansion_in_custom_dirs() {
        let home = tempdir().unwrap();
        skill_with_desc(
            &home.path().join(".claude/skills"),
            "test-skill",
            "From claude",
        );

        let custom = vec![PathBuf::from("~/.claude/skills")];
        let skills = discover_layered(None, home.path(), &custom);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "test-skill");
    }

    // --- Catalog tests ---

    #[test]
    fn catalog_empty_when_no_skills() {
        let catalog = build_skill_catalog(&[]);
        assert!(catalog.is_empty());
    }

    #[test]
    fn catalog_contains_skill_info() {
        let home = tempdir().unwrap();
        skill_with_desc(&home.path().join(".phx/skills"), "my-skill", "Does things");

        let skills = discover_layered(None, home.path(), &[]);
        let catalog = build_skill_catalog(&skills);
        assert!(catalog.contains("<available_skills>"));
        assert!(catalog.contains("<name>my-skill</name>"));
        assert!(catalog.contains("<description>Does things</description>"));
        assert!(catalog.contains("<location>"));
        assert!(catalog.contains("</available_skills>"));
        assert!(catalog.contains("use your file-read tool"));
    }

    // --- Activation tests ---

    #[test]
    fn load_skill_body_strips_frontmatter_with_wrapping() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        std::fs::create_dir_all(skills_dir.join("fm")).unwrap();
        std::fs::write(
            skills_dir.join("fm/skill.md"),
            "---\nname: fm\ndescription: test\n---\n# Body only",
        )
        .unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        let body = load_skill_body(&skills[0]).unwrap();
        assert!(body.contains("<skill_content name=\"fm\">"));
        assert!(body.contains("# Body only"));
        assert!(body.contains("Skill directory:"));
        assert!(body.contains("</skill_content>"));
    }

    #[test]
    fn load_skill_body_lists_resources() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills/resourceful");
        std::fs::create_dir_all(skills_dir.join("scripts")).unwrap();
        std::fs::write(skills_dir.join("scripts/run.sh"), "#!/bin/bash").unwrap();
        std::fs::create_dir_all(skills_dir.join("references")).unwrap();
        std::fs::write(skills_dir.join("references/api.md"), "# API").unwrap();
        std::fs::write(
            skills_dir.join("skill.md"),
            "---\nname: resourceful\ndescription: Has resources\n---\n# Instructions",
        )
        .unwrap();

        let skills = discover_in(dir.path(), SkillSource::User);
        let body = load_skill_body(&skills[0]).unwrap();
        assert!(body.contains("<skill_resources>"));
        assert!(body.contains("<file>scripts/run.sh</file>"));
        assert!(body.contains("<file>references/api.md</file>"));
        assert!(body.contains("</skill_resources>"));
    }

    #[test]
    fn load_skill_body_no_resources() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        skill_with_desc(&skills_dir, "plain", "A plain skill");

        let skills = discover_in(dir.path(), SkillSource::User);
        let body = load_skill_body(&skills[0]).unwrap();
        assert!(body.contains("<skill_content"));
        assert!(!body.contains("<skill_resources>"));
    }
}

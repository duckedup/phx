use async_trait::async_trait;
use serde_json::{Value, json};

use crate::session::skills::{Skill, load_skill_body};

use super::traits::{InputRequester, Tool, ToolError, ToolResult, ToolSchema};

pub struct SkillToolAdapter {
    skill: Skill,
}

impl SkillToolAdapter {
    pub fn new(skill: Skill) -> Self {
        Self { skill }
    }

    pub fn skill_name(&self) -> &str {
        &self.skill.name
    }
}

#[async_trait]
impl Tool for SkillToolAdapter {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: format!("skill_{}", self.skill.name.replace('-', "_")),
            description: format!(
                "Activate the '{}' skill: {}",
                self.skill.name, self.skill.description
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "arguments": {
                        "type": "string",
                        "description": "Optional arguments for the skill"
                    }
                }
            }),
        }
    }

    async fn invoke(
        &self,
        _args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        match load_skill_body(&self.skill) {
            Ok(body) => Ok(ToolResult::success(body)),
            Err(e) => Err(ToolError::ExecutionFailed(format!(
                "failed to load skill '{}': {e}",
                self.skill.name
            ))),
        }
    }
}

const BUILTIN_TOOL_NAMES: &[&str] = &[
    "bash",
    "read",
    "write",
    "edit",
    "spawn_agent",
    "check_agents",
    "collect_agent",
    "cancel_agent",
    "merge_agent",
];

pub fn is_builtin_tool(name: &str) -> bool {
    let normalized = format!("skill_{}", name.replace('-', "_"));
    BUILTIN_TOOL_NAMES.contains(&normalized.as_str()) || BUILTIN_TOOL_NAMES.contains(&name)
}

pub fn register_skill_tools(skills: &[Skill], registry: &mut super::traits::ToolRegistry) {
    for skill in skills.iter().filter(|s| s.is_tool) {
        if is_builtin_tool(&skill.name) {
            tracing::warn!(
                "skill '{}' has isTool=true but collides with a built-in tool, skipping",
                skill.name
            );
            continue;
        }
        let adapter = SkillToolAdapter::new(skill.clone());
        tracing::info!("registering skill-tool '{}'", skill.name);
        registry.register(std::sync::Arc::new(adapter));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::skills::{Skill, SkillSource};
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn test_skill(dir: &std::path::Path) -> Skill {
        let skill_dir = dir.join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: test-skill\ndescription: A test skill\nisTool: true\n---\n# Test instructions\nDo the thing.",
        )
        .unwrap();
        Skill {
            name: "test-skill".into(),
            description: "A test skill".into(),
            dir: skill_dir.clone(),
            skill_md: skill_dir.join("skill.md"),
            source: SkillSource::Project,
            compatibility: None,
            license: None,
            metadata: BTreeMap::new(),
            allowed_tools: None,
            is_tool: true,
            hooks: vec![],
            can_block: vec![],
        }
    }

    #[test]
    fn schema_uses_skill_name() {
        let dir = tempdir().unwrap();
        let skill = test_skill(dir.path());
        let adapter = SkillToolAdapter::new(skill);
        let schema = adapter.schema();
        assert_eq!(schema.name, "skill_test_skill");
        assert!(schema.description.contains("test-skill"));
    }

    #[tokio::test]
    async fn invoke_returns_skill_body() {
        let dir = tempdir().unwrap();
        let skill = test_skill(dir.path());
        let adapter = SkillToolAdapter::new(skill);
        let noop = crate::tools::traits::NoopInputRequester;
        let result = adapter.invoke(json!({}), &noop).await.unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("skill_content"));
        assert!(result.output.contains("Do the thing"));
    }

    #[test]
    fn builtin_collision_detected() {
        assert!(is_builtin_tool("bash"));
        assert!(is_builtin_tool("read"));
        assert!(!is_builtin_tool("my-custom-skill"));
    }
}

use std::sync::Arc;

use async_trait::async_trait;
use phoenix_shared::hook_types::HookAction;

use crate::session::skills::{Skill, load_skill_body};

use super::hooks::HookSubscriber;

pub struct SkillHookSubscriber {
    skill: Skill,
}

impl SkillHookSubscriber {
    pub fn new(skill: Skill) -> Self {
        Self { skill }
    }
}

#[async_trait]
impl HookSubscriber for SkillHookSubscriber {
    async fn on_notify(&self, event: &str, _data: serde_json::Value) -> anyhow::Result<()> {
        tracing::debug!(
            "skill '{}' received hook notification: {event}",
            self.skill.name
        );
        Ok(())
    }

    async fn on_hook(&self, event: &str, _data: serde_json::Value) -> anyhow::Result<HookAction> {
        tracing::debug!(
            "skill '{}' received blockable hook: {event}",
            self.skill.name
        );
        match load_skill_body(&self.skill) {
            Ok(_body) => Ok(HookAction::Allow),
            Err(e) => {
                tracing::warn!("skill hook '{}' failed to load body: {e}", self.skill.name);
                Ok(HookAction::Allow)
            }
        }
    }

    fn name(&self) -> &str {
        &self.skill.name
    }
}

pub fn register_skill_hooks(skills: &[Skill], dispatcher: &super::hooks::HookDispatcher) {
    let rt = tokio::runtime::Handle::try_current();
    for skill in skills.iter().filter(|s| !s.hooks.is_empty()) {
        let subscriber = Arc::new(SkillHookSubscriber::new(skill.clone()));
        let subscribe = skill.hooks.clone();
        let can_block = skill.can_block.clone();
        if let Ok(handle) = &rt {
            handle.block_on(dispatcher.register_subscriber(
                subscriber as Arc<dyn HookSubscriber>,
                subscribe,
                can_block,
            ));
        } else {
            tracing::warn!(
                "no tokio runtime available to register skill hook '{}'",
                skill.name
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::skills::SkillSource;
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    fn hook_skill(dir: &std::path::Path) -> Skill {
        let skill_dir = dir.join("guard");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("skill.md"),
            "---\nname: guard\ndescription: Guard skill\nhooks: tool_call_start\ncan-block: tool_call_start\n---\nBlock dangerous ops.",
        )
        .unwrap();
        Skill {
            name: "guard".into(),
            description: "Guard skill".into(),
            dir: skill_dir.clone(),
            skill_md: skill_dir.join("skill.md"),
            source: SkillSource::Project,
            compatibility: None,
            license: None,
            metadata: BTreeMap::new(),
            allowed_tools: None,
            is_tool: false,
            hooks: vec!["tool_call_start".into()],
            can_block: vec!["tool_call_start".into()],
        }
    }

    #[tokio::test]
    async fn skill_hook_subscriber_allows_by_default() {
        let dir = tempdir().unwrap();
        let skill = hook_skill(dir.path());
        let sub = SkillHookSubscriber::new(skill);
        let action = sub
            .on_hook("tool_call_start", serde_json::json!({}))
            .await
            .unwrap();
        assert!(matches!(action, HookAction::Allow));
    }

    #[tokio::test]
    async fn skill_hook_subscriber_name() {
        let dir = tempdir().unwrap();
        let skill = hook_skill(dir.path());
        let sub = SkillHookSubscriber::new(skill);
        assert_eq!(sub.name(), "guard");
    }
}

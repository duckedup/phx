use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillMetadataShared {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub is_tool: bool,
    #[serde(default)]
    pub hooks: Vec<String>,
    #[serde(default)]
    pub can_block: Vec<String>,
    #[serde(default)]
    pub allowed_tools: Option<String>,
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillResultData {
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub toast: String,
    #[serde(default)]
    pub widget: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_metadata_defaults() {
        let meta: SkillMetadataShared = serde_json::from_str(r#"{"name": "test"}"#).unwrap();
        assert_eq!(meta.name, "test");
        assert!(!meta.is_tool);
        assert!(meta.hooks.is_empty());
    }

    #[test]
    fn skill_metadata_with_is_tool() {
        let meta: SkillMetadataShared =
            serde_json::from_str(r#"{"name": "guard", "is_tool": true}"#).unwrap();
        assert!(meta.is_tool);
    }

    #[test]
    fn skill_result_data_default() {
        let r = SkillResultData::default();
        assert!(r.context.is_empty());
        assert!(r.toast.is_empty());
        assert!(r.widget.is_empty());
    }
}

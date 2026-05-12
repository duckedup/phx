use phoenix_shared::ui_field_types::UiField;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
    #[serde(default)]
    pub tools: Vec<PluginToolDef>,
    #[serde(default)]
    pub events: PluginEvents,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommand {
    pub name: String,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_params")]
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub needs_context: bool,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub keybind: String,
    #[serde(default)]
    pub ui_fields: Vec<UiField>,
}

fn default_params() -> serde_json::Value {
    serde_json::json!({"type": "object", "properties": {}})
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginEvents {
    #[serde(default)]
    pub subscribe: Vec<String>,
    #[serde(default)]
    pub can_block: Vec<String>,
}

pub fn load_manifest(dir: &Path) -> anyhow::Result<PluginManifest> {
    let path = dir.join("plugin.json");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let manifest: PluginManifest = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("invalid plugin.json in {}: {e}", dir.display()))?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(m: &PluginManifest) -> anyhow::Result<()> {
    if m.name.is_empty() {
        anyhow::bail!("plugin name is required");
    }
    if m.command.is_empty() {
        anyhow::bail!("plugin command is required");
    }
    for cmd in &m.commands {
        if cmd.name.is_empty() {
            anyhow::bail!("plugin command name is required");
        }
    }
    for tool in &m.tools {
        if tool.name.is_empty() {
            anyhow::bail!("plugin tool name is required");
        }
    }
    for event in &m.events.can_block {
        if !m.events.subscribe.contains(event) {
            anyhow::bail!("can_block event '{event}' must also be in subscribe list");
        }
    }
    Ok(())
}

pub fn resolve_command(manifest: &PluginManifest, plugin_dir: &Path) -> PathBuf {
    let cmd = &manifest.command;
    let path = Path::new(cmd);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        plugin_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_valid_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{
                "name": "test-plugin",
                "command": "./run.sh",
                "commands": [{"name": "greet", "summary": "Say hello"}],
                "tools": [{"name": "my_tool", "description": "Does stuff"}],
                "events": {"subscribe": ["tool_call_start"], "can_block": ["tool_call_start"]}
            }"#,
        )
        .unwrap();

        let m = load_manifest(dir.path()).unwrap();
        assert_eq!(m.name, "test-plugin");
        assert_eq!(m.commands.len(), 1);
        assert_eq!(m.tools.len(), 1);
        assert!(m.events.can_block.contains(&"tool_call_start".to_string()));
    }

    #[test]
    fn missing_name_fails() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "", "command": "./run.sh"}"#,
        )
        .unwrap();
        assert!(load_manifest(dir.path()).is_err());
    }

    #[test]
    fn can_block_not_in_subscribe_fails() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "bad", "command": "./run.sh", "events": {"subscribe": [], "can_block": ["token"]}}"#,
        )
        .unwrap();
        assert!(load_manifest(dir.path()).is_err());
    }

    #[test]
    fn resolve_relative_command() {
        let m = PluginManifest {
            name: "test".into(),
            version: String::new(),
            description: String::new(),
            command: "./bin/plugin".into(),
            args: vec![],
            commands: vec![],
            tools: vec![],
            events: PluginEvents::default(),
        };
        let resolved = resolve_command(&m, Path::new("/plugins/test"));
        assert_eq!(resolved, PathBuf::from("/plugins/test/./bin/plugin"));
    }

    #[test]
    fn resolve_absolute_command() {
        let m = PluginManifest {
            name: "test".into(),
            version: String::new(),
            description: String::new(),
            command: "/usr/bin/my-plugin".into(),
            args: vec![],
            commands: vec![],
            tools: vec![],
            events: PluginEvents::default(),
        };
        let resolved = resolve_command(&m, Path::new("/plugins/test"));
        assert_eq!(resolved, PathBuf::from("/usr/bin/my-plugin"));
    }
}

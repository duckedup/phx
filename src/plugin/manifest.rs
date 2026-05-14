use phx_shared::ui_field_types::UiField;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub bin: String,
    #[serde(default)]
    pub bin_args: Vec<String>,
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
    #[serde(default)]
    pub shell: Option<String>,
    #[serde(default)]
    pub bin: Option<String>,
    #[serde(default)]
    pub bin_args: Vec<String>,
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
    let has_top_level_bin = !m.bin.is_empty();
    for cmd in &m.commands {
        if cmd.name.is_empty() {
            anyhow::bail!("plugin command name is required");
        }
    }
    for tool in &m.tools {
        if tool.name.is_empty() {
            anyhow::bail!("plugin tool name is required");
        }
        let has_shell = tool.shell.is_some();
        let has_bin = tool.bin.is_some();
        if has_shell && has_bin {
            anyhow::bail!("tool '{}' cannot have both 'shell' and 'bin'", tool.name);
        }
        if !has_shell && !has_bin && !has_top_level_bin {
            anyhow::bail!(
                "tool '{}' has no execution strategy: needs 'shell', 'bin', or a top-level 'bin'",
                tool.name
            );
        }
    }
    for event in &m.events.can_block {
        if !m.events.subscribe.contains(event) {
            anyhow::bail!("can_block event '{event}' must also be in subscribe list");
        }
    }
    Ok(())
}

pub fn resolve_bin(manifest: &PluginManifest, plugin_dir: &Path) -> PathBuf {
    let bin = &manifest.bin;
    let path = Path::new(bin);
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
                "bin": "./run.sh",
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
            r#"{"name": "", "bin": "./run.sh"}"#,
        )
        .unwrap();
        assert!(load_manifest(dir.path()).is_err());
    }

    #[test]
    fn can_block_not_in_subscribe_fails() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "bad", "bin": "./run.sh", "events": {"subscribe": [], "can_block": ["token"]}}"#,
        )
        .unwrap();
        assert!(load_manifest(dir.path()).is_err());
    }

    #[test]
    fn shell_tool_no_top_level_command_ok() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "git-tools", "tools": [{"name": "diff", "shell": "git diff"}]}"#,
        )
        .unwrap();
        let m = load_manifest(dir.path()).unwrap();
        assert_eq!(m.tools[0].shell.as_deref(), Some("git diff"));
    }

    #[test]
    fn tool_no_execution_strategy_fails() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "bad", "tools": [{"name": "t"}]}"#,
        )
        .unwrap();
        assert!(load_manifest(dir.path()).is_err());
    }

    #[test]
    fn tool_both_shell_and_bin_fails() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("plugin.json"),
            r#"{"name": "bad", "tools": [{"name": "t", "shell": "echo hi", "bin": "./foo"}]}"#,
        )
        .unwrap();
        assert!(load_manifest(dir.path()).is_err());
    }

    #[test]
    fn resolve_relative_bin() {
        let m = PluginManifest {
            name: "test".into(),
            version: String::new(),
            description: String::new(),
            bin: "./bin/plugin".into(),
            bin_args: vec![],
            commands: vec![],
            tools: vec![],
            events: PluginEvents::default(),
        };
        let resolved = resolve_bin(&m, Path::new("/plugins/test"));
        assert_eq!(resolved, PathBuf::from("/plugins/test/./bin/plugin"));
    }

    #[test]
    fn resolve_absolute_bin() {
        let m = PluginManifest {
            name: "test".into(),
            version: String::new(),
            description: String::new(),
            bin: "/usr/bin/my-plugin".into(),
            bin_args: vec![],
            commands: vec![],
            tools: vec![],
            events: PluginEvents::default(),
        };
        let resolved = resolve_bin(&m, Path::new("/plugins/test"));
        assert_eq!(resolved, PathBuf::from("/usr/bin/my-plugin"));
    }
}

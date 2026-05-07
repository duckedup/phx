use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::tools::traits::ToolRegistry;

use super::handle::PluginHandle;
use super::hooks::HookDispatcher;
use super::manifest;
use super::tool_adapter::PluginToolAdapter;

pub struct PluginManager {
    handles: Vec<Arc<PluginHandle>>,
    commands: HashMap<String, Arc<PluginHandle>>,
    pub hooks: Arc<HookDispatcher>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            handles: Vec::new(),
            commands: HashMap::new(),
            hooks: Arc::new(HookDispatcher::new()),
        }
    }

    pub async fn load_and_start(
        &mut self,
        dirs: Vec<PathBuf>,
        project: &Path,
        tool_registry: &mut ToolRegistry,
    ) {
        for dir in dirs {
            if let Err(e) = self.load_plugin(dir, project, tool_registry).await {
                tracing::warn!("failed to load plugin: {e}");
            }
        }
    }

    async fn load_plugin(
        &mut self,
        dir: PathBuf,
        project: &Path,
        tool_registry: &mut ToolRegistry,
    ) -> anyhow::Result<()> {
        let manifest = manifest::load_manifest(&dir)?;
        let name = manifest.name.clone();

        tracing::info!("loading plugin '{name}' from {}", dir.display());

        let handle = PluginHandle::spawn(manifest.clone(), dir)?;
        let handle = Arc::new(handle);

        match handle.initialize(project).await {
            Ok(_) => tracing::info!("plugin '{name}' initialized"),
            Err(e) => {
                tracing::warn!("plugin '{name}' init failed: {e}");
                anyhow::bail!("plugin '{name}' init failed: {e}");
            }
        }

        for cmd in &manifest.commands {
            self.commands.insert(cmd.name.clone(), Arc::clone(&handle));
        }

        for tool_def in &manifest.tools {
            let adapter = PluginToolAdapter::new(Arc::clone(&handle), tool_def.clone());
            tool_registry.register(Arc::new(adapter));
        }

        self.hooks
            .register(
                Arc::clone(&handle),
                manifest.events.subscribe.clone(),
                manifest.events.can_block.clone(),
            )
            .await;

        self.handles.push(handle);
        Ok(())
    }

    pub fn get_command_handler(&self, name: &str) -> Option<Arc<PluginHandle>> {
        self.commands.get(name).cloned()
    }

    pub fn plugin_commands(&self) -> Vec<(&str, &str)> {
        let mut cmds = Vec::new();
        for handle in &self.handles {
            for cmd in &handle.manifest.commands {
                cmds.push((cmd.name.as_str(), cmd.summary.as_str()));
            }
        }
        cmds
    }

    pub fn plugin_count(&self) -> usize {
        self.handles.len()
    }

    pub async fn shutdown_all(&self) {
        for handle in &self.handles {
            handle.shutdown().await;
        }
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn discover_plugin_dirs(
    project: Option<&Path>,
    home: &Path,
    extra: &[PathBuf],
) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Some(p) = project {
        scan_plugins_dir(&p.join(".phoenix/plugins"), &mut dirs);
    }

    scan_plugins_dir(&home.join(".phoenix/plugins"), &mut dirs);

    for dir in extra {
        let expanded = expand_tilde(dir, home);
        if expanded.join("plugin.json").exists() {
            dirs.push(expanded);
        } else {
            scan_plugins_dir(&expanded, &mut dirs);
        }
    }

    dirs
}

fn scan_plugins_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            let path = entry.path();
            if path.join("plugin.json").exists() {
                out.push(path);
            }
        }
    }
}

fn expand_tilde(path: &Path, home: &Path) -> PathBuf {
    if let Ok(rest) = path.strip_prefix("~") {
        home.join(rest)
    } else {
        path.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn discover_empty_dirs() {
        let dirs = discover_plugin_dirs(None, Path::new("/nonexistent"), &[]);
        assert!(dirs.is_empty());
    }

    #[test]
    fn discover_project_plugins() {
        let project = tempdir().unwrap();
        let plugin_dir = project.path().join(".phoenix/plugins/my-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name":"my-plugin","command":"./run"}"#,
        )
        .unwrap();

        let dirs = discover_plugin_dirs(Some(project.path()), Path::new("/nonexistent"), &[]);
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with("my-plugin"));
    }

    #[test]
    fn discover_home_plugins() {
        let home = tempdir().unwrap();
        let plugin_dir = home.path().join(".phoenix/plugins/global-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.json"),
            r#"{"name":"global-plugin","command":"./run"}"#,
        )
        .unwrap();

        let dirs = discover_plugin_dirs(None, home.path(), &[]);
        assert_eq!(dirs.len(), 1);
    }

    #[test]
    fn discover_extra_dirs() {
        let extra = tempdir().unwrap();
        std::fs::write(
            extra.path().join("plugin.json"),
            r#"{"name":"extra","command":"./run"}"#,
        )
        .unwrap();

        let dirs = discover_plugin_dirs(
            None,
            Path::new("/nonexistent"),
            &[extra.path().to_path_buf()],
        );
        assert_eq!(dirs.len(), 1);
    }

    #[test]
    fn manager_starts_empty() {
        let mgr = PluginManager::new();
        assert_eq!(mgr.plugin_count(), 0);
        assert!(mgr.get_command_handler("anything").is_none());
        assert!(mgr.plugin_commands().is_empty());
    }
}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use phoenix_shared::ui_field_types::{ToolUiConfig, UiField};

#[derive(Clone, Debug)]
pub struct UnifiedToolMeta {
    pub name: String,
    pub description: String,
    pub parameters_json: String,
    pub command: String,
    pub keybind: String,
    pub ui: ToolUiConfig,
}

pub struct ToolExecResult {
    pub output: String,
    pub is_error: bool,
    pub toast: String,
    pub widget: String,
}

pub struct BuildOutput {
    pub name: String,
    pub package_name: String,
    pub success: bool,
    pub stderr: String,
    pub binary_dir: Option<PathBuf>,
}

pub struct ReloadResult {
    pub builds: Vec<BuildOutput>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(serde::Deserialize)]
struct ManifestJson {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    command: String,
    tools: Vec<ToolMetaJson>,
}

#[derive(serde::Deserialize)]
struct ToolMetaJson {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default = "default_params_str")]
    parameters: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    keybind: String,
    #[serde(default)]
    ui_fields: Vec<UiField>,
}

fn default_params_str() -> String {
    r#"{"type":"object","properties":{}}"#.to_string()
}

#[derive(serde::Deserialize)]
struct ToolResultJson {
    #[serde(default)]
    output: String,
    #[serde(default)]
    is_error: bool,
    #[serde(default)]
    toast: String,
    #[serde(default)]
    widget: String,
}

struct LoadedPlugin {
    binary_path: PathBuf,
    tools: Vec<UnifiedToolMeta>,
}

pub struct PluginRuntime {
    plugins: HashMap<String, LoadedPlugin>,
    tool_index: HashMap<String, String>,
    active_tools: HashSet<String>,
    project_dir: PathBuf,
}

impl PluginRuntime {
    pub fn new(project: PathBuf) -> Self {
        Self {
            plugins: HashMap::new(),
            tool_index: HashMap::new(),
            active_tools: HashSet::new(),
            project_dir: project,
        }
    }

    /// Scan a plugins base directory for subdirectories containing `manifest.json`.
    /// Layout: `<dir>/<plugin-name>/manifest.json` + the executable referenced by `command`.
    pub fn load_from_dir(&mut self, dir: &Path) -> anyhow::Result<Vec<UnifiedToolMeta>> {
        let mut loaded = Vec::new();

        if !dir.is_dir() {
            return Ok(loaded);
        }

        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("manifest.json");
            if !manifest_path.is_file() {
                continue;
            }
            match self.load_plugin_dir(&path) {
                Ok(tools) => {
                    for t in &tools {
                        tracing::info!("loaded plugin tool: {}", t.name);
                    }
                    loaded.extend(tools);
                }
                Err(e) => {
                    tracing::warn!(
                        dir = %path.display(),
                        error = %e,
                        "failed to load plugin"
                    );
                }
            }
        }

        Ok(loaded)
    }

    /// Load a single plugin from a directory containing `manifest.json`.
    pub fn load_plugin_dir(&mut self, plugin_dir: &Path) -> anyhow::Result<Vec<UnifiedToolMeta>> {
        let manifest_path = plugin_dir.join("manifest.json");
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", manifest_path.display()))?;

        let manifest: ManifestJson = serde_json::from_str(&content).map_err(|e| {
            anyhow::anyhow!("invalid manifest.json in {}: {e}", plugin_dir.display())
        })?;

        if manifest.tools.is_empty() {
            anyhow::bail!("plugin in {} exports no tools", plugin_dir.display());
        }

        if manifest.command.is_empty() {
            anyhow::bail!(
                "plugin in {} has no command field in manifest.json",
                plugin_dir.display()
            );
        }

        let binary_path = resolve_command(&manifest.command, plugin_dir);
        if !binary_path.exists() {
            anyhow::bail!(
                "plugin command not found: {} (resolved to {})",
                manifest.command,
                binary_path.display()
            );
        }

        let key = plugin_dir.to_string_lossy().to_string();

        let tools: Vec<UnifiedToolMeta> = manifest
            .tools
            .into_iter()
            .map(|t| UnifiedToolMeta {
                name: t.name,
                description: t.description,
                parameters_json: t.parameters,
                command: t.command,
                keybind: t.keybind,
                ui: ToolUiConfig::new(t.ui_fields),
            })
            .collect();

        if tools.iter().any(|t| self.tool_index.contains_key(&t.name)) {
            tracing::debug!(key, "skipping plugin — tools already registered");
            return Ok(Vec::new());
        }

        let result = tools.clone();

        for tool in &tools {
            self.tool_index.insert(tool.name.clone(), key.clone());
        }

        self.plugins
            .insert(key, LoadedPlugin { binary_path, tools });

        Ok(result)
    }

    pub fn invoke_tool(
        &mut self,
        tool_name: &str,
        args_json: &str,
    ) -> anyhow::Result<ToolExecResult> {
        let plugin_key = self
            .tool_index
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {tool_name}"))?
            .clone();

        let loaded = self
            .plugins
            .get(&plugin_key)
            .ok_or_else(|| anyhow::anyhow!("plugin not found: {plugin_key}"))?;

        let output = Command::new(&loaded.binary_path)
            .args(["invoke", tool_name, args_json])
            .current_dir(&self.project_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to invoke tool {tool_name} via {}: {e}",
                    loaded.binary_path.display()
                )
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = if stderr.is_empty() {
                stdout.to_string()
            } else {
                stderr.to_string()
            };
            return Err(anyhow::anyhow!("tool {tool_name} error: {msg}"));
        }

        let result: ToolResultJson = serde_json::from_str(&stdout).unwrap_or(ToolResultJson {
            output: stdout.to_string(),
            is_error: false,
            toast: String::new(),
            widget: String::new(),
        });

        Ok(ToolExecResult {
            output: result.output,
            is_error: result.is_error,
            toast: result.toast,
            widget: result.widget,
        })
    }

    pub fn exit_tool(&mut self, tool_name: &str) -> anyhow::Result<ToolExecResult> {
        let plugin_key = self
            .tool_index
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {tool_name}"))?
            .clone();

        let loaded = self
            .plugins
            .get(&plugin_key)
            .ok_or_else(|| anyhow::anyhow!("plugin not found: {plugin_key}"))?;

        let output = Command::new(&loaded.binary_path)
            .args(["exit", tool_name])
            .current_dir(&self.project_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to exit tool {tool_name} via {}: {e}",
                    loaded.binary_path.display()
                )
            })?;

        self.active_tools.remove(tool_name);

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result: ToolResultJson = serde_json::from_str(&stdout).unwrap_or(ToolResultJson {
            output: String::new(),
            is_error: false,
            toast: String::new(),
            widget: String::new(),
        });

        Ok(ToolExecResult {
            output: result.output,
            is_error: result.is_error,
            toast: result.toast,
            widget: result.widget,
        })
    }

    pub fn toggle_tool(
        &mut self,
        tool_name: &str,
        args_json: &str,
    ) -> anyhow::Result<ToolExecResult> {
        if self.active_tools.contains(tool_name) {
            self.exit_tool(tool_name)
        } else {
            self.active_tools.insert(tool_name.to_string());
            self.invoke_tool(tool_name, args_json)
        }
    }

    pub fn is_active(&self, tool_name: &str) -> bool {
        self.active_tools.contains(tool_name)
    }

    pub fn tool_for_keybind(&self, keybind: &str) -> Option<&str> {
        for loaded in self.plugins.values() {
            for tool in &loaded.tools {
                if !tool.keybind.is_empty() && tool.keybind == keybind {
                    return Some(&tool.name);
                }
            }
        }
        None
    }

    pub fn command_tools(&self) -> Vec<&UnifiedToolMeta> {
        let mut result = Vec::new();
        for loaded in self.plugins.values() {
            for tool in &loaded.tools {
                if !tool.command.is_empty() {
                    result.push(tool);
                }
            }
        }
        result
    }

    pub fn tool_ui(&self, tool_name: &str) -> Option<&ToolUiConfig> {
        let plugin_key = self.tool_index.get(tool_name)?;
        let loaded = self.plugins.get(plugin_key)?;
        loaded
            .tools
            .iter()
            .find(|t| t.name == tool_name)
            .filter(|t| !t.ui.is_empty())
            .map(|t| &t.ui)
    }

    pub fn all_tool_schemas(&self) -> Vec<&UnifiedToolMeta> {
        let mut result = Vec::new();
        for loaded in self.plugins.values() {
            for tool in &loaded.tools {
                result.push(tool);
            }
        }
        result
    }

    pub fn has_command(&self, command: &str) -> bool {
        self.plugins
            .values()
            .any(|p| p.tools.iter().any(|t| t.command == command))
    }

    pub fn commands(&self) -> Vec<(&str, &str)> {
        let mut result = Vec::new();
        for loaded in self.plugins.values() {
            for tool in &loaded.tools {
                if !tool.command.is_empty() {
                    result.push((tool.command.as_str(), tool.description.as_str()));
                }
            }
        }
        result
    }

    pub fn tool_count(&self) -> usize {
        self.plugins.values().map(|p| p.tools.len()).sum()
    }

    pub fn load_bundled(&mut self) -> Vec<UnifiedToolMeta> {
        Vec::new()
    }

    pub fn reload(&mut self, plugin_dirs: &[PathBuf], source_dirs: &[PathBuf]) -> ReloadResult {
        self.reload_inner(plugin_dirs, source_dirs, false)
    }

    pub fn reload_skip_build(
        &mut self,
        plugin_dirs: &[PathBuf],
        source_dirs: &[PathBuf],
    ) -> ReloadResult {
        self.reload_inner(plugin_dirs, source_dirs, true)
    }

    fn reload_inner(
        &mut self,
        plugin_dirs: &[PathBuf],
        source_dirs: &[PathBuf],
        skip_build: bool,
    ) -> ReloadResult {
        let old_tools: Vec<String> = self.tool_index.keys().cloned().collect();
        self.plugins.clear();
        self.tool_index.clear();

        let mut added = Vec::new();
        let mut errors = Vec::new();
        let mut builds = Vec::new();

        let install_base = plugin_dirs
            .first()
            .cloned()
            .unwrap_or_else(|| self.project_dir.join(".phoenix/plugins"));

        if !skip_build {
            let workspace_release = self.project_dir.join("target/release");
            let deduped = dedup_paths(source_dirs);
            for dir in &deduped {
                let build = build_plugin(dir);
                if build.success {
                    if let Err(e) =
                        install_built_plugin(&build, &install_base, Some(&workspace_release))
                    {
                        errors.push(format!("install {} failed: {e}", build.name));
                    }
                } else {
                    errors.push(format!("build {} failed", build.name));
                }
                builds.push(build);
            }
        }

        for dir in plugin_dirs {
            match self.load_from_dir(dir) {
                Ok(tools) => {
                    for t in tools {
                        added.push(t.name.clone());
                    }
                }
                Err(e) => {
                    errors.push(format!("{}: {e}", dir.display()));
                }
            }
        }

        let new_tools: Vec<String> = self.tool_index.keys().cloned().collect();
        let removed: Vec<String> = old_tools
            .iter()
            .filter(|t| !new_tools.contains(t))
            .cloned()
            .collect();

        ReloadResult {
            builds,
            added,
            removed,
            errors,
        }
    }

    pub fn discover_dirs(project: Option<&Path>, home: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        if let Some(p) = project {
            let d = p.join(".phoenix/plugins");
            if d.is_dir() {
                dirs.push(d);
            }
        }

        let d = home.join(".phoenix/plugins");
        if d.is_dir() {
            dirs.push(d);
        }

        dirs
    }

    pub fn discover_source_dirs(project: Option<&Path>) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        if let Some(p) = project {
            let plugins_dir = p.join("plugins");
            if plugins_dir.is_dir()
                && let Ok(entries) = std::fs::read_dir(&plugins_dir)
            {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() && path.join("Cargo.toml").exists() {
                        dirs.push(path);
                    }
                }
            }
        }

        dirs
    }
}

fn resolve_command(command: &str, plugin_dir: &Path) -> PathBuf {
    let path = Path::new(command);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        let stripped = command.strip_prefix("./").unwrap_or(command);
        plugin_dir.join(stripped)
    }
}

fn dedup_paths(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for d in dirs {
        let canonical = d.canonicalize().unwrap_or_else(|_| d.clone());
        if seen.insert(canonical) {
            out.push(d.clone());
        }
    }
    out
}

fn read_package_name(source_dir: &Path) -> Option<String> {
    let cargo_toml = source_dir.join("Cargo.toml");
    let content = std::fs::read_to_string(cargo_toml).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("name")
            && let Some(val) = line.split('=').nth(1)
        {
            return Some(val.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn build_plugin(source_dir: &Path) -> BuildOutput {
    use std::process::Stdio;

    let name = source_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let package_name = read_package_name(source_dir).unwrap_or_else(|| name.clone());

    let result = Command::new("cargo")
        .args(["build", "--release", "--color=never"])
        .current_dir(source_dir)
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();

    let output = match result {
        Ok(o) => o,
        Err(e) => {
            return BuildOutput {
                name,
                package_name,
                success: false,
                stderr: format!("failed to run cargo: {e}"),
                binary_dir: None,
            };
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return BuildOutput {
            name,
            package_name,
            success: false,
            stderr,
            binary_dir: None,
        };
    }

    let release_dir = source_dir.join("target/release");
    let binary_dir = if release_dir.is_dir() {
        Some(release_dir)
    } else {
        None
    };

    BuildOutput {
        name,
        package_name,
        success: true,
        stderr,
        binary_dir,
    }
}

fn install_built_plugin(
    build: &BuildOutput,
    install_base: &Path,
    workspace_release: Option<&Path>,
) -> anyhow::Result<()> {
    // Look for the exact binary by package name in candidate directories.
    // Workspace members output to the workspace root's target/release/,
    // standalone crates output to their own target/release/.
    let bin_name = format!("{}{}", build.package_name, std::env::consts::EXE_SUFFIX);

    let mut search_dirs: Vec<&Path> = Vec::new();
    if let Some(dir) = build.binary_dir.as_deref() {
        search_dirs.push(dir);
    }
    if let Some(ws) = workspace_release
        && ws.is_dir()
    {
        search_dirs.push(ws);
    }

    let mut binary_path = None;
    for dir in &search_dirs {
        let candidate = dir.join(&bin_name);
        if candidate.is_file() && is_executable(&candidate) {
            binary_path = Some(candidate);
            break;
        }
    }

    let binary = binary_path.ok_or_else(|| {
        anyhow::anyhow!(
            "binary '{}' not found (searched: {:?})",
            bin_name,
            search_dirs
        )
    })?;

    // Strip phoenix-plugin- prefix for the install directory name
    let short_name = build
        .package_name
        .strip_prefix("phoenix-plugin-")
        .or_else(|| build.package_name.strip_prefix("phoenix_plugin_"))
        .unwrap_or(&build.package_name);

    let install_dir = install_base.join(short_name);

    let output = Command::new(&binary)
        .args(["install", &install_dir.to_string_lossy()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run install for {}: {e}", build.package_name))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("install for {} failed: {}", build.package_name, stderr);
    }

    Ok(())
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = path.metadata() {
            return meta.permissions().mode() & 0o111 != 0;
        }
        false
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_dirs_empty_when_no_dirs_exist() {
        let dirs = PluginRuntime::discover_dirs(None, Path::new("/nonexistent"));
        assert!(dirs.is_empty());
    }

    #[test]
    fn discover_dirs_finds_project_dir() {
        let project = tempfile::tempdir().unwrap();
        let plugin_dir = project.path().join(".phoenix/plugins");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        let dirs = PluginRuntime::discover_dirs(Some(project.path()), Path::new("/nonexistent"));
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], plugin_dir);
    }

    #[test]
    fn resolve_command_relative() {
        let dir = Path::new("/plugins/test");
        assert_eq!(
            resolve_command("./my-binary", dir),
            PathBuf::from("/plugins/test/my-binary")
        );
        assert_eq!(
            resolve_command("my-binary", dir),
            PathBuf::from("/plugins/test/my-binary")
        );
    }

    #[test]
    fn resolve_command_absolute() {
        let dir = Path::new("/plugins/test");
        assert_eq!(
            resolve_command("/usr/bin/my-plugin", dir),
            PathBuf::from("/usr/bin/my-plugin")
        );
    }

    #[test]
    fn load_plugin_dir_reads_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("test-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        // Write a manifest that points to a nonexistent binary — should error
        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{"name":"test","command":"./nonexistent","tools":[{"name":"t","description":"d"}]}"#,
        )
        .unwrap();

        let mut rt = PluginRuntime::new(PathBuf::from("."));
        let result = rt.load_plugin_dir(&plugin_dir);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("command not found")
        );
    }
}

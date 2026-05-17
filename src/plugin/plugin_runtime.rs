use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::shared::ui_field_types::{ToolUiConfig, UiField};

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

impl ToolExecResult {
    pub fn empty() -> Self {
        Self {
            output: String::new(),
            is_error: false,
            toast: String::new(),
            widget: String::new(),
        }
    }
}

#[derive(Debug)]
pub struct BuildOutput {
    pub name: String,
    pub package_name: String,
    pub success: bool,
    pub stderr: String,
    pub binary_dir: Option<PathBuf>,
}

#[derive(Debug)]
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
    bin: String,
    #[serde(default)]
    bin_args: Vec<String>,
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
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    bin: Option<String>,
    #[serde(default)]
    bin_args: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum ToolExecKind {
    Shell(String),
    Bin { path: PathBuf, args: Vec<String> },
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
    tools: Vec<UnifiedToolMeta>,
    tool_exec: HashMap<String, ToolExecKind>,
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

        let top_bin_path = if !manifest.bin.is_empty() {
            let path = resolve_command(&manifest.bin, plugin_dir);
            if !path.exists() {
                anyhow::bail!(
                    "plugin bin not found: {} (resolved to {})",
                    manifest.bin,
                    path.display()
                );
            }
            Some(path)
        } else {
            None
        };

        let key = plugin_dir.to_string_lossy().to_string();

        let mut tool_exec = HashMap::new();
        let mut tools = Vec::new();

        for t in manifest.tools {
            let exec_kind = if let Some(ref shell) = t.shell {
                ToolExecKind::Shell(shell.clone())
            } else if let Some(ref bin) = t.bin {
                let bin_path = resolve_bin_path(bin, plugin_dir, &self.project_dir);
                if !bin_path.exists() {
                    anyhow::bail!(
                        "tool '{}' bin not found: {} (resolved to {})",
                        t.name,
                        bin,
                        bin_path.display()
                    );
                }
                ToolExecKind::Bin {
                    path: bin_path,
                    args: t.bin_args.clone(),
                }
            } else if let Some(ref top_path) = top_bin_path {
                ToolExecKind::Bin {
                    path: top_path.clone(),
                    args: manifest.bin_args.clone(),
                }
            } else {
                anyhow::bail!(
                    "tool '{}' in {} has no execution strategy (shell, bin, or top-level bin)",
                    t.name,
                    plugin_dir.display()
                );
            };

            tool_exec.insert(t.name.clone(), exec_kind);
            tools.push(UnifiedToolMeta {
                name: t.name,
                description: t.description,
                parameters_json: t.parameters,
                command: t.command,
                keybind: t.keybind,
                ui: ToolUiConfig::new(t.ui_fields),
            });
        }

        if tools.iter().any(|t| self.tool_index.contains_key(&t.name)) {
            tracing::debug!(key, "skipping plugin — tools already registered");
            return Ok(Vec::new());
        }

        let result = tools.clone();

        for tool in &tools {
            self.tool_index.insert(tool.name.clone(), key.clone());
        }

        self.plugins.insert(key, LoadedPlugin { tools, tool_exec });

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

        let exec_kind = loaded
            .tool_exec
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("no exec strategy for tool: {tool_name}"))?
            .clone();

        match exec_kind {
            ToolExecKind::Shell(template) => self.invoke_shell(tool_name, &template, args_json),
            ToolExecKind::Bin { path, args } => {
                self.invoke_binary(&path, &args, tool_name, args_json)
            }
        }
    }

    pub fn request_dynamic_ui(&self, tool_name: &str, args_json: &str) -> Option<Vec<UiField>> {
        let plugin_key = self.tool_index.get(tool_name)?;
        let loaded = self.plugins.get(plugin_key)?;
        let exec_kind = loaded.tool_exec.get(tool_name)?;

        let ToolExecKind::Bin { path, args } = exec_kind else {
            return None;
        };

        let mut cmd = Command::new(path);
        cmd.args(args.iter());
        cmd.args(["ui", tool_name, args_json]);
        cmd.current_dir(&self.project_dir);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let output = cmd.output().ok()?;
        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let fields: Vec<UiField> = serde_json::from_str(&stdout).ok()?;
        if fields.is_empty() {
            return None;
        }
        Some(fields)
    }

    fn invoke_shell(
        &self,
        tool_name: &str,
        template: &str,
        args_json: &str,
    ) -> anyhow::Result<ToolExecResult> {
        let args: serde_json::Value = serde_json::from_str(args_json)
            .unwrap_or(serde_json::Value::Object(Default::default()));

        let cmd = substitute_template(template, &args);

        let output = Command::new("sh")
            .args(["-c", &cmd])
            .current_dir(&self.project_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| anyhow::anyhow!("failed to run shell tool {tool_name}: {e}"))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            let msg = if stderr.is_empty() { &stdout } else { &stderr };
            return Ok(ToolExecResult {
                output: msg.to_string(),
                is_error: true,
                toast: String::new(),
                widget: String::new(),
            });
        }

        Ok(ToolExecResult {
            output: stdout,
            is_error: false,
            toast: String::new(),
            widget: String::new(),
        })
    }

    fn invoke_binary(
        &self,
        binary_path: &Path,
        static_args: &[String],
        tool_name: &str,
        args_json: &str,
    ) -> anyhow::Result<ToolExecResult> {
        let mut cmd = Command::new(binary_path);
        cmd.args(static_args);
        cmd.args(["invoke", tool_name, args_json]);
        cmd.current_dir(&self.project_dir);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let output = cmd.output().map_err(|e| {
            anyhow::anyhow!(
                "failed to invoke tool {tool_name} via {}: {e}",
                binary_path.display()
            )
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = if !stderr.is_empty() {
                stderr.to_string()
            } else if !stdout.is_empty() {
                stdout.to_string()
            } else {
                format!(
                    "process exited with status {} (binary: {})",
                    output.status,
                    binary_path.display()
                )
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

        let exec_kind = loaded.tool_exec.get(tool_name).cloned();

        self.active_tools.remove(tool_name);

        let binary_path = match &exec_kind {
            Some(ToolExecKind::Shell(_)) | None => return Ok(ToolExecResult::empty()),
            Some(ToolExecKind::Bin { path, .. }) => path.clone(),
        };

        let output = Command::new(&binary_path)
            .args(["exit", tool_name])
            .current_dir(&self.project_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .output()
            .map_err(|e| {
                anyhow::anyhow!(
                    "failed to exit tool {tool_name} via {}: {e}",
                    binary_path.display()
                )
            })?;

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
        let resolved = self.resolve_command_to_tool(tool_name).unwrap_or(tool_name);
        let resolved = resolved.to_string();
        if self.active_tools.contains(&resolved) {
            self.exit_tool(&resolved)
        } else {
            self.active_tools.insert(resolved.clone());
            self.invoke_tool(&resolved, args_json)
        }
    }

    fn resolve_command_to_tool<'a>(&'a self, command: &'a str) -> Option<&'a str> {
        if self.tool_index.contains_key(command) {
            return Some(command);
        }
        for loaded in self.plugins.values() {
            for tool in &loaded.tools {
                if tool.command == command {
                    return Some(&tool.name);
                }
            }
        }
        None
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

    pub fn tool_ui(&self, name_or_command: &str) -> Option<&ToolUiConfig> {
        let resolved = self
            .resolve_command_to_tool(name_or_command)
            .unwrap_or(name_or_command);
        let plugin_key = self.tool_index.get(resolved)?;
        let loaded = self.plugins.get(plugin_key)?;
        loaded
            .tools
            .iter()
            .find(|t| t.name == resolved)
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

    pub fn project_dir(&self) -> &Path {
        &self.project_dir
    }

    pub fn tool_exec_info(&self, tool_name: &str) -> anyhow::Result<(ToolExecKind, PathBuf)> {
        let plugin_key = self
            .tool_index
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {tool_name}"))?
            .clone();
        let loaded = self
            .plugins
            .get(&plugin_key)
            .ok_or_else(|| anyhow::anyhow!("plugin not found: {plugin_key}"))?;
        let exec_kind = loaded
            .tool_exec
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("no exec strategy for tool: {tool_name}"))?
            .clone();
        Ok((exec_kind, self.project_dir.clone()))
    }

    pub fn dynamic_ui_info(&self, tool_name: &str) -> Option<(PathBuf, Vec<String>, PathBuf)> {
        let plugin_key = self.tool_index.get(tool_name)?;
        let loaded = self.plugins.get(plugin_key)?;
        let exec_kind = loaded.tool_exec.get(tool_name)?;
        let ToolExecKind::Bin { path, args } = exec_kind else {
            return None;
        };
        Some((path.clone(), args.clone(), self.project_dir.clone()))
    }

    pub fn prepare_toggle(
        &mut self,
        tool_name: &str,
    ) -> anyhow::Result<(bool, String, ToolExecKind, PathBuf)> {
        let resolved = self
            .resolve_command_to_tool(tool_name)
            .unwrap_or(tool_name)
            .to_string();
        let is_exit = self.active_tools.contains(&resolved);
        if is_exit {
            self.active_tools.remove(&resolved);
        } else {
            self.active_tools.insert(resolved.clone());
        }
        let (exec_kind, project_dir) = self.tool_exec_info(&resolved)?;
        Ok((is_exit, resolved, exec_kind, project_dir))
    }

    pub fn prepare_exit(
        &mut self,
        tool_name: &str,
    ) -> anyhow::Result<(Option<ToolExecKind>, PathBuf)> {
        let plugin_key = self
            .tool_index
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {tool_name}"))?
            .clone();
        let loaded = self
            .plugins
            .get(&plugin_key)
            .ok_or_else(|| anyhow::anyhow!("plugin not found: {plugin_key}"))?;
        let exec_kind = loaded.tool_exec.get(tool_name).cloned();
        self.active_tools.remove(tool_name);
        Ok((exec_kind, self.project_dir.clone()))
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
        self.active_tools.clear();

        let mut added = Vec::new();
        let mut errors = Vec::new();
        let mut builds = Vec::new();

        let install_base = plugin_dirs
            .first()
            .cloned()
            .unwrap_or_else(|| self.project_dir.join(".phx/plugins"));

        if !skip_build {
            let workspace_release = self.project_dir.join("target/release");
            let deduped = dedup_paths(source_dirs);
            for dir in &deduped {
                tracing::info!(dir = %dir.display(), "building plugin");
                let build = build_plugin(dir);
                if build.success {
                    match install_built_plugin(&build, &install_base, Some(&workspace_release)) {
                        Ok(()) => {
                            tracing::info!(plugin = %build.name, "plugin installed");
                        }
                        Err(e) => {
                            tracing::error!(plugin = %build.name, error = %e, "install failed");
                            errors.push(format!("install {} failed: {e}", build.name));
                        }
                    }
                } else {
                    tracing::error!(
                        plugin = %build.name,
                        stderr = %build.stderr,
                        "build failed"
                    );
                    errors.push(format!("build {} failed: {}", build.name, build.stderr));
                }
                builds.push(build);
            }
        }

        for dir in Self::discover_manifest_only_dirs(Some(&self.project_dir)) {
            if let Err(e) = install_manifest_plugin(&dir, &install_base) {
                errors.push(format!("install manifest plugin failed: {e}"));
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
            let d = p.join(".phx/plugins");
            if d.is_dir() {
                dirs.push(d);
            }
        }

        let d = home.join(".phx/plugins");
        if d.is_dir() {
            dirs.push(d);
        }

        dirs
    }

    pub fn discover_source_dirs(project: Option<&Path>) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        if let Some(p) = project {
            for parent in ["plugins", "examples/plugins"] {
                let plugins_dir = p.join(parent);
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
        }

        dirs
    }

    pub fn discover_manifest_only_dirs(project: Option<&Path>) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        if let Some(p) = project {
            for parent in ["plugins", "examples/plugins"] {
                let plugins_dir = p.join(parent);
                if plugins_dir.is_dir()
                    && let Ok(entries) = std::fs::read_dir(&plugins_dir)
                {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir()
                            && path.join("manifest.json").exists()
                            && !path.join("Cargo.toml").exists()
                        {
                            dirs.push(path);
                        }
                    }
                }
            }
        }

        dirs
    }
}

// ---------------------------------------------------------------------------
// Async execution (lock-free) — call these after releasing the Mutex
// ---------------------------------------------------------------------------

pub async fn invoke_tool_async(
    exec_kind: ToolExecKind,
    tool_name: &str,
    args_json: &str,
    project_dir: &Path,
) -> anyhow::Result<ToolExecResult> {
    match exec_kind {
        ToolExecKind::Shell(template) => {
            invoke_shell_async(tool_name, &template, args_json, project_dir).await
        }
        ToolExecKind::Bin { path, args } => {
            invoke_binary_async(&path, &args, tool_name, args_json, project_dir).await
        }
    }
}

pub async fn invoke_shell_async(
    tool_name: &str,
    template: &str,
    args_json: &str,
    project_dir: &Path,
) -> anyhow::Result<ToolExecResult> {
    let args: serde_json::Value =
        serde_json::from_str(args_json).unwrap_or(serde_json::Value::Object(Default::default()));
    let cmd = substitute_template(template, &args);

    let output = tokio::process::Command::new("sh")
        .args(["-c", &cmd])
        .current_dir(project_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("failed to run shell tool {tool_name}: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        let msg = if stderr.is_empty() { &stdout } else { &stderr };
        return Ok(ToolExecResult {
            output: msg.to_string(),
            is_error: true,
            toast: String::new(),
            widget: String::new(),
        });
    }

    Ok(ToolExecResult {
        output: stdout,
        is_error: false,
        toast: String::new(),
        widget: String::new(),
    })
}

pub async fn invoke_binary_async(
    binary_path: &Path,
    static_args: &[String],
    tool_name: &str,
    args_json: &str,
    project_dir: &Path,
) -> anyhow::Result<ToolExecResult> {
    let mut cmd = tokio::process::Command::new(binary_path);
    cmd.args(static_args);
    cmd.args(["invoke", tool_name, args_json]);
    cmd.current_dir(project_dir);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.output().await.map_err(|e| {
        anyhow::anyhow!(
            "failed to invoke tool {tool_name} via {}: {e}",
            binary_path.display()
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            format!(
                "process exited with status {} (binary: {})",
                output.status,
                binary_path.display()
            )
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

pub async fn exit_tool_async(
    exec_kind: Option<ToolExecKind>,
    tool_name: &str,
    project_dir: &Path,
) -> anyhow::Result<ToolExecResult> {
    let binary_path = match &exec_kind {
        Some(ToolExecKind::Shell(_)) | None => return Ok(ToolExecResult::empty()),
        Some(ToolExecKind::Bin { path, .. }) => path.clone(),
    };

    let output = tokio::process::Command::new(&binary_path)
        .args(["exit", tool_name])
        .current_dir(project_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to exit tool {tool_name} via {}: {e}",
                binary_path.display()
            )
        })?;

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

pub async fn request_dynamic_ui_async(
    binary_path: &Path,
    static_args: &[String],
    tool_name: &str,
    args_json: &str,
    project_dir: &Path,
) -> Option<Vec<UiField>> {
    let mut cmd = tokio::process::Command::new(binary_path);
    cmd.args(static_args.iter());
    cmd.args(["ui", tool_name, args_json]);
    cmd.current_dir(project_dir);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let output = cmd.output().await.ok()?;
    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let fields: Vec<UiField> = serde_json::from_str(&stdout).ok()?;
    if fields.is_empty() {
        return None;
    }
    Some(fields)
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

fn resolve_bin_path(bin: &str, plugin_dir: &Path, project_dir: &Path) -> PathBuf {
    let path = Path::new(bin);
    if path.is_absolute() {
        path.to_path_buf()
    } else if bin.starts_with("./") {
        let stripped = bin.strip_prefix("./").unwrap_or(bin);
        plugin_dir.join(stripped)
    } else {
        project_dir.join(bin)
    }
}

fn substitute_template(template: &str, args: &serde_json::Value) -> String {
    let mut result = template.to_string();
    if let serde_json::Value::Object(map) = args {
        for (key, val) in map {
            let placeholder = format!("{{{{{key}}}}}");
            let replacement = match val {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            result = result.replace(&placeholder, &replacement);
        }
    }
    result
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

    let short_name = build
        .package_name
        .strip_prefix("phx-plugin-")
        .or_else(|| build.package_name.strip_prefix("phx_plugin_"))
        .unwrap_or(&build.package_name);

    let install_dir = install_base.join(short_name);

    let src_size = std::fs::metadata(&binary).map(|m| m.len()).unwrap_or(0);
    tracing::info!(
        plugin = %build.package_name,
        binary = %binary.display(),
        src_size,
        install_dir = %install_dir.display(),
        "running install command"
    );

    let output = Command::new(&binary)
        .args(["install", &install_dir.to_string_lossy()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run install for {}: {e}", build.package_name))?;

    let stderr_text = String::from_utf8_lossy(&output.stderr);
    if !stderr_text.is_empty() {
        tracing::debug!(
            plugin = %build.package_name,
            stderr = %stderr_text,
            "install stderr"
        );
    }

    if !output.status.success() {
        anyhow::bail!("install for {} failed: {}", build.package_name, stderr_text);
    }

    let manifest_dest = install_dir.join("manifest.json");
    if !manifest_dest.is_file() {
        anyhow::bail!(
            "install for {} produced no manifest.json at {}",
            build.package_name,
            manifest_dest.display()
        );
    }

    let bin_name_installed = format!("{}{}", build.package_name, std::env::consts::EXE_SUFFIX);
    let installed_binary = install_dir.join(&bin_name_installed);
    if installed_binary.is_file() {
        let dest_size = std::fs::metadata(&installed_binary)
            .map(|m| m.len())
            .unwrap_or(0);
        tracing::info!(
            plugin = %build.package_name,
            src_size,
            dest_size,
            path = %installed_binary.display(),
            "binary installed"
        );
        if dest_size != src_size {
            tracing::warn!(
                plugin = %build.package_name,
                src_size,
                dest_size,
                "binary size mismatch after install"
            );
        }
    } else {
        let has_any_binary = std::fs::read_dir(&install_dir)
            .ok()
            .map(|entries| {
                entries.flatten().any(|e| {
                    let p = e.path();
                    p.is_file() && p.file_name().is_some_and(|n| n != "manifest.json")
                })
            })
            .unwrap_or(false);

        if !has_any_binary {
            anyhow::bail!(
                "install for {} produced no binary in {}",
                build.package_name,
                install_dir.display()
            );
        }
    }

    tracing::info!(
        plugin = %build.package_name,
        dir = %install_dir.display(),
        "plugin installed"
    );

    Ok(())
}

fn install_manifest_plugin(source_dir: &Path, install_base: &Path) -> anyhow::Result<()> {
    let dir_name = source_dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("invalid plugin source dir"))?
        .to_string_lossy();

    let install_dir = install_base.join(&*dir_name);
    std::fs::create_dir_all(&install_dir)?;

    let entries = std::fs::read_dir(source_dir).map_err(|e| {
        anyhow::anyhow!(
            "failed to read plugin source dir {}: {e}",
            source_dir.display()
        )
    })?;

    let mut copied = 0u32;
    for entry in entries {
        let entry = entry.map_err(|e| {
            anyhow::anyhow!("failed to read entry in {}: {e}", source_dir.display())
        })?;
        let src = entry.path();
        let dest = install_dir.join(entry.file_name());
        if src.is_file() {
            let bytes = std::fs::copy(&src, &dest).map_err(|e| {
                anyhow::anyhow!("failed to copy {} → {}: {e}", src.display(), dest.display())
            })?;
            tracing::debug!(
                src = %src.display(),
                dest = %dest.display(),
                bytes,
                "copied plugin file"
            );
            copied += 1;
        }
    }

    if copied == 0 {
        anyhow::bail!(
            "no files found in plugin source dir {}",
            source_dir.display()
        );
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

#[cfg(all(test, not(miri)))]
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
        let plugin_dir = project.path().join(".phx/plugins");
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

        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{"name":"test","bin":"./nonexistent","tools":[{"name":"t","description":"d"}]}"#,
        )
        .unwrap();

        let mut rt = PluginRuntime::new(PathBuf::from("."));
        let result = rt.load_plugin_dir(&plugin_dir);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bin not found"));
    }

    #[test]
    fn load_shell_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("echo-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{"name":"echo","tools":[{"name":"echo_tool","description":"echoes","shell":"echo hello"}]}"#,
        )
        .unwrap();

        let mut rt = PluginRuntime::new(PathBuf::from("."));
        let tools = rt.load_plugin_dir(&plugin_dir).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "echo_tool");
    }

    #[test]
    fn invoke_shell_tool() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("echo-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{"name":"echo","tools":[{"name":"echo_tool","description":"echoes","shell":"echo hello"}]}"#,
        )
        .unwrap();

        let mut rt = PluginRuntime::new(PathBuf::from("."));
        rt.load_plugin_dir(&plugin_dir).unwrap();
        let result = rt.invoke_tool("echo_tool", "{}").unwrap();
        assert!(!result.is_error);
        assert_eq!(result.output.trim(), "hello");
    }

    #[test]
    fn invoke_shell_tool_with_template() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("greet-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{"name":"greet","tools":[{"name":"greet_tool","description":"greets","shell":"echo hello {{name}}"}]}"#,
        )
        .unwrap();

        let mut rt = PluginRuntime::new(PathBuf::from("."));
        rt.load_plugin_dir(&plugin_dir).unwrap();
        let result = rt.invoke_tool("greet_tool", r#"{"name":"world"}"#).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.output.trim(), "hello world");
    }

    #[test]
    fn shell_tool_reports_failure() {
        let dir = tempfile::tempdir().unwrap();
        let plugin_dir = dir.path().join("fail-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        std::fs::write(
            plugin_dir.join("manifest.json"),
            r#"{"name":"fail","tools":[{"name":"fail_tool","description":"fails","shell":"exit 1"}]}"#,
        )
        .unwrap();

        let mut rt = PluginRuntime::new(PathBuf::from("."));
        rt.load_plugin_dir(&plugin_dir).unwrap();
        let result = rt.invoke_tool("fail_tool", "{}").unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn substitute_template_replaces_placeholders() {
        let args = serde_json::json!({"name": "world", "count": 5});
        assert_eq!(
            substitute_template("hello {{name}} {{count}}", &args),
            "hello world 5"
        );
    }

    #[test]
    fn substitute_template_missing_key_left_as_is() {
        let args = serde_json::json!({"name": "world"});
        assert_eq!(
            substitute_template("hello {{name}} {{missing}}", &args),
            "hello world {{missing}}"
        );
    }

    #[test]
    fn resolve_bin_path_absolute() {
        assert_eq!(
            resolve_bin_path("/usr/bin/foo", Path::new("/p"), Path::new("/proj")),
            PathBuf::from("/usr/bin/foo")
        );
    }

    #[test]
    fn resolve_bin_path_dot_relative() {
        assert_eq!(
            resolve_bin_path("./my-bin", Path::new("/plugins/test"), Path::new("/proj")),
            PathBuf::from("/plugins/test/my-bin")
        );
    }

    #[test]
    fn resolve_bin_path_project_relative() {
        assert_eq!(
            resolve_bin_path(
                "target/release/foo",
                Path::new("/plugins/test"),
                Path::new("/proj")
            ),
            PathBuf::from("/proj/target/release/foo")
        );
    }
}

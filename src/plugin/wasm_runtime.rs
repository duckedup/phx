use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use wasmtime::component::{Component, Linker, ResourceTable, bindgen};
use wasmtime::{Engine, Store, bail};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

bindgen!({
    path: "src/wit/plugin.wit",
    world: "tool-plugin",
});

struct PluginState {
    wasi: WasiCtx,
    table: ResourceTable,
    project_dir: PathBuf,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

pub struct BuildOutput {
    pub name: String,
    pub success: bool,
    pub stderr: String,
    pub wasm_dir: Option<PathBuf>,
}

pub struct ReloadResult {
    pub builds: Vec<BuildOutput>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub errors: Vec<String>,
}

pub struct ToolExecResult {
    pub output: String,
    pub is_error: bool,
    pub toast: String,
    pub widget: String,
}

use phoenix_shared::ui_field_types::{
    ToolUiConfig, UiField as SharedUiField, UiFieldKind as SharedUiFieldKind,
};

#[derive(Clone)]
pub struct UnifiedToolMeta {
    pub name: String,
    pub description: String,
    pub parameters_json: String,
    pub command: String,
    pub keybind: String,
    pub ui: ToolUiConfig,
}

struct LoadedPlugin {
    plugin: ToolPlugin,
    store: Store<PluginState>,
    tools: Vec<UnifiedToolMeta>,
}

pub struct WasmRuntime {
    engine: Arc<Engine>,
    linker: Linker<PluginState>,
    plugins: HashMap<String, LoadedPlugin>,
    tool_index: HashMap<String, String>,
    active_tools: std::collections::HashSet<String>,
    project_dir: PathBuf,
}

static SHARED_ENGINE: std::sync::LazyLock<Arc<Engine>> =
    std::sync::LazyLock::new(|| Arc::new(Engine::default()));

impl WasmRuntime {
    pub fn new_with_project(project: PathBuf) -> wasmtime::Result<Self> {
        let engine = Arc::clone(&SHARED_ENGINE);
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        linker.root().func_wrap(
            "run-command",
            |caller: wasmtime::StoreContextMut<'_, PluginState>,
             (program, args): (String, Vec<String>)| {
                let project_dir = caller.data().project_dir.clone();
                let result = Command::new(&program)
                    .args(&args)
                    .current_dir(&project_dir)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .output();

                match result {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                        let exit_code = output.status.code().unwrap_or(-1);
                        Ok((Ok::<_, String>(CommandOutput {
                            stdout,
                            stderr,
                            exit_code,
                        }),))
                    }
                    Err(e) => Ok((Err(format!("failed to run {program}: {e}")),)),
                }
            },
        )?;
        Ok(Self {
            engine,
            linker,
            plugins: HashMap::new(),
            tool_index: HashMap::new(),
            active_tools: std::collections::HashSet::new(),
            project_dir: project,
        })
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> wasmtime::Result<Vec<UnifiedToolMeta>> {
        let mut loaded = Vec::new();

        if !dir.is_dir() {
            return Ok(loaded);
        }

        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().and_then(OsStr::to_str) == Some("wasm")
                && let Ok(tools) = self.load_plugin(&path)
            {
                for t in &tools {
                    tracing::info!("loaded WASM tool: {}", t.name);
                }
                loaded.extend(tools);
            }
        }

        Ok(loaded)
    }

    pub fn load_plugin(&mut self, path: &Path) -> wasmtime::Result<Vec<UnifiedToolMeta>> {
        let component = Component::from_file(&self.engine, path)?;
        let key = path.to_string_lossy();
        self.load_plugin_component(component, &key)
    }

    pub fn load_from_bytes(
        &mut self,
        bytes: &[u8],
        key: &str,
    ) -> wasmtime::Result<Vec<UnifiedToolMeta>> {
        let component = Component::from_binary(&self.engine, bytes)?;
        self.load_plugin_component(component, key)
    }

    fn load_plugin_component(
        &mut self,
        component: Component,
        key: &str,
    ) -> wasmtime::Result<Vec<UnifiedToolMeta>> {
        let wasi = WasiCtxBuilder::new().build();
        let state = PluginState {
            project_dir: self.project_dir.clone(),
            wasi,
            table: ResourceTable::new(),
        };
        let mut store = Store::new(&self.engine, state);
        let plugin = ToolPlugin::instantiate(&mut store, &component, &self.linker)?;

        let raw_tools = plugin.call_get_tool_metadata(&mut store)?;
        if raw_tools.is_empty() {
            bail!("tool plugin exports no tools");
        }

        let tools: Vec<UnifiedToolMeta> = raw_tools
            .into_iter()
            .map(|t| {
                let ui_fields: Vec<SharedUiField> = t
                    .ui_fields
                    .into_iter()
                    .map(|f| {
                        let kind = match f.field_kind {
                            UiFieldKind::TextInput => SharedUiFieldKind::TextInput,
                            UiFieldKind::TextArea => SharedUiFieldKind::TextArea,
                            UiFieldKind::Toggle => SharedUiFieldKind::Toggle,
                        };
                        SharedUiField {
                            key: f.key,
                            label: f.label,
                            field: kind,
                            required: f.required,
                            placeholder: f.placeholder,
                            default_value: String::new(),
                        }
                    })
                    .collect();
                UnifiedToolMeta {
                    name: t.name,
                    description: t.description,
                    parameters_json: t.parameters_json,
                    command: t.command,
                    keybind: t.keybind,
                    ui: ToolUiConfig::new(ui_fields),
                }
            })
            .collect();

        if tools.iter().any(|t| self.tool_index.contains_key(&t.name)) {
            tracing::debug!(key, "skipping plugin — tools already registered");
            return Ok(Vec::new());
        }

        let result = tools.clone();

        for tool in &tools {
            self.tool_index.insert(tool.name.clone(), key.to_string());
        }

        self.plugins.insert(
            key.to_string(),
            LoadedPlugin {
                plugin,
                store,
                tools,
            },
        );

        Ok(result)
    }

    pub fn invoke_tool(
        &mut self,
        tool_name: &str,
        args_json: &str,
    ) -> wasmtime::Result<ToolExecResult> {
        let plugin_key = self
            .tool_index
            .get(tool_name)
            .ok_or_else(|| wasmtime::Error::msg(format!("unknown tool: {tool_name}")))?
            .clone();

        let loaded = self
            .plugins
            .get_mut(&plugin_key)
            .ok_or_else(|| wasmtime::Error::msg(format!("plugin not found: {plugin_key}")))?;

        let result = loaded
            .plugin
            .call_invoke_tool(&mut loaded.store, tool_name, args_json)?;

        match result {
            Ok(tr) => Ok(ToolExecResult {
                output: tr.output,
                is_error: tr.is_error,
                toast: tr.toast,
                widget: tr.widget,
            }),
            Err(e) => Err(wasmtime::Error::msg(format!("tool error: {e}"))),
        }
    }

    pub fn exit_tool(&mut self, tool_name: &str) -> wasmtime::Result<ToolExecResult> {
        let plugin_key = self
            .tool_index
            .get(tool_name)
            .ok_or_else(|| wasmtime::Error::msg(format!("unknown tool: {tool_name}")))?
            .clone();

        let loaded = self
            .plugins
            .get_mut(&plugin_key)
            .ok_or_else(|| wasmtime::Error::msg(format!("plugin not found: {plugin_key}")))?;

        let result = loaded
            .plugin
            .call_on_exit_tool(&mut loaded.store, tool_name)?;

        self.active_tools.remove(tool_name);

        match result {
            Ok(tr) => Ok(ToolExecResult {
                output: tr.output,
                is_error: tr.is_error,
                toast: tr.toast,
                widget: tr.widget,
            }),
            Err(e) => Err(wasmtime::Error::msg(format!("tool exit error: {e}"))),
        }
    }

    pub fn toggle_tool(
        &mut self,
        tool_name: &str,
        args_json: &str,
    ) -> wasmtime::Result<ToolExecResult> {
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
        let mut loaded = Vec::new();

        let bundled: &[(&str, &[u8])] = &[(
            "conductor",
            include_bytes!("../../bundled/phoenix_plugin_conductor.wasm"),
        )];

        for (name, bytes) in bundled {
            match self.load_from_bytes(bytes, name) {
                Ok(tools) => {
                    for tool in &tools {
                        tracing::info!(plugin = name, tool = %tool.name, "loaded bundled tool");
                    }
                    loaded.extend(tools);
                }
                Err(e) => {
                    tracing::warn!(plugin = name, error = %e, "failed to load bundled plugin");
                }
            }
        }

        loaded
    }

    pub fn reload(&mut self, wasm_dirs: &[PathBuf], source_dirs: &[PathBuf]) -> ReloadResult {
        self.reload_inner(wasm_dirs, source_dirs, false)
    }

    pub fn reload_skip_build(
        &mut self,
        wasm_dirs: &[PathBuf],
        source_dirs: &[PathBuf],
    ) -> ReloadResult {
        self.reload_inner(wasm_dirs, source_dirs, true)
    }

    fn reload_inner(
        &mut self,
        wasm_dirs: &[PathBuf],
        source_dirs: &[PathBuf],
        skip_build: bool,
    ) -> ReloadResult {
        let old_tools: Vec<String> = self.tool_index.keys().cloned().collect();
        self.plugins.clear();
        self.tool_index.clear();

        let mut added = Vec::new();
        let mut errors = Vec::new();
        let mut builds = Vec::new();
        let mut built_dirs = Vec::new();

        if !skip_build {
            let deduped = dedup_paths(source_dirs);
            for dir in &deduped {
                let build = build_plugin(dir);
                if let Some(ref wasm_dir) = build.wasm_dir {
                    built_dirs.push(wasm_dir.clone());
                }
                if !build.success {
                    let name = &build.name;
                    errors.push(format!("build {name} failed"));
                }
                builds.push(build);
            }
        } else {
            for dir in source_dirs {
                let release_dir = dir.join("target/wasm32-wasip2/release");
                if release_dir.is_dir() {
                    built_dirs.push(release_dir);
                }
            }
        }

        let all_dirs: Vec<&Path> = wasm_dirs
            .iter()
            .map(|p| p.as_path())
            .chain(built_dirs.iter().map(|p| p.as_path()))
            .collect();

        for dir in all_dirs {
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
            let d = p.join(".phoenix/wasm-plugins");
            if d.is_dir() {
                dirs.push(d);
            }
        }

        let d = home.join(".phoenix/wasm-plugins");
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

fn dedup_paths(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for d in dirs {
        let canonical = d.canonicalize().unwrap_or_else(|_| d.clone());
        if seen.insert(canonical) {
            out.push(d.clone());
        }
    }
    out
}

fn build_plugin(source_dir: &Path) -> BuildOutput {
    use std::process::Stdio;

    let name = source_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let result = Command::new("cargo")
        .args([
            "build",
            "--target",
            "wasm32-wasip2",
            "--release",
            "--color=never",
        ])
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
                success: false,
                stderr: format!("failed to run cargo: {e}"),
                wasm_dir: None,
            };
        }
    };

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return BuildOutput {
            name,
            success: false,
            stderr,
            wasm_dir: None,
        };
    }

    let release_dir = source_dir.join("target/wasm32-wasip2/release");
    let wasm_dir = if release_dir.is_dir() {
        Some(release_dir)
    } else {
        None
    };

    BuildOutput {
        name,
        success: true,
        stderr,
        wasm_dir,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_dirs_empty_when_no_dirs_exist() {
        let dirs = WasmRuntime::discover_dirs(None, Path::new("/nonexistent"));
        assert!(dirs.is_empty());
    }

    #[test]
    fn discover_dirs_finds_project_dir() {
        let project = tempfile::tempdir().unwrap();
        let plugin_dir = project.path().join(".phoenix/wasm-plugins");
        std::fs::create_dir_all(&plugin_dir).unwrap();

        let dirs = WasmRuntime::discover_dirs(Some(project.path()), Path::new("/nonexistent"));
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0], plugin_dir);
    }
}

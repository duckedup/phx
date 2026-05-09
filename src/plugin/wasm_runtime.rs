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
    world: "skill-plugin",
});

mod tool_bindings {
    use wasmtime::component::bindgen;
    bindgen!({
        path: "src/wit/plugin.wit",
        world: "tool-plugin",
    });
}

mod hookable_bindings {
    use wasmtime::component::bindgen;
    bindgen!({
        path: "src/wit/plugin.wit",
        world: "hookable-skill-plugin",
    });
}

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

pub struct SkillExecResult {
    pub context: String,
    pub toast: String,
    pub widget: String,
}

pub struct WasmSkillMeta {
    pub name: String,
    pub command: String,
    pub description: String,
    pub keybind: String,
    pub is_tool: bool,
}

pub struct WasmToolMeta {
    pub name: String,
    pub description: String,
    pub parameters_json: String,
}

enum SkillVariant {
    Basic(SkillPlugin),
    Hookable(hookable_bindings::HookableSkillPlugin),
}

struct LoadedSkill {
    variant: SkillVariant,
    store: Store<PluginState>,
    meta: WasmSkillMeta,
    has_hooks: bool,
}

struct LoadedTool {
    plugin: tool_bindings::ToolPlugin,
    store: Store<PluginState>,
    tools: Vec<WasmToolMeta>,
}

pub struct WasmRuntime {
    engine: Arc<Engine>,
    linker: Linker<PluginState>,
    skills: HashMap<String, LoadedSkill>,
    tool_plugins: HashMap<String, LoadedTool>,
    active_skills: std::collections::HashSet<String>,
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
            skills: HashMap::new(),
            tool_plugins: HashMap::new(),
            active_skills: std::collections::HashSet::new(),
            project_dir: project,
        })
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> wasmtime::Result<Vec<WasmSkillMeta>> {
        let mut loaded = Vec::new();

        if !dir.is_dir() {
            return Ok(loaded);
        }

        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(OsStr::to_str) == Some("wasm") {
                // Try skill-plugin first, then tool-plugin
                if let Ok(meta) = self.load_skill_plugin(&path) {
                    loaded.push(meta);
                } else if let Ok(tools) = self.load_tool_plugin(&path) {
                    for t in &tools {
                        tracing::info!("loaded WASM tool: {}", t.name);
                    }
                }
            }
        }

        Ok(loaded)
    }

    fn load_skill_plugin(&mut self, path: &Path) -> wasmtime::Result<WasmSkillMeta> {
        let component = Component::from_file(&self.engine, path)?;
        self.load_skill_component(component)
    }

    pub fn load_from_bytes(&mut self, bytes: &[u8]) -> wasmtime::Result<WasmSkillMeta> {
        let component = Component::from_binary(&self.engine, bytes)?;
        self.load_skill_component(component)
    }

    fn load_skill_component(&mut self, component: Component) -> wasmtime::Result<WasmSkillMeta> {
        // Try hookable variant first
        if let Ok(ret) = self.try_load_hookable_skill(&component) {
            return Ok(ret);
        }

        // Fall back to basic skill-plugin
        let wasi = WasiCtxBuilder::new().build();
        let state = PluginState {
            project_dir: self.project_dir.clone(),
            wasi,
            table: ResourceTable::new(),
        };
        let mut store = Store::new(&self.engine, state);
        let plugin = SkillPlugin::instantiate(&mut store, &component, &self.linker)?;
        let metadata = plugin.call_get_metadata(&mut store)?;

        if metadata.name.is_empty() {
            bail!("plugin metadata 'name' is empty");
        }
        if metadata.command.is_empty() {
            bail!("plugin metadata 'command' is empty");
        }

        let meta = WasmSkillMeta {
            name: metadata.name.clone(),
            command: metadata.command.clone(),
            description: metadata.description.clone(),
            keybind: metadata.keybind.clone(),
            is_tool: metadata.is_tool,
        };
        let ret = WasmSkillMeta {
            name: metadata.name.clone(),
            command: metadata.command.clone(),
            description: metadata.description,
            keybind: metadata.keybind,
            is_tool: metadata.is_tool,
        };

        self.skills.insert(
            meta.command.clone(),
            LoadedSkill {
                variant: SkillVariant::Basic(plugin),
                store,
                meta,
                has_hooks: false,
            },
        );

        Ok(ret)
    }

    fn try_load_hookable_skill(
        &mut self,
        component: &Component,
    ) -> wasmtime::Result<WasmSkillMeta> {
        let wasi = WasiCtxBuilder::new().build();
        let state = PluginState {
            project_dir: self.project_dir.clone(),
            wasi,
            table: ResourceTable::new(),
        };
        let mut store = Store::new(&self.engine, state);
        let plugin = hookable_bindings::HookableSkillPlugin::instantiate(
            &mut store,
            component,
            &self.linker,
        )?;
        let metadata = plugin.call_get_metadata(&mut store)?;

        if metadata.name.is_empty() {
            bail!("plugin metadata 'name' is empty");
        }
        if metadata.command.is_empty() {
            bail!("plugin metadata 'command' is empty");
        }

        let meta = WasmSkillMeta {
            name: metadata.name.clone(),
            command: metadata.command.clone(),
            description: metadata.description.clone(),
            keybind: metadata.keybind.clone(),
            is_tool: metadata.is_tool,
        };
        let ret = WasmSkillMeta {
            name: metadata.name.clone(),
            command: metadata.command.clone(),
            description: metadata.description,
            keybind: metadata.keybind,
            is_tool: metadata.is_tool,
        };

        tracing::info!("loaded hookable WASM skill: {}", meta.name);

        self.skills.insert(
            meta.command.clone(),
            LoadedSkill {
                variant: SkillVariant::Hookable(plugin),
                store,
                meta,
                has_hooks: true,
            },
        );

        Ok(ret)
    }

    fn load_tool_plugin(&mut self, path: &Path) -> wasmtime::Result<Vec<WasmToolMeta>> {
        let component = Component::from_file(&self.engine, path)?;
        let wasi = WasiCtxBuilder::new().build();
        let state = PluginState {
            project_dir: self.project_dir.clone(),
            wasi,
            table: ResourceTable::new(),
        };
        let mut store = Store::new(&self.engine, state);
        let plugin = tool_bindings::ToolPlugin::instantiate(&mut store, &component, &self.linker)?;

        let raw_tools = plugin.call_get_tool_metadata(&mut store)?;
        if raw_tools.is_empty() {
            bail!("tool plugin exports no tools");
        }

        let tools: Vec<WasmToolMeta> = raw_tools
            .into_iter()
            .map(|t| WasmToolMeta {
                name: t.name,
                description: t.description,
                parameters_json: t.parameters_json,
            })
            .collect();

        let key = path.to_string_lossy().to_string();
        self.tool_plugins.insert(
            key,
            LoadedTool {
                plugin,
                store,
                tools: tools
                    .iter()
                    .map(|t| WasmToolMeta {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters_json: t.parameters_json.clone(),
                    })
                    .collect(),
            },
        );

        Ok(tools)
    }

    pub fn invoke_wasm_tool(
        &mut self,
        plugin_key: &str,
        tool_name: &str,
        args_json: &str,
    ) -> wasmtime::Result<(String, bool)> {
        let loaded = self
            .tool_plugins
            .get_mut(plugin_key)
            .ok_or_else(|| wasmtime::Error::msg(format!("unknown tool plugin: {plugin_key}")))?;

        let result = loaded
            .plugin
            .call_invoke_tool(&mut loaded.store, tool_name, args_json)?;

        match result {
            Ok(tr) => Ok((tr.output, tr.is_error)),
            Err(e) => Err(wasmtime::Error::msg(format!("tool plugin error: {e}"))),
        }
    }

    pub fn tool_plugin_schemas(&self) -> Vec<(String, WasmToolMeta)> {
        let mut out = Vec::new();
        for (key, loaded) in &self.tool_plugins {
            for t in &loaded.tools {
                out.push((
                    key.clone(),
                    WasmToolMeta {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters_json: t.parameters_json.clone(),
                    },
                ));
            }
        }
        out
    }

    pub fn load_bundled(&mut self) -> Vec<WasmSkillMeta> {
        let mut loaded = Vec::new();

        let bundled: &[(&str, &[u8])] = &[(
            "conductor",
            include_bytes!("../../bundled/phoenix_plugin_conductor.wasm"),
        )];

        for (name, bytes) in bundled {
            match self.load_from_bytes(bytes) {
                Ok(meta) => {
                    tracing::info!(plugin = name, command = %meta.command, "loaded bundled plugin");
                    loaded.push(meta);
                }
                Err(e) => {
                    tracing::warn!(plugin = name, error = %e, "failed to load bundled plugin");
                }
            }
        }

        loaded
    }

    pub fn execute(&mut self, command: &str, arguments: &str) -> wasmtime::Result<SkillExecResult> {
        let skill = self
            .skills
            .get_mut(command)
            .ok_or_else(|| wasmtime::Error::msg(format!("unknown wasm skill: {command}")))?;

        let result = match &mut skill.variant {
            SkillVariant::Basic(p) => p.call_execute(&mut skill.store, arguments)?,
            SkillVariant::Hookable(p) => p
                .call_execute(&mut skill.store, arguments)?
                .map(|sr| SkillResult {
                    context: sr.context,
                    toast: sr.toast,
                    widget: sr.widget,
                })
                .map_err(|e| e.to_string()),
        };

        match result {
            Ok(sr) => Ok(SkillExecResult {
                context: sr.context,
                toast: sr.toast,
                widget: sr.widget,
            }),
            Err(e) => Err(wasmtime::Error::msg(format!("plugin error: {e}"))),
        }
    }

    pub fn execute_exit(&mut self, command: &str) -> wasmtime::Result<SkillExecResult> {
        let skill = self
            .skills
            .get_mut(command)
            .ok_or_else(|| wasmtime::Error::msg(format!("unknown wasm skill: {command}")))?;

        let result = match &mut skill.variant {
            SkillVariant::Basic(p) => p.call_on_exit(&mut skill.store)?,
            SkillVariant::Hookable(p) => p
                .call_on_exit(&mut skill.store)?
                .map(|sr| SkillResult {
                    context: sr.context,
                    toast: sr.toast,
                    widget: sr.widget,
                })
                .map_err(|e| e.to_string()),
        };

        self.active_skills.remove(command);

        match result {
            Ok(sr) => Ok(SkillExecResult {
                context: sr.context,
                toast: sr.toast,
                widget: sr.widget,
            }),
            Err(e) => Err(wasmtime::Error::msg(format!("plugin error: {e}"))),
        }
    }

    pub fn invoke_hook(
        &mut self,
        command: &str,
        event: &str,
        data: &str,
    ) -> wasmtime::Result<Option<String>> {
        let skill = self
            .skills
            .get_mut(command)
            .ok_or_else(|| wasmtime::Error::msg(format!("unknown wasm skill: {command}")))?;

        match &mut skill.variant {
            SkillVariant::Basic(_) => Ok(None),
            SkillVariant::Hookable(p) => {
                let result = p.call_on_hook(&mut skill.store, event, data)?;
                match result {
                    Ok(hr) => {
                        let json = serde_json::json!({
                            "action": hr.action,
                            "reason": hr.reason,
                            "data": hr.data,
                        });
                        Ok(Some(json.to_string()))
                    }
                    Err(e) => Err(wasmtime::Error::msg(format!("hook error: {e}"))),
                }
            }
        }
    }

    pub fn hookable_commands(&self) -> Vec<&str> {
        self.skills
            .values()
            .filter(|s| s.has_hooks)
            .map(|s| s.meta.command.as_str())
            .collect()
    }

    pub fn toggle(&mut self, command: &str, arguments: &str) -> wasmtime::Result<SkillExecResult> {
        if self.active_skills.contains(command) {
            self.execute_exit(command)
        } else {
            self.active_skills.insert(command.to_string());
            self.execute(command, arguments)
        }
    }

    pub fn is_active(&self, command: &str) -> bool {
        self.active_skills.contains(command)
    }

    pub fn command_for_keybind(&self, keybind: &str) -> Option<&str> {
        self.skills
            .values()
            .find(|s| !s.meta.keybind.is_empty() && s.meta.keybind == keybind)
            .map(|s| s.meta.command.as_str())
    }

    pub fn has_command(&self, command: &str) -> bool {
        self.skills.contains_key(command)
    }

    pub fn commands(&self) -> Vec<(&str, &str)> {
        self.skills
            .values()
            .map(|s| (s.meta.command.as_str(), s.meta.description.as_str()))
            .collect()
    }

    pub fn tool_skill_commands(&self) -> Vec<(&str, &str)> {
        self.skills
            .values()
            .filter(|s| s.meta.is_tool)
            .map(|s| (s.meta.command.as_str(), s.meta.description.as_str()))
            .collect()
    }

    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }

    pub fn tool_count(&self) -> usize {
        self.tool_plugins.values().map(|t| t.tools.len()).sum()
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
        let old_commands: Vec<String> = self.skills.keys().cloned().collect();
        self.skills.clear();
        self.tool_plugins.clear();

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
                Ok(metas) => {
                    for m in metas {
                        added.push(m.command.clone());
                    }
                }
                Err(e) => {
                    errors.push(format!("{}: {e}", dir.display()));
                }
            }
        }

        let new_commands: Vec<String> = self.skills.keys().cloned().collect();
        let removed: Vec<String> = old_commands
            .iter()
            .filter(|c| !new_commands.contains(c))
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

// Wasmtime Store teardown races with the multi-threaded test harness,
// causing SIGABRT. Tests that create WasmRuntime live in
// tests/wasm_runtime_tests.rs (isolated binary).
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

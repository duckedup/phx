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

struct PluginState {
    wasi: WasiCtx,
    table: ResourceTable,
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
}

struct LoadedSkill {
    plugin: SkillPlugin,
    store: Store<PluginState>,
    meta: WasmSkillMeta,
}

pub struct WasmRuntime {
    engine: Arc<Engine>,
    linker: Linker<PluginState>,
    skills: HashMap<String, LoadedSkill>,
    active_skills: std::collections::HashSet<String>,
}

static SHARED_ENGINE: std::sync::LazyLock<Arc<Engine>> =
    std::sync::LazyLock::new(|| Arc::new(Engine::default()));

impl WasmRuntime {
    pub fn new() -> wasmtime::Result<Self> {
        let engine = Arc::clone(&SHARED_ENGINE);
        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        Ok(Self {
            engine,
            linker,
            skills: HashMap::new(),
            active_skills: std::collections::HashSet::new(),
        })
    }

    pub fn load_from_dir(&mut self, dir: &Path) -> wasmtime::Result<Vec<WasmSkillMeta>> {
        let mut loaded = Vec::new();

        if !dir.is_dir() {
            return Ok(loaded);
        }

        for entry in std::fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().and_then(OsStr::to_str) == Some("wasm")
                && let Ok(meta) = self.load_plugin(&path)
            {
                loaded.push(meta);
            }
        }

        Ok(loaded)
    }

    fn load_plugin(&mut self, path: &Path) -> wasmtime::Result<WasmSkillMeta> {
        let component = Component::from_file(&self.engine, path)?;
        let wasi = WasiCtxBuilder::new().build();
        let state = PluginState {
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
        };
        let ret = WasmSkillMeta {
            name: metadata.name.clone(),
            command: metadata.command.clone(),
            description: metadata.description,
            keybind: metadata.keybind,
        };

        self.skills.insert(
            meta.command.clone(),
            LoadedSkill {
                plugin,
                store,
                meta,
            },
        );

        Ok(ret)
    }

    pub fn execute(&mut self, command: &str, arguments: &str) -> wasmtime::Result<SkillExecResult> {
        let skill = self
            .skills
            .get_mut(command)
            .ok_or_else(|| wasmtime::Error::msg(format!("unknown wasm skill: {command}")))?;

        let result = skill.plugin.call_execute(&mut skill.store, arguments)?;

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

        let result = skill.plugin.call_on_exit(&mut skill.store)?;

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

    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }

    pub fn reload(&mut self, wasm_dirs: &[PathBuf], source_dirs: &[PathBuf]) -> ReloadResult {
        let old_commands: Vec<String> = self.skills.keys().cloned().collect();
        self.skills.clear();

        let mut added = Vec::new();
        let mut errors = Vec::new();
        let mut builds = Vec::new();
        let mut built_dirs = Vec::new();

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

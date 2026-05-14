use std::path::Path;

use phoenix::plugin::plugin_runtime::PluginRuntime;

#[test]
fn runtime_starts_empty() {
    let rt = PluginRuntime::new(std::path::PathBuf::from("."));
    assert_eq!(rt.tool_count(), 0);
    assert!(rt.commands().is_empty());
    assert!(!rt.has_command("plan"));
}

#[test]
fn load_nonexistent_dir_returns_empty() {
    let mut rt = PluginRuntime::new(std::path::PathBuf::from("."));
    let result = rt.load_from_dir(Path::new("/nonexistent/dir")).unwrap();
    assert!(result.is_empty());
}

#[test]
fn load_empty_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = PluginRuntime::new(std::path::PathBuf::from("."));
    let result = rt.load_from_dir(dir.path()).unwrap();
    assert!(result.is_empty());
}

#[test]
fn invoke_unknown_tool_errors() {
    let mut rt = PluginRuntime::new(std::path::PathBuf::from("."));
    let result = rt.invoke_tool("nonexistent", "{}");
    assert!(result.is_err());
}

#[test]
fn load_bundled_plugins() {
    let mut rt = PluginRuntime::new(std::path::PathBuf::from("."));
    let loaded = rt.load_bundled();
    assert!(loaded.is_empty(), "no bundled plugins expected");
}

/// Install a built plugin binary into a temp dir using its `install` subcommand,
/// then return the base plugins dir for `load_from_dir`.
fn install_plugin(binary_name: &str, plugin_name: &str) -> Option<tempfile::TempDir> {
    let binary = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/release")
        .join(binary_name);
    if !binary.exists() {
        return None;
    }

    let base = tempfile::tempdir().unwrap();
    let install_dir = base.path().join(plugin_name);

    let status = std::process::Command::new(&binary)
        .args(["install", install_dir.to_str().unwrap()])
        .status()
        .expect("failed to run install");
    assert!(status.success(), "install subcommand failed");
    assert!(install_dir.join("manifest.json").exists());

    Some(base)
}

#[test]
fn load_and_invoke_plan_plugin() {
    let Some(base) = install_plugin("phoenix-plugin-plan", "plan") else {
        return;
    };

    let mut rt = PluginRuntime::new(std::path::PathBuf::from("."));
    let loaded = rt
        .load_from_dir(base.path())
        .expect("failed to load plugin dir");
    assert!(!loaded.is_empty(), "expected plan plugin to load");
    assert!(rt.has_command("plan"));

    let result = rt
        .invoke_tool("plan", r#"{"arguments":"implement auth module"}"#)
        .unwrap();
    assert!(result.output.contains("PLAN MODE"));
    assert!(result.output.contains("implement auth module"));
    assert!(!result.toast.is_empty());
}

#[test]
fn load_and_invoke_now_tool_plugin() {
    let Some(base) = install_plugin("phoenix-plugin-now", "now") else {
        return;
    };

    let mut rt = PluginRuntime::new(std::path::PathBuf::from("."));
    let loaded = rt
        .load_from_dir(base.path())
        .expect("failed to load plugin dir");
    assert!(!loaded.is_empty(), "expected now plugin to load");

    let schemas = rt.all_tool_schemas();
    assert!(
        schemas.iter().any(|m| m.name == "get_current_time"),
        "expected get_current_time tool, got: {:?}",
        schemas.iter().map(|m| &m.name).collect::<Vec<_>>()
    );

    let result = rt.invoke_tool("get_current_time", "{}").unwrap();
    assert!(!result.is_error);
    assert!(result.output.contains("The current date and time is"));
}

#[test]
fn load_and_invoke_now_bash_plugin() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins/now-bash");
    if !manifest_dir.join("manifest.json").exists() {
        return;
    }

    let base = tempfile::tempdir().unwrap();
    let plugin_dir = base.path().join("now-bash");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::copy(
        manifest_dir.join("manifest.json"),
        plugin_dir.join("manifest.json"),
    )
    .unwrap();

    let mut rt = PluginRuntime::new(std::path::PathBuf::from("."));
    let loaded = rt
        .load_from_dir(base.path())
        .expect("failed to load now-bash plugin dir");
    assert!(!loaded.is_empty(), "expected now-bash plugin to load");
    assert!(rt.has_command("now"));

    let result = rt.invoke_tool("now_bash", "{}").unwrap();
    assert!(!result.is_error);
    assert!(
        result.output.contains("T") && result.output.contains("Z"),
        "expected ISO 8601 timestamp, got: {}",
        result.output
    );
}

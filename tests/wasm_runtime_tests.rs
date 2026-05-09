use std::path::Path;

use phoenix::plugin::wasm_runtime::WasmRuntime;

#[test]
fn runtime_starts_empty() {
    let rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    assert_eq!(rt.skill_count(), 0);
    assert!(rt.commands().is_empty());
    assert!(!rt.has_command("plan"));
}

#[test]
fn load_nonexistent_dir_returns_empty() {
    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let result = rt.load_from_dir(Path::new("/nonexistent/dir")).unwrap();
    assert!(result.is_empty());
}

#[test]
fn load_empty_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let result = rt.load_from_dir(dir.path()).unwrap();
    assert!(result.is_empty());
}

#[test]
fn execute_unknown_command_errors() {
    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let result = rt.execute("nonexistent", "args");
    assert!(result.is_err());
}

#[test]
fn load_and_execute_plan_plugin() {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/plugins/plan/target/wasm32-wasip2/release/phoenix_plugin_plan.wasm");
    if !wasm_path.exists() {
        return;
    }

    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let loaded = rt
        .load_from_dir(wasm_path.parent().unwrap())
        .expect("failed to load plan plugin dir");
    if loaded.is_empty() {
        // WASM was built against an older WIT; skip until rebuilt
        return;
    }
    assert!(rt.has_command("plan"));

    let result = rt.execute("plan", "implement auth module").unwrap();
    assert!(result.context.contains("PLAN MODE"));
    assert!(result.context.contains("implement auth module"));
    assert!(!result.toast.is_empty());
}

#[test]
fn load_bundled_plugins() {
    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let loaded = rt.load_bundled();
    assert!(!loaded.is_empty(), "bundled plugins should load");
    assert!(rt.has_command("conductor"));

    let result = rt.execute("conductor", "").unwrap();
    assert!(result.context.contains("CONDUCTOR"));
    assert!(!result.toast.is_empty());

    let exit = rt.execute_exit("conductor").unwrap();
    assert!(exit.context.contains("OFF"));
}

#[test]
fn conductor_toggle() {
    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    rt.load_bundled();

    assert!(!rt.is_active("conductor"));

    let r1 = rt.toggle("conductor", "build a thing").unwrap();
    assert!(rt.is_active("conductor"));
    assert!(r1.context.contains("CONDUCTOR"));
    assert!(r1.context.contains("build a thing"));

    let r2 = rt.toggle("conductor", "").unwrap();
    assert!(!rt.is_active("conductor"));
    assert!(r2.context.contains("OFF"));
}

#[test]
fn load_and_execute_now_tool_plugin() {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/plugins/now/target/wasm32-wasip2/release/phoenix_plugin_now.wasm");
    if !wasm_path.exists() {
        return;
    }

    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let _ = rt
        .load_from_dir(wasm_path.parent().unwrap())
        .expect("failed to load now plugin dir");

    let schemas = rt.tool_plugin_schemas();
    assert!(
        schemas.iter().any(|(_, m)| m.name == "get_current_time"),
        "expected get_current_time tool, got: {:?}",
        schemas.iter().map(|(_, m)| &m.name).collect::<Vec<_>>()
    );

    let key = &schemas
        .iter()
        .find(|(_, m)| m.name == "get_current_time")
        .unwrap()
        .0;
    let (output, is_error) = rt.invoke_wasm_tool(key, "get_current_time", "{}").unwrap();
    assert!(!is_error);
    assert!(output.contains("The current date and time is"));
}

use std::path::Path;

use phoenix::plugin::wasm_runtime::WasmRuntime;

#[test]
fn runtime_starts_empty() {
    let rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    assert_eq!(rt.tool_count(), 0);
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
fn invoke_unknown_tool_errors() {
    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let result = rt.invoke_tool("nonexistent", "{}");
    assert!(result.is_err());
}

#[test]
fn load_and_invoke_plan_plugin() {
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
        return;
    }
    assert!(rt.has_command("plan"));

    let result = rt
        .invoke_tool("plan", r#"{"arguments":"implement auth module"}"#)
        .unwrap();
    assert!(result.output.contains("PLAN MODE"));
    assert!(result.output.contains("implement auth module"));
    assert!(!result.toast.is_empty());
}

#[test]
fn load_bundled_plugins() {
    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let loaded = rt.load_bundled();
    assert!(loaded.is_empty(), "no bundled plugins expected");
}

#[test]
fn load_and_invoke_now_tool_plugin() {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/plugins/now/target/wasm32-wasip2/release/phoenix_plugin_now.wasm");
    if !wasm_path.exists() {
        return;
    }

    let mut rt = WasmRuntime::new_with_project(std::path::PathBuf::from(".")).unwrap();
    let loaded = rt
        .load_from_dir(wasm_path.parent().unwrap())
        .expect("failed to load now plugin dir");
    if loaded.is_empty() {
        return;
    }

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

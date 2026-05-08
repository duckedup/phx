use std::path::Path;

use phoenix::plugin::wasm_runtime::WasmRuntime;

#[test]
fn runtime_starts_empty() {
    let rt = WasmRuntime::new().unwrap();
    assert_eq!(rt.skill_count(), 0);
    assert!(rt.commands().is_empty());
    assert!(!rt.has_command("plan"));
}

#[test]
fn load_nonexistent_dir_returns_empty() {
    let mut rt = WasmRuntime::new().unwrap();
    let result = rt.load_from_dir(Path::new("/nonexistent/dir")).unwrap();
    assert!(result.is_empty());
}

#[test]
fn load_empty_dir_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = WasmRuntime::new().unwrap();
    let result = rt.load_from_dir(dir.path()).unwrap();
    assert!(result.is_empty());
}

#[test]
fn execute_unknown_command_errors() {
    let mut rt = WasmRuntime::new().unwrap();
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

    let mut rt = WasmRuntime::new().unwrap();
    let loaded = rt
        .load_from_dir(wasm_path.parent().unwrap())
        .expect("failed to load plan plugin dir");
    assert!(!loaded.is_empty());
    assert!(rt.has_command("plan"));

    let result = rt.execute("plan", "implement auth module").unwrap();
    assert!(result.context.contains("PLAN MODE"));
    assert!(result.context.contains("implement auth module"));
    assert!(!result.toast.is_empty());
}

#[test]
fn load_and_execute_now_plugin() {
    let wasm_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples/plugins/now/target/wasm32-wasip2/release/phoenix_plugin_now.wasm");
    if !wasm_path.exists() {
        return;
    }

    let mut rt = WasmRuntime::new().unwrap();
    let loaded = rt
        .load_from_dir(wasm_path.parent().unwrap())
        .expect("failed to load now plugin dir");
    assert!(!loaded.is_empty());

    assert!(rt.has_command("now"));

    let result = rt.execute("now", "").unwrap();
    assert!(result.context.contains("The current date and time is"));
    assert!(!result.widget.is_empty());
}

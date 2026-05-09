use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;

use crate::tools::traits::{InputRequester, Tool, ToolError, ToolResult, ToolSchema};

use super::wasm_runtime::WasmRuntime;

pub struct WasmToolAdapter {
    runtime: Arc<Mutex<WasmRuntime>>,
    plugin_key: String,
    tool_name: String,
    description: String,
    parameters: Value,
}

impl WasmToolAdapter {
    pub fn new(
        runtime: Arc<Mutex<WasmRuntime>>,
        plugin_key: String,
        tool_name: String,
        description: String,
        parameters_json: &str,
    ) -> Self {
        let parameters = serde_json::from_str(parameters_json)
            .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
        Self {
            runtime,
            plugin_key,
            tool_name,
            description,
            parameters,
        }
    }
}

#[async_trait]
impl Tool for WasmToolAdapter {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.tool_name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let args_json = serde_json::to_string(&args).unwrap_or_default();
        let mut rt = self.runtime.lock();
        match rt.invoke_wasm_tool(&self.plugin_key, &self.tool_name, &args_json) {
            Ok((output, is_error)) => Ok(ToolResult {
                output,
                truncated: false,
                is_error,
            }),
            Err(e) => Err(ToolError::ExecutionFailed(format!(
                "WASM tool '{}' failed: {e}",
                self.tool_name
            ))),
        }
    }
}

pub struct WasmSkillToolAdapter {
    runtime: Arc<Mutex<WasmRuntime>>,
    command: String,
    description: String,
}

impl WasmSkillToolAdapter {
    pub fn new(runtime: Arc<Mutex<WasmRuntime>>, command: String, description: String) -> Self {
        Self {
            runtime,
            command,
            description,
        }
    }
}

#[async_trait]
impl Tool for WasmSkillToolAdapter {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.command.clone(),
            description: self.description.clone(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "arguments": {
                        "type": "string",
                        "description": "Optional arguments"
                    }
                }
            }),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let arguments = args.get("arguments").and_then(|v| v.as_str()).unwrap_or("");
        let mut rt = self.runtime.lock();
        match rt.toggle(&self.command, arguments) {
            Ok(result) => {
                let mut output = result.context;
                if !result.toast.is_empty() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&result.toast);
                }
                Ok(ToolResult::success(output))
            }
            Err(e) => Err(ToolError::ExecutionFailed(format!(
                "WASM skill '{}' failed: {e}",
                self.command
            ))),
        }
    }
}

pub fn register_wasm_tools(
    runtime: &Arc<Mutex<WasmRuntime>>,
    registry: &mut crate::tools::traits::ToolRegistry,
) {
    let rt = runtime.lock();

    for (plugin_key, meta) in rt.tool_plugin_schemas() {
        let adapter = WasmToolAdapter::new(
            Arc::clone(runtime),
            plugin_key,
            meta.name.clone(),
            meta.description,
            &meta.parameters_json,
        );
        tracing::info!("registering WASM tool '{}'", meta.name);
        registry.register(Arc::new(adapter));
    }

    for (command, description) in rt.tool_skill_commands() {
        let adapter = WasmSkillToolAdapter::new(
            Arc::clone(runtime),
            command.to_string(),
            format!("Toggle skill: {description}"),
        );
        tracing::info!("registering WASM skill as tool '{}'", command);
        registry.register(Arc::new(adapter));
    }
}

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;

use crate::tools::traits::{InputRequester, Tool, ToolError, ToolResult, ToolSchema};

use super::wasm_runtime::WasmRuntime;

pub struct UnifiedWasmToolAdapter {
    runtime: Arc<Mutex<WasmRuntime>>,
    tool_name: String,
    description: String,
    parameters: Value,
}

impl UnifiedWasmToolAdapter {
    pub fn new(
        runtime: Arc<Mutex<WasmRuntime>>,
        tool_name: String,
        description: String,
        parameters_json: &str,
    ) -> Self {
        let parameters = serde_json::from_str(parameters_json)
            .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));
        Self {
            runtime,
            tool_name,
            description,
            parameters,
        }
    }
}

#[async_trait]
impl Tool for UnifiedWasmToolAdapter {
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
        match rt.invoke_tool(&self.tool_name, &args_json) {
            Ok(result) => Ok(ToolResult {
                output: result.output,
                truncated: false,
                is_error: result.is_error,
                toast: if result.toast.is_empty() {
                    None
                } else {
                    Some(result.toast)
                },
                widget_json: if result.widget.is_empty() {
                    None
                } else {
                    Some(result.widget)
                },
            }),
            Err(e) => Err(ToolError::ExecutionFailed(format!(
                "WASM tool '{}' failed: {e}",
                self.tool_name
            ))),
        }
    }
}

pub fn register_wasm_tools(
    runtime: &Arc<Mutex<WasmRuntime>>,
    registry: &mut crate::tools::traits::ToolRegistry,
) {
    let rt = runtime.lock();

    for meta in rt.all_tool_schemas() {
        let adapter = UnifiedWasmToolAdapter::new(
            Arc::clone(runtime),
            meta.name.clone(),
            meta.description.clone(),
            &meta.parameters_json,
        );
        tracing::info!("registering WASM tool '{}'", meta.name);
        registry.register(Arc::new(adapter));
    }
}

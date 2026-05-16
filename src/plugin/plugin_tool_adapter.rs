use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;

use crate::tools::traits::{InputRequester, Tool, ToolError, ToolResult, ToolSchema};

use super::plugin_runtime::{PluginRuntime, invoke_tool_async};

pub struct PluginToolAdapter {
    runtime: Arc<Mutex<PluginRuntime>>,
    tool_name: String,
    description: String,
    parameters: Value,
}

impl PluginToolAdapter {
    pub fn new(
        runtime: Arc<Mutex<PluginRuntime>>,
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
impl Tool for PluginToolAdapter {
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

        let (exec_kind, project_dir) = {
            let rt = self.runtime.lock();
            rt.tool_exec_info(&self.tool_name)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?
        };

        match invoke_tool_async(exec_kind, &self.tool_name, &args_json, &project_dir).await {
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
                "Plugin tool '{}' failed: {e}",
                self.tool_name
            ))),
        }
    }
}

pub fn register_plugin_tools(
    runtime: &Arc<Mutex<PluginRuntime>>,
    registry: &mut crate::tools::traits::ToolRegistry,
) {
    let rt = runtime.lock();

    for meta in rt.all_tool_schemas() {
        let adapter = PluginToolAdapter::new(
            Arc::clone(runtime),
            meta.name.clone(),
            meta.description.clone(),
            &meta.parameters_json,
        );
        tracing::info!("registering plugin tool '{}'", meta.name);
        registry.register(Arc::new(adapter));
    }
}

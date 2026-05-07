use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::tools::traits::{Tool, ToolError, ToolResult, ToolSchema};

use super::handle::PluginHandle;
use super::manifest::PluginToolDef;

pub struct PluginToolAdapter {
    handle: Arc<PluginHandle>,
    def: PluginToolDef,
}

impl PluginToolAdapter {
    pub fn new(handle: Arc<PluginHandle>, def: PluginToolDef) -> Self {
        Self { handle, def }
    }
}

#[async_trait]
impl Tool for PluginToolAdapter {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            // Leak is fine: plugin tools are registered once at startup and live
            // for the process lifetime, matching the &'static lifetime that
            // ToolSchema expects.
            name: Box::leak(self.def.name.clone().into_boxed_str()),
            description: Box::leak(self.def.description.clone().into_boxed_str()),
            parameters: self.def.parameters.clone(),
        }
    }

    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
        let call_id = uuid::Uuid::now_v7().to_string();

        match self
            .handle
            .invoke_tool(&self.def.name, args, &call_id)
            .await
        {
            Ok(result) => {
                let output = result
                    .get("output")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let is_error = result
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                Ok(ToolResult {
                    output,
                    truncated: false,
                    is_error,
                })
            }
            Err(e) => Err(ToolError::ExecutionFailed(format!(
                "plugin tool '{}' failed: {e}",
                self.def.name
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_tool_def_fields() {
        let def = PluginToolDef {
            name: "test_tool".into(),
            description: "A test tool".into(),
            parameters: serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}}),
        };

        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
        assert!(def.parameters["properties"]["x"].is_object());
    }
}

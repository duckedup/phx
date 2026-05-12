use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use phoenix_shared::context_types::{ContextMutation, SessionContext};

use crate::tools::traits::{InputRequester, Tool, ToolError, ToolResult, ToolSchema};

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

    fn parse_result(&self, result: Value) -> ToolResult {
        let output = result
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let is_error = result
            .get("is_error")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        ToolResult {
            output,
            truncated: false,
            is_error,
            toast: None,
            widget_json: None,
        }
    }

    fn parse_mutations(&self, result: &Value) -> Vec<ContextMutation> {
        result
            .get("mutations")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| serde_json::from_value::<ContextMutation>(v.clone()).ok())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[async_trait]
impl Tool for PluginToolAdapter {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.def.name.clone(),
            description: self.def.description.clone(),
            parameters: self.def.parameters.clone(),
        }
    }

    fn needs_context(&self) -> bool {
        self.def.needs_context
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let call_id = uuid::Uuid::now_v7().to_string();
        match self
            .handle
            .invoke_tool(&self.def.name, args, &call_id)
            .await
        {
            Ok(result) => Ok(self.parse_result(result)),
            Err(e) => Err(ToolError::ExecutionFailed(format!(
                "plugin tool '{}' failed: {e}",
                self.def.name
            ))),
        }
    }

    async fn invoke_with_context(
        &self,
        args: Value,
        context: &SessionContext,
        _input: &dyn InputRequester,
    ) -> Result<(ToolResult, Vec<ContextMutation>), ToolError> {
        let call_id = uuid::Uuid::now_v7().to_string();
        let mut params = serde_json::json!({
            "name": self.def.name,
            "args": args,
            "call_id": call_id,
        });
        if let Ok(ctx_value) = serde_json::to_value(context) {
            params["context"] = ctx_value;
        }

        match self.handle.request("tool/invoke", params).await {
            Ok(result) => {
                let mutations = self.parse_mutations(&result);
                Ok((self.parse_result(result), mutations))
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
            needs_context: false,
            command: String::new(),
            keybind: String::new(),
            ui_fields: vec![],
        };

        assert_eq!(def.name, "test_tool");
        assert_eq!(def.description, "A test tool");
        assert!(def.parameters["properties"]["x"].is_object());
    }

    #[test]
    fn plugin_tool_def_with_context() {
        let json = r#"{"name": "ctx_tool", "description": "Needs context", "needs_context": true}"#;
        let def: PluginToolDef = serde_json::from_str(json).unwrap();
        assert!(def.needs_context);
    }

    #[test]
    fn plugin_tool_def_defaults_no_context() {
        let json = r#"{"name": "basic", "description": "No context"}"#;
        let def: PluginToolDef = serde_json::from_str(json).unwrap();
        assert!(!def.needs_context);
    }
}

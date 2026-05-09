use std::sync::Arc;

use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::ui::{InputRequest, PluginWidget};
use crate::config::schema::Config;
use crate::session::orchestration::SessionPool;
use crate::tools::traits::ToolRegistry;

pub struct HostHandler {
    tools: Arc<ToolRegistry>,
    pool: Option<Arc<SessionPool>>,
    config: Arc<Config>,
    input_tx: Option<mpsc::Sender<InputRequest>>,
}

impl HostHandler {
    pub fn new(tools: Arc<ToolRegistry>, config: Arc<Config>) -> Self {
        Self {
            tools,
            pool: None,
            config,
            input_tx: None,
        }
    }

    pub fn set_pool(&mut self, pool: Arc<SessionPool>) {
        self.pool = Some(pool);
    }

    pub fn set_input_channel(&mut self, tx: mpsc::Sender<InputRequest>) {
        self.input_tx = Some(tx);
    }

    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "host/tool_call" => self.handle_tool_call(params).await,
            "host/get_config" => self.handle_get_config(params),
            "host/request_input" => self.handle_request_input(params).await,
            _ => Err(format!("unknown host method: {method}")),
        }
    }

    async fn handle_tool_call(&self, params: Value) -> Result<Value, String> {
        let name = params["name"]
            .as_str()
            .ok_or("missing 'name' in host/tool_call")?;
        let args = params["args"].clone();

        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| format!("unknown tool: {name}"))?;

        let noop = crate::tools::traits::NoopInputRequester;
        match tool.invoke(args, &noop).await {
            Ok(result) => Ok(json!({
                "output": result.output,
                "is_error": result.is_error,
            })),
            Err(e) => Ok(json!({
                "output": e.to_string(),
                "is_error": true,
            })),
        }
    }

    fn handle_get_config(&self, params: Value) -> Result<Value, String> {
        let keys = params["keys"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
            .unwrap_or_default();

        let mut result = json!({});
        for key in &keys {
            match *key {
                "providers" => {
                    result["providers"] =
                        serde_json::to_value(&self.config.providers).unwrap_or_default();
                }
                "sessions" => {
                    result["sessions"] =
                        serde_json::to_value(&self.config.sessions).unwrap_or_default();
                }
                _ => {}
            }
        }

        Ok(result)
    }

    async fn handle_request_input(&self, params: Value) -> Result<Value, String> {
        let Some(input_tx) = &self.input_tx else {
            return Err("input not available in this context".into());
        };

        let widget: PluginWidget =
            serde_json::from_value(params).map_err(|e| format!("invalid widget: {e}"))?;

        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        let request = InputRequest {
            widget,
            response_tx,
        };

        input_tx
            .send(request)
            .await
            .map_err(|_| "input channel closed".to_string())?;

        let response = response_rx
            .await
            .map_err(|_| "input response cancelled".to_string())?;

        serde_json::to_value(&response).map_err(|e| format!("failed to serialize response: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::Config;
    use crate::tools::traits::ToolRegistry;

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let handler = HostHandler::new(Arc::new(ToolRegistry::new()), Arc::new(Config::default()));
        let result = handler.handle("host/nonexistent", json!({})).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tool_call_unknown_tool() {
        let handler = HostHandler::new(Arc::new(ToolRegistry::new()), Arc::new(Config::default()));
        let result = handler
            .handle("host/tool_call", json!({"name": "nonexistent", "args": {}}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn get_config_returns_sections() {
        let handler = HostHandler::new(Arc::new(ToolRegistry::new()), Arc::new(Config::default()));
        let result = handler
            .handle(
                "host/get_config",
                json!({"keys": ["providers", "sessions"]}),
            )
            .await
            .unwrap();
        assert!(result["providers"].is_object());
        assert!(result["sessions"].is_object());
    }

    #[tokio::test]
    async fn get_config_empty_keys() {
        let handler = HostHandler::new(Arc::new(ToolRegistry::new()), Arc::new(Config::default()));
        let result = handler
            .handle("host/get_config", json!({"keys": []}))
            .await
            .unwrap();
        assert_eq!(result, json!({}));
    }

    #[tokio::test]
    async fn request_input_without_channel_returns_error() {
        let handler = HostHandler::new(Arc::new(ToolRegistry::new()), Arc::new(Config::default()));
        let result = handler
            .handle(
                "host/request_input",
                json!({"widget": "confirm_dialog", "data": {"message": "Sure?", "options": [{"id": "yes", "label": "Yes"}]}}),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("input not available"));
    }
}

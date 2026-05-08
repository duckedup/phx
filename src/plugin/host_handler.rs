use std::sync::Arc;

use serde_json::{Value, json};

use crate::config::schema::Config;
use crate::session::orchestration::SessionPool;
use crate::tools::traits::ToolRegistry;

pub struct HostHandler {
    tools: Arc<ToolRegistry>,
    pool: Option<Arc<SessionPool>>,
    config: Arc<Config>,
}

impl HostHandler {
    pub fn new(tools: Arc<ToolRegistry>, config: Arc<Config>) -> Self {
        Self {
            tools,
            pool: None,
            config,
        }
    }

    pub fn set_pool(&mut self, pool: Arc<SessionPool>) {
        self.pool = Some(pool);
    }

    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "host/tool_call" => self.handle_tool_call(params).await,
            "host/get_config" => self.handle_get_config(params),
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

        match tool.invoke(args).await {
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
}

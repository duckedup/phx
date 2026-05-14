use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

// ---------------------------------------------------------------------------
// ToolResult
// ---------------------------------------------------------------------------

/// The output of a single tool invocation.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub truncated: bool,
    pub is_error: bool,
    pub toast: Option<String>,
    pub widget_json: Option<String>,
}

impl ToolResult {
    /// Convenience: successful (non-error, non-truncated) result.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            truncated: false,
            is_error: false,
            toast: None,
            widget_json: None,
        }
    }

    /// Convenience: error result.
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            truncated: false,
            is_error: true,
            toast: None,
            widget_json: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ToolSchema
// ---------------------------------------------------------------------------

/// Metadata about a tool, including its JSON Schema for parameters.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

// ---------------------------------------------------------------------------
// ToolError
// ---------------------------------------------------------------------------

/// Errors that can occur while invoking a tool.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("invalid arguments: {0}")]
    InvalidArgs(String),

    #[error("tool execution timed out")]
    Timeout,

    #[error("execution failed: {0}")]
    ExecutionFailed(String),
}

// ---------------------------------------------------------------------------
// InputRequester
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("user cancelled")]
    Cancelled,
    #[error("input not available in this context")]
    NotAvailable,
}

#[async_trait]
pub trait InputRequester: Send + Sync {
    async fn confirm(
        &self,
        title: &str,
        message: &str,
        options: &[(&str, &str)],
    ) -> Result<String, InputError>;

    async fn pick(
        &self,
        title: &str,
        items: &[(String, String, String)],
    ) -> Result<String, InputError>;

    async fn text_input(
        &self,
        title: &str,
        prompt: &str,
        default: &str,
        masked: bool,
    ) -> Result<String, InputError>;
}

pub struct NoopInputRequester;

#[async_trait]
impl InputRequester for NoopInputRequester {
    async fn confirm(&self, _: &str, _: &str, _: &[(&str, &str)]) -> Result<String, InputError> {
        Err(InputError::NotAvailable)
    }
    async fn pick(&self, _: &str, _: &[(String, String, String)]) -> Result<String, InputError> {
        Err(InputError::NotAvailable)
    }
    async fn text_input(&self, _: &str, _: &str, _: &str, _: bool) -> Result<String, InputError> {
        Err(InputError::NotAvailable)
    }
}

// ---------------------------------------------------------------------------
// Tool trait
// ---------------------------------------------------------------------------

/// Every built-in tool implements this trait.
#[async_trait]
pub trait Tool: Send + Sync {
    fn schema(&self) -> ToolSchema;

    async fn invoke(
        &self,
        args: Value,
        input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError>;

    fn needs_context(&self) -> bool {
        false
    }

    async fn invoke_with_context(
        &self,
        args: Value,
        _context: &crate::shared::context_types::SessionContext,
        input: &dyn InputRequester,
    ) -> Result<
        (
            ToolResult,
            Vec<crate::shared::context_types::ContextMutation>,
        ),
        ToolError,
    > {
        self.invoke(args, input).await.map(|r| (r, vec![]))
    }
}

// ---------------------------------------------------------------------------
// ToolRegistry
// ---------------------------------------------------------------------------

/// A collection of tools keyed by name.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Overwrites any existing tool with the same name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.schema().name.to_string();
        self.tools.insert(name, tool);
    }

    /// Look up a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Remove a tool by name. Returns true if it was present.
    pub fn unregister(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }

    /// Number of registered tools.
    pub fn count(&self) -> usize {
        self.tools.len()
    }

    /// Return the schemas for every registered tool (in arbitrary order).
    pub fn list_schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|t| t.schema()).collect()
    }

    pub fn retain_builtins(&mut self) {
        self.tools.retain(|name, _| {
            matches!(
                name.as_str(),
                "bash"
                    | "read"
                    | "write"
                    | "edit"
                    | "spawn_agent"
                    | "check_agents"
                    | "collect_agent"
                    | "cancel_agent"
                    | "merge_agent"
            )
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_success_defaults() {
        let r = ToolResult::success("ok");
        assert_eq!(r.output, "ok");
        assert!(!r.truncated);
        assert!(!r.is_error);
    }

    #[test]
    fn tool_result_error_defaults() {
        let r = ToolResult::error("bad");
        assert_eq!(r.output, "bad");
        assert!(!r.truncated);
        assert!(r.is_error);
    }

    #[test]
    fn registry_starts_empty() {
        let reg = ToolRegistry::new();
        assert_eq!(reg.count(), 0);
        assert!(reg.get("anything").is_none());
    }

    #[test]
    fn registry_list_schemas_empty() {
        let reg = ToolRegistry::new();
        assert!(reg.list_schemas().is_empty());
    }
}

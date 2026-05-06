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
}

impl ToolResult {
    /// Convenience: successful (non-error, non-truncated) result.
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            truncated: false,
            is_error: false,
        }
    }

    /// Convenience: error result.
    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            truncated: false,
            is_error: true,
        }
    }
}

// ---------------------------------------------------------------------------
// ToolSchema
// ---------------------------------------------------------------------------

/// Static metadata about a tool, including its JSON Schema for parameters.
#[derive(Debug, Clone)]
pub struct ToolSchema {
    pub name: &'static str,
    pub description: &'static str,
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
// Tool trait
// ---------------------------------------------------------------------------

/// Every built-in tool implements this trait.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Returns the JSON Schema description of this tool.
    fn schema(&self) -> ToolSchema;

    /// Execute the tool with the given JSON arguments.
    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError>;
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

    /// Number of registered tools.
    pub fn count(&self) -> usize {
        self.tools.len()
    }

    /// Return the schemas for every registered tool (in arbitrary order).
    pub fn list_schemas(&self) -> Vec<ToolSchema> {
        self.tools.values().map(|t| t.schema()).collect()
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

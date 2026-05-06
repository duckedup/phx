use async_trait::async_trait;
use serde_json::{Value, json};

use super::traits::{Tool, ToolError, ToolResult, ToolSchema};

pub struct WriteTool;

#[async_trait]
impl Tool for WriteTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "write",
            description: "Write content to a file. Creates the file if it doesn't exist, \
                          overwrites if it does. Automatically creates parent directories.",
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to write"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    }
                },
                "required": ["file_path", "content"]
            }),
        }
    }

    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required field 'file_path'".into()))?;

        let content = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required field 'content'".into()))?;

        // Create parent directories if needed.
        let path = std::path::Path::new(file_path);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "cannot create parent directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        tokio::fs::write(file_path, content)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("cannot write {file_path}: {e}")))?;

        let preview_lines: Vec<&str> = content.lines().take(30).collect();
        let total_lines = content.lines().count();
        let mut output = format!(
            "wrote {file_path} ({} bytes, {total_lines} lines)\n",
            content.len()
        );
        for line in &preview_lines {
            output.push_str(&format!("+ {line}\n"));
        }
        if total_lines > 30 {
            output.push_str(&format!("... ({} more lines)\n", total_lines - 30));
        }
        Ok(ToolResult::success(output))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn write_creates_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.txt");
        let path_str = path.to_str().unwrap();

        let result = WriteTool
            .invoke(json!({"file_path": path_str, "content": "hello world"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert!(result.output.contains("wrote"));
        assert!(result.output.contains("11 bytes"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");
    }

    #[tokio::test]
    async fn write_creates_nested_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("a").join("b").join("c").join("file.txt");
        let path_str = path.to_str().unwrap();

        let result = WriteTool
            .invoke(json!({"file_path": path_str, "content": "nested"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "nested");
    }

    #[tokio::test]
    async fn write_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("overwrite.txt");
        std::fs::write(&path, "OLD CONTENT THAT IS LONGER").unwrap();
        let path_str = path.to_str().unwrap();

        let result = WriteTool
            .invoke(json!({"file_path": path_str, "content": "new"}))
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }

    #[tokio::test]
    async fn write_missing_content() {
        let result = WriteTool.invoke(json!({"file_path": "/tmp/x"})).await;
        match result {
            Err(ToolError::InvalidArgs(msg)) => {
                assert!(msg.contains("content"), "msg was: {msg}");
            }
            other => panic!("expected InvalidArgs, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn write_missing_path() {
        let result = WriteTool.invoke(json!({"content": "data"})).await;
        match result {
            Err(ToolError::InvalidArgs(msg)) => {
                assert!(msg.contains("file_path"), "msg was: {msg}");
            }
            other => panic!("expected InvalidArgs, got: {other:?}"),
        }
    }
}

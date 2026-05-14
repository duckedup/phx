use async_trait::async_trait;
use serde_json::{Value, json};

use super::traits::{InputRequester, Tool, ToolError, ToolResult, ToolSchema};

/// Default number of lines returned when `limit` is not specified.
const DEFAULT_LIMIT: usize = 2000;

/// Maximum file size we will read (512 KB).
const MAX_BYTES: usize = 512 * 1024;

pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "read".into(),
            description:
                "Read the contents of a file. Returns line-numbered output (cat -n style). \
                          Defaults to the first 2000 lines. Use offset/limit for large files."
                    .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to read"
                    },
                    "offset": {
                        "type": "integer",
                        "minimum": 0,
                        "description": "Line number to start reading from (0-indexed, default 0)"
                    },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Maximum number of lines to read (default 2000)"
                    }
                },
                "required": ["file_path"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let file_path = args
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required field 'file_path'".into()))?;

        let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(DEFAULT_LIMIT);

        // Read the file (capped at MAX_BYTES).
        let raw = match tokio::fs::read(file_path).await {
            Ok(bytes) => {
                if bytes.len() > MAX_BYTES {
                    bytes[..MAX_BYTES].to_vec()
                } else {
                    bytes
                }
            }
            Err(e) => {
                return Err(ToolError::ExecutionFailed(format!(
                    "cannot read {file_path}: {e}"
                )));
            }
        };

        let content = String::from_utf8_lossy(&raw);
        let lines: Vec<&str> = content.lines().collect();

        // Apply offset and limit.
        let start = offset.min(lines.len());
        let end = (start + limit).min(lines.len());
        let selected = &lines[start..end];

        // Format with line numbers (1-indexed, matching `cat -n` style).
        let mut out = String::new();
        for (i, line) in selected.iter().enumerate() {
            let line_num = start + i + 1; // 1-indexed
            out.push_str(&format!("{line_num:>6}\t{line}\n"));
        }

        let truncated = raw.len() >= MAX_BYTES || end < lines.len();

        Ok(ToolResult {
            output: out,
            truncated,
            is_error: false,
            toast: None,
            widget_json: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::traits::NoopInputRequester;
    use tempfile::TempDir;

    fn write_temp(dir: &TempDir, name: &str, content: &str) -> String {
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        path.to_str().unwrap().to_string()
    }

    #[tokio::test]
    async fn read_whole_small_file() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "test.txt", "alpha\nbeta\ngamma\n");

        let result = ReadTool
            .invoke(json!({"file_path": path}), &NoopInputRequester)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("alpha"));
        assert!(result.output.contains("beta"));
        assert!(result.output.contains("gamma"));
        // Check line numbers are present.
        assert!(result.output.contains("1\t"));
        assert!(result.output.contains("2\t"));
        assert!(result.output.contains("3\t"));
    }

    #[tokio::test]
    async fn read_with_offset_and_limit() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "slice.txt", "one\ntwo\nthree\nfour\nfive\n");

        let result = ReadTool
            .invoke(
                json!({"file_path": path, "offset": 1, "limit": 2}),
                &NoopInputRequester,
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        // offset=1 means skip line 1 ("one"), start at line 2 ("two").
        assert!(result.output.contains("two"));
        assert!(result.output.contains("three"));
        assert!(!result.output.contains("\tone\n"));
        assert!(!result.output.contains("\tfour\n"));
    }

    #[tokio::test]
    async fn read_offset_beyond_file() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "short.txt", "one\ntwo\n");

        let result = ReadTool
            .invoke(
                json!({"file_path": path, "offset": 100}),
                &NoopInputRequester,
            )
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.output, "");
    }

    #[tokio::test]
    async fn read_missing_file() {
        let result = ReadTool
            .invoke(
                json!({"file_path": "/tmp/nonexistent_phx_test_file.txt"}),
                &NoopInputRequester,
            )
            .await;
        match result {
            Err(ToolError::ExecutionFailed(msg)) => {
                assert!(msg.contains("cannot read"), "msg was: {msg}");
            }
            other => panic!("expected ExecutionFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_missing_path_arg() {
        let result = ReadTool.invoke(json!({}), &NoopInputRequester).await;
        match result {
            Err(ToolError::InvalidArgs(msg)) => {
                assert!(msg.contains("file_path"), "msg was: {msg}");
            }
            other => panic!("expected InvalidArgs, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn read_line_numbers_are_one_indexed() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "nums.txt", "aaa\nbbb\n");

        let result = ReadTool
            .invoke(json!({"file_path": path}), &NoopInputRequester)
            .await
            .unwrap();
        let first_line = result.output.lines().next().unwrap();
        // Should start with "     1\t"
        assert!(
            first_line.trim_start().starts_with("1\t"),
            "first line was: {first_line}"
        );
    }

    #[tokio::test]
    async fn read_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "empty.txt", "");

        let result = ReadTool
            .invoke(json!({"file_path": path}), &NoopInputRequester)
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(result.output, "");
    }
}

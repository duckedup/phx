use async_trait::async_trait;
use serde_json::{Value, json};

use super::traits::{InputRequester, Tool, ToolError, ToolResult, ToolSchema};

pub struct EditTool;

#[async_trait]
impl Tool for EditTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "edit".into(),
            description: "Edit a file by replacing exact text. The old_string must match exactly \
                          (including whitespace). Fails if old_string is not unique unless \
                          replace_all is true."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {
                        "type": "string",
                        "description": "Absolute path to the file to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Exact text to find and replace (must match exactly)"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "New text to replace the old text with"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "If true, replace all occurrences; otherwise fail on non-unique match",
                        "default": false
                    }
                },
                "required": ["file_path", "old_string", "new_string"]
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

        let old_string = args
            .get("old_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required field 'old_string'".into()))?;

        let new_string = args
            .get("new_string")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required field 'new_string'".into()))?;

        let replace_all = args
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if old_string.is_empty() {
            return Err(ToolError::InvalidArgs(
                "'old_string' must not be empty".into(),
            ));
        }

        // Read the file.
        let original = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("cannot read {file_path}: {e}")))?;

        // Count occurrences.
        let count = original.matches(old_string).count();

        if count == 0 {
            return Err(ToolError::ExecutionFailed(format!(
                "old_string not found in {file_path}"
            )));
        }

        if count > 1 && !replace_all {
            return Err(ToolError::ExecutionFailed(format!(
                "old_string found {count} times in {file_path}; use replace_all to replace all occurrences"
            )));
        }

        // Perform the replacement.
        let updated = if replace_all {
            original.replace(old_string, new_string)
        } else {
            original.replacen(old_string, new_string, 1)
        };

        tokio::fs::write(file_path, &updated)
            .await
            .map_err(|e| ToolError::ExecutionFailed(format!("cannot write {file_path}: {e}")))?;

        let mut diff = format!("edited {file_path}: replaced {count} occurrence(s)\n");
        for line in old_string.lines() {
            diff.push_str(&format!("- {line}\n"));
        }
        for line in new_string.lines() {
            diff.push_str(&format!("+ {line}\n"));
        }
        Ok(ToolResult::success(diff))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(all(test, not(miri)))]
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
    async fn edit_replaces_unique_string() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "test.txt", "hello world");

        let result = EditTool
            .invoke(
                json!({
                    "file_path": path,
                    "old_string": "hello",
                    "new_string": "goodbye"
                }),
                &NoopInputRequester,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye world");
    }

    #[tokio::test]
    async fn edit_fails_on_non_unique() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "dup.txt", "aaa bbb aaa");

        let result = EditTool
            .invoke(
                json!({
                    "file_path": path,
                    "old_string": "aaa",
                    "new_string": "zzz"
                }),
                &NoopInputRequester,
            )
            .await;

        match result {
            Err(ToolError::ExecutionFailed(msg)) => {
                assert!(msg.contains("2 times"), "msg was: {msg}");
                assert!(msg.contains("replace_all"), "msg was: {msg}");
            }
            other => panic!("expected ExecutionFailed, got: {other:?}"),
        }
        // File should be unchanged.
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "aaa bbb aaa");
    }

    #[tokio::test]
    async fn edit_replace_all() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "all.txt", "aaa bbb aaa ccc aaa");

        let result = EditTool
            .invoke(
                json!({
                    "file_path": path,
                    "old_string": "aaa",
                    "new_string": "zzz",
                    "replace_all": true
                }),
                &NoopInputRequester,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "zzz bbb zzz ccc zzz"
        );
    }

    #[tokio::test]
    async fn edit_old_string_not_found() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "nf.txt", "nothing here");

        let result = EditTool
            .invoke(
                json!({
                    "file_path": path,
                    "old_string": "absent",
                    "new_string": "present"
                }),
                &NoopInputRequester,
            )
            .await;

        match result {
            Err(ToolError::ExecutionFailed(msg)) => {
                assert!(msg.contains("not found"), "msg was: {msg}");
            }
            other => panic!("expected ExecutionFailed, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_empty_old_string() {
        let result = EditTool
            .invoke(
                json!({
                    "file_path": "/tmp/x",
                    "old_string": "",
                    "new_string": "y"
                }),
                &NoopInputRequester,
            )
            .await;

        match result {
            Err(ToolError::InvalidArgs(msg)) => {
                assert!(msg.contains("old_string"), "msg was: {msg}");
            }
            other => panic!("expected InvalidArgs, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_missing_file() {
        let result = EditTool
            .invoke(
                json!({
                    "file_path": "/tmp/nonexistent_phx_edit_test.txt",
                    "old_string": "a",
                    "new_string": "b"
                }),
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
    async fn edit_preserves_whitespace() {
        let dir = TempDir::new().unwrap();
        let path = write_temp(&dir, "ws.txt", "  hello  \n  world  \n");

        let result = EditTool
            .invoke(
                json!({
                    "file_path": path,
                    "old_string": "  hello  ",
                    "new_string": "  goodbye  "
                }),
                &NoopInputRequester,
            )
            .await
            .unwrap();

        assert!(!result.is_error);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "  goodbye  \n  world  \n"
        );
    }
}

use async_trait::async_trait;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::AsyncReadExt;

use super::traits::{InputRequester, Tool, ToolError, ToolResult, ToolSchema};

/// Maximum bytes captured from stdout or stderr.
const MAX_OUTPUT_BYTES: usize = 512 * 1024;

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: "bash".into(),
            description: "Execute a bash command in the current working directory. \
                          Returns stdout and stderr. Optionally provide a timeout in seconds."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Bash command to execute"
                    },
                    "timeout": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Timeout in seconds (optional, no default timeout)"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    async fn invoke(
        &self,
        args: Value,
        _input: &dyn InputRequester,
    ) -> Result<ToolResult, ToolError> {
        let command = args
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("missing required field 'command'".into()))?;

        let timeout_secs = args.get("timeout").and_then(|v| v.as_u64());

        let mut child = tokio::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(command)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        let stdout_handle = child.stdout.take();
        let stderr_handle = child.stderr.take();

        // Read stdout and stderr concurrently, capped at MAX_OUTPUT_BYTES.
        let read_capped = |mut reader: Option<tokio::process::ChildStdout>,
                           mut reader_err: Option<tokio::process::ChildStderr>|
         -> (
            tokio::task::JoinHandle<Vec<u8>>,
            tokio::task::JoinHandle<Vec<u8>>,
        ) {
            let out_handle = tokio::spawn(async move {
                let mut buf = vec![0u8; MAX_OUTPUT_BYTES];
                let mut total = 0usize;
                if let Some(ref mut r) = reader {
                    loop {
                        if total >= MAX_OUTPUT_BYTES {
                            break;
                        }
                        match r.read(&mut buf[total..]).await {
                            Ok(0) => break,
                            Ok(n) => total += n,
                            Err(_) => break,
                        }
                    }
                }
                buf.truncate(total);
                buf
            });

            let err_handle = tokio::spawn(async move {
                let mut buf = vec![0u8; MAX_OUTPUT_BYTES];
                let mut total = 0usize;
                if let Some(ref mut r) = reader_err {
                    loop {
                        if total >= MAX_OUTPUT_BYTES {
                            break;
                        }
                        match r.read(&mut buf[total..]).await {
                            Ok(0) => break,
                            Ok(n) => total += n,
                            Err(_) => break,
                        }
                    }
                }
                buf.truncate(total);
                buf
            });

            (out_handle, err_handle)
        };

        let (out_jh, err_jh) = read_capped(stdout_handle, stderr_handle);

        // Wait for exit, optionally with a timeout.
        let status = if let Some(secs) = timeout_secs {
            let deadline = tokio::time::Duration::from_secs(secs);
            match tokio::time::timeout(deadline, child.wait()).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => {
                    return Err(ToolError::ExecutionFailed(format!(
                        "failed to wait on child: {e}"
                    )));
                }
                Err(_elapsed) => {
                    // Kill the child on timeout.
                    let _ = child.kill().await;
                    return Err(ToolError::Timeout);
                }
            }
        } else {
            child
                .wait()
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("failed to wait on child: {e}")))?
        };

        let stdout_bytes = out_jh.await.unwrap_or_default();
        let stderr_bytes = err_jh.await.unwrap_or_default();

        let stdout = String::from_utf8_lossy(&stdout_bytes);
        let stderr = String::from_utf8_lossy(&stderr_bytes);

        let exit_code = status.code().unwrap_or(-1);
        let truncated =
            stdout_bytes.len() >= MAX_OUTPUT_BYTES || stderr_bytes.len() >= MAX_OUTPUT_BYTES;

        // Build combined output.
        let mut out = String::new();
        out.push_str(&format!("exit code: {exit_code}\n"));

        if !stdout.is_empty() {
            out.push_str("--- stdout ---\n");
            out.push_str(&stdout);
            if !stdout.ends_with('\n') {
                out.push('\n');
            }
        }
        if !stderr.is_empty() {
            out.push_str("--- stderr ---\n");
            out.push_str(&stderr);
            if !stderr.ends_with('\n') {
                out.push('\n');
            }
        }

        Ok(ToolResult {
            output: out,
            truncated,
            is_error: exit_code != 0,
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

    #[tokio::test]
    async fn bash_captures_stdout() {
        let tool = BashTool;
        let result = tool
            .invoke(json!({"command": "echo hello"}), &NoopInputRequester)
            .await
            .unwrap();
        assert!(result.output.contains("hello"));
        assert!(result.output.contains("exit code: 0"));
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn bash_nonzero_exit() {
        let result = BashTool
            .invoke(json!({"command": "exit 42"}), &NoopInputRequester)
            .await
            .unwrap();
        assert!(result.output.contains("exit code: 42"));
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn bash_captures_stderr() {
        let result = BashTool
            .invoke(json!({"command": "echo err >&2"}), &NoopInputRequester)
            .await
            .unwrap();
        assert!(result.output.contains("--- stderr ---"));
        assert!(result.output.contains("err"));
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn bash_timeout_kills_command() {
        let result = BashTool
            .invoke(
                json!({"command": "sleep 60", "timeout": 1}),
                &NoopInputRequester,
            )
            .await;
        match result {
            Err(ToolError::Timeout) => {} // expected
            other => panic!("expected Timeout error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn bash_missing_command() {
        let result = BashTool.invoke(json!({}), &NoopInputRequester).await;
        match result {
            Err(ToolError::InvalidArgs(msg)) => {
                assert!(msg.contains("command"), "message was: {msg}");
            }
            other => panic!("expected InvalidArgs, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn bash_stdout_and_stderr_combined() {
        let result = BashTool
            .invoke(
                json!({"command": "echo out; echo err >&2"}),
                &NoopInputRequester,
            )
            .await
            .unwrap();
        assert!(result.output.contains("--- stdout ---"));
        assert!(result.output.contains("out"));
        assert!(result.output.contains("--- stderr ---"));
        assert!(result.output.contains("err"));
    }

    #[tokio::test]
    async fn bash_truncated_flag_on_large_output() {
        // Generate just over MAX_OUTPUT_BYTES of stdout.
        // 512KB = 524288 bytes. We'll ask dd to produce exactly that plus some.
        let cmd = format!(
            "dd if=/dev/zero bs=1024 count={} 2>/dev/null | tr '\\0' 'A'",
            (MAX_OUTPUT_BYTES / 1024) + 1
        );
        let result = BashTool
            .invoke(json!({"command": cmd}), &NoopInputRequester)
            .await
            .unwrap();
        assert!(result.truncated);
    }
}

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::host_handler::HostHandler;

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcNotification {
    method: String,
    params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(default)]
    pub result: Option<serde_json::Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
    // Reverse-direction request from plugin
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

pub struct PluginTransport {
    child: Child,
    stdin: Mutex<tokio::process::ChildStdin>,
    stdout: Mutex<BufReader<tokio::process::ChildStdout>>,
    next_id: Mutex<u64>,
    host_handler: Mutex<Option<Arc<HostHandler>>>,
}

impl PluginTransport {
    pub fn spawn(command: &Path, args: &[String], plugin_dir: &Path) -> anyhow::Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .current_dir(plugin_dir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn plugin {}: {e}", command.display()))?;

        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();

        Ok(Self {
            child,
            stdin: Mutex::new(stdin),
            stdout: Mutex::new(BufReader::new(stdout)),
            next_id: Mutex::new(1),
            host_handler: Mutex::new(None),
        })
    }

    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = {
            let mut next = self.next_id.lock().await;
            let id = *next;
            *next += 1;
            id
        };

        let req = JsonRpcRequest {
            id,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_vec(&req)?;
        line.push(b'\n');

        {
            let mut stdin = self.stdin.lock().await;
            stdin.write_all(&line).await?;
            stdin.flush().await?;
        }

        let resp = self.read_response().await?;

        if resp.id != Some(id) {
            anyhow::bail!("response id mismatch: expected {id}, got {:?}", resp.id);
        }

        if let Some(err) = resp.error {
            anyhow::bail!("plugin error ({}): {}", err.code, err.message);
        }

        Ok(resp.result.unwrap_or(serde_json::Value::Null))
    }

    pub async fn notify(&self, method: &str, params: serde_json::Value) -> anyhow::Result<()> {
        let notif = JsonRpcNotification {
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_vec(&notif)?;
        line.push(b'\n');

        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&line).await?;
        stdin.flush().await?;
        Ok(())
    }

    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: std::time::Duration,
    ) -> anyhow::Result<serde_json::Value> {
        match tokio::time::timeout(timeout, self.request(method, params)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!("plugin request timed out after {:?}", timeout),
        }
    }

    pub async fn set_host_handler(&self, handler: Arc<HostHandler>) {
        *self.host_handler.lock().await = Some(handler);
    }

    async fn read_response(&self) -> anyhow::Result<JsonRpcResponse> {
        loop {
            let mut stdout = self.stdout.lock().await;
            let mut line = String::new();
            let n = stdout.read_line(&mut line).await?;
            if n == 0 {
                anyhow::bail!("plugin process closed stdout");
            }
            drop(stdout);

            let resp: JsonRpcResponse = serde_json::from_str(line.trim())?;

            if let Some(method) = &resp.method
                && method.starts_with("host/")
            {
                let req_id = resp.id;
                let params = resp.params.clone().unwrap_or_default();
                let handler = self.host_handler.lock().await;

                let result = if let Some(h) = handler.as_ref() {
                    h.handle(method, params).await
                } else {
                    Err(format!("no host handler registered for {method}"))
                };

                let response_json = match result {
                    Ok(val) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "result": val,
                    }),
                    Err(msg) => serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": req_id,
                        "error": { "code": -32000, "message": msg },
                    }),
                };

                let mut response_bytes = serde_json::to_vec(&response_json)?;
                response_bytes.push(b'\n');
                let mut stdin = self.stdin.lock().await;
                stdin.write_all(&response_bytes).await?;
                stdin.flush().await?;

                continue;
            }

            return Ok(resp);
        }
    }

    pub async fn shutdown(&self) {
        let _ = self.notify("shutdown", serde_json::json!({})).await;
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    pub async fn kill(&mut self) {
        let _ = self.child.kill().await;
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn spawn_nonexistent_binary_fails() {
        let result = PluginTransport::spawn(
            &PathBuf::from("/nonexistent/binary"),
            &[],
            &PathBuf::from("/tmp"),
        );
        assert!(result.is_err());
    }
}

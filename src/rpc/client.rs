use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct RpcClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
}

#[derive(Debug)]
pub struct ConfigSnapshot {
    pub raw: serde_json::Value,
}

#[derive(Debug)]
pub struct DispatchResult {
    pub result_type: String,
    pub data: serde_json::Value,
}

impl RpcClient {
    pub async fn spawn(config_path: Option<&Path>) -> anyhow::Result<Self> {
        let exe = std::env::current_exe()?;
        let mut cmd = tokio::process::Command::new(exe);
        cmd.arg("rpc");

        if let Some(path) = config_path {
            cmd.arg("--config").arg(path);
        }

        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");

        Ok(Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
        })
    }

    pub async fn send(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let req = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let mut line = serde_json::to_vec(&req)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;

        let mut response_line = String::new();
        self.reader.read_line(&mut response_line).await?;

        let resp: serde_json::Value = serde_json::from_str(response_line.trim())?;
        Ok(resp)
    }

    pub async fn send_streaming(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<Vec<serde_json::Value>> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let req = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });

        let mut line = serde_json::to_vec(&req)?;
        line.push(b'\n');
        self.stdin.write_all(&line).await?;
        self.stdin.flush().await?;

        let mut events = vec![];
        loop {
            let mut response_line = String::new();
            self.reader.read_line(&mut response_line).await?;
            if response_line.trim().is_empty() {
                continue;
            }

            let resp: serde_json::Value = serde_json::from_str(response_line.trim())?;
            let is_done = resp.get("event").and_then(|e| e.as_str()) == Some("done");
            let is_error = resp.get("error").is_some();
            events.push(resp);

            if is_done || is_error {
                break;
            }
        }

        Ok(events)
    }

    pub async fn config_get(&mut self) -> anyhow::Result<ConfigSnapshot> {
        let resp = self.send("config.get", serde_json::json!({})).await?;
        Ok(ConfigSnapshot {
            raw: resp.get("result").cloned().unwrap_or_default(),
        })
    }

    pub async fn dispatch(&mut self, input: &str) -> anyhow::Result<DispatchResult> {
        let resp = self
            .send("command.dispatch", serde_json::json!({"input": input}))
            .await?;
        let result = resp.get("result").cloned().unwrap_or_default();
        Ok(DispatchResult {
            result_type: result
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .into(),
            data: result,
        })
    }

    pub async fn shutdown(mut self) -> anyhow::Result<()> {
        drop(self.stdin);
        self.child.kill().await?;
        Ok(())
    }
}

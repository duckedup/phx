use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::TcpStream;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::mpsc;

const CHANNEL_CAPACITY: usize = 256;

type Pending = Arc<Mutex<HashMap<u64, mpsc::Sender<serde_json::Value>>>>;

pub struct RemoteClient {
    writer: tokio::sync::Mutex<BufWriter<OwnedWriteHalf>>,
    pending: Pending,
    reader_task: tokio::task::JoinHandle<()>,
    next_id: AtomicU64,
}

impl RemoteClient {
    pub async fn connect(endpoint: &str) -> anyhow::Result<Self> {
        let stream = TcpStream::connect(endpoint).await?;
        let (read_half, write_half) = stream.into_split();
        let writer = BufWriter::new(write_half);
        let reader = BufReader::new(read_half);

        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let pending_for_task = Arc::clone(&pending);
        let reader_task = tokio::spawn(read_loop(reader, pending_for_task));

        Ok(Self {
            writer: tokio::sync::Mutex::new(writer),
            pending,
            reader_task,
            next_id: AtomicU64::new(1),
        })
    }

    pub async fn send(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let mut rx = self.send_streaming(method, params).await?;
        match rx.recv().await {
            Some(v) => Ok(v),
            None => Err(anyhow::anyhow!("connection closed before response")),
        }
    }

    pub async fn send_streaming(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<mpsc::Receiver<serde_json::Value>> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        self.pending.lock().insert(id, tx);

        let req = serde_json::json!({
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_vec(&req)?;
        line.push(b'\n');

        let mut writer = self.writer.lock().await;
        if let Err(e) = writer.write_all(&line).await {
            self.pending.lock().remove(&id);
            return Err(e.into());
        }
        if let Err(e) = writer.flush().await {
            self.pending.lock().remove(&id);
            return Err(e.into());
        }

        Ok(rx)
    }
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        self.reader_task.abort();
    }
}

async fn read_loop(mut reader: BufReader<OwnedReadHalf>, pending: Pending) {
    let disconnect_reason = loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => break "server disconnected",
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "remote client read error");
                break "read error";
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, line = %trimmed, "remote client parse error");
                continue;
            }
        };

        let id = match value.get("id").and_then(|v| v.as_u64()) {
            Some(id) => id,
            None => continue,
        };

        let is_terminal = value
            .get("event")
            .and_then(|e| e.as_str())
            .map(|s| s == "done")
            .unwrap_or(false)
            || value.get("error").is_some()
            || value.get("result").is_some();

        let sender = if is_terminal {
            pending.lock().remove(&id)
        } else {
            pending.lock().get(&id).cloned()
        };

        if let Some(sender) = sender {
            let _ = sender.send(value).await;
        }
    };

    let orphans: Vec<_> = pending.lock().drain().collect();
    for (id, sender) in orphans {
        let _ = sender
            .send(serde_json::json!({
                "id": id,
                "error": {"code": -1, "message": disconnect_reason},
            }))
            .await;
    }
}

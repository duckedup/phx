use std::sync::Arc;

use tokio::net::TcpListener;

use crate::config::Config;
use crate::rpc;

pub async fn run(config: Config, host: String, port: u16) -> anyhow::Result<()> {
    let addr = format!("{host}:{port}");
    let listener = TcpListener::bind(&addr).await?;
    let config = Arc::new(config);

    tracing::info!(%addr, "phx server listening");

    let accept_loop = async {
        loop {
            let (stream, peer) = match listener.accept().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(error = %e, "accept failed");
                    continue;
                }
            };
            let config = Arc::clone(&config);
            tokio::spawn(async move {
                tracing::info!(%peer, "client connected");
                let (read_half, write_half) = stream.into_split();
                let reader = tokio::io::BufReader::new(read_half);
                let writer = tokio::io::BufWriter::new(write_half);
                if let Err(e) = rpc::server::run((*config).clone(), reader, writer).await {
                    tracing::warn!(%peer, error = %e, "client session ended with error");
                } else {
                    tracing::info!(%peer, "client disconnected");
                }
            });
        }
    };

    tokio::select! {
        _ = accept_loop => {}
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received");
        }
    }

    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigint = match signal(SignalKind::interrupt()) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut sigterm = match signal(SignalKind::terminate()) {
        Ok(s) => s,
        Err(_) => return,
    };
    tokio::select! {
        _ = sigint.recv() => {}
        _ = sigterm.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(all(test, not(miri)))]
mod tests {
    use crate::config::Config;
    use crate::remote::client::RemoteClient;

    #[tokio::test]
    async fn client_can_call_config_get_over_tcp() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, w) = stream.into_split();
            let r = tokio::io::BufReader::new(r);
            let w = tokio::io::BufWriter::new(w);
            crate::rpc::server::run(Config::default(), r, w)
                .await
                .unwrap();
        });

        let client = RemoteClient::connect(&addr.to_string()).await.unwrap();
        let resp = client
            .send("config.get", serde_json::json!({}))
            .await
            .unwrap();
        assert!(resp.get("result").is_some());

        drop(client);
        let _ = server.await;
    }
}

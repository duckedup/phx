use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use super::host_handler::HostHandler;
use super::manifest::PluginManifest;
use super::transport::PluginTransport;

pub struct PluginHandle {
    pub manifest: PluginManifest,
    pub dir: PathBuf,
    transport: Arc<PluginTransport>,
}

impl PluginHandle {
    pub fn spawn(manifest: PluginManifest, dir: PathBuf) -> anyhow::Result<Self> {
        let bin = super::manifest::resolve_bin(&manifest, &dir);
        let transport = PluginTransport::spawn(&bin, &manifest.bin_args, &dir)?;

        Ok(Self {
            manifest,
            dir,
            transport: Arc::new(transport),
        })
    }

    pub async fn initialize(&self, project_dir: &Path) -> anyhow::Result<serde_json::Value> {
        let params = serde_json::json!({
            "phx_version": env!("CARGO_PKG_VERSION"),
            "project_dir": project_dir,
            "plugin_dir": self.dir,
        });

        self.transport
            .request_with_timeout("initialize", params, Duration::from_secs(5))
            .await
    }

    pub async fn shutdown(&self) {
        self.transport.shutdown().await;
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub async fn request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        self.transport.request(method, params).await
    }

    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> anyhow::Result<serde_json::Value> {
        self.transport
            .request_with_timeout(method, params, timeout)
            .await
    }

    pub async fn notify(&self, method: &str, params: serde_json::Value) -> anyhow::Result<()> {
        self.transport.notify(method, params).await
    }

    pub async fn execute_command(
        &self,
        name: &str,
        args: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let params = serde_json::json!({
            "name": name,
            "args": args,
        });
        self.transport
            .request_with_timeout("command/execute", params, Duration::from_secs(10))
            .await
    }

    pub async fn set_host_handler(&self, handler: Arc<HostHandler>) {
        self.transport.set_host_handler(handler).await;
    }

    pub async fn invoke_tool(
        &self,
        name: &str,
        args: serde_json::Value,
        call_id: &str,
    ) -> anyhow::Result<serde_json::Value> {
        let params = serde_json::json!({
            "name": name,
            "args": args,
            "call_id": call_id,
        });
        self.transport
            .request_with_timeout("tool/invoke", params, Duration::from_secs(120))
            .await
    }
}

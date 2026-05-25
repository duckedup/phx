#![allow(dead_code)]

mod commands;
mod config;
mod crash;
mod http;
mod otel;
mod plugin;
mod providers;
mod remote;
mod rpc;
pub mod sdk;
mod session;
pub mod shared;
mod store;
mod tools;
mod tui;
mod worktree;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "phx",
    version,
    about = "A lightweight, fast, minimalistic agent harness"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[arg(long)]
    config: Option<PathBuf>,

    #[arg(long = "plugin-dir", short = 'p')]
    plugin_dirs: Vec<PathBuf>,

    /// Run as headless TCP server.
    #[arg(long, requires = "port")]
    host: Option<String>,

    /// TCP port for server mode.
    #[arg(long, requires = "host")]
    port: Option<u16>,

    /// Connect TUI to a remote phx server. Format: host:port
    #[arg(long, conflicts_with_all = ["host", "port"])]
    remote: Option<String>,
}

#[derive(clap::Subcommand)]
enum Command {
    Tui,
    Rpc,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    crash::install_panic_hook();

    let cli = Cli::parse();
    let cfg = config::loader::load(cli.config.as_deref())?;

    let _otel_handle = otel::init(otel::TelemetryInit {
        otlp_endpoint: None,
        service_name: "phx".into(),
        ring_capacity: 4096,
        log_level: cfg.log_level.clone(),
    });

    if let Some(Command::Rpc) = cli.command {
        let stdin = tokio::io::BufReader::new(tokio::io::stdin());
        let stdout = tokio::io::stdout();
        let stdout = tokio::io::BufWriter::new(stdout);
        rpc::server::run(cfg, stdin, stdout).await?;
        return Ok(());
    }

    if let (Some(host), Some(port)) = (cli.host, cli.port) {
        remote::server::run(cfg, host, port).await?;
        return Ok(());
    }

    let remote_client = match cli.remote.as_deref() {
        Some(endpoint) => {
            let client = remote::client::RemoteClient::connect(endpoint)
                .await
                .map_err(|e| anyhow::anyhow!("failed to connect to {endpoint}: {e}"))?;
            tracing::info!(%endpoint, "connected to remote phx server");
            Some((client, endpoint.to_string()))
        }
        None => None,
    };

    let needs_onboarding = !config::loader::active_provider_usable(&cfg);
    tui::run(cfg, needs_onboarding, cli.plugin_dirs, remote_client).await?;

    Ok(())
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn no_args_runs_default() {
        let cli = Cli::try_parse_from(["phx"]).unwrap();
        assert!(cli.host.is_none());
        assert!(cli.port.is_none());
        assert!(cli.remote.is_none());
    }

    #[test]
    fn host_port_parses() {
        let cli = Cli::try_parse_from(["phx", "--host", "0.0.0.0", "--port", "4200"]).unwrap();
        assert_eq!(cli.host.as_deref(), Some("0.0.0.0"));
        assert_eq!(cli.port, Some(4200));
    }

    #[test]
    fn remote_conflicts_with_host() {
        let result = Cli::try_parse_from(["phx", "--remote", "x:1", "--host", "y", "--port", "2"]);
        assert!(result.is_err());
    }

    #[test]
    fn remote_alone_parses() {
        let cli = Cli::try_parse_from(["phx", "--remote", "127.0.0.1:4200"]).unwrap();
        assert_eq!(cli.remote.as_deref(), Some("127.0.0.1:4200"));
    }

    #[test]
    fn host_without_port_fails() {
        let result = Cli::try_parse_from(["phx", "--host", "0.0.0.0"]);
        assert!(result.is_err());
    }

    #[test]
    fn port_without_host_fails() {
        let result = Cli::try_parse_from(["phx", "--port", "4200"]);
        assert!(result.is_err());
    }
}

#![allow(dead_code)]

mod commands;
mod config;
mod crash;
mod otel;
mod plugin;
mod providers;
mod rpc;
mod session;
mod store;
mod tools;
mod tui;
mod worktree;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "phoenix",
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
        service_name: "phoenix".into(),
        ring_capacity: 4096,
    });

    match cli.command {
        None | Some(Command::Tui) => {
            let needs_onboarding = !config::loader::active_provider_usable(&cfg);
            tui::run(cfg, needs_onboarding, cli.plugin_dirs).await?;
        }
        Some(Command::Rpc) => {
            let stdin = tokio::io::BufReader::new(tokio::io::stdin());
            let stdout = tokio::io::stdout();
            let stdout = tokio::io::BufWriter::new(stdout);
            rpc::server::run(cfg, stdin, stdout).await?;
        }
    }

    Ok(())
}

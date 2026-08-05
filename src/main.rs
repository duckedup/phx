//! phx — spawns isolated coding-agent instances in a monorepo, driven by tmux keybindings.
//!
//! Every subcommand is invoked by a tmux binding, never typed by hand. Arguments arrive
//! through tmux format substitutions. See DESIGN.md.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "phx", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print a `display-menu` spec for the configured harnesses.
    MenuSpec,
    /// Materialize an isolated worktree and open it in a new tmux window.
    New {
        /// Harness key from config.
        #[arg(long)]
        harness: String,
        /// Repo root, supplied by tmux as `#{pane_current_path}`.
        #[arg(long)]
        root: String,
    },
    /// Land the instance owned by a tmux window.
    Done {
        /// Window id, supplied by tmux as `#{window_id}`.
        #[arg(long)]
        window: String,
    },
    /// Discard an instance and its worktree.
    Discard {
        #[arg(long)]
        window: String,
    },
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("PHX_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::MenuSpec => anyhow::bail!("not implemented"),
        Command::New { .. } => anyhow::bail!("not implemented"),
        Command::Done { .. } => anyhow::bail!("not implemented"),
        Command::Discard { .. } => anyhow::bail!("not implemented"),
    }
}

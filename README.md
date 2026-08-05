# phx

A tmux plugin for spawning isolated coding-agent instances in a monorepo.

Press a key, pick a harness (Claude Code, Codex, whatever you configure), and get a fresh
git worktree that is *immediately usable* — dependencies cloned, `.env` and MCP config in
place, ports and container names that don't collide with your other agents. Press another
key to land it and clean up.

phx does not manage panes, draw a UI, or talk to any model API. tmux owns the terminal;
phx owns the environment.

> **Status:** rebuild in progress. The previous agent-harness implementation has been
> removed. See [DESIGN.md](DESIGN.md) for the current design and build sequencing.

## Design

- **No daemon.** Every invocation is transactional and exits.
- **tmux is the state store.** Instance metadata lives in window options; the window id is
  the primary key. No registry file, nothing to desync.
- **The recipe lives in the monorepo.** A `.phx/config.toml` in the repo declares how to
  materialize an isolated instance of itself. phx stays generic.

## Contributing

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2024, stable toolchain)
- [just](https://github.com/casey/just) — command runner

### Setup

```bash
git clone https://github.com/duckedup/phx.git
cd phx
just build
```

### Common Commands

```bash
just build          # Build the binary
just test           # Run all tests
just lint           # Format + clippy
just check          # All quality gates
```

### Workflow

1. Fork the repo and create a feature branch.
2. Make your changes.
3. Run `just check` to verify.
4. Submit a pull request.

## License

[MIT](LICENSE)

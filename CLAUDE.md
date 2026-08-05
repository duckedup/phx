# Agent Instructions

This file provides instructions and context for AI coding agents working on this project.

## Task Tracking

Real work is tracked in **Jira and Linear**. This repo has no local issue tracker — do not
add one, and do not create markdown TODO files. Use your own in-session task list for
scratch work; anything durable belongs in the ticket it came from.

## Build & Test

Always use `just` commands instead of running `cargo` directly.

```bash
just check             # Run all quality gates: lint, build, test, lockfile
just lint              # Run cargo fmt + clippy
just test              # Run cargo test --all-features
just build             # Run cargo build
```

## Architecture Overview

phx is a **tmux plugin** for spawning isolated coding-agent instances in a polyglot
monorepo (Go, Bun, Rust). It is a `phx.tmux` shell shim that binds keys, plus a small Rust
binary those bindings invoke.

Every subcommand is invoked by a tmux keybinding, never typed by hand — arguments arrive
through tmux format substitutions (`#{pane_current_path}`, `#{window_id}`).

Two properties are load-bearing and should not be traded away:

- **No daemon.** Every invocation is transactional and exits. There is no server, no
  socket, no background process, nothing with a lifecycle.
- **tmux is the state store.** Instance metadata lives in tmux window options
  (`@phx_worktree`, `@phx_branch`, `@phx_harness`, `@phx_ports`); the window id is the
  primary key. There is no registry file and no database, so there is nothing to desync.

The core of the tool is *materialization*: bringing up a git worktree that is immediately
usable — APFS `clonefile` for `node_modules` and `target`, copied `.env` and MCP config,
and per-instance env for ports, compose project name, and `sccache`.

See `DESIGN.md` for the full design, open questions, and build sequencing.

### Explicit non-goals

- Not a terminal emulator — tmux owns panes, input, resize, scrollback, detach/reattach.
- Not a sidebar — opensessions owns session list and agent state display.
- Not a model harness — no provider APIs, no token accounting, no RPC, no plugins.
- **No ticket integration.** phx never calls Jira or Linear. The agent inside an instance
  does that through its own MCP servers. phx only provisions access by copying config and
  tokens into the worktree.

## Conventions & Patterns

Simplicity over cleverness.
Readable over complex.
Dont be unsafe.
Code should be extracted for reusability.
Everything that can be reused across modules should be extracted into a specific top-level
module — never duplicate logic across sibling modules. Name modules for what they do, not
generic catch-alls.

## Logging Standard

All logging uses the `tracing` crate with leveled, structured fields. phx is a short-lived
CLI, so logs go to **stderr** — inside a `display-popup` that is exactly where you want
them, and tmux shows them if a binding fails. The subscriber is initialized once in
`main.rs`; never set up a second one.

### Levels

- `tracing::error!` — failures that stop an operation (git failure, missing config, failed clone)
- `tracing::warn!` — recoverable issues (fallback used, optional copy target missing)
- `tracing::info!` — significant lifecycle events (worktree created, window spawned, instance torn down)
- `tracing::debug!` — verbose diagnostics (resolved config, each copied path, command lines)

### Rules

- Always include structured fields, not just a message string:
  `tracing::error!(%branch, %path, code = out.status.code(), "git worktree add failed")`
- Every external command must log: `debug!` before running (with argv and cwd), `error!` on
  non-zero exit (with stderr captured)
- Never use `println!`, `eprintln!`, or `dbg!` for diagnostics — all output goes through
  `tracing`. The one exception is `menu-spec`, whose stdout is consumed by tmux.
- Log level is set by the `PHX_LOG` env var. Default is `info`.

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

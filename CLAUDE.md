# Project Instructions for AI Agents

This file provides instructions and context for AI coding agents working on this project.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:ca08a54f -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for persistent knowledge — do NOT use MEMORY.md files

## Session Completion

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd dolt push
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
<!-- END BEADS INTEGRATION -->


## Build & Test

Always use `just` commands instead of running `cargo` directly.

```bash
just check             # Run all quality gates: lint, build, test, lockfile
just lint              # Run cargo fmt + clippy
just test              # Run cargo test --all-features
just build             # Run cargo build
```

## Architecture Overview

phx is rpc based harness that uses a tui. It should use an event loop to process requests. Themain goals of the harness are observability, flexibilty and usability.

## Conventions & Patterns

Simplicity over cleverness.
Readable over complex.
Dont be unsafe.
Code should be extracted for reusability.
Everything that can be reused across modules should be extracted into a specific top-level module (e.g. `src/http/`, `src/config/`) — never duplicate logic across sibling modules. Name modules for what they do, not generic catch-alls.

## Logging Standard

All logging uses the `tracing` crate with leveled, structured fields. Logs are written to `~/.phx/phx.log` and the in-memory ring buffer (viewable in the TUI observer). The telemetry stack is initialized in `src/otel/mod.rs` — never set up a second subscriber.

### Levels

- `tracing::error!` — failures that stop an operation (HTTP errors, bad API responses, tool failures)
- `tracing::warn!` — recoverable issues (fallback used, config missing, non-critical failure)
- `tracing::info!` — significant lifecycle events (session start/end, plugin loaded, provider response with usage)
- `tracing::debug!` — verbose diagnostics (request URLs, config loading, stream start)

### Rules

- Always include structured fields, not just a message string: `tracing::error!(provider = "claude", %url, %status, response_body = %text, "bad response from API")`
- Every outbound HTTP request must log: `debug!` before send (with URL and provider), `error!` on failure (with URL, status, response body), `debug!` on stream start
- Use spans from `otel::spans` for provider calls, tool execution, and sessions
- Never use `println!`, `eprintln!`, or `dbg!` — all output goes through `tracing`
- Log level is configurable via `"log_level"` in `phx.json` (project `.phx/phx.json` overrides global `~/.phx/phx.json`). `PHX_LOG` env var overrides both. Default is `info`

# `spawn_agent` — Cross-Provider Agent Spawning

> Status: Draft — PHX-3c6
> Author: Austin + Claude
> Date: 2026-05-08

## Problem

phx's DESIGN.md specifies orchestration tools (`spawn_session`, `check_sessions`, `collect_session`, `cancel_session`) that work within a single provider. But the real power move is **cross-provider spawning**: a Claude Opus orchestrator spawning OpenAI o3 workers, Gemini researchers, and local Ollama coders — each on the model best suited to its subtask.

The existing `SessionPool` in `src/session/orchestration.rs` is a stub. It tracks child state but never actually runs a child session. This spec fills that gap and extends it to support heterogeneous model selection.

---

## Design Principles

1. **A tool, not a framework.** `spawn_agent` is a tool the model calls. The harness doesn't decide topology — the model (or plugin) does. This matches DESIGN.md §5.3: the harness never auto-spawns.

2. **Provider is a parameter, not a fixed binding.** Each spawned agent can target any configured provider+model. The orchestrator says "use the `gpt` provider with model `o3`" — the harness resolves credentials and creates the provider instance.

3. **Fully traced.** Every child agent gets its own OTEL span tree — session span, provider spans, tool spans — with parent-child relationships linking back to the orchestrator. All streaming I/O flows through the same tracing pipeline as the main agent. Observability is the contract that makes vibes mode safe (DESIGN.md §11).

4. **Worktree-isolated by default.** Each child agent gets its own git worktree via embedded [worktrunk](https://worktrunk.dev). No file conflicts, no lock contention, no shared mutable filesystem state. Children work in parallel on the same repo without stepping on each other. Results merge back via worktrunk's merge operations.

5. **Plugin-driven, not model-driven.** Orchestration tools are **never** in the main agent's default tool list. The main agent doesn't call `spawn_agent` unless the user explicitly configures it into a session profile, or a plugin/skill triggers it. The normal path is the **orchestrate plugin** (Phase 5, building on Josh's plugin system) which invokes orchestration tools via the bidirectional plugin RPC protocol (`host/tool_call`). The model never sees these tools unless you want it to.

---

## Tool Surface

Five tools. Four match DESIGN.md §5A.1 (with provider awareness and worktree isolation added), plus `merge_agent` for integrating child work back into the parent branch:

**The full lifecycle:** `spawn_agent` creates a worktree + starts the child → the child does its work in isolation → `collect_agent` retrieves the result + diff → `merge_agent` squashes/rebases the child's branch back into the parent → worktree cleaned up.

### `spawn_agent`

Creates a child agent session, starts it running, returns immediately with an ID.

```json
{
  "name": "spawn_agent",
  "description": "Spawn a child agent session on any configured provider. Returns immediately with a session ID. The child runs asynchronously.",
  "parameters": {
    "type": "object",
    "properties": {
      "prompt": {
        "type": "string",
        "description": "The task to give the child agent."
      },
      "provider": {
        "type": "string",
        "description": "Name of a configured provider profile (e.g. 'claude', 'gpt', 'ollama'). Defaults to the orchestrator's own provider."
      },
      "model": {
        "type": "string",
        "description": "Model override. If omitted, uses the provider profile's default model."
      },
      "profile": {
        "type": "string",
        "description": "Session profile name from config (determines tools, system prompt, etc.). Defaults to 'coder'."
      },
      "tools": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Override tool list for this child. If omitted, uses the profile's tools."
      },
      "persist": {
        "type": "boolean",
        "description": "Whether to persist this child session to disk. Default: false (ephemeral)."
      },
      "worktree": {
        "type": "boolean",
        "description": "Whether to create an isolated git worktree for this child. Default: true. Set to false for read-only tasks (research, analysis) that don't modify files."
      },
      "context": {
        "type": "array",
        "items": { "type": "string" },
        "description": "File paths to pre-load into the child's conversation context. The harness reads each file and prepends them before the prompt so the child starts with the relevant code already in context."
      }
    },
    "required": ["prompt"]
  }
}
```

**Returns:**
```json
{
  "session_id": "c-a3f2dd",
  "provider": "gpt",
  "model": "o3",
  "status": "queued",
  "worktree": "/Users/austin/Projects/.phx-worktrees/phx.c-a3f2dd",
  "branch": "phx/agent/c-a3f2dd"
}
```

### `check_agents`

Poll status of child agents. Cheap — just reads from the pool state.

```json
{
  "name": "check_agents",
  "parameters": {
    "type": "object",
    "properties": {
      "ids": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Filter to specific session IDs. Omit for all children."
      }
    }
  }
}
```

**Returns:**
```json
{
  "children": [
    {
      "session_id": "c-01",
      "provider": "gpt",
      "model": "o3",
      "profile": "coder",
      "status": "running",
      "active_tool": "bash",
      "tokens": { "input": 12400, "output": 3200 },
      "elapsed_s": 45.2
    },
    {
      "session_id": "c-02",
      "provider": "claude",
      "model": "claude-sonnet-4-6",
      "profile": "coder",
      "status": "done",
      "tokens": { "input": 8100, "output": 2100 },
      "elapsed_s": 32.1
    }
  ],
  "summary": {
    "total": 2,
    "running": 1,
    "done": 1,
    "queued": 0,
    "error": 0,
    "tokens_total": { "input": 20500, "output": 5300 }
  }
}
```

### `collect_agent`

Retrieve the final output from a completed child. Fails if still running.

```json
{
  "name": "collect_agent",
  "parameters": {
    "type": "object",
    "properties": {
      "session_id": {
        "type": "string",
        "description": "ID of the child session to collect."
      }
    },
    "required": ["session_id"]
  }
}
```

**Returns:**
```json
{
  "session_id": "c-02",
  "status": "done",
  "output": "Implemented the feature. Modified 3 files: ...",
  "tokens": { "input": 8100, "output": 2100 },
  "tool_calls": 5,
  "elapsed_s": 32.1,
  "worktree": {
    "branch": "phx/agent/c-02",
    "files_changed": 3,
    "insertions": 47,
    "deletions": 12,
    "diff_summary": "src/lib.rs | 20 ++++---\nsrc/main.rs | 15 +++\ntests/test.rs | 24 +++++++"
  }
}
```

For error'd children, returns the error message and any partial output.

The `worktree` field is only present when the child ran in a worktree. The orchestrator can review the diff summary before deciding to merge.

### `cancel_agent`

Cancel a running or queued child. Asynchronous — returns immediately.

```json
{
  "name": "cancel_agent",
  "parameters": {
    "type": "object",
    "properties": {
      "session_id": { "type": "string" },
      "reason": { "type": "string" }
    },
    "required": ["session_id"]
  }
}
```

### `merge_agent`

Merge a completed child's worktree branch back into the parent branch. Only available when the child ran in a worktree.

```json
{
  "name": "merge_agent",
  "parameters": {
    "type": "object",
    "properties": {
      "session_id": {
        "type": "string",
        "description": "ID of the completed child session to merge."
      },
      "strategy": {
        "type": "string",
        "enum": ["squash", "rebase", "merge"],
        "description": "How to integrate changes. Default: squash."
      },
      "message": {
        "type": "string",
        "description": "Commit message for the merge. If omitted, auto-generated from the child's work summary."
      },
      "cleanup": {
        "type": "boolean",
        "description": "Remove the worktree and branch after merging. Default: true."
      }
    },
    "required": ["session_id"]
  }
}
```

**Returns:**
```json
{
  "session_id": "c-02",
  "merged": true,
  "strategy": "squash",
  "commit": "a3f2dd1",
  "files_changed": 3,
  "worktree_removed": true
}
```

This uses worktrunk's merge operation under the hood — squash, rebase, or merge with automatic cleanup.

---

## Architecture

### Worktree Isolation (via embedded worktrunk)

phx embeds [worktrunk](https://worktrunk.dev) as a Rust library dependency (not a CLI subprocess). Worktrunk provides git worktree management with a library crate gated behind a `cli` feature — we depend on the library without pulling in terminal UI deps.

```toml
# Cargo.toml
[dependencies]
worktrunk = { version = "0.48", default-features = false }
```

#### Why worktrees, not shared filesystem

When multiple agents write code in parallel, filesystem conflicts are the #1 failure mode:
- Agent A writes `src/lib.rs`, Agent B writes `src/lib.rs` → last write wins, work lost.
- Agent A runs `cargo test` while Agent B is mid-write → build failures from partial state.
- Agent A installs a dep, Agent B's `Cargo.lock` diverges → merge hell.

Git worktrees solve this at the filesystem level. Each worktree is a full checkout of the repo sharing the same `.git` object store. Agents get true isolation with near-zero storage overhead.

#### Worktree lifecycle

```
spawn_agent(prompt, worktree=true)
│
├── 1. Create branch: phx/agent/{child_id}
├── 2. Create worktree via worktrunk::git::Repository
│       path: {project}/../.phx-worktrees/{project}.{child_id}
├── 3. Run child session with cwd = worktree path
├── 4. Child completes → auto-commit changes on its branch
│
├── collect_agent(child_id)
│   └── Returns diff summary (files changed, insertions, deletions)
│
├── merge_agent(child_id, strategy="squash")
│   ├── Squash/rebase/merge child branch → parent branch
│   └── Remove worktree + delete branch (cleanup=true)
│
└── OR cancel_agent(child_id)
    └── Remove worktree + delete branch (no merge)
```

#### Integration layer: `src/worktree.rs`

A thin wrapper around worktrunk's library API, scoped to what phx needs:

```rust
use std::path::{Path, PathBuf};

pub struct WorktreeManager {
    repo_root: PathBuf,
    worktree_base: PathBuf,  // e.g., ../.phx-worktrees/
}

impl WorktreeManager {
    pub fn create(&self, child_id: &str) -> Result<WorktreeInfo, WorktreeError>;
    pub fn remove(&self, child_id: &str, delete_branch: bool) -> Result<(), WorktreeError>;
    pub fn merge(&self, child_id: &str, strategy: MergeStrategy, message: Option<&str>) -> Result<MergeResult, WorktreeError>;
    pub fn diff_summary(&self, child_id: &str) -> Result<DiffSummary, WorktreeError>;
    pub fn list(&self) -> Result<Vec<WorktreeInfo>, WorktreeError>;
}

pub enum MergeStrategy { Squash, Rebase, Merge }
pub struct WorktreeInfo { pub path: PathBuf, pub branch: String, pub child_id: String }
pub struct DiffSummary { pub files_changed: usize, pub insertions: usize, pub deletions: usize, pub summary: String }
pub struct MergeResult { pub commit: String, pub files_changed: usize, pub conflicts: Vec<String> }
```

#### Worktree path convention

```
~/Projects/
├── phx/                    # main repo
├── .phx-worktrees/
│   ├── phx.c-01/           # child c-01's worktree
│   ├── phx.c-02/           # child c-02's worktree
│   └── phx.c-03/           # child c-03's worktree
```

#### When NOT to use worktrees

- **Read-only tasks** (research, analysis, code review) — set `worktree: false`
- **Tasks that don't touch files** (API calls, web search) — set `worktree: false`
- **Non-git projects** — worktrees require git; phx falls back to shared cwd

Default: `worktree: true` for any child with write/edit tools in its profile, `worktree: false` for read-only profiles.

#### Auto-commit on completion

When a child completes in a worktree, phx auto-commits any uncommitted changes on the child's branch. The commit message: `phx: agent {child_id} — {first_line_of_prompt}`.

#### Build cache sharing

Worktrunk supports build cache sharing between worktrees. phx sets this up automatically so `cargo build` in one worktree reuses artifacts from others.

### Provider Resolution

When `spawn_agent` is called with a `provider` parameter:

1. Look up the named provider profile in `Config.providers`.
2. If `model` is specified, clone the profile and override the model field.
3. Call `create_provider(name, &profile)` to get a `Box<dyn Provider>`.
4. The child session uses this provider for all its API calls.

If no provider is specified, the child inherits the orchestrator's provider.

```
Orchestrator (Claude Opus)
├── spawn_agent(prompt="write tests", provider="gpt", model="o3")
│   └── Child runs on OpenAI o3
├── spawn_agent(prompt="research API", provider="gemini")
│   └── Child runs on Gemini (profile default model)
└── spawn_agent(prompt="lint code")
    └── Child inherits orchestrator's Claude Opus
```

### Session Pool (Enhanced)

The existing `SessionPool` stub becomes real:

```rust
pub struct SessionPool {
    children: Arc<Mutex<HashMap<String, ChildHandle>>>,
    config: Arc<Config>,
    store: Arc<SessionStore>,
    worktrees: WorktreeManager,
    project: PathBuf,
    max_concurrent: usize,
    semaphore: Arc<Semaphore>,
}

pub struct ChildHandle {
    pub id: SessionId,
    pub provider_name: String,
    pub model_name: String,
    pub profile_name: String,
    pub prompt: String,
    pub status: ChildStatus,
    pub output: Option<String>,
    pub tokens: Usage,
    pub active_tool: Option<String>,
    pub started_at: Instant,
    pub events_rx: broadcast::Receiver<SessionEvent>,
    pub cancel_tx: Option<oneshot::Sender<()>>,
    pub worktree: Option<WorktreeInfo>,
    pub span: tracing::Span,  // OTEL span for this child
}
```

**Spawn flow:**

1. `spawn_agent` tool is invoked with args.
2. Resolve provider from config (or inherit from parent).
3. Resolve session profile (tools, system prompt).
4. **If `worktree: true`:** call `WorktreeManager::create(child_id)` to create an isolated worktree + branch.
5. Build tool registry for the child.
6. Create `Session::new()` with the resolved profile.
7. **Create OTEL span:** `session_span(child_id)` linked to the orchestrator's span via `follows_from`.
8. Insert `ChildHandle` into pool with status `Queued`.
9. Acquire semaphore permit (or queue if full).
10. Spawn a tokio task **inside the child's span**:
    ```rust
    tokio::spawn(async move {
        let _permit = semaphore.acquire().await;
        let _guard = child_span.enter();
        tracing::info!(provider = %provider_name, model = %model_name, "child agent started");

        child.status = Running;
        session.add_message(Message::user(prompt));
        session.run_with_hooks(&provider, &tools, &store, &project_dir, &skills, hooks).await;

        if let Some(ref wt) = child.worktree {
            auto_commit_worktree(&wt.path, &child.id, &child.prompt);
        }
        child.status = Done;
        child.output = extract_final_output(&session);
        tracing::info!(tokens_in = session.token_input, tokens_out = session.token_output, "child agent completed");
    });
    ```
11. Return `session_id` + worktree info immediately to the orchestrator.

**Cancellation:**

- Each child task holds a `cancel_rx: oneshot::Receiver<()>`.
- `cancel_agent` sends on `cancel_tx`.
- The child's agent loop checks for cancellation at each tool-dispatch boundary.
- On cancellation: transition to `Cancelled`, clean up worktree, emit `tracing::info!("child agent cancelled")`.

### Event Streaming

Each child session has its own `broadcast::Sender<SessionEvent>`. This enables:

- **RPC**: `session.events(child_id)` streams a specific child.
- **Orchestrator tool tracking**: `check_agents` reads the latest `active_tool` from events.
- **Future TUI/dashboard**: consumes child event streams (not in scope for this plan).

New event types:

```rust
pub enum SessionEvent {
    // ... existing variants ...
    ChildSpawned { id: String, provider: String, model: String },
    ChildStatusChanged { id: String, status: ChildStatus },
    ChildCompleted { id: String, success: bool },
}
```

---

## Observability — OTEL Tracing

phx already has OTEL infrastructure (`src/otel/`) with a ring buffer, broadcast subscriber, and span factory functions (`session_span`, `provider_span`, `tool_span`). These span factories exist but **are not yet wired into the agent loop**. This spec requires wiring them up for both the main agent and all spawned children.

### Span Hierarchy

Every child agent gets the same span tree structure as the main agent:

```
session(session_id="orchestrator-s00")
├── provider(provider="anthropic", model="claude-opus-4-7")
│   └── done(tokens_in=500, tokens_out=200)
├── tool(tool="spawn_agent")
│   └── child_spawned(child_id="c-01", provider="openai", model="o3")
├── tool(tool="spawn_agent")
│   └── child_spawned(child_id="c-02", provider="gemini", model="2.5-flash")
│
├── session(session_id="c-01")  [follows_from: orchestrator-s00]
│   ├── provider(provider="openai", model="o3")
│   │   └── done(tokens_in=1200, tokens_out=400)
│   ├── tool(tool="read")
│   │   └── done(file="src/lib.rs", bytes=2048)
│   ├── provider(provider="openai", model="o3")
│   │   └── done(tokens_in=1800, tokens_out=600)
│   ├── tool(tool="write")
│   │   └── done(file="src/lib.rs", bytes=3012)
│   └── tool(tool="bash")
│       └── done(cmd="cargo test", exit_code=0, duration_ms=8300)
│
└── session(session_id="c-02")  [follows_from: orchestrator-s00]
    ├── provider(provider="gemini", model="2.5-flash")
    └── tool(tool="read")
```

### What gets traced

| Event | Span/Event | Fields |
|---|---|---|
| Child session start | `session_span(child_id)` | `session_id`, `parent_session_id`, `provider`, `model`, `profile` |
| Provider API call | `provider_span(provider, model)` | `provider`, `model` |
| Provider response | event on provider span | `tokens_in`, `tokens_out`, `cache_read`, `cache_creation`, `stop_reason` |
| Tool invocation | `tool_span(tool_name)` | `tool`, `call_id` |
| Tool result | event on tool span | `output_bytes`, `is_error`, `duration_ms` |
| Worktree create | event on session span | `branch`, `path` |
| Worktree merge | event on session span | `strategy`, `files_changed`, `commit` |
| Child completion | event on session span | `status`, `tokens_total`, `elapsed_s` |
| Child cancellation | event on session span | `reason` |
| Compaction | event on session span | `removed_count`, `remaining_count` |

### Wiring into the agent loop

The existing `Session::run_with_hooks()` in `src/session/agent_loop.rs` needs three instrumentation points:

```rust
// 1. Session-level span (wraps the entire run)
let session_span = crate::otel::spans::session_span(&self.id.0);
let _session_guard = session_span.enter();

// 2. Provider call span (wraps each provider.send())
let provider_span = crate::otel::spans::provider_span(&self.provider_name, &self.model_name);
let _provider_guard = provider_span.enter();
let stream = provider.send(opts).await?;
// ... on Done event:
tracing::info!(parent: &provider_span, tokens_in = usage.input_tokens, tokens_out = usage.output_tokens);

// 3. Tool call span (wraps each tool.invoke())
let tool_span = crate::otel::spans::tool_span(&tc.name);
let _tool_guard = tool_span.enter();
let result = tool.invoke(args).await;
tracing::info!(parent: &tool_span, output_bytes = result.output.len(), is_error = result.is_error);
```

This applies identically to the main agent and all spawned children — they all run `Session::run_with_hooks()`. Instrument it once, every agent gets full tracing.

### Ring buffer and broadcast

The existing `RingBuffer` in `src/otel/ring.rs` captures all tracing events into an in-memory ring with broadcast subscribers. Child agent spans land in the same ring as the parent — a single subscriber sees the full session tree. The `session_id` field on every span lets consumers filter by child.

### OTLP export (future)

The OTLP HTTP exporter is feature-gated but not wired up. When enabled, the full span tree (orchestrator + all children) exports to any OTLP-compatible backend (Jaeger, Grafana Tempo, Honeycomb). Each child session appears as a linked trace, giving full distributed-tracing-style visibility into multi-agent runs.

---

## Configuration

### Provider Profiles (existing — no changes needed)

```json
{
  "providers": {
    "claude": {
      "kind": "claude",
      "model": "claude-opus-4-7",
      "active": true,
      "auth": { "env": "ANTHROPIC_API_KEY" }
    },
    "gpt": {
      "kind": "openai",
      "model": "o3",
      "auth": { "env": "OPENAI_API_KEY" }
    },
    "gemini": {
      "kind": "gemini",
      "model": "gemini-2.5-flash",
      "auth": { "env": "GEMINI_API_KEY" }
    },
    "local": {
      "kind": "ollama",
      "model": "qwen3:32b",
      "endpoint": "http://localhost:11434"
    }
  }
}
```

### Session Profiles

```json
{
  "sessions": {
    "orchestrator": {
      "extends": "default",
      "tools": ["read", "bash", "spawn_agent", "check_agents", "collect_agent", "merge_agent", "cancel_agent"]
    },
    "coder": {
      "system_prompt_path": "./prompts/coder.md",
      "tools": ["bash", "read", "write", "edit"],
      "persist": false
    },
    "researcher": {
      "system_prompt_path": "./prompts/researcher.md",
      "tools": ["bash", "read"],
      "persist": false
    }
  }
}
```

### Runtime Config

```json
{
  "runtime": {
    "max_concurrent_sessions": "auto"
  }
}
```

`"auto"` = `2 * num_cpus`. Each child session is I/O-bound (waiting on API calls), not CPU-bound.

---

## Implementation Plan

This plan covers core infrastructure only. No TUI work. The orchestration strategy lives in a plugin built on Josh's WASM plugin system.

### Phase 1: OTEL Wiring

**Files:**
- `src/session/agent_loop.rs` — wrap provider calls in `provider_span`, tool calls in `tool_span`, session run in `session_span`
- `src/otel/spans.rs` — extend with `child_session_span(child_id, parent_id)` that sets `follows_from`

**Goal:** The existing main agent gets full OTEL tracing. This is a prerequisite — once the agent loop is instrumented, every child agent inherits it for free because they all run `Session::run_with_hooks()`.

### Phase 2: Worktree Integration Layer

**Files:**
- `Cargo.toml` — add `worktrunk = { version = "0.48", default-features = false }`
- `src/worktree.rs` — `WorktreeManager` wrapper around worktrunk's library API
- Tests for create/remove/merge/list/diff_summary

**Key risk:** Worktrunk's library API is marked unstable. We wrap it behind our own `WorktreeManager` interface so we can absorb breaking changes in one place. If worktrunk's internals prove too coupled to CLI assumptions, fallback is shelling out to `git worktree` directly (~30 lines of `tokio::process::Command`).

### Phase 3: Core Pool + Orchestration Tools

**Files:**
- `src/session/orchestration.rs` — enhance `SessionPool` with real task spawning, worktree lifecycle, OTEL spans
- `src/tools/spawn_agent.rs` — `spawn_agent` tool
- `src/tools/check_agents.rs` — `check_agents` tool
- `src/tools/collect_agent.rs` — `collect_agent` tool (includes diff summary from worktree)
- `src/tools/cancel_agent.rs` — `cancel_agent` tool (includes worktree cleanup)
- `src/tools/merge_agent.rs` — `merge_agent` tool (squash/rebase/merge via WorktreeManager)
- `src/tools/mod.rs` — register orchestration tools in the tool factory table

**Approach:**
```rust
pub struct SpawnAgentTool {
    pool: Arc<SessionPool>,
    config: Arc<Config>,
}

#[async_trait]
impl Tool for SpawnAgentTool {
    fn schema(&self) -> ToolSchema { /* ... */ }

    async fn invoke(&self, args: Value) -> Result<ToolResult, ToolError> {
        let prompt = args["prompt"].as_str().ok_or(/* ... */)?;
        let provider_name = args["provider"].as_str();
        let model_override = args["model"].as_str();
        let profile_name = args["profile"].as_str().unwrap_or("coder");
        let use_worktree = args["worktree"].as_bool().unwrap_or(true);

        let provider = self.pool.resolve_provider(&self.config, provider_name, model_override)?;
        let profile = self.config.sessions.get(profile_name).cloned().unwrap_or_default();
        let worktree = if use_worktree { Some(self.pool.worktrees.create(&child_id)?) } else { None };
        let child_id = self.pool.spawn(provider, profile, prompt, worktree).await;

        Ok(ToolResult::success(serde_json::to_string(&child_id)?))
    }
}
```

### Phase 4: Cancellation + Graceful Shutdown

**Files:**
- `src/session/agent_loop.rs` — check cancellation token between tool calls
- `src/session/orchestration.rs` — `cancel()` sends signal + cleans up worktree; `cancel_all()` for shutdown

**Cancellation contract:**
- The child's agent loop checks `cancel_rx` between tool invocations.
- Running tools (especially `bash`) are killed via process signal.
- The child transitions to `Cancelled` with partial output captured.
- **Worktree cleanup:** cancelled children's worktrees are removed and branches deleted. No debris.
- SIGINT on the orchestrator cancels all children + cleans up all worktrees (DESIGN.md §13.1).

### Phase 5: Bidirectional Plugin RPC

**Files:**
- `src/plugin/transport.rs` — make the transport bidirectional (host listens for plugin → host requests)
- `src/plugin/host_handler.rs` (new) — routes `host/tool_call`, `host/session_events`, `host/get_config` to the appropriate subsystems
- `src/plugin/handle.rs` — wire the host handler into the plugin handle's read loop

**New methods (plugin → host):**
- `host/tool_call(name, args)` — plugin invokes a registered tool (e.g. `spawn_agent`) and gets the result back
- `host/session_events(session_id)` — plugin subscribes to a child's event stream (host sends notifications)
- `host/get_config(keys)` — plugin reads provider/session config

This is the critical piece that makes plugins first-class orchestrators. Without it, plugins can only return context and hope the model calls the right tool.

### Phase 6: Orchestrate Plugin

Build on Josh's plugin system (`src/plugin/`). The orchestrate plugin is a **subprocess plugin** (not WASM — it needs async, long-running state, and its own model calls) that provides orchestration as a mode.

**Plugin manifest** (`plugins/orchestrate/plugin.json`):
```json
{
  "name": "orchestrate",
  "command": "./orchestrate",
  "commands": [
    { "name": "orchestrate", "summary": "Break a task into subtasks and fan out to child agents" }
  ],
  "tools": [],
  "events": {
    "subscribe": ["session_start", "tool_call_end"],
    "can_block": []
  }
}
```

**What the plugin does:**
1. User invokes `/orchestrate <task description>`.
2. Plugin receives the task via `command/execute`.
3. Plugin calls `host/get_config` to discover available providers and models.
4. Plugin makes its own model call to decompose the task into subtasks.
5. Plugin calls `host/tool_call` with `spawn_agent` for each subtask — choosing the best provider/model for each, passing relevant `context` files.
6. Plugin subscribes to child events via `host/session_events`.
7. Plugin polls progress via `host/tool_call` with `check_agents`, handles errors, and merges results via `merge_agent`.
8. Plugin returns a synthesized summary to the user.

**Why subprocess, not WASM:** Josh's WASM plugins run synchronously via `execute(arguments) -> result`. Orchestration is inherently async and long-running — it needs to make model calls, track state across multiple tool-call cycles, and react to events. The subprocess plugin protocol (`src/plugin/handle.rs`) already supports request/response, notifications, and event subscriptions.

**Key invariant:** The main agent never sees orchestration tools. The plugin calls them directly via `host/tool_call`. The model's tool list is unchanged — no `spawn_agent` in sight.

---

## Resolved Questions

1. **~~Child working directories~~** → Resolved: embedded worktrunk provides git worktree isolation per child.

2. **~~Tool conflicts~~** → Resolved: worktrees eliminate filesystem conflicts entirely. Merging happens explicitly via `merge_agent`.

3. **~~Observability~~** → Resolved: OTEL span hierarchy with `session_span` → `provider_span` → `tool_span`, shared ring buffer, `follows_from` links between parent and child spans.

4. **Cost attribution.** Each provider's API key handles its own billing. phx tracks token usage per child for visibility. The aggregate token budget applies across all providers, converted to USD estimates via the cost table in `model_info.rs`.

## Resolved Questions

5. **~~Merge conflicts~~** → The orchestrator model decides. `merge_agent` returns the conflict details (conflicting files, conflict markers) as structured output. The orchestrator can spawn a child to resolve, ask the user, or skip. The child's worktree stays alive until `cleanup: true` or `cancel_agent`, so the orchestrator has a working directory to resolve in.

6. **~~Context sharing~~** → `spawn_agent` accepts a `context` array of file paths pre-loaded into the child's first message. The child still starts in its own worktree (which is a checkout of HEAD), but its conversation begins with the contents of the specified files already in context — no wasted tool calls for the child to `read` files the orchestrator already knows it needs.

    ```json
    {
      "prompt": "Add error handling to the API routes",
      "context": ["src/api/routes.rs", "src/api/errors.rs", "DESIGN.md"],
      "provider": "gpt",
      "model": "o3"
    }
    ```

    The harness reads each file and prepends them as a system-level context block before the user prompt. The child sees the files as if it had already read them.

7. **~~Nested orchestration~~** → Supported from day one. A child whose session profile includes orchestration tools (`spawn_agent`, etc.) can spawn grandchildren, each with their own worktree and OTEL span tree. The span hierarchy just nests deeper — `follows_from` links chain through the tree. The `max_concurrent_sessions` semaphore is global, so grandchildren compete for the same pool slots. This prevents runaway fan-out without a separate config knob.

    ```
    orchestrator (claude-opus)
    ├── c-01 (gpt-o3, coder) — writes feature
    ├── c-02 (claude-sonnet, orchestrator) — handles test suite
    │   ├── c-02-a (ollama/qwen, coder) — unit tests
    │   └── c-02-b (ollama/qwen, coder) — integration tests
    └── c-03 (gemini-flash, researcher) — API docs
    ```

    The only guard: the global semaphore. If you set `max_concurrent_sessions = 8` and the orchestrator spawns 6 children, one of which is itself an orchestrator that tries to spawn 4 grandchildren — only 2 of those grandchildren start immediately; the other 2 queue until slots free up.

## Open Questions

1. **Worktrunk API stability.** Pin to a specific version. If unstable, fall back to shelling out to `git worktree` directly.

---

## Plugin RPC Extensions

The existing subprocess plugin protocol (`src/plugin/transport.rs`) supports `request(method, params)`, `notify(method, params)`, and event hooks. For the orchestrate plugin to be truly powerful, it needs to invoke tools on the host directly — not just return context that hopes the model will call the right tool.

### New RPC methods (host → plugin)

These already exist:
- `initialize` — plugin startup handshake
- `tool/invoke` — host asks plugin to execute one of its registered tools
- `command/execute` — host asks plugin to run a slash command
- `event/hook` — host notifies plugin of a hookable event

### New RPC methods (plugin → host)

These are new — the plugin calls *into* the host:

#### `host/tool_call`

Plugin asks the host to invoke a registered tool and return the result.

```json
{
  "jsonrpc": "2.0",
  "method": "host/tool_call",
  "id": 42,
  "params": {
    "name": "spawn_agent",
    "args": {
      "prompt": "Write unit tests for src/api/routes.rs",
      "provider": "gpt",
      "model": "o3",
      "profile": "coder",
      "context": ["src/api/routes.rs"]
    }
  }
}
```

Response:
```json
{
  "jsonrpc": "2.0",
  "id": 42,
  "result": {
    "output": "{\"session_id\":\"c-01\",\"provider\":\"gpt\",\"model\":\"o3\",\"status\":\"queued\"}",
    "is_error": false
  }
}
```

This lets the orchestrate plugin call `spawn_agent`, `check_agents`, `collect_agent`, `merge_agent`, and `cancel_agent` directly — no model round-trip needed. The plugin drives the orchestration loop itself.

#### `host/session_events`

Plugin subscribes to a child session's event stream.

```json
{
  "jsonrpc": "2.0",
  "method": "host/session_events",
  "id": 43,
  "params": {
    "session_id": "c-01"
  }
}
```

The host begins streaming events as notifications:
```json
{ "jsonrpc": "2.0", "method": "event/child_session", "params": { "session_id": "c-01", "event": "Token", "data": "Writing tests..." } }
{ "jsonrpc": "2.0", "method": "event/child_session", "params": { "session_id": "c-01", "event": "ToolCallStart", "data": { "name": "write", "id": "tc-1" } } }
```

This gives the plugin real-time visibility into child progress without polling `check_agents`.

#### `host/get_config`

Plugin reads host configuration (provider profiles, session profiles).

```json
{
  "jsonrpc": "2.0",
  "method": "host/get_config",
  "id": 44,
  "params": { "keys": ["providers", "sessions"] }
}
```

This lets the orchestrate plugin discover available providers/models and make intelligent decisions about which model to assign to each subtask.

### Transport changes

The current transport (`src/plugin/transport.rs`) assumes the host always initiates requests. For plugin → host calls, the host needs to:

1. **Listen for incoming requests on the plugin's stdout** (currently only reads responses and notifications).
2. **Route** `host/*` methods to a `HostHandler` that has access to the tool registry, session pool, config, and event streams.
3. **Send responses** back on the plugin's stdin.

This is a straightforward extension of the existing JSON-RPC transport — it just becomes bidirectional. The `id` field already handles request/response matching in both directions.

```rust
// src/plugin/host_handler.rs (new)
pub struct HostHandler {
    tools: Arc<ToolRegistry>,
    pool: Arc<SessionPool>,
    config: Arc<Config>,
}

impl HostHandler {
    pub async fn handle(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "host/tool_call" => {
                let name = params["name"].as_str().ok_or("missing tool name")?;
                let args = params["args"].clone();
                let tool = self.tools.get(name).ok_or(format!("unknown tool: {name}"))?;
                let result = tool.invoke(args).await.map_err(|e| e.to_string())?;
                Ok(serde_json::json!({ "output": result.output, "is_error": result.is_error }))
            }
            "host/session_events" => { /* subscribe to child event stream */ }
            "host/get_config" => { /* return requested config sections */ }
            _ => Err(format!("unknown host method: {method}")),
        }
    }
}
```

---

## Naming

DESIGN.md has been updated to use `spawn_agent`/`check_agents`/`collect_agent`/`merge_agent`/`cancel_agent`. The "agent" framing reflects the cross-provider, worktree-isolated nature of spawned children — they're autonomous agents, not just sessions. Internal implementation still uses `Session` as the underlying struct.

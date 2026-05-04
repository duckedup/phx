# Phoenix — Agent Harness Design

> Status: Draft
> Last updated: 2026-05-01

Phoenix is a lightweight, fast, minimalistic agent harness written in Zig. It provides a runtime for agents (LLM-driven or otherwise) that is explicitly *not* opinionated about model providers or subagent topology. The harness gives you the loop, the I/O, the shared state, and the extension points — you bring the rest.

---

## 1. Goals & Non-Goals

### Goals

- **Tiny core.** A single Zig binary with minimal runtime allocation, fast startup, and predictable resource use. The user should not feel the harness — they should feel the agent.
- **Two deployment shapes from one core.** Terminal UI (interactive) and headless RPC (for tooling/CI).
- **Vibes-mode by default.** No permission prompts. The user is the operator; if they don't trust the agent, they shouldn't run it. (See §7.)
- **Total visibility.** The user can see everything every agent is doing, in real time, at every level of the session tree. Observability is the contract that makes vibes mode safe. (See §11.)
- **Configurable orchestration.** Phoenix can be a single agent, or it can drive other agents — but it never spawns a subagent without being told to. Orchestration is a shape the user opts into via config, not a default.
- **Just.** All harness-side commands (build, test, fmt, run, package) flow through a `justfile`. No bespoke shell scripts in the repo root.
- **Extension over feature creep.** Phoenix exposes hooks; you write the plugin. Model-specific transports, telemetry sinks — all live outside the core.

### Non-Goals

- **MCP.** Phoenix does not support MCP. Not in core, not as a plugin. If you need MCP, use a tool that speaks it.
- **Tmux integration.** Multi-pane work happens via Phoenix's own tab system inside the TUI.
- **Auto-spawning subagents.** The harness will never decide on its own to fan out. Sub-agent invocation is a user-configured tool, the same as any other tool.
- **Blanket permission prompts.** No "may I run this command?" UX on every tool call. If a tool is enabled, the agent uses it.
- **Cross-platform parity on day one.** Linux first; macOS second; Windows is a "patches welcome" target.

---

## 2. High-Level Architecture

```
┌──────────────────────────────────────────────┐
│                  Frontends                    │
│                                              │
│   ┌──────────────┐    ┌──────────────┐       │
│   │  TUI         │    │  RPC server  │       │
│   │  (opentui)   │    │  (stdio/uds) │       │
│   └──────┬───────┘    └──────┬───────┘       │
│          │                   │               │
└──────────┼───────────────────┼───────────────┘
           │                   │
           └───────┬───────────┘
                   ▼
                  ┌─────────────────────────┐
                  │      Core runtime       │
                  │                         │
                  │  ┌───────────────────┐  │
                  │  │ Session / agent   │  │
                  │  │ loop              │  │
                  │  └───────────────────┘  │
                  │  ┌───────────────────┐  │
                  │  │ Tool dispatch     │  │
                  │  └───────────────────┘  │
                  │  ┌───────────────────┐  │
                  │  │ Provider adapters │  │
                  │  └───────────────────┘  │
                  └────────────┬────────────┘
                               │
                  ┌────────────┴────────────┐
                  │     Shared store        │
                  │     (todos + state)     │
                  │     beans / dolt-lite   │
                  └─────────────────────────┘
```

The core runtime is a library (`libphoenix`). Each frontend is a thin shell that owns I/O and renders, but delegates all reasoning, state, and tool work to the core.

---

## 3. Language, Build, and Tooling

- **Zig** — primary language. We target the latest stable Zig at the time of merge; version is pinned in `.zigversion` and in the `justfile`.
- **just** — the only command surface. Recipes:
  - `just build` — debug build of all frontends.
  - `just build-release` — `ReleaseSafe` for distribution.
  - `just run` — launch the TUI against the local config.
  - `just rpc` — launch the headless RPC server.
  - `just test` — Zig tests + integration tests.
  - `just fmt` / `just lint` — `zig fmt` + a tiny custom checker.
  - `just bench` — startup time, tool dispatch latency, store throughput.
  - `just package` — produce stripped static binaries for release.
- **No package manager beyond `build.zig.zon`.** Vendor anything we can't pull cleanly.
- **No code generation in the hot path.** Schemas (e.g., for tools, RPC) may be codegen'd at build time, but the runtime touches only the generated artifacts.

---

## 4. Runtime Shapes

Phoenix ships two frontends that both link the same core. They are subcommands of the `phoenix` binary (`phoenix tui`, `phoenix rpc`).

### 4.1 Terminal (TUI) — `opentui`

- Built on **opentui** for rendering. Phoenix owns layout and event routing.
- **Tabs, not panes.** A tab is one session. The user creates tabs explicitly; agents do not. Switching tabs is `Ctrl+Tab` / `Ctrl+Shift+Tab`. A tab can host:
  - A primary agent session.
  - A tool view (e.g., a file diff).
  - A read-only log/observer view.
- **No tmux.** Phoenix does not shell out to tmux, does not require it, does not integrate with it. If users want tmux *outside* Phoenix, that's their call.
- **No permission prompts.** The TUI displays what an agent is doing in real time and lets the user cancel, but it never asks "approve y/n?".
- Status line shows: provider, model, token use, and active tool, if any.

### 4.2 Headless RPC

- Default transport: a **Unix domain socket**, with a **stdio fallback** for child-process embedding (CI, scripts).
- Wire format: **length-prefixed JSON** for v0. We keep it boring on purpose; we'll evaluate MessagePack/CBOR after the surface stabilizes.
- Methods (initial set):
  - `session.create`, `session.destroy`
  - `session.send` (user message → agent)
  - `session.events` (server-streamed events: tokens, tool calls, tool results)
  - `tool.invoke` (out-of-band tool call from a controller)
  - `store.todo.*` (CRUD over the shared todo store)
- Designed so that the TUI is, in principle, just an RPC client of the same server. This keeps the surface honest.

---

## 5. Sessions, Agents, and Orchestration

### 5.1 Session

A **session** is the unit of agent state: message history, tool registry, allocator arena, and a handle to the shared store. Sessions are cheap to create and serialize.

### 5.1A Session persistence

Sessions persist to disk at `~/.phoenix/sessions/{project}/{session_id}/`, where `{project}` is the basename of the working directory the harness was launched from. The session directory contains:

- **`messages.jsonl`** — the conversation history, one JSON object per message. Appended as messages are committed (after each user message, assistant turn, tool call, and tool result). This is the source of truth for session resume.
- **`state.json`** — session metadata: derived display name, provider+model at session start, token accounting, creation/updated timestamps.

**Resume.** `/resume` lists the persisted sessions for the current project and lets the user pick one to rehydrate. Over RPC, `session.list` returns all persisted sessions for the current project; `session.resume(id)` rehydrates one and replaces the active in-memory session.

Rehydration loads the message history into memory. The agent picks up where the last committed message left off. Because Phoenix appends after every committed message rather than at end-of-turn, partial tool-use rounds are recoverable to the last commit point — the only data lost on a hard kill is whatever was still in the streaming token buffer when the signal arrived.

**Lifecycle.** Sessions are persisted by default. `session.destroy` (RPC) or closing a tab with the destroy keybind removes the session directory. Old sessions are never auto-deleted; the user or a cleanup tool manages retention.

**Child sessions (orchestration).** Child sessions spawned by an orchestrator are ephemeral by default — they are not persisted to disk. Their results are captured by `collect_session` and their invocations are recorded in the tool log. If a child session's profile sets `persist = true`, it is persisted like any other session.

```toml
[session.default]
persist = true              # write session state to disk (default)

[session.ephemeral]
persist = false             # in-memory only; lost on exit
```

### 5.2 Agent vs. orchestrator (same machinery)

Phoenix doesn't have separate "agent" and "orchestrator" code paths. There is one loop *per session*. What differs is the **tool set** the session is configured with:

- A plain agent has tools like `read_file`, `run_shell`, `search`.
- An orchestrator has, in addition, orchestration tools (see §5A) that let the model manage child sessions.

This means orchestration is just configuration. There is no daemon, no scheduler, no implicit fan-out. The agent calls orchestration tools because the user gave it those tools; the harness obeys.

### 5.3 No automatic subagent spawn

Restated, because it's load-bearing: the **harness** never spawns a subagent. The **model** can request a spawn *only if* the user enabled orchestration tools in this session's config. Two consequences:

1. A default config is single-agent. You have to opt in to orchestration.
2. Removing the orchestration tools from a config is a hard guarantee that no subagents will appear, regardless of what the model tries to call.

---

## 5A. Async Orchestration

Orchestration in Phoenix is async-first: the orchestrator fans out work to child sessions that run concurrently, polls for completion, and collects results. This section is the complete specification.

### 5A.1 Orchestration tools

An orchestrator session gets four tools:

- **`spawn_session(profile, prompt)`** — creates a child session using a named `[session.*]` profile from config, starts it running on a worker thread, and returns immediately with a `session_id`. Non-blocking. The orchestrator can call this multiple times in a single tool-use turn to fan out work in parallel.
- **`check_sessions(ids?)`** — returns the status of child sessions: `queued`, `running`, `done`, `error`. Optionally filtered by a list of IDs; without args, returns all children of this orchestrator. The orchestrator calls this to decide when to collect.
- **`collect_session(session_id)`** — retrieves the final output of a completed child session. Fails if the child is still running. Once collected, the child's result is cleared from the pool (the audit log in the store retains it).
- **`cancel_session(session_id)`** — cancels a running or queued child session. The child stops at the next tool-dispatch boundary and transitions to `cancelled`. Returns immediately; the cancellation is asynchronous.

### 5A.2 Lifecycle

```
spawn_session()         check_sessions()        collect_session()
      │                       │                        │
      ▼                       ▼                        ▼
 ┌─────────┐  thread avail  ┌─────────┐  session    ┌─────────┐
 │ queued  │───────────────▶│ running │──completes─▶│  done   │──▶ collected
 └─────────┘                └─────────┘             └─────────┘
                               │                       │
                               ▼                       ▼
                            ┌─────────┐            ┌─────────┐
                            │cancelled│            │  error  │
                            └─────────┘            └─────────┘
```

1. **Fan-out.** The orchestrator model calls `spawn_session` N times. Each call returns immediately with a `session_id`. If a thread is available, the child starts immediately (`running`); otherwise it enters a FIFO queue (`queued`).
2. **Poll.** The orchestrator's next model turn calls `check_sessions()`. The harness returns a list of `{id, status, profile, prompt_summary}` objects. The model decides whether to wait (call `check_sessions` again next turn) or collect finished results.
3. **Collect.** For each child in `done` or `error` state, the model calls `collect_session(id)`. The response includes the child's final output (or error message). The model can now synthesize, report to the user, or spawn further children.
4. **Cancel.** The user can cancel the orchestrator session (which cancels all running children), or the orchestrator model can call `cancel_session(id)` if it decides a child is no longer needed. Cancelled children stop at the next tool-dispatch boundary.

### 5A.3 Thread pool

Each child session runs its agent loop on its own OS thread, drawn from a fixed-size pool.

- **Default pool size:** `2 * std.Thread.getCpuCount()`. Child sessions are I/O-bound (waiting on provider API calls), not CPU-bound, so a pool sized to core count would leave threads idle while waiting on network. 2x cores is a reasonable starting default; tune via config based on actual workload.
- **Configurable:** `[runtime] max_concurrent_sessions = "auto"`. `"auto"` resolves to `2 * available_cores` at startup; an explicit integer overrides it.
- **Overflow behavior:** if all pool threads are occupied, `spawn_session` queues the child. The orchestrator still gets a `session_id` back immediately; the child transitions from `queued` to `running` when a thread frees up. Queue order is FIFO.
- **Orchestrator thread:** the orchestrator's own session loop runs on the main thread (TUI) or a dedicated thread (RPC), *not* in the pool. The pool is exclusively for child sessions.

### 5A.4 Session isolation

Sessions are independent: each has its own allocator arena, message history, tool registry, and provider connection. The shared store (§9) is the only cross-session coordination surface, and it serializes writes internally.

A child session:
- **Can** read and write to the shared store (todos, KV scratch, tool log).
- **Cannot** access the orchestrator's message history or tool state.
- **Cannot** spawn its own children (unless the child's profile also includes orchestration tools — nested orchestration is possible but must be explicitly configured).

### 5A.5 Frontend integration

#### TUI: orchestrator dashboard

When an orchestrator spawns children, Phoenix opens an **orchestrator dashboard** tab. This is a live, structured view of the entire session tree — not a raw output stream.

The dashboard is a table, one row per child session:

```
 ID       PROFILE   STATUS    TOOL          TOKENS   LAST OUTPUT
 c-01     coder     running   write_file    12.4k    writing src/core/dispatch.zig...
 c-02     coder     running   run_shell     8.1k     $ zig build test
 c-03     coder     done      —             6.2k     ✓ completed (3 files changed)
 c-04     coder     error     —             4.0k     ✗ provider 429 after retries
 c-05     coder     queued    —             0        waiting for thread
```

**Columns:** session ID, profile name, status (`queued`/`running`/`done`/`error`/`cancelled`), active tool (if running), cumulative token use, last line of output (truncated to fit).

**Aggregate status line** at the top: `5 children: 2 running, 1 done, 1 error, 1 queued | 30.7k tokens total`

**Interactions:**
- **Select a row and press Enter** to drill into that child's full streaming output in a new tab. The tab is read-only — you're observing, not interacting.
- **Press `c`** on a selected row to cancel that child session.
- **Press `q`** to return to the orchestrator's own conversation tab (where the orchestrator model's reasoning and tool calls are displayed as normal).

The dashboard updates in real time. Rows reorder by status: running first, then queued, then done/error/cancelled. Token counts tick up as provider responses stream in.

**No automatic tab creation.** Children do not create tabs on their own. The dashboard is the default view for orchestration. The user explicitly drills in when they want to see a specific child's output. This keeps the tab bar manageable even with 20+ children.

#### RPC

`session.events` for the orchestrator emits `child_spawned`, `child_status_changed`, and `child_completed` events in addition to the orchestrator's own token stream. RPC clients can also call `session.events` on a child session ID directly to stream its output. The dashboard's data model — child list with status, tool, tokens, last output — is available via `session.children` as a single RPC call for clients that want to build their own dashboard UI.

### 5A.6 Example config

```toml
[runtime]
max_concurrent_sessions = "auto"   # defaults to 2x available cores

[session.coder]
system_prompt_path = "./prompts/coder.md"
tools = ["read_file", "write_file", "run_shell", "search"]

[session.orchestrator]
extends = "default"
tools = ["read_file", "search", "spawn_session", "check_sessions", "collect_session", "cancel_session"]
```

A user on an 8-core machine runs the orchestrator. The model breaks a task into 5 subtasks, calls `spawn_session("coder", ...)` five times. All five start immediately (5 < 8 cores). The orchestrator polls with `check_sessions()`, collects results as they finish, and synthesizes a final response.

---

## 6. Configuration

Config is layered, lowest to highest precedence:

1. Built-in defaults (compiled in).
2. `~/.config/phoenix/config.toml` — user defaults.
3. `./.phoenix/config.toml` — project-local.
4. `--config` CLI flag and env vars.
5. Per-session overrides at session creation time (via TUI menu or RPC arg).

Format: **TOML**. Reasoning: human-editable, comment-friendly, and unambiguous about strings vs. lists, which JSON5 and YAML both fumble.

Sketch:

```toml
[runtime]
mode = "tui"           # "tui" | "rpc"
log_level = "info"
max_concurrent_sessions = "auto"  # "auto" = 2x available cores, or an explicit int

[provider.default]
kind = "claude"            # "claude" | "openai" | "ollama" | "llamacpp"
model = "claude-opus-4-7"
# api_key sourced from env; never written in config

[provider.gpt]
kind = "openai"
model = "gpt-5"
# base_url override supported for OpenAI-compatible gateways

[provider.local]
kind = "ollama"
model = "qwen3:32b"
endpoint = "http://localhost:11434"

[provider.raw]
kind = "llamacpp"
endpoint = "http://localhost:8080"

[session.default]
system_prompt_path = "./prompts/system.md"
tools = ["read_file", "write_file", "run_shell", "search"]

[session.orchestrator]
extends = "default"
tools = ["read_file", "search", "spawn_session", "check_sessions", "collect_session", "cancel_session"]

[store]
backend = "beans"          # "beans" | "doltlite" | "memory"
path = "./.phoenix/store"

[plugins]
load = []   # dlopen shared objects; see §8.1
```

Reload semantics: the TUI watches its config file and hot-reloads non-structural changes (prompts, tool lists). Structural changes (provider, store backend) require restart and Phoenix says so explicitly.

---

## 7. Permissions: Vibes Only

Phoenix does not prompt for tool permission. Ever. The philosophy:

- **Allowlist at config time.** The set of tools a session may call is fixed when the session starts. There is no "elevate" path mid-session — restart with a different config. If a tool is in the session's tool list, the agent can call it unconditionally.
- **Cancel, don't gate.** The TUI surfaces a live "currently running" indicator with a cancel keybind. RPC clients can call `session.cancel`. This is the user's lever — watch what the agent is doing, and stop it if you don't like it.
- **Audit, always.** Every tool invocation is appended to the shared store with timestamp, args, and result. No prompts does not mean no record.
- **Isolation is the OS's job.** If you need hard guardrails — if the consequence of a bad tool call is data loss, credential exposure, or production impact — run Phoenix inside a container, VM, or namespace with filesystem and network restrictions enforced by the OS. The harness does not pretend to be a security boundary. Pattern-matching on serialized arguments cannot reason about shell semantics, command equivalence, or intent. Don't build safety theater into the agent loop; use real isolation where it matters.

If you don't trust the agent with a tool, don't give it the tool. If you give it the tool, let it work.

---

## 8. Extensibility

Two extension surfaces, plus a plugin loader for binary-only distribution.

### 8.1 Tools

A tool is a Zig struct with either a one-shot or streaming invoke function:

```zig
const Tool = struct {
    name: []const u8,
    schema: []const u8,           // JSON schema for args
    invoke: *const fn (ctx: *Ctx, args: Value) anyerror!Value,
    invoke_stream: ?*const fn (ctx: *Ctx, args: Value) anyerror!ChunkIterator,
    max_output_bytes: usize,      // truncation cap; default 512 KiB
};

const ChunkIterator = struct {
    next: *const fn (self: *ChunkIterator) anyerror!?[]const u8,
};
```

Most tools implement `invoke` (one-shot, returns a complete result). Tools that produce large or incremental output — `run_shell` is the primary case — implement `invoke_stream` instead. The session loop consumes the `ChunkIterator`, forwarding chunks to the frontend in real time (TUI renders lines as they arrive; RPC emits `tool_chunk` events). When the tool completes, the accumulated output is committed to the message history.

**Truncation.** If a tool's output (one-shot or accumulated stream) exceeds `max_output_bytes`, Phoenix truncates it and appends a `[truncated at 512 KiB]` marker. The full output is still available in the tool log for audit, but the truncated version is what enters the conversation history and counts against the context budget. This prevents a single `find /` or verbose build log from consuming the entire context window.

Tools are registered at session start. Built-in tools live in `src/tools/`.

**Third-party tools are primarily build-time Zig modules.** The intended extension path is: write a Zig file that exports a `Tool`, add it as a dependency in `build.zig.zon`, and list it in the build configuration. This preserves Zig's strengths — comptime validation, error sets, tagged unions, static analysis — across the extension boundary. There is no C ABI to maintain, no vtable to version, and the tool is compiled and optimized alongside the core.

**`dlopen` is the escape hatch, not the primary path.** For cases where source isn't available (proprietary tools, binary-only distribution), Phoenix supports loading shared objects at runtime from `[plugins].load`. Plugins loaded this way cross a C-ABI boundary: they use a C-compatible vtable, cannot use Zig-specific types in the interface, and are unsandboxed. This is a deliberate trade-off for the small number of tools that genuinely need binary distribution.

### 8.2 Providers

A **provider** turns a session's message list into a stream of events (tokens, tool calls, completions). Providers implement an adapter interface and live in `src/providers/`.

```zig
const Provider = struct {
    send: *const fn (
        messages: []const Message,
        tools: []const Tool,
        config: ProviderConfig,
    ) anyerror!EventIterator,
};
```

The adapter takes the conversation history and tool schemas, makes the provider-specific API call, and returns an iterator that yields Phoenix's internal event types (`token`, `tool_call`, `tool_result`, `done`, `error`). The session loop consumes this iterator and stays provider-agnostic.

Day-one providers (shipped in core):

- **`claude`** — Anthropic Messages API. Native tool-use schema; primary target for the Opus / Sonnet / Haiku 4.x family. Streaming over SSE.
- **`openai`** — OpenAI Chat Completions / Responses API. Used for GPT-class models and any OpenAI-compatible endpoint that follows the same wire format (so self-hosted gateways, Azure OpenAI, etc., flow through this adapter with a base-URL override).
- **`ollama`** — Ollama REST API. For locally-hosted models managed by Ollama. Streaming over its native JSON stream. Ollama's API is OpenAI-compatible in many cases, but a dedicated adapter handles its model management endpoints (`/api/tags`, `/api/pull`) and its specific streaming format without relying on compatibility shims.
- **`llamacpp`** — Direct HTTP interface to a llama.cpp server (`--server` mode). For users running raw llama.cpp without Ollama's management layer. Supports its native completion and chat endpoints with grammar-constrained generation for tool-use schemas.

Other providers (Bedrock, Vertex, etc.) are build-time Zig modules following the same `Provider` interface.

### 8.3 Opencode bridge — a tool, not a provider

[Opencode](https://opencode.ai) integration is a **tool**, not a provider adapter. A provider's job is to turn messages into a token stream from a single model. Delegating a turn to another *agent harness* is a fundamentally different operation — it involves the other harness's own tool dispatch, context management, and session state.

The `opencode_delegate` tool:

- Takes a prompt and an opencode session config.
- Spawns (or connects to) an opencode process.
- Streams opencode's output back as the tool result.
- Returns the final output to the calling Phoenix session.

This fits naturally as a tool the orchestrator (or any session) can call. It avoids forcing a harness-to-harness delegation into the provider interface, where it would create impedance mismatches around tool dispatch, cancellation, and event normalization.

```toml
[session.default]
tools = ["read_file", "write_file", "run_shell", "search", "opencode_delegate"]

[tools.opencode_delegate]
endpoint = "http://localhost:4096"
```

---

## 9. Shared Store (Todos & State)

Multiple sessions need to coordinate without stepping on each other. The shared store is the only sanctioned coordination mechanism.

### 9.1 Surface

- **Todos** — id, title, body, status (`open`/`in_progress`/`done`/`cancelled`), assignee (session id), tags, created/updated.
- **Tool log** — append-only audit of every tool call across sessions.
- **KV scratch** — small typed key-value store for session metadata (last working dir, last branch, etc.).

### 9.2 Backend choice

Two candidates, both single-process / embedded — we are not running a separate database server.

- **Beans** — append-log + index, optimized for high write throughput and crash safety. Good fit if we lean on the tool log being the source of truth.
- **Dolt-lite (custom)** — versioned, branchable rows. Heavier to build but gives us cheap snapshots ("what did the orchestrator see when it spawned this session?") and conflict-aware merges between concurrent sessions.

We will start with Beans (or memory, for tests), behind a `Store` interface, and revisit Dolt-lite once we have real concurrent-session traffic and can measure whether branchable history is worth the build cost.

### 9.3 Concurrency

- One writer process at a time owns the store directory; others connect via the RPC server. The TUI either *is* that owner (when run standalone) or *is* a client (when an RPC server is already up).
- Reads are lock-free where the backend allows; writes are serialized.

---

## 10. Compaction (Context Window Management)

Every long-running session will eventually approach the model's context limit. Phoenix handles this automatically via **compaction** — reducing the conversation history to fit within budget while preserving the information the agent needs to continue working.

### 10.1 Mechanics (core-owned)

The session tracks token usage against the active model's context budget. When usage crosses a configurable threshold, Phoenix triggers a compaction pass:

1. **Partition.** The message history is split into three regions:
   - **Pinned head** — the system prompt. Always preserved verbatim.
   - **Evictable middle** — older turns between the system prompt and the preserved tail.
   - **Preserved tail** — the last N user/assistant exchanges (configurable). Always kept intact so the agent has immediate working context.
2. **Compact.** The evictable middle is passed to a `Compactor` (see §10.2). The compactor returns a shorter sequence of messages — typically a single summary message — that replaces the evicted turns.
3. **Splice.** The session history becomes: pinned head + compacted middle + preserved tail. The session continues with no interruption to the agent loop.
4. **Log.** The compaction event is recorded in the store: timestamp, turns evicted, tokens before/after, compactor used. The original evicted messages are not retained in memory but are recoverable from the tool log if the store backend supports it.

Compaction is **visible to the user**. The TUI displays "compacted 43 turns into summary" in the status area. The summary itself is viewable by scrolling up. RPC emits a `session.compacted` event with the same metadata.

### 10.2 Compactor interface

The compaction *strategy* is pluggable. The core defines the interface; implementations decide what to keep and how to summarize.

```zig
const Compactor = struct {
    compact: *const fn (
        system_prompt: []const Message,
        history: []const Message,
        budget_tokens: usize,
    ) anyerror![]const Message,
};
```

The compactor receives the evictable history and a token budget for its output. It returns a replacement message sequence that fits within that budget. The core handles everything else — threshold detection, partitioning, splicing, logging.

### 10.3 Built-in compactor

One compactor ships in core:

- **`truncate`** — drops the evictable middle entirely, keeping only the pinned head and preserved tail. Zero cost, no API call, fully deterministic. The agent can re-read files or re-run searches from disk if it needs detail it lost.

### 10.4 Extension compactors (not in core)

The `Compactor` interface is the extension point for more sophisticated strategies. Custom compactors are registered the same way as custom tools — as Zig modules at build time, or via `dlopen` plugins at runtime.

### 10.5 Pinning

Messages can be marked `pinned` by the user (TUI command or RPC call) or by the agent via a `pin_message` tool (if enabled). Pinned messages are moved to the pinned-head region and survive compaction indefinitely. Use case: a critical constraint stated early in the session ("never modify the migration files") that would otherwise be evicted once the conversation grows long enough.

### 10.6 Token budgets

Compaction manages the context window. Token budgets manage *spend*. They are separate concerns with separate config.

A session tracks cumulative tokens sent and received across all provider calls. When a budget threshold is crossed, Phoenix acts:

- **Warn threshold.** The TUI displays a persistent warning in the status line ("session at 80% of token budget"). RPC emits a `session.budget_warning` event. The session continues.
- **Hard cap.** The session pauses. The TUI shows "token budget exhausted" and offers the user a choice: increase the budget, or end the session. RPC emits `session.budget_exhausted`; the client must call `session.setBudget` with a new limit or `session.destroy` before the loop resumes. The agent is never silently cut off — the user always decides.

For orchestrator sessions, the budget applies to the **aggregate** across the orchestrator and all its children. Each child's token use is charged to the parent's budget. This prevents an orchestrator from evading a budget by fanning out to many children.

```toml
[session.default]
compaction = "truncate"        # "truncate" | custom registered name
compaction_threshold = 0.8     # trigger at 80% of model context
compaction_tail_turns = 3      # always preserve last N user/assistant exchanges
token_budget = "unlimited"     # max tokens (input + output) per session; int or "unlimited"
token_budget_warn = 0.8        # warn at 80% of budget

[session.expensive-task]
extends = "default"
token_budget = 1000000         # hard cap at 1M tokens
```

Local providers (`ollama`, `llamacpp`) don't have per-token costs, but the budget still applies as a runaway-prevention mechanism — a local model stuck in a loop is burning GPU time and wall-clock time, not dollars, but the user still wants a kill switch.

---

## 11. Observability

Observability is the point of Phoenix. The harness runs in vibes mode — no permission gates, no safety prompts — because the user can *see everything*. That contract only holds if the observability surface is comprehensive, real-time, and usable at scale (20 concurrent children, not just 2).

### 11.1 Live visibility (TUI)

**Single-session.** The TUI shows streaming token output, active tool name + arguments in the status line, and tool results inline. This is table stakes.

**Orchestration.** The orchestrator dashboard (§5A.5) is the primary observability surface during fan-out. It answers the three questions the user always has during orchestration:

1. **What's happening right now?** Active tool per child, last output line, live token counters.
2. **Is anything stuck?** Status column + wall-clock duration since last status change. A child that's been `running` for 5 minutes with no tool activity is visually distinct from one that just started.
3. **How much is this costing me?** Per-child and aggregate token counts, updated in real time. The aggregate rolls up to the orchestrator's token budget (§10.6).

**Drill-down.** Selecting a child in the dashboard opens a read-only tab showing its full streaming output — every token, every tool call and result, same as watching a single-session agent. The user can flip between the dashboard (macro view) and a child tab (micro view) with `q` / Enter.

### 11.2 Session tree (structured audit)

Every tool invocation in the tool log carries a `session_id` and a `parent_session_id` (null for the root orchestrator). This creates a **session tree** — a structured trace of the full fan-out.

`phoenix trace <session_id>` reconstructs and displays the tree:

```
orchestrator (s-00)  [42.1k tokens, 3m12s]
├── c-01 coder       [12.4k tokens, 1m45s]  ✓ done
│   ├── read_file src/core/session.zig       [0.2s]
│   ├── write_file src/core/session.zig      [0.1s]
│   └── run_shell zig build test             [8.3s]
├── c-02 coder       [8.1k tokens, 2m01s]   ✓ done
│   ├── read_file src/tools/search.zig       [0.1s]
│   └── run_shell zig build test             [12.1s]
├── c-03 coder       [6.2k tokens, 0m58s]   ✓ done
├── c-04 coder       [4.0k tokens, 0m32s]   ✗ error: provider 429
└── c-05 coder       [0 tokens, 0m00s]       cancelled (never started)
```

Flags:
- `--flat` — skip the tree, dump every tool invocation in chronological order across all sessions.
- `--session <id>` — show only one child's tool log.
- `--json` — machine-readable output for piping into other tools.

### 11.3 Structured logs

Structured logs to stderr in JSONL. Every log line carries `session_id`, `parent_session_id`, timestamp, and event type. The TUI renders a pretty-printed subset; the raw JSONL is the full record.

Event types: `tool_call`, `tool_result`, `tool_error`, `tool_timeout`, `provider_request`, `provider_response`, `provider_retry`, `child_spawned`, `child_status_changed`, `child_completed`, `compaction`, `budget_warning`, `budget_exhausted`.

### 11.4 Metrics

Token counts per provider call, tool latency, store write latency, child session lifetime, queue wait time. Exported on demand via RPC (`metrics.snapshot`); no built-in Prometheus endpoint (plugin territory).

---

## 12. Error Recovery

Phoenix is "vibes mode" — no permission prompts, no hand-holding. That only works if the harness handles failures without user intervention. There are four failure domains.

### 12.1 Provider failures

- **Transient (429, 500, 502, 503, connection timeout).** Retry with exponential backoff + jitter. Default: 3 retries, base delay 1s, max delay 30s. The TUI shows "retrying provider call (attempt 2/3)." After exhausting retries, the error surfaces as a system message to the model: "Provider call failed after 3 attempts: [error]." The model decides whether to retry, adjust, or report the failure to the user.
- **Permanent (401, 403, malformed request).** No retry. For auth errors (401/403), surface to the *user* directly — the model cannot fix a bad API key. For malformed requests, surface to the model as a tool error so it can adjust.
- **Mid-stream failure (connection drops during a streamed response).** Discard the partial response — partial tokens are never shown to the user or committed to history. Retry the full provider call. If the same call fails mid-stream twice, surface the error.

```toml
[provider.default]
max_retries = 3
retry_base_delay_ms = 1000
retry_max_delay_ms = 30000
request_timeout_ms = 120000
```

### 12.2 Tool failures

- **Timeout.** Every tool invocation has a deadline. Default: 120s for `run_shell`, 30s for all others, configurable per-tool. On timeout, the running process is killed (SIGTERM, then SIGKILL after 5s). The model receives a tool error with the partial output captured so far: "Tool timed out after 120s. Partial output: [...]".
- **Crash / non-zero exit.** Return stderr + exit code to the model as a tool error. Models are generally good at interpreting these and adjusting.
- **Built-in tool hang.** If a non-shell tool (e.g., a store operation or provider call inside an orchestration tool) blocks past its deadline, the session loop cancels it via the allocator arena — deallocating the arena invalidates all the tool's in-flight memory. This is Zig's advantage: arena-scoped cleanup is reliable and total.

```toml
[tools.run_shell]
timeout_ms = 120000

[tools.default]
timeout_ms = 30000
kill_grace_period_ms = 5000
```

### 12.3 Store failures

- **Audit log writes are best-effort.** If a store write fails, log the failure to stderr, increment a metric counter, and continue. The session does not halt because the audit system is unavailable — audit is an observer, not a dependency.
- **Todo / KV writes retry once**, then surface as a tool error to the model. The model decides whether the write was important enough to retry or work around.

### 12.4 Child session failures (orchestration)

A child session that errors out transitions to `error` state with a captured error message. The orchestrator's `collect_session` call returns the error. The orchestrator model decides what to do — spawn a replacement, skip the subtask, or report to the user. The harness never auto-retries a child session; that decision belongs to the orchestrator model.

---

## 13. Security Posture

- No network calls except those a provider or tool makes explicitly.
- Secrets (API keys) are read from env or from a user-specified keychain command; never from config files committed to repos.
- Plugins are unsandboxed. Loading a plugin is equivalent to running a binary. We document this loudly; we do not pretend otherwise.

### 13.1 Signal handling

The TUI and RPC frontends install signal handlers.

- **`SIGINT` (Ctrl+C).** First signal cancels the active tool invocation (equivalent to the TUI cancel keybind). Second signal within 2s initiates graceful shutdown: cancel all running child sessions, flush the store, exit. Third signal forces immediate exit.
- **`SIGTERM`.** Graceful shutdown: cancel all sessions, flush store, exit. No second-chance escalation — the process manager can follow up with `SIGKILL` if needed.
- **`SIGPIPE`.** Ignored. Provider connections and RPC pipes can break at any time; the relevant I/O call returns an error, which the retry logic (§12) handles.
- **`SIGHUP`.** TUI: trigger config hot-reload (same as file-watch). RPC: ignored (daemon convention; systemd sends `SIGTERM` for shutdown).

---

## 14. Repository Layout (proposed)

```
phoenix/
├── DESIGN.md              # this file
├── README.md
├── justfile
├── build.zig
├── build.zig.zon
├── .zigversion
├── src/
│   ├── core/              # session loop, dispatch
│   ├── providers/         # claude, openai, ollama, llamacpp
│   ├── tools/             # built-in tools
│   ├── store/             # beans backend, store interface
│   ├── tui/               # opentui frontend
│   ├── rpc/               # RPC server + client
│   └── main.zig           # subcommand dispatch (tui | rpc)
├── plugins/               # example plugins
├── tests/
│   ├── unit/
│   └── integration/
└── docs/
    ├── config.md
    └── extending.md
```

---

## 15. Roadmap (rough)

- **M0 — Skeleton.** `build.zig`, `justfile`, hello-world TUI, hello-world RPC, store interface stub, `claude` provider, two tools (`read_file`, `run_shell`).
- **M0.5 — Provider parity.** Add `openai`, `ollama`, and `llamacpp` provider adapters; share the event-normalization layer; integration tests against a recorded fixture per provider.
- **M1 — Sessions & loop.** Real session loop, tool dispatch, audit log to Beans, cancel semantics, token accounting, compaction with `truncate` and `summarize` compactors, session persistence and resume, token budgets, streaming tool output.
- **M2 — Tabs & orchestration.** TUI tabs; orchestration tools (`spawn_session`, `check_sessions`, `collect_session`, `cancel_session`); thread pool (`2 * getCpuCount()` default); multi-session shared store.
- **M3 — Plugin loader.** `dlopen` path for binary-only tool distribution.
- **M4 — Polish.** Config hot-reload, `phoenix trace`, benchmarks, packaging.

Each milestone ends with: `just test` green, `just bench` recorded, docs updated.

---

## 16. Open Questions

- **Beans vs. Dolt-lite — when do we revisit?** Probably once we have two real workloads driving the store. Until then, pick the cheaper option.
- **Streaming format on RPC.** JSON for v0 is fine, but we should profile before committing. If frame sizes get large (image attachments, big diffs), we move to length-prefixed CBOR.
- **Provider auth UX.** Without permission prompts, how do we handle a missing API key gracefully? Probably a startup-time check with a clear error, never a mid-session interactive flow.
- **Plugin ABI stability.** Pinning a C ABI early constrains us; not pinning it means plugins break on every minor. Likely answer: declare the ABI unstable until M3 and version it from there.
- **Compaction token accounting accuracy.** Exact token counts require a tokenizer per provider. For v0 we can estimate (chars / 4), but this drifts. Do we ship a tokenizer, call the provider's count endpoint, or live with estimates? Likely: estimates for threshold triggering, exact counts for the status line via provider response headers.

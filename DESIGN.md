# Phoenix — Agent Harness Design

> Status: Draft
> Last updated: 2026-04-29

Phoenix is a lightweight, fast, minimalistic agent harness written in Zig. It provides a runtime for agents (LLM-driven or otherwise) that is explicitly *not* opinionated about model providers, MCP, or subagent topology. The harness gives you the loop, the I/O, the shared state, and the extension points — you bring the rest.

---

## 1. Goals & Non-Goals

### Goals

- **Tiny core.** A single Zig binary with minimal runtime allocation, fast startup, and predictable resource use. The user should not feel the harness — they should feel the agent.
- **Three deployment shapes from one core.** Terminal UI (interactive), headless RPC (for tooling/CI), and an embeddable Zig library (drop into a Zig web service or other long-running Zig host).
- **Vibes-mode by default.** No permission prompts. The user is the operator; if they don't trust the agent, they shouldn't run it. (See §8.)
- **Configurable orchestration.** Phoenix can be a single agent, or it can drive other agents — but it never spawns a subagent without being told to. Orchestration is a shape the user opts into via config, not a default.
- **Just.** All harness-side commands (build, test, fmt, run, package) flow through a `justfile`. No bespoke shell scripts in the repo root.
- **Extension over feature creep.** Phoenix exposes hooks; you write the plugin. MCP, model-specific transports, telemetry sinks — all live outside the core.

### Non-Goals

- **MCP in core.** MCP support is a plugin, not a built-in. The core knows nothing about it.
- **Tmux integration.** Multi-pane work happens via Phoenix's own tab system inside the TUI.
- **Auto-spawning subagents.** The harness will never decide on its own to fan out. Sub-agent invocation is a user-configured tool, the same as any other tool.
- **Blanket permission prompts.** No "may I run this command?" UX on every tool call. If a tool is enabled and not denied, the agent uses it. (The deny list — §8 — is an opt-in gate for specific destructive actions, not a general permission system.)
- **Cross-platform parity on day one.** Linux first; macOS second; Windows is a "patches welcome" target.

---

## 2. High-Level Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                          Frontends                               │
│                                                                  │
│   ┌──────────────┐    ┌──────────────┐    ┌──────────────────┐   │
│   │  TUI         │    │  RPC server  │    │  Embedded (lib)  │   │
│   │  (opentui)   │    │  (stdio/uds) │    │  Zig web service │   │
│   └──────┬───────┘    └──────┬───────┘    └────────┬─────────┘   │
│          │                   │                     │             │
└──────────┼───────────────────┼─────────────────────┼─────────────┘
           │                   │                     │
           └───────────────────┼─────────────────────┘
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
                  │  │ Plan mode         │  │
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

Phoenix ships three frontends that all link the same core. The TUI and RPC frontends are subcommands of the `phoenix` binary (`phoenix tui`, `phoenix rpc`). The embeddable frontend is a Zig module that another Zig program — typically a web service — imports and drives directly.

### 4.1 Terminal (TUI) — `opentui`

- Built on **opentui** for rendering. Phoenix owns layout and event routing.
- **Tabs, not panes.** A tab is one session. The user creates tabs explicitly; agents do not. Switching tabs is `Ctrl+Tab` / `Ctrl+Shift+Tab`. A tab can host:
  - A primary agent session.
  - A tool view (e.g., a file diff, a plan).
  - A read-only log/observer view.
- **No tmux.** Phoenix does not shell out to tmux, does not require it, does not integrate with it. If users want tmux *outside* Phoenix, that's their call.
- **No blanket permission prompts.** The TUI may *display* what an agent is doing in real time (and let the user cancel), but it never asks "approve y/n?" unless the invocation matches a deny rule (§8).
- Status line shows: provider, model, token use, `[plan]` indicator (if in plan mode), and active tool, if any.

### 4.2 Headless RPC

- Default transport: a **Unix domain socket**, with a **stdio fallback** for child-process embedding (CI, scripts).
- Wire format: **length-prefixed JSON** for v0. We keep it boring on purpose; we'll evaluate MessagePack/CBOR after the surface stabilizes.
- Methods (initial set):
  - `session.create`, `session.destroy`
  - `session.send` (user message → agent)
  - `session.events` (server-streamed events: tokens, tool calls, tool results, plan transitions)
  - `tool.invoke` (out-of-band tool call from a controller)
  - `plan.enter`, `plan.accept`, `plan.reject`
  - `store.todo.*` (CRUD over the shared todo store)
- Designed so that the TUI is, in principle, just an RPC client of the same server. This keeps the surface honest.

### 4.3 Embeddable Library (Zig)

The embeddable target is a **Zig module** — not a C ABI, not a FFI surface. It is intended to be imported by a Zig host program, most commonly a Zig web service that wants to expose agent capability behind its own HTTP/WebSocket endpoints.

- **Distribution:** consumed via `build.zig.zon` as a Zig dependency (`@import("phoenix")`), or vendored. There is no `libphoenix.a` / `phoenix.h` deliverable; the API is Zig-native and free to use Zig-only types (tagged unions, error sets, slices, comptime).
- **Memory model:** caller-provided `std.mem.Allocator`. Phoenix does not install signal handlers, does not start threads it doesn't own, does not touch global state. A web service can run many Phoenix sessions concurrently from its own request-handling threads or async tasks.
- **No I/O assumptions.** The embedded core never touches stdout/stderr or a TTY. Logs are emitted via a caller-supplied sink. Provider HTTP calls go through a caller-supplied HTTP client (so the host's existing connection pool, timeouts, and tracing apply).
- **Concurrency:** the core API is `Allocator`- and `Session`-scoped. Sessions are independent; the host owns their lifetimes. The shared store (§10) handles cross-session coordination when the host runs more than one.
- **Typical shape:** a Zig HTTP handler accepts a request, looks up or creates a `phoenix.Session`, calls `session.send(...)`, streams the resulting events out as SSE / WebSocket frames, and returns. The web service owns auth, rate limiting, persistence of session ids, and request lifecycle. Phoenix owns the agent loop.

Non-goals for the embeddable target:

- **No C ABI.** Other-language hosts (Node, Python, Go) should talk to Phoenix over the RPC frontend (§4.2), not link it.
- **No long-lived background threads owned by Phoenix.** If the host wants a background scheduler, it builds one and calls into Phoenix from it.

---

## 5. Sessions, Agents, and Orchestration

### 5.1 Session

A **session** is the unit of agent state: message history, plan state, tool registry, allocator arena, and a handle to the shared store. Sessions are cheap to create and serialize.

### 5.1A Session persistence

Sessions persist to disk at `.phoenix/sessions/{session_id}/`. The session directory contains:

- **`messages.jsonl`** — the conversation history, one JSON object per message. Appended after each completed turn (user message + assistant response + tool results). This is the source of truth for session resume.
- **`state.json`** — session metadata: provider config, tool set, plan mode state, token accounting, creation time, last-active time.
- **`plan.md`** — symlink to the plan file in `.phoenix/plans/` if plan mode was used.

**Resume.** When Phoenix starts, it scans `.phoenix/sessions/` and offers to resume existing sessions. In the TUI, each persisted session appears as a restorable tab. Over RPC, `session.list` returns all persisted sessions; `session.resume(id)` rehydrates one. The embedded library exposes `Session.resume(allocator, path)`.

Rehydration loads the message history into memory and re-registers the tool set from `state.json`. The agent picks up where it left off — or more precisely, where the last completed turn ended. If Phoenix was killed mid-tool-call, that incomplete turn is not persisted; the agent sees the conversation as of the last clean turn boundary. The tool log in the store may contain the partial invocation for forensics.

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
- **Orchestrator thread:** the orchestrator's own session loop runs on the main thread (TUI) or a dedicated thread (RPC/embed), *not* in the pool. The pool is exclusively for child sessions.

### 5A.4 Session isolation

Sessions are independent: each has its own allocator arena, message history, tool registry, and provider connection. The shared store (§10) is the only cross-session coordination surface, and it serializes writes internally.

A child session:
- **Can** read and write to the shared store (todos, KV scratch, tool log).
- **Cannot** access the orchestrator's message history or tool state.
- **Cannot** spawn its own children (unless the child's profile also includes orchestration tools — nested orchestration is possible but must be explicitly configured).

### 5A.5 Frontend integration

- **TUI:** each running child appears as a tab (read-only by default, showing streaming output). The orchestrator tab shows a summary: child IDs, statuses, and a live count of `queued / running / done`. The user can switch to a child tab to watch its progress, or cancel it with the standard cancel keybind.
- **RPC:** `session.events` for the orchestrator emits `child_spawned`, `child_status_changed`, and `child_completed` events in addition to the orchestrator's own token stream. RPC clients can also call `session.events` on a child session ID directly to stream its output.
- **Embed:** the host program receives child lifecycle events through the same event iterator it uses for the orchestrator. The host owns thread management for its own request-handling; Phoenix's internal thread pool is separate.

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
mode = "tui"           # "tui" | "rpc" | "embed"
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
plan_mode = "on_request"   # "off" | "on_request" | "always"
tools = ["read_file", "write_file", "run_shell", "search"]

# Deny: gate destructive actions behind user confirmation (see §8)
[[session.default.deny]]
tool = "run_shell"
kind = "pattern"
args_match = "rm\\s+-rf|git\\s+push.*--force|git\\s+reset\\s+--hard"
reason = "destructive shell command"

[session.orchestrator]
extends = "default"
tools = ["read_file", "search", "spawn_session", "check_sessions", "collect_session", "cancel_session"]

[store]
backend = "beans"          # "beans" | "doltlite" | "memory"
path = "./.phoenix/store"

[plugins]
load = ["./plugins/mcp_bridge.so"]   # optional
```

Reload semantics: the TUI watches its config file and hot-reloads non-structural changes (prompts, tool lists). Structural changes (provider, store backend) require restart and Phoenix says so explicitly.

---

## 7. Plan Mode

Plan mode is a lightweight, file-backed mechanism for letting the agent explore before it acts. It is a two-state toggle on the session (`planning` / `executing`), a markdown file, a read-only tool restriction, and a user gate.

### How it works

1. **Enter plan mode.** The user types `/plan` (TUI), calls `plan.enter` (RPC), or starts a session with `plan_mode = "always"` in config. When plan mode activates, Phoenix does two things:
   - Restricts the session's tool set to **read-only tools only** (`read_file`, `search`, `list_files`, `run_shell` with a read-only flag). No writes, no mutations.
   - Creates (or opens) a plan file at `.phoenix/plans/{session_id}.md`.

2. **Explore and accumulate context.** The agent reads code, searches, and builds understanding. As it works, Phoenix **appends** each piece of context to the plan file — files read, search results, observations the agent makes. The plan file is an append-only log of everything the agent looked at and concluded. The agent can read this file at any time to review what it has gathered so far.

3. **Propose a plan.** When the agent has enough context, it writes a `## Plan` section at the end of the file: a numbered list of concrete steps (files to change, what to change, why). This is the proposal.

4. **User decides.** The plan is presented to the user in the TUI (or surfaced via `plan.events` over RPC). The user can:
   - **Accept** — plan mode exits, the session's full tool set is restored, and the plan file stays on disk as a reference. The plan content is appended to the agent's context so it knows what was agreed.
   - **Reject** — plan mode exits, no tools are restored beyond the session's normal set, plan file is kept for reference but marked rejected.
   - **Ask for revision** — the user sends a message; the agent continues exploring and revises the plan in-place.

### The plan file

The plan file (`.phoenix/plans/{session_id}.md`) is plain markdown. Phoenix appends to it; the agent reads from it. Structure:

```markdown
# Plan: {user's initial prompt, truncated}

## Context

- [file] src/core/session.zig — read lines 1-50, session struct definition
- [search] "tool dispatch" — 3 results in src/core/dispatch.zig
- [observation] The dispatch table is a comptime-built array of Tool structs.
- [file] src/tools/read_file.zig — read lines 1-30, tool registration pattern

## Plan

1. Add a `plan_active: bool` field to `Session` in `src/core/session.zig`.
2. In `src/core/dispatch.zig`, check `plan_active` before dispatching — if true, reject any tool not in the read-only set.
3. ...
```

The context section is machine-appended as the agent explores. The plan section is agent-written when it's ready to propose. That's the whole format.

### Properties

- **Per-session.** Plan mode is a flag on the session, not a global state.
- **Toggleable mid-session.** `/plan` enters; acceptance or rejection exits.
- **No execution tracking.** Plan mode does not track whether the agent follows the plan after acceptance. The plan is a communication artifact between agent and user, not a contract the harness enforces.
- **Plans persist on disk.** The `.phoenix/plans/` directory accumulates plan files. They're useful for audits, for context in future sessions, and for the user to `grep` later. Old plans are never auto-deleted.

---

## 8. Permissions: Vibes + Deny

Phoenix does not prompt for tool permission *by default*. Instead:

- **Allowlist at config time.** The set of tools a session may call is fixed when the session starts. There is no "elevate" path mid-session — restart with a different config.
- **Cancel, don't gate.** The TUI surfaces a live "currently running" indicator with a cancel keybind. RPC clients can call `session.cancel`. This is the user's lever.
- **Audit, always.** Every tool invocation is appended to the shared store with timestamp, args, and result. "No prompts" does not mean "no record."

This is the default posture. For users who want guardrails without giving up dangerous tools entirely, there is a **deny list** — but it is important to understand what it is and what it is not.

### 8.1 Deny list (trip wires, not a security boundary)

The deny list is a per-session configuration that marks specific tools or argument patterns as **gated**: the tool remains in the session's toolset, but the harness pauses execution and requires explicit user confirmation before invoking it.

**What deny rules catch:** the model reaching for `rm -rf` when it meant `rm file.txt`, an accidental `git push --force` to main, a write to `.env` when the model was asked to edit `.env.example`. Deny rules catch *accidents and common destructive patterns* — the 90% case where the model takes a dangerous action through the obvious, direct path.

**What deny rules do not catch:** a model that constructs equivalent destructive operations through indirect paths (`find . -delete`, a Python one-liner, a shell script that wraps the forbidden command, `rm -r -f` instead of `rm -rf`). Pattern matching on serialized arguments is a regex over a string — it cannot reason about shell semantics, command equivalence, or intent.

**Deny rules are not a security boundary.** If you need hard isolation — if the consequence of a bypass is data loss, credential exposure, or production impact — run Phoenix inside a container, VM, or namespace with filesystem and network restrictions enforced by the OS. Phoenix's deny rules are defense-in-depth: a useful first layer that catches honest mistakes, not a perimeter you should rely on against a determined or creative agent.

Deny rules are a flat array on the session. Each entry has a `kind` — either `"tool"` (gate every invocation of the named tool) or `"pattern"` (gate only when the serialized args match a regex).

```toml
[session.default]
tools = ["read_file", "write_file", "run_shell", "search"]

# Gate every invocation of run_shell
[[session.default.deny]]
kind = "tool"
tool = "run_shell"
reason = "all shell commands require confirmation"

# Gate only specific destructive patterns
[[session.default.deny]]
kind = "pattern"
tool = "run_shell"
args_match = "rm\\s+-rf|git\\s+push.*--force|git\\s+reset\\s+--hard"
reason = "destructive shell command"

[[session.default.deny]]
kind = "pattern"
tool = "write_file"
args_match = "\\.env|credentials|secrets"
reason = "writing to sensitive file"
```

### 8.2 Resolution order

When the agent requests a tool invocation, Phoenix evaluates the deny array in order:

1. **First matching `kind = "tool"` entry** — if the tool name matches, gate it regardless of arguments.
2. **First matching `kind = "pattern"` entry** — if the tool name matches and the serialized arguments match `args_match`, gate it.
3. **No match** — the tool is in the session's `tools` list and no deny rule matched. Execute immediately, no prompt.

Evaluation stops at the first match. `kind = "tool"` is the blunt instrument; `kind = "pattern"` is the scalpel. Most users will use patterns to gate specific destructive invocations while leaving the tool itself ungated for normal use.

### 8.3 Gate UX

When a deny rule triggers:

- **TUI.** Phoenix pauses the agent loop, displays the tool name, arguments, and the `reason` from the matching deny rule (or a generic "tool is on the deny list" if it's a tool-level deny). The user presses `y` to allow or `n` to reject. Rejection returns a tool error to the model ("user denied this invocation") so it can adjust.
- **RPC.** The server emits a `tool_denied` event containing the tool name, arguments, and reason. The RPC client must respond with `tool_denied.allow` or `tool_denied.reject` before the session loop resumes. If no response arrives within a configurable timeout (`deny_timeout`, default 120s), the invocation is rejected automatically.
- **Embed.** The host program receives the deny gate through the event iterator as a `DenyGate` event. The host calls `session.resolveDeny(allow: bool)` to unblock the loop. This lets the host wire up its own approval UI or policy engine.

### 8.4 Properties

- **Deny is not remove.** A denied tool is still advertised to the model in the tool schema. The model can still request it; it just can't execute without confirmation. This preserves the model's ability to reason about the tool and propose its use — the user decides whether to permit each invocation.
- **Config-layered.** Deny rules follow the same precedence as all config (§6): built-in defaults → user config → project config → CLI flag → session override. Higher layers can add deny rules but **cannot remove** deny rules set by a lower layer. A project-level deny on `rm -rf` cannot be overridden by a session flag. This is intentional: a project maintainer can set a floor that individual sessions cannot drop below.
- **No "allow-once-for-session" shortcut.** Every gated invocation requires its own confirmation. This is deliberate friction for destructive actions — if it's annoying, narrow the pattern or remove the deny rule.
- **Deny + plan mode.** Deny rules still apply after a plan is accepted. A plan that includes a denied action will still gate when the agent reaches that step during execution.
- **Audit marks denials.** The tool log records both allowed and rejected deny-gate events, including which rule triggered and the user's decision.

---

## 9. Extensibility

Two extension surfaces, plus a plugin loader for binary-only distribution.

### 9.1 Tools

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

Most tools implement `invoke` (one-shot, returns a complete result). Tools that produce large or incremental output — `run_shell` is the primary case — implement `invoke_stream` instead. The session loop consumes the `ChunkIterator`, forwarding chunks to the frontend in real time (TUI renders lines as they arrive; RPC emits `tool_chunk` events; embed yields chunks through the event iterator). When the tool completes, the accumulated output is committed to the message history.

**Truncation.** If a tool's output (one-shot or accumulated stream) exceeds `max_output_bytes`, Phoenix truncates it and appends a `[truncated at 512 KiB]` marker. The full output is still available in the tool log for audit, but the truncated version is what enters the conversation history and counts against the context budget. This prevents a single `find /` or verbose build log from consuming the entire context window.

Tools are registered at session start. Built-in tools live in `src/tools/`.

**Third-party tools are primarily build-time Zig modules.** The intended extension path is: write a Zig file that exports a `Tool`, add it as a dependency in `build.zig.zon`, and list it in the build configuration. This preserves Zig's strengths — comptime validation, error sets, tagged unions, static analysis — across the extension boundary. There is no C ABI to maintain, no vtable to version, and the tool is compiled and optimized alongside the core.

**`dlopen` is the escape hatch, not the primary path.** For cases where source isn't available (proprietary tools, binary-only distribution), Phoenix supports loading shared objects at runtime from `[plugins].load`. Plugins loaded this way cross a C-ABI boundary: they use a C-compatible vtable, cannot use Zig-specific types in the interface, and are unsandboxed. This is a deliberate trade-off for the small number of tools that genuinely need binary distribution.

### 9.2 Providers

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

### 9.3 Opencode bridge — a tool, not a provider

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

### 9.4 MCP — explicitly external

There is no MCP code in core. We will publish a reference MCP bridge (`phoenix-mcp-bridge`) that exposes MCP servers as Phoenix tools. The bridge is a build-time Zig module (or a `dlopen` plugin for binary distribution). If we never write the bridge, the core is unaffected.

---

## 10. Shared Store (Todos & State)

Multiple sessions need to coordinate without stepping on each other. The shared store is the only sanctioned coordination mechanism.

### 10.1 Surface

- **Todos** — id, title, body, status (`open`/`in_progress`/`done`/`cancelled`), assignee (session id), tags, created/updated.
- **Plans** — plan files live on disk at `.phoenix/plans/` (see §7), not in the store. The store records only a pointer (session id → plan file path) for cross-session visibility.
- **Tool log** — append-only audit of every tool call across sessions.
- **KV scratch** — small typed key-value store for session metadata (last working dir, last branch, etc.).

### 10.2 Backend choice

Two candidates, both single-process / embedded — we are not running a separate database server.

- **Beans** — append-log + index, optimized for high write throughput and crash safety. Good fit if we lean on the tool log being the source of truth.
- **Dolt-lite (custom)** — versioned, branchable rows. Heavier to build but gives us cheap snapshots ("what did the orchestrator see when it spawned this session?") and conflict-aware merges between concurrent sessions.

We will start with Beans (or memory, for tests), behind a `Store` interface, and revisit Dolt-lite once we have real concurrent-session traffic and can measure whether branchable history is worth the build cost.

### 10.3 Concurrency

- One writer process at a time owns the store directory; others connect via the RPC server. The TUI either *is* that owner (when run standalone) or *is* a client (when an RPC server is already up).
- Reads are lock-free where the backend allows; writes are serialized.

---

## 11. Compaction (Context Window Management)

Every long-running session will eventually approach the model's context limit. Phoenix handles this automatically via **compaction** — reducing the conversation history to fit within budget while preserving the information the agent needs to continue working.

### 11.1 Mechanics (core-owned)

The session tracks token usage against the active model's context budget. When usage crosses a configurable threshold, Phoenix triggers a compaction pass:

1. **Partition.** The message history is split into three regions:
   - **Pinned head** — the system prompt. Always preserved verbatim.
   - **Evictable middle** — older turns between the system prompt and the preserved tail.
   - **Preserved tail** — the last N user/assistant exchanges (configurable). Always kept intact so the agent has immediate working context.
2. **Compact.** The evictable middle is passed to a `Compactor` (see §11.2). The compactor returns a shorter sequence of messages — typically a single summary message — that replaces the evicted turns.
3. **Splice.** The session history becomes: pinned head + compacted middle + preserved tail. The session continues with no interruption to the agent loop.
4. **Log.** The compaction event is recorded in the store: timestamp, turns evicted, tokens before/after, compactor used. The original evicted messages are not retained in memory but are recoverable from the tool log if the store backend supports it.

Compaction is **visible to the user**. The TUI displays "compacted 43 turns into summary" in the status area. The summary itself is viewable by scrolling up. RPC emits a `session.compacted` event with the same metadata.

### 11.2 Compactor interface

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

### 11.3 Built-in compactors

Two compactors ship in core:

- **`truncate`** — drops the evictable middle entirely, keeping only the pinned head and preserved tail. Zero cost, no API call, fully deterministic. Suitable for short tasks or cost-sensitive usage where losing old context is acceptable.
- **`summarize`** — feeds the evictable history to the session's current provider with a summarization prompt, producing a single assistant message that captures key decisions, findings, file paths referenced, and current state. Costs one provider round-trip; the result quality depends on the model.

### 11.4 Extension compactors (not in core)

The `Compactor` interface is the extension point for more sophisticated strategies:

- **Code-aware** — preserves file paths, function signatures, and architectural decisions; aggressively drops raw tool output (file contents, search results, shell output) since the agent can re-read those from disk.
- **Topic-segmented** — groups related turns into topics before summarizing, so a session that worked on three separate files produces three topic summaries rather than one lossy blend.
- **Cheaper-model** — routes the summarization call to a faster/cheaper model (e.g., Haiku) while the session itself runs on a more capable model. Reduces compaction cost for Opus-class sessions.

Custom compactors are registered the same way as custom tools — as Zig modules at build time, or via `dlopen` plugins at runtime.

### 11.5 Interaction with plan mode

Plan files (§7) live on disk at `.phoenix/plans/`, outside the conversation history. They survive compaction entirely. After a compaction pass, the agent can re-read the plan file to recover context that was evicted from the conversation. This makes plan mode a natural complement to compaction: exploring in plan mode externalizes context to a durable artifact, reducing the session's dependence on conversation history.

### 11.6 Pinning

Messages can be marked `pinned` by the user (TUI command or RPC call) or by the agent via a `pin_message` tool (if enabled). Pinned messages are moved to the pinned-head region and survive compaction indefinitely. Use case: a critical constraint stated early in the session ("never modify the migration files") that would otherwise be evicted once the conversation grows long enough.

### 11.7 Token budgets

Compaction manages the context window. Token budgets manage *spend*. They are separate concerns with separate config.

A session tracks cumulative tokens sent and received across all provider calls. When a budget threshold is crossed, Phoenix acts:

- **Warn threshold.** The TUI displays a persistent warning in the status line ("session at 80% of token budget"). RPC emits a `session.budget_warning` event. The session continues.
- **Hard cap.** The session pauses. The TUI shows "token budget exhausted" and offers the user a choice: increase the budget, or end the session. RPC emits `session.budget_exhausted`; the client must call `session.setBudget` with a new limit or `session.destroy` before the loop resumes. The agent is never silently cut off — the user always decides.

For orchestrator sessions, the budget applies to the **aggregate** across the orchestrator and all its children. Each child's token use is charged to the parent's budget. This prevents an orchestrator from evading a budget by fanning out to many children.

```toml
[session.default]
compaction = "summarize"       # "truncate" | "summarize" | custom registered name
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

## 12. Observability

- Structured logs to stderr (JSONL) with a TUI-friendly pretty mode.
- A `phoenix trace` subcommand replays the tool log for a session.
- Metrics: token counts per provider call, tool latency, store write latency. Exported on demand via RPC; no built-in Prometheus endpoint (plugin territory).

---

## 13. Error Recovery

Phoenix is "vibes mode" — no permission prompts, no hand-holding. That only works if the harness handles failures without user intervention. There are four failure domains.

### 13.1 Provider failures

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

### 13.2 Tool failures

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

### 13.3 Store failures

- **Audit log writes are best-effort.** If a store write fails, log the failure to stderr, increment a metric counter, and continue. The session does not halt because the audit system is unavailable — audit is an observer, not a dependency.
- **Todo / KV writes retry once**, then surface as a tool error to the model. The model decides whether the write was important enough to retry or work around.

### 13.4 Child session failures (orchestration)

A child session that errors out transitions to `error` state with a captured error message. The orchestrator's `collect_session` call returns the error. The orchestrator model decides what to do — spawn a replacement, skip the subtask, or report to the user. The harness never auto-retries a child session; that decision belongs to the orchestrator model.

---

## 14. Security Posture

- No network calls except those a provider or tool makes explicitly.
- Secrets (API keys) are read from env or from a user-specified keychain command; never from config files committed to repos.
- Plugins are unsandboxed. Loading a plugin is equivalent to running a binary. We document this loudly; we do not pretend otherwise.

### 14.1 Signal handling

The TUI and RPC frontends install signal handlers. The embedded library (§4.3) does not — the host owns signals.

- **`SIGINT` (Ctrl+C).** First signal cancels the active tool invocation (equivalent to the TUI cancel keybind). Second signal within 2s initiates graceful shutdown: cancel all running child sessions, flush the store, exit. Third signal forces immediate exit.
- **`SIGTERM`.** Graceful shutdown: cancel all sessions, flush store, exit. No second-chance escalation — the process manager can follow up with `SIGKILL` if needed.
- **`SIGPIPE`.** Ignored. Provider connections and RPC pipes can break at any time; the relevant I/O call returns an error, which the retry logic (§13) handles.
- **`SIGHUP`.** TUI: trigger config hot-reload (same as file-watch). RPC: ignored (daemon convention; systemd sends `SIGTERM` for shutdown).

---

## 15. Repository Layout (proposed)

```
phoenix/
├── DESIGN.md              # this file
├── README.md
├── justfile
├── build.zig
├── build.zig.zon
├── .zigversion
├── src/
│   ├── core/              # session loop, plan mode, dispatch
│   ├── providers/         # claude, openai, ollama, llamacpp
│   ├── tools/             # built-in tools
│   ├── store/             # beans backend, store interface
│   ├── tui/               # opentui frontend
│   ├── rpc/               # RPC server + client
│   ├── embed/             # Zig module exposed to host web services
│   └── main.zig           # subcommand dispatch (tui | rpc)
├── plugins/               # example plugins (mcp bridge, etc.)
├── tests/
│   ├── unit/
│   └── integration/
└── docs/
    ├── config.md
    ├── plan-mode.md
    └── extending.md
```

---

## 16. Roadmap (rough)

- **M0 — Skeleton.** `build.zig`, `justfile`, hello-world TUI, hello-world RPC, store interface stub, `claude` provider, two tools (`read_file`, `run_shell`).
- **M0.5 — Provider parity.** Add `openai`, `ollama`, and `llamacpp` provider adapters; share the event-normalization layer; integration tests against a recorded fixture per provider.
- **M1 — Sessions & loop.** Real session loop, tool dispatch, audit log to Beans, cancel semantics, token accounting, compaction with `truncate` and `summarize` compactors, session persistence and resume, token budgets, streaming tool output.
- **M2 — Plan mode.** Read-only tool restriction, plan file append logic, TUI `/plan` command, RPC `plan.*` methods.
- **M3 — Tabs & orchestration.** TUI tabs; orchestration tools (`spawn_session`, `check_sessions`, `collect_session`, `cancel_session`); thread pool (`2 * getCpuCount()` default); multi-session shared store.
- **M4 — Embeddable.** Zig module surface (`@import("phoenix")`), a sample Zig HTTP service that exposes a session over SSE, docs on threading/allocator/HTTP-client injection.
- **M5 — Plugin loader.** `dlopen` path; reference MCP bridge plugin lives in a sibling repo.
- **M6 — Polish.** Config hot-reload, `phoenix trace`, benchmarks, packaging.

Each milestone ends with: `just test` green, `just bench` recorded, docs updated.

---

## 17. Open Questions

- **Beans vs. Dolt-lite — when do we revisit?** Probably once we have two real workloads driving the store. Until then, pick the cheaper option.
- **Streaming format on RPC.** JSON for v0 is fine, but we should profile before committing. If frame sizes get large (image attachments, big diffs), we move to length-prefixed CBOR.
- **Provider auth UX.** Without permission prompts, how do we handle a missing API key gracefully? Probably a startup-time check with a clear error, never a mid-session interactive flow.
- **Plugin ABI stability.** Pinning a C ABI early constrains us; not pinning it means plugins break on every minor. Likely answer: declare the ABI unstable until M5 and version it from there.
- **Embedded API stability.** The Zig module surface (§4.3) will churn through M0–M3. We should freeze a `phoenix.embed` namespace at M4 and treat changes there as semver-breaking, while leaving deeper internals free to move.
- **Compaction token accounting accuracy.** Exact token counts require a tokenizer per provider. For v0 we can estimate (chars / 4), but this drifts. Do we ship a tokenizer, call the provider's count endpoint, or live with estimates? Likely: estimates for threshold triggering, exact counts for the status line via provider response headers.

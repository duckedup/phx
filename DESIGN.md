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
- **Permission prompts.** No "may I run this command?" UX. If a tool is enabled, the agent uses it.
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
                  │  │ Plan mode FSM     │  │
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
- **No modal permission prompts.** The TUI may *display* what an agent is doing in real time (and let the user cancel), but it never asks "approve y/n?".
- Status line shows: provider, model, token use, current plan step (if in plan mode), and active tool, if any.

### 4.2 Headless RPC

- Default transport: a **Unix domain socket**, with a **stdio fallback** for child-process embedding (CI, scripts).
- Wire format: **length-prefixed JSON** for v0. We keep it boring on purpose; we'll evaluate MessagePack/CBOR after the surface stabilizes.
- Methods (initial set):
  - `session.create`, `session.destroy`
  - `session.send` (user message → agent)
  - `session.events` (server-streamed events: tokens, tool calls, tool results, plan transitions)
  - `tool.invoke` (out-of-band tool call from a controller)
  - `plan.enter`, `plan.exit`, `plan.approve`
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

### 5.2 Agent vs. orchestrator (same machinery)

Phoenix doesn't have separate "agent" and "orchestrator" code paths. There is one loop. What differs is the **tool set** the session is configured with:

- A plain agent has tools like `read_file`, `run_shell`, `search`.
- An orchestrator has, in addition, a `spawn_session` tool that lets the model open a new session (which the user sees as a new tab in the TUI, or as a child session over RPC).

This means orchestration is just configuration. There is no daemon, no scheduler, no implicit fan-out. The agent calls `spawn_session` because the user gave it that tool; the harness obeys.

### 5.3 No automatic subagent spawn

Restated, because it's load-bearing: the **harness** never spawns a subagent. The **model** can request a spawn *only if* the user enabled the spawn tool in this session's config. Two consequences:

1. A default config is single-agent. You have to opt in to orchestration.
2. Removing the `spawn_session` tool from a config is a hard guarantee that no subagents will appear, regardless of what the model tries to call.

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

[provider.default]
kind = "anthropic"
model = "claude-opus-4-7"
# api_key sourced from env; never written in config

[session.default]
system_prompt_path = "./prompts/system.md"
plan_mode = "on_request"   # "off" | "on_request" | "always"
tools = ["read_file", "write_file", "run_shell", "search"]

[session.orchestrator]
extends = "default"
tools = ["read_file", "search", "spawn_session"]

[store]
backend = "beans"          # "beans" | "doltlite" | "memory"
path = "./.phoenix/store"

[plugins]
load = ["./plugins/mcp_bridge.so"]   # optional
```

Reload semantics: the TUI watches its config file and hot-reloads non-structural changes (prompts, tool lists). Structural changes (provider, store backend) require restart and Phoenix says so explicitly.

---

## 7. Plan Mode

Plan mode is a first-class FSM in the core, not a UI convention.

States:

- **`drafting`** — agent emits a plan; no destructive tools are dispatched. The plan is structured (steps, rationale, files touched), not free text.
- **`awaiting_approval`** — the plan is shown to the user (TUI), or surfaced over RPC (`plan.events`). User can approve, edit, or reject.
- **`executing`** — approved plan runs step-by-step; the agent narrates progress and may revise *within the approved scope*.
- **`done`** — plan complete; session returns to normal mode.

Key properties:

- Plan mode is per-session and toggleable mid-session.
- An approved plan is recorded in the shared store so other sessions (and post-hoc audits) can see what was agreed and what shipped.
- A plan can reference todo IDs from the shared store; checking off a todo can advance the FSM.

---

## 8. Permissions: Vibes

Phoenix does not prompt for tool permission. Instead:

- **Allowlist at config time.** The set of tools a session may call is fixed when the session starts. There is no "elevate" path mid-session — restart with a different config.
- **Cancel, don't gate.** The TUI surfaces a live "currently running" indicator with a cancel keybind. RPC clients can call `session.cancel`. This is the user's lever; there is no other.
- **Audit, always.** Every tool invocation is appended to the shared store with timestamp, args, and result. "No prompts" does not mean "no record."

This is a deliberate posture. Users who want gated execution should run Phoenix with a smaller toolset, not bolt prompts onto a bigger one.

---

## 9. Extensibility

Two extension surfaces:

### 9.1 Tools

A tool is a Zig struct (or, for plugins, a C-ABI vtable) that satisfies:

```zig
const Tool = struct {
    name: []const u8,
    schema: []const u8,           // JSON schema for args
    invoke: fn (ctx: *Ctx, args: Value) anyerror!Value,
};
```

Tools are registered at session start. Built-in tools live in `src/tools/`. Third-party tools ship as either:

- **In-tree Zig modules** added at build time, or
- **Shared objects** loaded at runtime (`dlopen`) from `[plugins].load`.

### 9.2 Providers

A **provider** turns a session's message list into a stream of events (tokens, tool calls, completions). Providers implement a small adapter interface and live in `src/providers/`. Day-one targets: Anthropic, OpenAI-compatible. Local models (llama.cpp, etc.) are a plugin concern.

### 9.3 MCP — explicitly external

There is no MCP code in core. We will publish a reference plugin (`phoenix-mcp-bridge`) that exposes MCP servers as Phoenix tools. The bridge is just another shared object loaded via `[plugins].load`. If we never write the bridge, the core is unaffected.

---

## 10. Shared Store (Todos & State)

Multiple sessions need to coordinate without stepping on each other. The shared store is the only sanctioned coordination mechanism.

### 10.1 Surface

- **Todos** — id, title, body, status (`open`/`in_progress`/`done`/`cancelled`), assignee (session id), tags, created/updated.
- **Plans** — recorded plan-mode artifacts (see §7).
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

## 11. Observability

- Structured logs to stderr (JSONL) with a TUI-friendly pretty mode.
- A `phoenix trace` subcommand replays the tool log for a session.
- Metrics: token counts per provider call, tool latency, store write latency. Exported on demand via RPC; no built-in Prometheus endpoint (plugin territory).

---

## 12. Security Posture

- No network calls except those a provider or tool makes explicitly.
- Secrets (API keys) are read from env or from a user-specified keychain command; never from config files committed to repos.
- Plugins are unsandboxed. Loading a plugin is equivalent to running a binary. We document this loudly; we do not pretend otherwise.

---

## 13. Repository Layout (proposed)

```
phoenix/
├── DESIGN.md              # this file
├── README.md
├── justfile
├── build.zig
├── build.zig.zon
├── .zigversion
├── src/
│   ├── core/              # session loop, plan FSM, dispatch
│   ├── providers/         # anthropic, openai-compat
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

## 14. Roadmap (rough)

- **M0 — Skeleton.** `build.zig`, `justfile`, hello-world TUI, hello-world RPC, store interface stub, single Anthropic provider, two tools (`read_file`, `run_shell`).
- **M1 — Sessions & loop.** Real session loop, tool dispatch, audit log to Beans, cancel semantics.
- **M2 — Plan mode.** FSM, TUI surface, RPC events, plan persistence in store.
- **M3 — Tabs & orchestration.** TUI tabs; `spawn_session` tool; multi-session shared store.
- **M4 — Embeddable.** Zig module surface (`@import("phoenix")`), a sample Zig HTTP service that exposes a session over SSE, docs on threading/allocator/HTTP-client injection.
- **M5 — Plugin loader.** `dlopen` path; reference MCP bridge plugin lives in a sibling repo.
- **M6 — Polish.** Config hot-reload, `phoenix trace`, benchmarks, packaging.

Each milestone ends with: `just test` green, `just bench` recorded, docs updated.

---

## 15. Open Questions

- **Beans vs. Dolt-lite — when do we revisit?** Probably once we have two real workloads driving the store. Until then, pick the cheaper option.
- **Streaming format on RPC.** JSON for v0 is fine, but we should profile before committing. If frame sizes get large (image attachments, big diffs), we move to length-prefixed CBOR.
- **Tab persistence.** Should TUI tabs survive a Phoenix restart? Leaning yes, via the shared store, but the UX of "rehydrate an in-flight tool call" is non-trivial.
- **Provider auth UX.** Without permission prompts, how do we handle a missing API key gracefully? Probably a startup-time check with a clear error, never a mid-session interactive flow.
- **Plugin ABI stability.** Pinning a C ABI early constrains us; not pinning it means plugins break on every minor. Likely answer: declare the ABI unstable until M5 and version it from there.
- **Embedded API stability.** The Zig module surface (§4.3) will churn through M0–M3. We should freeze a `phoenix.embed` namespace at M4 and treat changes there as semver-breaking, while leaving deeper internals free to move.

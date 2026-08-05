# phx — agent instance manager for tmux

**Status:** design draft, nothing built
**Supersedes:** the existing phx (RPC harness + provider modeling, ~34k LOC). Essentially none of it carries over.

---

## 1. Problem

Working a real polyglot monorepo (Go + Bun + Rust) with multiple coding agents in parallel. Each agent needs its own isolated instance of the project. Today that means manually creating a checkout, copying untracked config, reinstalling deps, waiting on cold builds, and dodging port and database collisions — five to fifteen minutes of ritual per instance. That friction is why you run two agents instead of four.

tmux already solves panes, persistence, and switching. opensessions already solves the sidebar and agent-state display. Neither solves **materializing an isolated, working instance of a monorepo in two keystrokes, and tearing it down in one.**

That gap is the product.

## 2. Non-goals

- Not a terminal emulator. tmux owns panes, input, resize, scrollback, detach/reattach.
- Not a sidebar. opensessions owns session list and agent state display.
- Not a model harness. No provider APIs, no token accounting, no RPC, no plugins.
- Not a merge queue. Teardown lands one branch; coordinating N branches is out of scope.
- Not a task tracker, and no shared state between instances. Instances are independent by
  design; the only thing that needs a view across all of them is you.
- **No ticket integration.** phx never calls Jira or Linear. The agent inside the instance
  does that through its own MCP servers and CLIs. phx's only responsibility is provisioning
  access — MCP config and tokens land in the worktree during materialization (§5) — and then
  it stays out of the way. No ticket ID in the spawn flow, no branch naming from tickets, no
  status sync on teardown.
- No daemon. Every action is transactional and exits.

## 3. Shape

A tmux plugin: a `phx.tmux` shell shim that binds keys, plus a Rust binary the bindings invoke.

```
prefix n   display-menu → pick harness → new isolated instance
prefix d   land the current window
prefix x   discard the current window + its worktree
prefix r   rename label (and branch, if no commits yet)
```

Switching comes from tmux and opensessions. There is no human-facing CLI — every argument reaches the binary through a tmux format substitution, never through your fingers.

### tmux is the state store

Instance metadata lives on the tmux window as user options, set at creation:

```
@phx_worktree  /Users/austin/Projects/.worktrees/phx-2
@phx_branch    agent/2
@phx_harness   claude
@phx_ports     4200
```

`prefix d` reads `@phx_worktree` off the focused window and acts on it. No registry file, no instance IDs, no daemon, nothing to desync. The window ID is the primary key, and when a window dies its state dies with it — correctly.

## 4. `prefix n` — the spawn path

1. `display-menu` lists harnesses from config; you pick one.
2. Allocate instance number `N` (lowest free, from existing `@phx_*` windows).
3. `git worktree add <root>/.worktrees/<repo>-N -b agent/N`
4. **Materialize** (§5) — the expensive part, made cheap.
5. `tmux new-window -c <worktree> -e ...` with the instance env.
6. Set `@phx_*` options on the new window; rename it.
7. Launch the harness at its normal empty prompt. No injected task — you type what you want.

Auto-labels `2`, auto-branches `agent/2`. Rename later with `prefix r`; nobody should have to name a thing before knowing what it is.

`prefix N` (shift) spawns in the current checkout with no worktree, for throwaway questions that don't need isolation. The sidebar marks it distinctly.

## 5. Materialization — the core

A monorepo worktree gets source and nothing else. Rebuilding from scratch is 10+ minutes and gigabytes; that would kill the whole premise.

### APFS clone

macOS `clonefile` (`cp -c`) is copy-on-write. Cloning large gitignored directories from the primary checkout costs metadata time and near-zero disk, diverging only as written. Unlike symlinking, there is no shared mutable state — safe for concurrent agents.

This is the enabling trick. Without it, worktree-per-agent isn't viable on a real monorepo.

### Per ecosystem

| Stack | Strategy |
|---|---|
| **Go** | Nothing. `GOCACHE` / `GOMODCACHE` are global and concurrency-safe. |
| **Rust** | Clone `target/`. Set `RUSTC_WRAPPER=sccache`. **Never** a shared `CARGO_TARGET_DIR` — cargo's exclusive lock serializes agents. |
| **Bun** | Clone `node_modules/`. Fallback `bun install` hardlinks from the global cache and is fast. |
| **Generated code** | Clone, or run the repo's codegen as part of `setup`. |
| **Untracked config** | Copy `.env` and friends at every level they appear. Copy, not symlink — agents edit these. |
| **Agent access** | Copy `.mcp.json` and `.claude/settings.local.json` so the harness comes up with its Jira/Linear MCP servers and tokens already wired. An instance that can't reach the tracker is useless; an instance where phx reaches it for them is the wrong design. |

### Collision matrix

| Resource | Mechanism |
|---|---|
| Working tree, git index | git worktree — separate index per worktree, no `index.lock` contention |
| Build caches | APFS clone + sccache; global caches where already safe (Go) |
| Ports | `PHX_PORT_BASE=4200+100N` |
| Containers, volumes, networks | `COMPOSE_PROJECT_NAME=phx-N` |
| Databases | `PHX_INSTANCE=N` → per-instance DB name |
| Ticket tracking | Not a collision. Jira and Linear are remote; N agents contend on nothing. |

**Prerequisite on the monorepo:** ports and DB names must actually be read from env. That's a change in the repo, not in phx, and it gates the services half of isolation.

**Cost:** N compose stacks means N times the RAM. Compose bring-up should be opt-in per instance rather than automatic.

## 6. The recipe lives in the monorepo

phx stays generic; the repo declares how to materialize an isolated instance of itself. `.phx/config.toml` at the monorepo root:

```toml
[harnesses.claude]
cmd = "claude"

[harnesses.codex]
cmd = "codex"

[harnesses.grok]
cmd = "grok"
env = { GROK_API_KEY = "..." }

[instance]
worktree_root = ".worktrees"
branch_prefix = "agent/"
port_base     = 4200
port_stride   = 100

# copy-on-write cloned from the primary checkout
clone = ["node_modules", "target", "packages/*/node_modules", "gen"]

# copied verbatim
copy  = [".env", ".env.local", "packages/*/.env", ".mcp.json", ".claude/settings.local.json"]

env   = { RUSTC_WRAPPER = "sccache", COMPOSE_PROJECT_NAME = "phx-{n}", PHX_PORT_BASE = "{port}" }

setup = "bun install --frozen-lockfile"        # after clone; should be a fast no-op
check = "turbo run test lint --filter=...[main]"   # affected-only; full suite is unusable here
```

Global `~/.config/phx/config.toml` holds harness defaults; the repo file overrides and adds the recipe.

## 7. `prefix d` — teardown

Teardown is slow (rebase + affected checks = minutes) and can ask questions on conflict. A blocking popup is the wrong container.

**Teardown opens its own tmux window.** It's a window you can switch away from and return to; opensessions shows its state like any other. Steps:

1. `git rebase <base>` — stop and leave the window open on conflict
2. run `check` from config
3. push, open PR
4. `git worktree remove`, delete branch, `docker compose down` for `phx-N`
5. kill the original window, then itself

Green means it vanishes. Red means it sits there with the output on screen.

`prefix x` is the discard path: kill window, remove worktree, drop branch, tear down the stack. Confirm if the branch has unpushed commits.

## 8. Attention signals

Delegated to opensessions, which watches agent state directly (Claude Code transcripts, Codex's SQLite DB, Amp and OpenCode threads) — more reliable than hooks or output quiescence.

Where phx adds value: `POST /set-status` on the opensessions API to surface worktree-level state — diff stat, teardown progress, check failures.

For harnesses opensessions has no watcher for (grok, cursor, anything new), fall back to tmux-native `monitor-bell` / `monitor-silence` / `monitor-activity`. Coarse, but zero integration and works for anything.

## 9. Open questions

1. **Clone freshness.** A cloned `node_modules` is a snapshot. When the primary checkout's lockfile moves ahead, instances drift. Does `setup` reconcile every spawn, or only on lockfile change? Cheap heuristic: hash the lockfiles at clone time, re-run `setup` only when they differ.
2. **Port convention.** How much monorepo change is needed before `PHX_PORT_BASE` actually works end to end? This gates the services half of isolation and is work in the repo, not in phx.
3. **Compose stacks.** Opt-in per instance, or always? RAM says opt-in; convenience says always.
4. **Dependency on opensessions.** It's early — its own README notes theme/config/plugin hooks parse but don't fully function. Acceptable risk, or vendor the sidebar later?

## 10. Scope

| Piece | Est. |
|---|---|
| `phx.tmux` shim + bindings + menu generation | ~50 lines shell |
| config load/merge (global + repo), glob expansion | ~150 |
| materialize: worktree, clonefile, copy, env, setup | ~350 |
| spawn: port alloc, tmux window, window options | ~150 |
| teardown + discard | ~250 |
| opensessions status push | ~50 |

Roughly **1,000–1,200 lines of Rust** plus the shim. No terminal emulation, no daemon, no IPC, no provider code.

## 11. Sequencing

1. `materialize` alone, invoked by hand — prove a monorepo worktree comes up warm in seconds, with two agents building concurrently and not colliding. **All the risk is here.** Everything after is mechanical.
2. `prefix n` with one hardcoded harness, no menu.
3. Harness menu from config.
4. `prefix d` / `prefix x`.
5. opensessions status push, `prefix r`.

Stop after 1 and reassess. If APFS cloning doesn't hold up on a real monorepo — or the services layer needs more repo surgery than expected — the rest of the design is worth revisiting before it gets built.

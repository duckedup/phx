# Spec: Decompose then TEA

## Overview

Two stages. First, split the god objects into small focused files so a human can navigate them. Then layer TEA on top so there's a system governing how state changes.

## Stage 1 — Decompose

`app.rs` (2300 lines) does five jobs: struct definition, rendering, event handling, conversation management, and terminal lifecycle. `message_handler.rs` (1400 lines) does three: command dispatch, conversation start/resume, and reload. Split each file by concern. No new abstractions — just `fn` moves.

After Stage 1:

| File | Lines | Single responsibility |
|---|---|---|
| `app.rs` | ~500 | `App` struct, fields, accessors, `render()` |
| `runtime.rs` | ~250 | Event loop, terminal setup/teardown, `redraw()` |
| `event_handler.rs` | ~550 | `handle_key()`, `handle_mouse()`, `handle_paste()` |
| `commands.rs` | ~500 | `handle_command()` dispatch (was in message_handler.rs) |
| `conversation.rs` (tui) | ~300 | `start_conversation`, `send_message`, `resume_session`, `drain_conversations` |
| `reload.rs` | ~100 | `apply_reload` |

`message_handler.rs` is deleted — its functions move to the files above.

Every file has one job. A developer looking for "what happens when I press a key" opens `event_handler.rs`. Looking for "how does a conversation start" opens `conversation.rs`. No grepping through 2300 lines.

## Stage 2 — TEA

With files small and focused, the system goes in:

| File | Single responsibility |
|---|---|
| `msg.rs` | What can happen (the `Msg` enum) |
| `update.rs` | What each action does to state |
| `cmd.rs` | What side effects exist |

The mutations in `event_handler.rs`, `commands.rs`, and `conversation.rs` get rerouted through `update()`. Each file shrinks further because the state-change logic moves to `update.rs`.

## Sub-specs

- `src/tui/spec.md` — all implementation details for both stages

## Ordering

1. Stage 1 (decompose) — one commit per extracted file, or one big commit. Each must pass `just lint` + `just test`.
2. Stage 2 (TEA) — scaffold first, then migrate domain by domain.

Stage 2 cannot start until Stage 1 is complete. TEA on top of god objects is painful. TEA on top of focused files is surgical.

## Global verification

After each stage:

```bash
just lint
just test
```

Manual: full end-to-end usage (type messages, scroll, open pickers, conductor mode, file viewer). Behavior identical throughout — these are refactors, not feature changes.

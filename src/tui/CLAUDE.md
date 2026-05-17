# TUI Layer — Agent Instructions

This file documents the rendering foundation of phx's TUI. Read it before changing anything under `src/tui/`.

## Rendering foundation

The TUI uses [ratatui](https://github.com/ratatui-org/ratatui) and a single-pass render model: every frame, the entire UI is rebuilt from `App` state, no diffing at the application layer.

### The width contract

There is exactly one source of truth for "how wide is the chat content area": the `Rect` returned by `layout::padded_chat_area(content_rect)`. That Rect's width is what gets stored in `app.chat_area.width`, and that is the value that flows into the render pipeline.

**Never pass `terminal.size().width` into `compute_display_lines` or any rendering function.** It is the *terminal* width, not the *content* width. The difference is the chat padding, the sidebar (if visible), and the file viewer tab bar (if visible). Passing the wrong one is what caused the original garbling bug.

The contract:

```
terminal.size() → main_layout() → chunks[0] → padded_chat_area() → app.chat_area
                                                                       ↓
                                          compute_display_lines(app.chat_area.width)
                                                                       ↓
                                                              Vec<DisplayLine>
                                                                       ↓
                                         render_chat_with_panel(frame, app.chat_area, ...)
```

The same width flows into both `compute_display_lines` and `render_chat_with_panel`. They cannot disagree because they read from the same field.

### Measuring text

`text.chars().count()` counts Unicode scalar values. Terminals render in *display columns*. The two diverge for:

- Tabs (1 char → 4 cols, by our convention)
- CJK characters and emoji (1 char → 2 cols)
- Combining marks (1 char → 0 cols)
- Control characters

Use `crate::tui::rendering::measure::*` for all display-width work:

| Function | Use for |
|---|---|
| `display_width(text)` | Measuring how wide a string will render |
| `expand_tabs(text)` | Preparing user-controlled content (file contents, tool output, code) before measurement |
| `truncate_to_width(text, n)` | Clipping a string to at most `n` columns, appending `…` if cut |
| `truncate_to_width_raw(text, n)` | Same, without the ellipsis — for interior cuts in multi-span lines |
| `pad_to_width(text, n)` | Right-padding a string to exactly `n` columns with spaces |

`TAB_WIDTH = 4`. If you need to change tab width, change it there — not in every callsite.

**Never** use `text.chars().count()` to measure display width.
**Never** use `text.len()` to measure display width — `.len()` is bytes.
**Acceptable** uses of `chars().count()`: counting graphemes for non-display purposes (e.g., stream-buffer chunk sizing in `helpers.rs`), or counting positions in a known-ASCII string where you're constructing arithmetic (e.g., header padding where the header text is a literal).

### Building padded indents

`" ".repeat(n)` is fine when `n` is already a column count (e.g., `CHAT_PADDING` or `header_pad = full_width - 1 - header_width`). One space is one column, always. The bug isn't producing spaces — it's measuring strings.

## Component architecture

Components under `src/tui/components/` are **stateless render functions**. They take state (passed by reference) and a `Rect` (or `Frame`), and they write to the frame. They do not hold state. They do not own data.

State lives on `App` (`src/tui/app.rs`). Event handling and state transitions live in `src/tui/message_handler.rs` and parts of `app.rs`.

When adding a new piece of chrome (header element, panel, modal, etc.):

1. Add a render function under `src/tui/components/<name>.rs` taking `&mut Frame`, the `Rect` it should render into, and any state it needs by reference.
2. Add fields to `App` for any state it owns.
3. Call the render function from `App::render()` (in `app.rs`).
4. If it needs to react to input, add a branch in `message_handler.rs`.

**Do not** add state to component modules. **Do not** make components own their Rect — the caller decides where they go.

## DisplayLine

`DisplayLine` is the intermediate representation between data (chat history, agent state, etc.) and ratatui's `Line`. It's a vector of styled spans plus an optional clickable `file_path`. Defined in `src/tui/rendering/display.rs`.

Builders produce `Vec<DisplayLine>`:
- `build_chat_display_lines` — for chat messages
- `build_assistant_display_lines` — for assistant streaming output
- `build_widget_display_lines` — for plugin UI widgets
- `build_file_summary_lines` / `build_context_loaded_lines` — for system events
- `compute_display_lines` (in `chat_view.rs`) — top-level orchestrator

Builders accept `content_width: usize` — the columns available for text content (after subtracting `CHAT_PADDING`-derived left/right margins). This is the same value `compute_display_lines` receives from the caller.

The render loop (`render_chat_with_panel`) enforces the width contract: each DisplayLine is measured, truncated to fit the actual Rect, and padded to fill the row. **This is the only place truncation/padding for the chat area should happen.** Builders should produce DisplayLines that *fit*, but the render loop is the safety net.

## Sidebar and panels

The agent panel (sidebar) is rendered *after* the chat content, on top, in a Rect computed by `agent_panel_rect`. The chat render loop is panel-aware: when computing per-row width, it shrinks `row_width` if the panel overlaps that row. This is the only reason `render_chat_with_panel` takes a `panel: Option<Rect>` parameter — to know which rows must yield horizontal space.

When adding a new overlay (modal, popup, toast), use `ratatui::widgets::Clear` to wipe the background before rendering the overlay's border block. Examples: `modal_picker.rs:140`, `toast.rs:59`, `sidebar.rs:303`, `app.rs:1045`. Without `Clear`, the underlying chat content shows through.

## Theme

Colors come from `Theme` (`src/tui/theme.rs`). Themes are JSON-loaded from `src/tui/themes.json`. To add a color, add the field to `Theme`, `ThemeJson`, and the JSON file. Convenience methods on `Theme` (e.g., `tool_border()`, `dim()`, `user_msg_bg()`) blend base colors — use them rather than hardcoding RGB values, so theme switching works.

## Conventions

- `chars().count()` for display width → use `measure::display_width` instead.
- `" ".repeat(n)` where `n` came from `chars().count()` → broken. Fix the measurement, not the production.
- `text.replace('\t', "    ")` ad-hoc → use `measure::expand_tabs`.
- Adding `chars().take(n).collect::<String>()` to truncate → use `measure::truncate_to_width` or `truncate_to_width_raw`.
- New width math in a component → look at the Rect you were given. The Rect is the budget. Do not re-derive widths from `size`.

## Build & verify

```bash
just lint     # cargo fmt + cargo clippy --all-targets --all-features -- -D warnings
just test    # cargo test --all-features
```

Both must pass with zero warnings before any TUI change merges.

## Tests

Rendering tests are sparse. The pattern (when adding them):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_handles_tabs() {
        let result = truncate_line(&line, 5);
        let width: usize = result.spans.iter().map(|s| display_width(&s.content)).sum();
        assert!(width <= 5);
    }
}
```

Tests live at the bottom of the source file in a `#[cfg(test)] mod tests` block. See `src/tui/theme.rs` and `src/tui/picker.rs` for established patterns.

## TEA architecture (The Elm Architecture)

The TUI uses TEA: Model → Update → View.

- **Model**: `App` struct in `app.rs` — all state lives here.
- **View**: `App::render()` + stateless component functions in `components/` — pure rendering.
- **Update**: `update(app, msg) -> Cmd` in `update.rs` — every state change goes through here.
- **Cmd**: Side effects returned by `update()` in `cmd.rs` — anything async is a Cmd, not an inline mutation.

### Where to put things

| "I need to..." | File |
|---|---|
| Add a new user action | `msg.rs` — add a variant to `Msg` |
| Handle that action | `update.rs` — add a match arm |
| Add a side effect (async work) | `cmd.rs` — add a variant to `Cmd` |
| Add UI chrome / rendering | `components/` — stateless render function |
| Route a key/mouse event | `event_handler.rs` — map event to `Msg`, call `update()` |

### Rules

1. **`update()` is synchronous.** No `.await`, no I/O, no task spawning. If it needs async work, return a `Cmd`.
2. **Msg granularity is user-intent level.** `Msg::ScrollUp(n)` not `Msg::SetScrollOffset(n)`. One decision = one Msg, even if it changes multiple fields.
3. **No direct state mutations outside `update.rs`.** Don't write `app.active_tab = n` in event_handler.rs — construct `Msg::TabSwitch(n)` and call `update()`.
4. **Cmd captures data by value.** A Cmd should contain everything it needs to execute, not borrow from App.
5. **Components don't mutate.** They take state by reference and a Rect. They render. That's it.

### Migration status

TEA is being adopted incrementally. Some mutation sites still bypass `update()` — these are being migrated domain by domain. When adding new functionality, always go through `Msg → update → Cmd`. Do not add new direct mutations.

## File organization

| File | Responsibility |
|---|---|
| `app.rs` | App struct, accessors, render() |
| `runtime.rs` | Event loop, terminal setup/teardown, redraw |
| `event_handler.rs` | Key/mouse → Msg conversion, handle_key, handle_paste |
| `commands.rs` | /command dispatch |
| `conversation.rs` | Conversation lifecycle, conductor mode |
| `reload.rs` | Plugin reload |
| `msg.rs` | The Msg enum |
| `update.rs` | The update function |
| `cmd.rs` | The Cmd enum and executor |

# Spec: Decompose then TEA — implementation

## Stage 1: Decompose

Split `app.rs` (2300 lines) and `message_handler.rs` (1400 lines) into focused files. No new types or abstractions. Functions move to where they logically belong. All `pub fn` signatures stay the same initially — callers just update their `use` paths.

### Step 1a: Extract `runtime.rs`

Create `/Users/austin/Projects/phx/src/tui/runtime.rs`.

Move from `app.rs`:
- `pub async fn start_app(...)` — terminal setup, app construction, calls `run_loop`
- `async fn run_loop(...)` — the main event loop (lines ~1797-2291)
- Terminal setup/teardown (enable_raw_mode, enter_alternate_screen, etc.)

Move from `message_handler.rs`:
- `pub fn redraw(...)` — recompute display lines + terminal draw

`runtime.rs` imports `App` from `app.rs` and calls `app.render()`. It's the shell — it reads events, dispatches them, and draws frames. It doesn't know what the events mean.

**What stays in `app.rs`**: `App` struct, `impl App` (accessors, `render()`, `recompute_display_lines`, `drain_conversations`, `drain_panels`, field helpers).

**Verification**: `just lint && just test`. Launch phx — same behavior.

### Step 1b: Extract `event_handler.rs`

Create `/Users/austin/Projects/phx/src/tui/event_handler.rs`.

Move from `app.rs`:
- `fn handle_key(app: &mut App, key: KeyEvent) -> bool` (lines ~487-1015)
- `fn handle_paste(app: &mut App, text: String)` (if it exists as a separate fn)
- Any mouse-event dispatch logic currently inline in `run_loop` — extract into `pub fn handle_mouse(app: &mut App, mouse: MouseEvent)`

The current `run_loop` has mouse handling inlined (~200 lines of match arms on mouse events, lines ~1971-2256). Extract those into `handle_mouse()` in `event_handler.rs`. The event loop in `runtime.rs` becomes:

```rust
CEvent::Key(key) => { event_handler::handle_key(app, key); }
CEvent::Mouse(mouse) => { event_handler::handle_mouse(app, mouse); }
CEvent::Paste(text) => { event_handler::handle_paste(app, text); }
```

**Verification**: `just lint && just test`. All keyboard/mouse interaction works.

### Step 1c: Extract `commands.rs`

Create `/Users/austin/Projects/phx/src/tui/commands.rs`.

Move from `message_handler.rs`:
- `pub async fn handle_command(app: &mut App, input: &str)` (lines ~169-654) — the giant command dispatcher
- Any helpers that are only called from `handle_command` (e.g., `format_bullet`, internal command helpers)

**Verification**: `/help`, `/theme`, `/model`, `/solo`, `/conductor`, and all other slash commands work.

### Step 1d: Extract `conversation.rs` (tui-level)

Create `/Users/austin/Projects/phx/src/tui/conversation.rs`.

Move from `message_handler.rs`:
- `pub fn start_conversation(app: &mut App, text: String)` (line ~15)
- `pub async fn send_message(...)` (line ~710)
- `pub async fn resume_session(app: &mut App, session_id: &str)` (line ~91)
- `pub fn default_system_prompt() -> &'static str` (line ~71)
- `pub async fn activate_conductor(app: &mut App)` (line ~663)
- `fn toggle_conductor_mode(...)`, `fn handle_conductor_command(...)`, `async fn deactivate_conductor_mode(...)` — conductor lifecycle

Move from `app.rs`:
- `fn drain_conversations(...)` (lines ~326-448) — polling ConvEvent receivers
- `fn drain_panels(...)` (lines ~299-324) — polling panel updates

These are the conversation lifecycle functions. After this move, neither `app.rs` nor `message_handler.rs` contains conversation logic.

**Verification**: Send messages, watch streaming, tool calls complete, conductor mode works.

### Step 1e: Extract `reload.rs`

Create `/Users/austin/Projects/phx/src/tui/reload.rs`.

Move from `message_handler.rs`:
- `pub fn apply_reload(app: &mut App, output: ReloadOutput)` (line ~1289)
- `fn run_interactive_form(...)` if it's reload-specific

This is small but it's its own concern. After this, `message_handler.rs` should be empty or nearly empty.

### Step 1f: Delete `message_handler.rs`

After steps 1c-1e, verify `message_handler.rs` has no remaining public functions. If any remain, move them to the appropriate new file. Then delete `message_handler.rs` and remove its `pub mod` from `mod.rs`.

**Verification**: `grep -rn 'message_handler' src/tui/` returns only the deletion commit's changes (no remaining imports). `just lint && just test`.

### Step 1g: Update `src/tui/mod.rs`

Add new modules, remove `message_handler`:

```rust
pub mod commands;
pub mod conversation;
pub mod event_handler;
pub mod reload;
pub mod runtime;
// remove: pub mod message_handler;
```

### Step 1 acceptance criteria

- [ ] `app.rs` is under 800 lines (struct + render + accessors + field helpers).
- [ ] `message_handler.rs` does not exist.
- [ ] Each new file (`runtime.rs`, `event_handler.rs`, `commands.rs`, `conversation.rs`, `reload.rs`) has a single responsibility described by its name.
- [ ] No behavioral changes — all manual tests pass.
- [ ] `just lint && just test` pass.

### Step 1 verification

```bash
just lint
just test
wc -l src/tui/app.rs                          # < 800
test ! -f src/tui/message_handler.rs           # deleted
wc -l src/tui/runtime.rs src/tui/event_handler.rs src/tui/commands.rs src/tui/conversation.rs src/tui/reload.rs
```

---

## Stage 2: TEA

With files small and focused, layer the system on top.

### Step 2a: Scaffold `msg.rs`, `update.rs`, `cmd.rs`

Create the three files as described in the previous spec version. Start with scroll variants as proof-of-concept.

Update `src/tui/CLAUDE.md` with a "TEA architecture" section.

Update `src/tui/mod.rs`:
```rust
pub mod cmd;
pub mod msg;
pub mod update;
```

### Step 2b: Migrate navigation & UI state

Move all synchronous UI mutations from `event_handler.rs` into `update()`:
- Scroll (already done in 2a)
- Tab switching, tab close
- Panel focus toggle
- Picker open/close/select
- File viewer navigation
- Sidebar scroll/select
- Selection clear, hover state
- Toast show/expire
- Quit

`event_handler.rs` becomes a thin adapter: match key/mouse → construct Msg → call `update()`. The 550-line file shrinks to ~150 (just the key→Msg mapping).

### Step 2c: Migrate input handling

Move from `event_handler.rs` into `update()`:
- `Msg::InputKey(KeyEvent)` — delegates to `tab.input.handle_key_event()`
- `Msg::InputPaste(String)`
- `Msg::InputSubmit` — interprets input as command or message
- Clipboard operations
- Command completion

### Step 2d: Migrate conversation flow + introduce Cmd

Move from `conversation.rs` into `update()` + `cmd.rs`:
- `Msg::SendMessage(String)` → returns `Cmd::StartConversation`
- `Msg::ConvEventReceived { tab, event }` — replaces `drain_conversations`
- `Msg::RunCommand(String)` → dispatches commands
- Session resume, conductor activate

`Cmd::execute()` is the only place async tasks spawn. `conversation.rs` becomes helpers called by `Cmd::execute()`, not called directly by the event loop.

### Step 2e: Cleanup

- `event_handler.rs` is now ~100 lines (key→Msg mapping only)
- `conversation.rs` is now helpers for `Cmd::execute()`, not called from event loop
- `commands.rs` logic moves into `update()` under `Msg::RunCommand` — file may shrink or merge
- Verify `src/tui/CLAUDE.md` TEA section is accurate

### Stage 2 acceptance criteria

- [ ] `msg.rs` has the complete `Msg` enum — every possible action.
- [ ] `update.rs` has every state mutation — one match arm per variant.
- [ ] `cmd.rs` has every side effect — spawning conversations, reload, quit.
- [ ] `event_handler.rs` is under 200 lines — just key/mouse → Msg conversion.
- [ ] No direct `app.field = value` mutations outside `update.rs` (except in `Cmd::execute` for bootstrapping async work).
- [ ] `src/tui/CLAUDE.md` documents the TEA system.
- [ ] `just lint && just test` pass.
- [ ] All manual verification passes: messages, streaming, tools, conductor, file viewer, pickers, scroll, input.

### Stage 2 verification

```bash
just lint
just test

# Confirm no direct mutations outside update.rs:
grep -rn 'app\.active_tab\s*=' src/tui/ --include='*.rs' | grep -v update.rs | grep -v 'fn new\|Default'
# (should return nothing or only initialization code)

# Confirm event_handler is thin:
wc -l src/tui/event_handler.rs    # < 200
```

---

## Code patterns to follow

### Function extraction pattern

When moving a function from `app.rs` to a new file, keep the signature identical:

```rust
// Before (in app.rs):
pub fn handle_key(app: &mut App, key: KeyEvent) -> bool { ... }

// After (in event_handler.rs):
use crate::tui::app::App;

pub fn handle_key(app: &mut App, key: KeyEvent) -> bool { ... }

// Call site (in runtime.rs):
use crate::tui::event_handler;
event_handler::handle_key(app, key);
```

No signature changes during Stage 1. Just move and re-import.

### Module wiring pattern

From existing `src/tui/mod.rs`:
```rust
pub mod app;
pub mod components;
pub mod layout;
// ... etc
```

Add new modules in alphabetical order. Remove `message_handler` after its functions have all been migrated.

### Test patterns

See `src/tui/picker.rs:127-194` and `src/tui/theme.rs:201-248` for unit test style. For `update()` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_up_decreases_offset() {
        // Setup minimal state, call update(), assert outcome
    }
}
```

## Out of scope

- `src/tui/rendering/*` — sealed, do not modify
- `src/tui/components/*` — stateless render functions, do not modify
- `src/tui/tabs.rs`, `src/tui/input.rs`, `src/tui/picker.rs` — state structs keep their methods
- `src/tui/theme.rs`, `src/tui/layout.rs` — stable infrastructure

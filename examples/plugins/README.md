# Phoenix WASM Plugins

Plugins extend Phoenix with custom slash commands. Each plugin is a Rust crate compiled to WebAssembly (WASI P2).

## Quick Start

### 1. Create a new plugin

```bash
mkdir -p examples/plugins/my-skill/src
```

**Cargo.toml:**
```toml
[package]
name = "phoenix-plugin-my-skill"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
phoenix-plugin-sdk = { path = "../../../crates/phoenix-plugin-sdk" }
wit-bindgen = "0.41"
```

**src/lib.rs:**
```rust
use phoenix_plugin_sdk::skill;

skill! {
    name: "my-skill",
    command: "myskill",
    description: "What this skill does",
    execute(arguments) {
        Ok(SkillResult {
            context: format!("Instructions for the LLM: {arguments}"),
            toast: "Skill activated.".into(),
            widget: String::new(),
        })
    }
}
```

### 2. Build

```bash
# One-time setup
rustup target add wasm32-wasip2

# Build
cd examples/plugins/my-skill
cargo build --target wasm32-wasip2 --release
```

### 3. Run

```bash
# Load plugins from the examples directory
just run-plugins

# Or pass the directory manually
phoenix --plugin-dir examples/plugins
```

Inside Phoenix, type `/reload` to rebuild and reload all plugins without restarting.

## SkillResult

Every `execute` function returns `Result<SkillResult, String>`. The three fields are independent — leave any empty to skip it.

| Field | What it does |
|-------|-------------|
| `context` | Sent to the LLM as a message (triggers a response) |
| `toast` | Shown briefly in the dynamic island / status area |
| `widget` | JSON UI tree rendered inline in the chat area |

## Macro Forms

### Basic — slash command only

```rust
skill! {
    name: "review",
    command: "review",
    description: "Review code for simplicity",
    execute(arguments) {
        Ok(SkillResult {
            context: format!("Review this code: {arguments}"),
            toast: "Review mode.".into(),
            widget: String::new(),
        })
    }
}
```

### Toggle — keybind with enter/exit

Adds a keyboard shortcut that toggles the skill on and off.

```rust
skill! {
    name: "plan",
    command: "plan",
    description: "Plan mode",
    keybind: "shift+tab",
    execute(arguments) {
        Ok(SkillResult {
            context: format!("You are in PLAN MODE. {arguments}"),
            toast: "Plan mode activated.".into(),
            widget: String::new(),
        })
    },
    on_exit() {
        Ok(SkillResult {
            context: "You are now in AGENT MODE.".into(),
            toast: "Agent mode resumed.".into(),
            widget: String::new(),
        })
    }
}
```

## Declarative UI Widgets

Plugins can render styled widgets in the chat area using the `phoenix_plugin_sdk::ui` builder. The host renders them with theme-aware styling:

```
  ▶ Current Time
  │ 17:30:00 UTC
  │ 8 May 2026
  ╰ done
```

### Builder API

```rust
use phoenix_plugin_sdk::ui;

// Bordered box with title and styled children
let widget = ui::bordered("Current Time", &[
    &ui::text("17:30:00 UTC").bold().fg("cyan").build(),
    &ui::text("8 May 2026").dim().build(),
]);

// Other widgets
ui::text("plain text").build()           // styled text
ui::text("bold").bold().fg("green").build()  // bold green
ui::column(&[...])                       // vertical stack
ui::row(&[...])                          // horizontal layout
ui::gauge("Progress", 0.75)              // progress bar
ui::spacer()                             // blank line
```

### Text Style Options

| Method | Effect |
|--------|--------|
| `.bold()` | Bold text |
| `.italic()` | Italic text |
| `.dim()` | Dimmed/muted text |
| `.fg("color")` | Set foreground color |

Colors: `red`, `green`, `yellow`, `blue`, `cyan`, `dim`, `primary`

## Plugin Capabilities

Plugins have access to Rust's `std` library (strings, collections, formatting, `SystemTime`, etc.) but run in a WASM sandbox with **no** filesystem, network, or environment variable access.

## Examples

- **plan** — Toggles plan mode via `/plan` or Shift+Tab. Prevents file modifications.
- **now** — `/now` injects the current UTC timestamp with a styled widget display.

## Development Loop

1. Edit your plugin's `src/lib.rs`
2. Type `/reload` in Phoenix (builds + reloads all plugins)
3. Test your slash command

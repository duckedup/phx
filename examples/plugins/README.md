# phx Plugins

Plugins extend phx with custom slash commands and tools. A plugin is a directory containing a `manifest.json` that declares one or more tools. Tools can be backed by a compiled binary or a shell command.

## Plugin Types

### Shell plugins (no binary required)

A `manifest.json` with `shell` on each tool. No compilation, no binary — just a manifest:

```json
{
  "name": "git-tools",
  "version": "0.1.0",
  "tools": [
    {
      "name": "git_diff",
      "description": "Show git diff",
      "command": "diff",
      "shell": "git diff {{branch}}",
      "parameters": {
        "type": "object",
        "properties": {
          "branch": { "type": "string", "description": "Branch to diff against" }
        }
      }
    },
    {
      "name": "git_log",
      "description": "Show recent commits",
      "command": "log",
      "shell": "git log --oneline -{{count}}",
      "parameters": {
        "type": "object",
        "properties": {
          "count": { "type": "integer", "description": "Number of commits" }
        }
      }
    }
  ]
}
```

Shell tools run via `sh -c` with `{{param}}` template substitution from the tool's parameters.

### Binary plugins

A compiled binary that handles `invoke <tool> <args_json>` and returns JSON output. Built with the `tool!` macro from `phx-plugin-sdk`:

```rust
use phx_plugin_sdk::{tool, ToolOutput};

tool! {
    name: "phx-plugin-plan",
    version: "0.1.0",
    tools: [
        {
            name: "plan",
            description: "Enter plan mode",
            parameters: r#"{"type":"object","properties":{"arguments":{"type":"string"}}}"#,
            command: "plan",
            keybind: "shift+tab",
            ui: vec![],
            invoke(_name, args) {
                let arguments = args.get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(ToolOutput::with_toast(
                    format!("You are in PLAN MODE. {arguments}"),
                    "Plan mode activated.",
                ))
            },
            on_exit() {
                Ok(ToolOutput::with_toast(
                    "You are now in AGENT MODE.",
                    "Agent mode resumed.",
                ))
            }
        }
    ]
}
```

### Mixed plugins

A single manifest can mix shell and binary tools. Tool-level `bin`/`bin_args` override the top-level values:

```json
{
  "name": "dev-tools",
  "version": "0.1.0",
  "bin": "./dev-tools-binary",
  "tools": [
    {
      "name": "analyze",
      "description": "Deep analysis (uses top-level binary)"
    },
    {
      "name": "quick_diff",
      "description": "Fast diff (shell)",
      "shell": "git diff --stat"
    },
    {
      "name": "lint",
      "description": "Run linter (different binary)",
      "bin": "./custom-linter",
      "bin_args": ["--format", "json"]
    }
  ]
}
```

## Manifest Format

### Top-level fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Plugin name |
| `version` | no | Semver version string |
| `description` | no | Human-readable description |
| `bin` | no | Path to default binary (relative to plugin dir with `./`, otherwise relative to project root) |
| `bin_args` | no | Static CLI args passed to the default binary |
| `tools` | yes | Array of tool definitions |
| `commands` | no | Slash command definitions (for JSON-RPC plugins) |
| `events` | no | Event subscriptions for hooks |

### Tool fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | yes | Tool identifier (used internally) |
| `description` | no | Shown in command palette and to the LLM |
| `command` | no | Slash command name (e.g. `"plan"` registers `/plan`) |
| `keybind` | no | Keyboard shortcut (e.g. `"shift+tab"`) |
| `parameters` | no | JSON Schema for tool arguments |
| `shell` | no | Shell command template (mutually exclusive with `bin`) |
| `bin` | no | Binary path override (mutually exclusive with `shell`) |
| `bin_args` | no | Static args override for this tool's binary |
| `ui_fields` | no | Form fields shown when invoked without arguments |

Every tool needs exactly one execution strategy:
- `shell` — run via `sh -c` with template substitution
- `bin` — tool-level binary override
- Top-level `bin` — fallback for tools without `shell` or `bin`

## ToolOutput

Binary tools return JSON with these fields:

| Field | Type | Description |
|-------|------|-------------|
| `output` | string | Sent to the LLM as context (triggers a response) |
| `is_error` | bool | Whether the output represents an error |
| `toast` | string | Shown briefly in the status area |
| `widget` | string | JSON UI tree rendered inline in chat |

The SDK provides helpers: `ToolOutput::success(msg)`, `ToolOutput::error(msg)`, `ToolOutput::with_toast(output, toast)`, `ToolOutput::toast_only(toast)`, `ToolOutput::empty()`.

## Declarative UI Widgets

Plugins can render styled widgets in the chat area using the `phx_plugin_sdk::ui` builder:

```
  > Current Time
  | 17:30:00 UTC
  | 8 May 2026
  ~ done
```

```rust
use phx_plugin_sdk::ui;

let widget = ui::bordered("Current Time", &[
    &ui::text("17:30:00 UTC").bold().fg("cyan").build(),
    &ui::text("8 May 2026").dim().build(),
]);

ui::text("plain text").build()
ui::text("bold").bold().fg("green").build()
ui::column(&[...])
ui::row(&[...])
ui::gauge("Progress", 0.75)
ui::spacer()
```

### Text Style Options

| Method | Effect |
|--------|--------|
| `.bold()` | Bold text |
| `.italic()` | Italic text |
| `.dim()` | Dimmed/muted text |
| `.fg("color")` | Set foreground color |

Colors: `red`, `green`, `yellow`, `blue`, `cyan`, `dim`, `primary`

## Installation

Plugins are installed to `.phx/plugins/<name>/`. The `build-plugins` step handles this automatically:

```bash
just build-plugins
```

This scans both `plugins/` and `examples/plugins/` for:
- **Cargo plugins** — builds with `cargo build --release`, then runs `<binary> install .phx/plugins/<name>`
- **Manifest-only plugins** — copies the directory to `.phx/plugins/<name>/`

Plugins can also be installed manually by placing a directory with `manifest.json` (and optionally a binary) in `.phx/plugins/` or `~/.phx/plugins/`.

## Examples

| Plugin | Type | Slash Command | Description |
|--------|------|---------------|-------------|
| **plan** | binary | `/plan` | Toggles plan mode via slash command or Shift+Tab |
| **review** | binary | `/review` | Diffs current branch against main for code review |
| **feature** | binary | `/feature` | Scaffolds a new feature workflow with ticket context |
| **now** | binary | — | Injects current UTC timestamp with a styled widget |
| **now-bash** | shell | `/now` | Gets current time via `date -u` (no binary needed) |

## Development Loop

1. Edit your plugin (source code or `manifest.json`)
2. Type `/reload` in phx (builds + installs + reloads all plugins)
3. Test your slash command

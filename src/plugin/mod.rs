pub mod handle;
pub mod hooks;
pub mod host_handler;
pub mod manager;
pub mod manifest;
pub mod skill_hook;
pub mod tool_adapter;
pub mod transport;
pub mod ui;
pub mod wasm_runtime;
pub mod wasm_tool_adapter;

pub use manager::{PluginManager, discover_plugin_dirs};
pub use phoenix_shared::hook_types::{HookAction, HookEvent};

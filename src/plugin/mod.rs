pub mod handle;
pub mod hooks;
pub mod host_handler;
pub mod manager;
pub mod manifest;
pub mod plugin_runtime;
pub mod plugin_tool_adapter;
pub mod skill_hook;
pub mod tool_adapter;
pub mod transport;
pub mod ui;

pub use manager::{PluginManager, discover_plugin_dirs};
pub use phx_shared::hook_types::{HookAction, HookEvent};

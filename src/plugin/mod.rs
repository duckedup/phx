pub mod handle;
pub mod hooks;
pub mod manager;
pub mod manifest;
pub mod tool_adapter;
pub mod transport;
pub mod ui;

pub use hooks::{HookAction, HookEvent};
pub use manager::{PluginManager, discover_plugin_dirs};

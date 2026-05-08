mod bash;
mod edit;
pub mod orchestration;
mod read;
pub mod traits;
mod write;

pub use bash::BashTool;
pub use edit::EditTool;
pub use read::ReadTool;
pub use traits::{Tool, ToolRegistry};
pub use write::WriteTool;

use std::sync::Arc;

/// All built-in tool constructors, keyed by name.
type ToolFactory = fn() -> Arc<dyn Tool>;
static TOOL_ENTRIES: &[(&str, ToolFactory)] = &[
    ("bash", || Arc::new(BashTool)),
    ("read", || Arc::new(ReadTool)),
    ("write", || Arc::new(WriteTool)),
    ("edit", || Arc::new(EditTool)),
];

/// Look up a tool by name. Returns `None` if no built-in tool matches.
pub fn lookup(name: &str) -> Option<Arc<dyn Tool>> {
    TOOL_ENTRIES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, ctor)| ctor())
}

/// Build a registry containing only the requested tools.
/// Unknown names are silently skipped.
pub fn build_registry(names: &[&str]) -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for name in names {
        if let Some(tool) = lookup(name) {
            reg.register(tool);
        }
    }
    reg
}

/// Build a registry containing every built-in tool.
pub fn build_registry_all() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    for (_, ctor) in TOOL_ENTRIES {
        reg.register(ctor());
    }
    reg
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_finds_known_tools() {
        assert!(lookup("bash").is_some());
        assert!(lookup("read").is_some());
        assert!(lookup("write").is_some());
        assert!(lookup("edit").is_some());
    }

    #[test]
    fn lookup_returns_none_for_unknown() {
        assert!(lookup("nope").is_none());
        assert!(lookup("").is_none());
    }

    #[test]
    fn build_registry_selective() {
        let reg = build_registry(&["bash", "read"]);
        assert_eq!(reg.count(), 2);
        assert!(reg.get("bash").is_some());
        assert!(reg.get("read").is_some());
        assert!(reg.get("write").is_none());
        assert!(reg.get("edit").is_none());
    }

    #[test]
    fn build_registry_skips_unknown() {
        let reg = build_registry(&["read", "nonsense", "bash"]);
        assert_eq!(reg.count(), 2);
        assert!(reg.get("read").is_some());
        assert!(reg.get("bash").is_some());
        assert!(reg.get("nonsense").is_none());
    }

    #[test]
    fn build_registry_all_has_four() {
        let reg = build_registry_all();
        assert_eq!(reg.count(), 4);
        assert!(reg.get("bash").is_some());
        assert!(reg.get("read").is_some());
        assert!(reg.get("write").is_some());
        assert!(reg.get("edit").is_some());
    }

    #[test]
    fn list_schemas_returns_all() {
        let reg = build_registry_all();
        let schemas = reg.list_schemas();
        assert_eq!(schemas.len(), 4);
        let names: Vec<&str> = schemas.iter().map(|s| s.name).collect();
        assert!(names.contains(&"bash"));
        assert!(names.contains(&"read"));
        assert!(names.contains(&"write"));
        assert!(names.contains(&"edit"));
    }

    #[test]
    fn schema_has_required_fields() {
        let reg = build_registry_all();
        for schema in reg.list_schemas() {
            assert!(!schema.name.is_empty());
            assert!(!schema.description.is_empty());
            assert!(schema.parameters.is_object());
            assert_eq!(schema.parameters["type"], "object");
            assert!(schema.parameters["properties"].is_object());
        }
    }
}

use phoenix_plugin_sdk::{tool, ToolOutput};

tool! {
    tools: [
        {
            name: "conductor",
            description: "Toggle conductor mode — orchestrate sub-agents",
            parameters: r#"{"type":"object","properties":{}}"#,
            command: "conductor",
            keybind: "",
            ui: vec![],
            invoke(_name, _args) {
                Ok(ToolOutput::toast_only("Conductor mode activated"))
            },
            on_exit() {
                Ok(ToolOutput::toast_only("Conductor mode deactivated"))
            }
        }
    ]
}

use phoenix_plugin_sdk::ui_field_types::UiField as ToolUiField;
use phoenix_plugin_sdk::{tool, ToolOutput};

tool! {
    tools: [
        {
            name: "plan",
            description: "Enter plan mode — research and formulate a plan without changing files",
            parameters: r#"{"type":"object","properties":{"arguments":{"type":"string","description":"Plan description or context"}}}"#,
            command: "plan",
            keybind: "shift+tab",
            ui: vec![
                ToolUiField::text_area("arguments", "Describe what you want to build"),
            ],
            invoke(_name, args) {
                let arguments = args.get("arguments")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                Ok(ToolOutput::with_toast(
                    format!(
                        "You are in PLAN MODE. Do not make any changes to source files. \
                         You are to research and formulate a plan for the requested changes only. \
                         {arguments}"
                    ),
                    "Plan mode activated.",
                ))
            },
            on_exit() {
                Ok(ToolOutput::with_toast(
                    "You are now in AGENT MODE. You are free to make changes to source files \
                     and execute the plan.",
                    "Agent mode — free to edit files.",
                ))
            }
        }
    ]
}

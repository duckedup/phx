use phx_plugin_sdk::ui_field_types::UiField as ToolUiField;
use phx_plugin_sdk::{tool, ToolOutput};

tool! {
    name: "phx-plugin-feature",
    version: "0.1.0",
    tools: [
        {
            name: "feature",
            description: "Start working on a new feature",
            parameters: r#"{"type":"object","properties":{"ticket":{"type":"string","description":"Ticket or issue number"},"notes":{"type":"string","description":"Additional context or notes"}}}"#,
            command: "feature",
            ui(_name, _args) {
                vec![
                    ToolUiField::text_area("ticket", "Ticket Number").placeholder("e.g. PROJ-123"),
                    ToolUiField::text_area("notes", "Additional Notes").placeholder("Any extra context..."),
                ]
            },
            invoke(_name, args) {
                let ticket = args.get("ticket").and_then(|v| v.as_str()).unwrap_or("");
                let notes = args.get("notes").and_then(|v| v.as_str()).unwrap_or("");

                let mut context = String::from(
                    "You are starting work on a new feature. Follow this workflow:\n\
                     \n\
                     1. Understand the requirements from the ticket and any notes provided.\n\
                     2. Research the codebase to find relevant files and patterns.\n\
                     3. Create a plan before making changes.\n\
                     4. Implement the feature incrementally, testing as you go.\n\
                     5. Ensure tests cover the new functionality.\n",
                );

                if !ticket.is_empty() {
                    context.push_str(&format!("\nTicket: {ticket}\n"));
                    context.push_str("Look up this ticket for requirements and acceptance criteria.\n");
                }

                if !notes.is_empty() {
                    context.push_str(&format!("\nNotes:\n{notes}\n"));
                }

                Ok(ToolOutput::success(context))
            }
        }
    ]
}

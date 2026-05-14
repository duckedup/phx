use phoenix_plugin_sdk::ui_field_types::{UiField as ToolUiField, UiOption};
use phoenix_plugin_sdk::{tool, ToolOutput};

tool! {
    name: "phoenix-plugin-ask-user-questions",
    version: "0.1.0",
    tools: [
        {
            name: "ask_user_questions",
            description: "Ask the user clarifying questions. The model provides up to 10 questions with up to 3 answer choices each, plus an optional free-form question. Returns the user's collected answers as context.",
            parameters: r#"{"type":"object","required":["questions"],"properties":{"questions":{"type":"array","description":"Up to 10 clarifying questions","maxItems":10,"items":{"type":"object","required":["description","answers"],"properties":{"description":{"type":"string","description":"Brief question description"},"answers":{"type":"array","description":"Up to 3 potential answers","maxItems":3,"items":{"type":"object","required":["title","description"],"properties":{"title":{"type":"string","description":"Brief answer title"},"description":{"type":"string","description":"Brief answer description"}}}}}}},"freeform_question":{"type":"string","description":"A final free-form question for additional user input"}}}"#,
            ui(_name, args) {
                let questions = args.get("questions")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();

                let mut fields = Vec::new();

                for (i, question) in questions.iter().enumerate() {
                    let desc = question.get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Question");
                    let answers = question.get("answers")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();

                    let options: Vec<UiOption> = answers.iter().filter_map(|a| {
                        let title = a.get("title")?.as_str()?;
                        let description = a.get("description").and_then(|v| v.as_str()).unwrap_or("");
                        Some(UiOption::new(title, title).with_description(description))
                    }).collect();

                    fields.push(
                        ToolUiField::select_paged(format!("q{}", i + 1), desc)
                            .options(options)
                            .required()
                    );
                }

                fields
            },
            invoke(_name, _args) {
                Ok(ToolOutput::success("No interactive UI available in this context."))
            }
        }
    ]
}

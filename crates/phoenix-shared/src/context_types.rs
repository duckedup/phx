use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionContext {
    pub messages: Vec<ContextMessage>,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub activated_skills: Vec<String>,
    #[serde(default)]
    pub touched_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_call: Option<ContextToolCall>,
    #[serde(default)]
    pub tool_result: Option<ContextToolResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub args_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextToolResult {
    pub id: String,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ContextMutation {
    AppendMessage { message: ContextMessage },
    InjectContext { content: String },
    Compact { keep_last: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_mutation_roundtrip() {
        let mutation = ContextMutation::InjectContext {
            content: "You are in plan mode.".into(),
        };
        let json = serde_json::to_string(&mutation).unwrap();
        let back: ContextMutation = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ContextMutation::InjectContext { .. }));
    }

    #[test]
    fn session_context_minimal() {
        let ctx = SessionContext {
            messages: vec![],
            system_prompt: String::new(),
            activated_skills: vec![],
            touched_files: vec![],
        };
        let json = serde_json::to_string(&ctx).unwrap();
        assert!(json.contains("messages"));
    }

    #[test]
    fn context_message_with_tool_call() {
        let msg = ContextMessage {
            role: "assistant".into(),
            content: String::new(),
            tool_call: Some(ContextToolCall {
                id: "tc_1".into(),
                name: "bash".into(),
                args_json: r#"{"command":"ls"}"#.into(),
            }),
            tool_result: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: ContextMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_call.unwrap().name, "bash");
    }
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    ToolCall,
    ToolResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub id: String,
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResult>,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
            tool_call: None,
            tool_result: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
            tool_call: None,
            tool_result: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
            tool_call: None,
            tool_result: None,
        }
    }

    pub fn tool_call(tc: ToolCall) -> Self {
        Self {
            role: Role::ToolCall,
            content: String::new(),
            tool_call: Some(tc),
            tool_result: None,
        }
    }

    pub fn tool_result(tr: ToolResult) -> Self {
        Self {
            role: Role::ToolResult,
            content: String::new(),
            tool_call: None,
            tool_result: Some(tr),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_message() {
        let msg = Message::user("hello");
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, "hello");
        assert!(msg.tool_call.is_none());
        assert!(msg.tool_result.is_none());
    }

    #[test]
    fn tool_call_message() {
        let tc = ToolCall {
            id: "tc_1".into(),
            name: "bash".into(),
            args_json: r#"{"command":"ls"}"#.into(),
        };
        let msg = Message::tool_call(tc.clone());
        assert_eq!(msg.role, Role::ToolCall);
        assert_eq!(msg.tool_call.as_ref().unwrap().name, "bash");
    }

    #[test]
    fn tool_result_message() {
        let tr = ToolResult {
            id: "tc_1".into(),
            output: "file.txt".into(),
            is_error: false,
        };
        let msg = Message::tool_result(tr);
        assert_eq!(msg.role, Role::ToolResult);
        assert!(!msg.tool_result.as_ref().unwrap().is_error);
    }

    #[test]
    fn message_round_trip_json() {
        let msg = Message::assistant("hello world");
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn tool_call_message_round_trip() {
        let msg = Message::tool_call(ToolCall {
            id: "tc_2".into(),
            name: "read".into(),
            args_json: r#"{"file_path":"/tmp/x"}"#.into(),
        });
        let json = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }
}

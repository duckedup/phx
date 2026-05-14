use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchemaData {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultData {
    pub output: String,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub is_error: bool,
}

impl ToolResultData {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            truncated: false,
            is_error: false,
        }
    }

    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            truncated: false,
            is_error: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_data_success() {
        let r = ToolResultData::success("ok");
        assert_eq!(r.output, "ok");
        assert!(!r.is_error);
        assert!(!r.truncated);
    }

    #[test]
    fn tool_result_data_error() {
        let r = ToolResultData::error("fail");
        assert!(r.is_error);
    }

    #[test]
    fn tool_schema_data_roundtrip() {
        let schema = ToolSchemaData {
            name: "bash".into(),
            description: "Run a command".into(),
            parameters: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_string(&schema).unwrap();
        let back: ToolSchemaData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "bash");
    }
}

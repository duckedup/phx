use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUiConfig {
    pub fields: Vec<UiField>,
}

impl ToolUiConfig {
    pub fn new(fields: Vec<UiField>) -> Self {
        Self { fields }
    }

    pub fn empty() -> Self {
        Self { fields: vec![] }
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiField {
    pub key: String,
    pub label: String,
    pub field: UiFieldKind,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub placeholder: String,
    #[serde(default)]
    pub default_value: String,
}

impl UiField {
    pub fn text_area(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            field: UiFieldKind::TextArea,
            required: false,
            placeholder: String::new(),
            default_value: String::new(),
        }
    }

    pub fn text_input(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            field: UiFieldKind::TextInput,
            required: false,
            placeholder: String::new(),
            default_value: String::new(),
        }
    }

    pub fn toggle(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            field: UiFieldKind::Toggle,
            required: false,
            placeholder: String::new(),
            default_value: String::new(),
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn placeholder(mut self, p: impl Into<String>) -> Self {
        self.placeholder = p.into();
        self
    }

    pub fn default_value(mut self, v: impl Into<String>) -> Self {
        self.default_value = v.into();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiFieldKind {
    TextInput,
    TextArea,
    Toggle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_api() {
        let config = ToolUiConfig::new(vec![
            UiField::text_area("notes", "Notes").placeholder("Enter notes..."),
            UiField::text_input("name", "Name").required(),
        ]);
        assert_eq!(config.fields.len(), 2);
        assert!(matches!(config.fields[0].field, UiFieldKind::TextArea));
        assert_eq!(config.fields[0].placeholder, "Enter notes...");
        assert!(config.fields[1].required);
    }

    #[test]
    fn empty_config() {
        let config = ToolUiConfig::empty();
        assert!(config.is_empty());
    }

    #[test]
    fn roundtrip_json() {
        let config = ToolUiConfig::new(vec![UiField::text_area("key", "Label").placeholder("ph")]);
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ToolUiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.fields[0].key, "key");
    }
}

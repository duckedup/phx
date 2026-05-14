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
pub struct UiOption {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
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
    #[serde(default)]
    pub options: Vec<UiOption>,
    #[serde(default)]
    pub allow_other: bool,
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
            options: Vec::new(),
            allow_other: false,
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
            options: Vec::new(),
            allow_other: false,
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
            options: Vec::new(),
            allow_other: false,
        }
    }

    pub fn select(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            field: UiFieldKind::Select,
            required: false,
            placeholder: String::new(),
            default_value: String::new(),
            options: Vec::new(),
            allow_other: false,
        }
    }

    pub fn select_paged(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            field: UiFieldKind::SelectPaged,
            required: false,
            placeholder: String::new(),
            default_value: String::new(),
            options: Vec::new(),
            allow_other: true,
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

    pub fn allow_other(mut self, allow: bool) -> Self {
        self.allow_other = allow;
        self
    }

    pub fn option(
        mut self,
        value: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.options.push(UiOption {
            value: value.into(),
            label: label.into(),
            description: description.into(),
        });
        self
    }

    pub fn options(mut self, opts: Vec<UiOption>) -> Self {
        self.options = opts;
        self
    }
}

impl UiOption {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: String::new(),
        }
    }

    pub fn with_description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiFieldKind {
    TextInput,
    TextArea,
    Toggle,
    Select,
    SelectPaged,
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

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

pub struct InputRequest {
    pub widget: PluginWidget,
    pub response_tx: oneshot::Sender<PluginWidgetResponse>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "widget")]
pub enum PluginWidget {
    Notification { data: NotificationData },
    ConfirmDialog { data: ConfirmDialogData },
    Picker { data: PickerData },
    TextInput { data: TextInputData },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationData {
    pub text: String,
    #[serde(default = "default_level")]
    pub level: String,
}

fn default_level() -> String {
    "info".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfirmDialogData {
    #[serde(default)]
    pub title: String,
    pub message: String,
    pub options: Vec<DialogOption>,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DialogOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickerData {
    #[serde(default)]
    pub title: String,
    pub items: Vec<PickerItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextInputData {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub masked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginWidgetResponse {
    Selection { selection: String },
    SelectedItem { selected: SelectedId },
    TextValue { value: String },
    Cancelled { cancelled: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectedId {
    pub id: String,
}

pub enum PluginUIState {
    ConfirmDialog {
        plugin_name: String,
        data: ConfirmDialogData,
        selected: usize,
    },
    Picker {
        plugin_name: String,
        data: PickerData,
        selected: usize,
        filter: String,
    },
    TextInput {
        plugin_name: String,
        data: TextInputData,
        buffer: String,
    },
}

impl PluginUIState {
    pub fn from_widget(plugin_name: String, widget: PluginWidget) -> Option<Self> {
        match widget {
            PluginWidget::Notification { .. } => None,
            PluginWidget::ConfirmDialog { data } => {
                let selected = data
                    .default
                    .as_ref()
                    .and_then(|d| data.options.iter().position(|o| o.id == *d))
                    .unwrap_or(0);
                Some(Self::ConfirmDialog {
                    plugin_name,
                    data,
                    selected,
                })
            }
            PluginWidget::Picker { data } => Some(Self::Picker {
                plugin_name,
                data,
                selected: 0,
                filter: String::new(),
            }),
            PluginWidget::TextInput { data } => {
                let buffer = data.default.clone();
                Some(Self::TextInput {
                    plugin_name,
                    data,
                    buffer,
                })
            }
        }
    }

    pub fn resolve(&self) -> PluginWidgetResponse {
        match self {
            Self::ConfirmDialog { data, selected, .. } => {
                let id = data
                    .options
                    .get(*selected)
                    .map(|o| o.id.clone())
                    .unwrap_or_default();
                PluginWidgetResponse::Selection { selection: id }
            }
            Self::Picker {
                data: _, selected, ..
            } => {
                let filtered = self.filtered_picker_items();
                let id = filtered
                    .get(*selected)
                    .map(|i| i.id.clone())
                    .unwrap_or_default();
                PluginWidgetResponse::SelectedItem {
                    selected: SelectedId { id },
                }
            }
            Self::TextInput { buffer, .. } => PluginWidgetResponse::TextValue {
                value: buffer.clone(),
            },
        }
    }

    pub fn cancel_response() -> PluginWidgetResponse {
        PluginWidgetResponse::Cancelled { cancelled: true }
    }

    pub fn filtered_picker_items(&self) -> Vec<&PickerItem> {
        match self {
            Self::Picker { data, filter, .. } => {
                if filter.is_empty() {
                    data.items.iter().collect()
                } else {
                    let lower = filter.to_lowercase();
                    data.items
                        .iter()
                        .filter(|i| {
                            i.label.to_lowercase().contains(&lower)
                                || i.description.to_lowercase().contains(&lower)
                        })
                        .collect()
                }
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_deserialize() {
        let json = r#"{"widget": "notification", "data": {"text": "hello"}}"#;
        let widget: PluginWidget = serde_json::from_str(json).unwrap();
        assert!(matches!(widget, PluginWidget::Notification { .. }));
    }

    #[test]
    fn confirm_dialog_deserialize() {
        let json = r#"{
            "widget": "confirm_dialog",
            "data": {
                "title": "Delete?",
                "message": "Are you sure?",
                "options": [{"id": "yes", "label": "Yes"}, {"id": "no", "label": "No"}],
                "default": "no"
            }
        }"#;
        let widget: PluginWidget = serde_json::from_str(json).unwrap();
        if let PluginWidget::ConfirmDialog { data } = widget {
            assert_eq!(data.options.len(), 2);
            assert_eq!(data.default, Some("no".into()));
        } else {
            panic!("expected ConfirmDialog");
        }
    }

    #[test]
    fn picker_deserialize() {
        let json = r#"{
            "widget": "picker",
            "data": {
                "title": "Choose",
                "items": [{"id": "a", "label": "Alpha", "description": "First"}]
            }
        }"#;
        let widget: PluginWidget = serde_json::from_str(json).unwrap();
        assert!(matches!(widget, PluginWidget::Picker { .. }));
    }

    #[test]
    fn text_input_deserialize() {
        let json = r#"{"widget": "text_input", "data": {"title": "Key", "prompt": "Enter:", "masked": true}}"#;
        let widget: PluginWidget = serde_json::from_str(json).unwrap();
        if let PluginWidget::TextInput { data } = widget {
            assert!(data.masked);
        } else {
            panic!("expected TextInput");
        }
    }

    #[test]
    fn confirm_dialog_state() {
        let widget = PluginWidget::ConfirmDialog {
            data: ConfirmDialogData {
                title: "Delete?".into(),
                message: "Sure?".into(),
                options: vec![
                    DialogOption {
                        id: "yes".into(),
                        label: "Yes".into(),
                    },
                    DialogOption {
                        id: "no".into(),
                        label: "No".into(),
                    },
                ],
                default: Some("no".into()),
            },
        };
        let state = PluginUIState::from_widget("test".into(), widget).unwrap();
        if let PluginUIState::ConfirmDialog { selected, .. } = &state {
            assert_eq!(*selected, 1); // "no" is index 1
        }
        let resp = state.resolve();
        if let PluginWidgetResponse::Selection { selection } = resp {
            assert_eq!(selection, "no");
        } else {
            panic!("expected Selection");
        }
    }

    #[test]
    fn notification_returns_none_state() {
        let widget = PluginWidget::Notification {
            data: NotificationData {
                text: "hello".into(),
                level: "info".into(),
            },
        };
        assert!(PluginUIState::from_widget("test".into(), widget).is_none());
    }

    #[test]
    fn picker_filter() {
        let widget = PluginWidget::Picker {
            data: PickerData {
                title: "Choose".into(),
                items: vec![
                    PickerItem {
                        id: "a".into(),
                        label: "Alpha".into(),
                        description: String::new(),
                    },
                    PickerItem {
                        id: "b".into(),
                        label: "Beta".into(),
                        description: String::new(),
                    },
                ],
            },
        };
        let mut state = PluginUIState::from_widget("test".into(), widget).unwrap();
        if let PluginUIState::Picker { ref mut filter, .. } = state {
            *filter = "bet".into();
        }
        let items = state.filtered_picker_items();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "b");
    }
}

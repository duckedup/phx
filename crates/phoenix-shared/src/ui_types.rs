use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UiNode {
    #[serde(rename = "text")]
    Text {
        content: String,
        #[serde(default)]
        style: TextStyle,
    },
    #[serde(rename = "box")]
    Box {
        #[serde(default)]
        title: String,
        #[serde(default)]
        children: Vec<UiNode>,
    },
    #[serde(rename = "column")]
    Column {
        #[serde(default)]
        children: Vec<UiNode>,
    },
    #[serde(rename = "row")]
    Row {
        #[serde(default)]
        children: Vec<UiNode>,
    },
    #[serde(rename = "gauge")]
    Gauge {
        #[serde(default)]
        label: String,
        #[serde(default)]
        ratio: f64,
    },
    #[serde(rename = "spacer")]
    Spacer,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TextStyle {
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub dim: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<String>,
}

impl UiNode {
    pub fn text(content: impl Into<String>) -> Self {
        UiNode::Text {
            content: content.into(),
            style: TextStyle::default(),
        }
    }

    pub fn text_styled(content: impl Into<String>, style: TextStyle) -> Self {
        UiNode::Text {
            content: content.into(),
            style,
        }
    }

    pub fn boxed(title: impl Into<String>, children: Vec<UiNode>) -> Self {
        UiNode::Box {
            title: title.into(),
            children,
        }
    }

    pub fn column(children: Vec<UiNode>) -> Self {
        UiNode::Column { children }
    }

    pub fn row(children: Vec<UiNode>) -> Self {
        UiNode::Row { children }
    }

    pub fn gauge(label: impl Into<String>, ratio: f64) -> Self {
        UiNode::Gauge {
            label: label.into(),
            ratio,
        }
    }

    pub fn spacer() -> Self {
        UiNode::Spacer
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

impl TextStyle {
    pub fn bold() -> Self {
        Self {
            bold: true,
            ..Default::default()
        }
    }

    pub fn dim() -> Self {
        Self {
            dim: true,
            ..Default::default()
        }
    }

    pub fn with_fg(mut self, color: impl Into<String>) -> Self {
        self.fg = Some(color.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_node_roundtrip() {
        let node = UiNode::text_styled(
            "hello",
            TextStyle {
                bold: true,
                fg: Some("cyan".into()),
                ..Default::default()
            },
        );
        let json = serde_json::to_string(&node).unwrap();
        let back: UiNode = serde_json::from_str(&json).unwrap();
        if let UiNode::Text { content, style } = back {
            assert_eq!(content, "hello");
            assert!(style.bold);
            assert_eq!(style.fg.as_deref(), Some("cyan"));
        } else {
            panic!("expected Text node");
        }
    }

    #[test]
    fn box_node_roundtrip() {
        let node = UiNode::boxed(
            "Title",
            vec![UiNode::text("child1"), UiNode::text("child2")],
        );
        let json = serde_json::to_string(&node).unwrap();
        let back: UiNode = serde_json::from_str(&json).unwrap();
        if let UiNode::Box { title, children } = back {
            assert_eq!(title, "Title");
            assert_eq!(children.len(), 2);
        } else {
            panic!("expected Box node");
        }
    }

    #[test]
    fn gauge_node() {
        let node = UiNode::gauge("Progress", 0.75);
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("gauge"));
        assert!(json.contains("0.75"));
    }

    #[test]
    fn deserialize_from_plugin_json() {
        let json = r#"{"type":"text","content":"hello","style":{"bold":true}}"#;
        let node: UiNode = serde_json::from_str(json).unwrap();
        assert!(matches!(node, UiNode::Text { .. }));
    }
}

/// Typed declarative UI nodes for Phoenix plugins.
///
/// Build a widget tree with full type safety, then call `.to_json()`
/// to serialize for the host.
///
/// # Example
/// ```rust,ignore
/// use phoenix_plugin_sdk::ui::{UiNode, TextNode, BoxNode};
///
/// let widget = BoxNode::new("Current Time")
///     .child(TextNode::new("17:30:00 UTC").bold().fg("cyan"))
///     .child(TextNode::new("8 May 2026").dim())
///     .into_node()
///     .to_json();
/// ```

/// A node in the declarative UI tree.
pub enum UiNode {
    Text(TextNode),
    Box(BoxNode),
    Column(Vec<UiNode>),
    Row(Vec<UiNode>),
    Gauge { label: String, ratio: f64 },
    Spacer,
}

impl UiNode {
    pub fn to_json(&self) -> String {
        match self {
            UiNode::Text(t) => t.to_json(),
            UiNode::Box(b) => b.to_json(),
            UiNode::Column(children) => {
                let kids: Vec<String> = children.iter().map(|c| c.to_json()).collect();
                format!(r#"{{"type":"column","children":[{}]}}"#, kids.join(","))
            }
            UiNode::Row(children) => {
                let kids: Vec<String> = children.iter().map(|c| c.to_json()).collect();
                format!(r#"{{"type":"row","children":[{}]}}"#, kids.join(","))
            }
            UiNode::Gauge { label, ratio } => {
                format!(
                    r#"{{"type":"gauge","label":{},"ratio":{ratio}}}"#,
                    json_str(label)
                )
            }
            UiNode::Spacer => r#"{"type":"spacer"}"#.to_string(),
        }
    }
}

/// Styled text node.
pub struct TextNode {
    pub content: String,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub fg: String,
}

impl TextNode {
    pub fn new(content: &str) -> Self {
        Self {
            content: content.to_string(),
            bold: false,
            italic: false,
            dim: false,
            fg: String::new(),
        }
    }

    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    /// Set foreground color: `"red"`, `"green"`, `"yellow"`, `"blue"`, `"cyan"`, `"dim"`, `"primary"`
    pub fn fg(mut self, color: &str) -> Self {
        self.fg = color.to_string();
        self
    }

    pub fn into_node(self) -> UiNode {
        UiNode::Text(self)
    }

    pub fn to_json(&self) -> String {
        let mut s = format!(r#"{{"type":"text","content":{}"#, json_str(&self.content));
        s.push_str(r#","style":{"#);
        let mut parts = Vec::new();
        if self.bold {
            parts.push(r#""bold":true"#.to_string());
        }
        if self.italic {
            parts.push(r#""italic":true"#.to_string());
        }
        if self.dim {
            parts.push(r#""dim":true"#.to_string());
        }
        if !self.fg.is_empty() {
            parts.push(format!(r#""fg":{}"#, json_str(&self.fg)));
        }
        s.push_str(&parts.join(","));
        s.push_str("}}");
        s
    }
}

/// Bordered box with a title header and child nodes.
///
/// Renders as:
/// ```text
///   ▶ Title
///   │ child content
///   ╰ done
/// ```
pub struct BoxNode {
    pub title: String,
    pub children: Vec<UiNode>,
}

impl BoxNode {
    pub fn new(title: &str) -> Self {
        Self {
            title: title.to_string(),
            children: Vec::new(),
        }
    }

    pub fn child(mut self, node: impl Into<UiNode>) -> Self {
        self.children.push(node.into());
        self
    }

    pub fn into_node(self) -> UiNode {
        UiNode::Box(self)
    }

    pub fn to_json(&self) -> String {
        let kids: Vec<String> = self.children.iter().map(|c| c.to_json()).collect();
        format!(
            r#"{{"type":"box","title":{},"children":[{}]}}"#,
            json_str(&self.title),
            kids.join(","),
        )
    }
}

impl From<TextNode> for UiNode {
    fn from(t: TextNode) -> Self {
        UiNode::Text(t)
    }
}

impl From<BoxNode> for UiNode {
    fn from(b: BoxNode) -> Self {
        UiNode::Box(b)
    }
}

/// Convenience: column of nodes.
pub fn column(children: Vec<UiNode>) -> UiNode {
    UiNode::Column(children)
}

/// Convenience: row of nodes.
pub fn row(children: Vec<UiNode>) -> UiNode {
    UiNode::Row(children)
}

/// Convenience: gauge widget.
pub fn gauge(label: &str, ratio: f64) -> UiNode {
    UiNode::Gauge {
        label: label.to_string(),
        ratio,
    }
}

/// Convenience: spacer.
pub fn spacer() -> UiNode {
    UiNode::Spacer
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            '\t' => out.push_str(r"\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

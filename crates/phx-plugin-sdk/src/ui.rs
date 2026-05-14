pub use phx_shared::ui_types::{TextStyle, UiNode};

pub struct TextNode {
    content: String,
    bold: bool,
    italic: bool,
    dim: bool,
    fg: String,
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

    pub fn fg(mut self, color: &str) -> Self {
        self.fg = color.to_string();
        self
    }

    pub fn into_node(self) -> UiNode {
        UiNode::Text {
            content: self.content,
            style: TextStyle {
                bold: self.bold,
                italic: self.italic,
                dim: self.dim,
                fg: if self.fg.is_empty() {
                    None
                } else {
                    Some(self.fg)
                },
            },
        }
    }
}

pub struct BoxNode {
    title: String,
    children: Vec<UiNode>,
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
        UiNode::Box {
            title: self.title,
            children: self.children,
        }
    }
}

impl From<TextNode> for UiNode {
    fn from(t: TextNode) -> Self {
        t.into_node()
    }
}

impl From<BoxNode> for UiNode {
    fn from(b: BoxNode) -> Self {
        b.into_node()
    }
}

pub fn column(children: Vec<UiNode>) -> UiNode {
    UiNode::Column { children }
}

pub fn row(children: Vec<UiNode>) -> UiNode {
    UiNode::Row { children }
}

pub fn gauge(label: &str, ratio: f64) -> UiNode {
    UiNode::Gauge {
        label: label.to_string(),
        ratio,
    }
}

pub fn spacer() -> UiNode {
    UiNode::Spacer
}

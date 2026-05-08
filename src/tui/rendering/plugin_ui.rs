use ratatui::prelude::*;
use serde::Deserialize;

use super::display::DisplayLine;
use super::helpers::wrap_text;
use crate::tui::theme::Theme;

#[derive(Deserialize)]
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

#[derive(Deserialize, Default)]
pub struct TextStyle {
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub dim: bool,
    #[serde(default)]
    pub fg: Option<String>,
}

pub fn render_ui_json(
    lines: &mut Vec<DisplayLine>,
    json: &str,
    theme: &Theme,
    content_width: usize,
    pad: u16,
) {
    let node: Result<UiNode, _> = serde_json::from_str(json);
    match node {
        Ok(node) => render_node(lines, &node, theme, content_width, pad),
        Err(e) => {
            let indent = " ".repeat(pad as usize);
            lines.push(DisplayLine::styled(
                &format!("{indent}  plugin UI error: {e}"),
                Style::default().fg(theme.error),
            ));
        }
    }
    lines.push(DisplayLine::empty());
}

fn render_node(lines: &mut Vec<DisplayLine>, node: &UiNode, theme: &Theme, width: usize, pad: u16) {
    let indent = " ".repeat(pad as usize);
    match node {
        UiNode::Text { content, style } => {
            let restyle = resolve_style(style, theme);
            for wl in wrap_text(content, width.saturating_sub(4)) {
                lines.push(DisplayLine::styled(&format!("{indent}  {wl}"), restyle));
            }
        }
        UiNode::Box { title, children } => {
            render_box(lines, title, children, theme, width, pad);
        }
        UiNode::Column { children } => {
            for child in children {
                render_node(lines, child, theme, width, pad);
            }
        }
        UiNode::Row { children } => {
            let mut spans = vec![(format!("{indent}  "), Style::default())];
            for child in children {
                collect_row_spans(&mut spans, child, theme);
                spans.push(("  ".to_string(), Style::default()));
            }
            lines.push(DisplayLine::multi(spans));
        }
        UiNode::Gauge { label, ratio } => {
            render_gauge(lines, label, *ratio, theme, width, pad);
        }
        UiNode::Spacer => {
            lines.push(DisplayLine::empty());
        }
    }
}

fn render_box(
    lines: &mut Vec<DisplayLine>,
    title: &str,
    children: &[UiNode],
    theme: &Theme,
    width: usize,
    pad: u16,
) {
    let indent = " ".repeat(pad as usize);
    let border_color = theme.info;
    let title_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let border_style = Style::default().fg(border_color);
    let inner_width = width.saturating_sub(6);

    if !title.is_empty() {
        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  "), Style::default()),
            (
                "▶ ".to_string(),
                Style::default()
                    .fg(border_color)
                    .add_modifier(Modifier::BOLD),
            ),
            (title.to_string(), title_style),
        ]));
    }

    for child in children {
        let mut child_lines = Vec::new();
        render_node_flat(&mut child_lines, child, theme, inner_width);
        for (text, style) in child_lines {
            lines.push(DisplayLine::multi(vec![
                (format!("{indent}  │ "), border_style),
                (text, style),
            ]));
        }
    }

    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  ╰ "), border_style),
        ("done".to_string(), Style::default().fg(theme.success)),
    ]));
}

fn render_node_flat(out: &mut Vec<(String, Style)>, node: &UiNode, theme: &Theme, width: usize) {
    match node {
        UiNode::Text { content, style } => {
            let restyle = resolve_style(style, theme);
            for wl in wrap_text(content, width) {
                out.push((wl, restyle));
            }
        }
        UiNode::Column { children } | UiNode::Box { children, .. } => {
            for child in children {
                render_node_flat(out, child, theme, width);
            }
        }
        UiNode::Row { children } => {
            let mut combined = String::new();
            for child in children {
                if let UiNode::Text { content, .. } = child {
                    if !combined.is_empty() {
                        combined.push_str("  ");
                    }
                    combined.push_str(content);
                }
            }
            out.push((combined, Style::default().fg(theme.foreground)));
        }
        UiNode::Gauge { label, ratio } => {
            let bar_width = width.saturating_sub(2);
            let filled = (bar_width as f64 * ratio.clamp(0.0, 1.0)) as usize;
            let empty = bar_width.saturating_sub(filled);
            let bar = format!("{}{}", "█".repeat(filled), "░".repeat(empty));
            if !label.is_empty() {
                out.push((label.clone(), Style::default().fg(theme.dim())));
            }
            out.push((bar, Style::default().fg(theme.info)));
        }
        UiNode::Spacer => {
            out.push((String::new(), Style::default()));
        }
    }
}

fn render_gauge(
    lines: &mut Vec<DisplayLine>,
    label: &str,
    ratio: f64,
    theme: &Theme,
    width: usize,
    pad: u16,
) {
    let indent = " ".repeat(pad as usize);
    let bar_width = width.saturating_sub(6);
    let filled = (bar_width as f64 * ratio.clamp(0.0, 1.0)) as usize;
    let empty = bar_width.saturating_sub(filled);
    let border_style = Style::default().fg(theme.info);

    if !label.is_empty() {
        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  │ "), border_style),
            (label.to_string(), Style::default().fg(theme.foreground)),
        ]));
    }

    let pct = format!("{:.0}%", ratio * 100.0);
    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  │ "), border_style),
        ("█".repeat(filled), Style::default().fg(theme.info)),
        ("░".repeat(empty), Style::default().fg(theme.dim())),
        (format!(" {pct}"), Style::default().fg(theme.foreground)),
    ]));
}

fn collect_row_spans(spans: &mut Vec<(String, Style)>, node: &UiNode, theme: &Theme) {
    match node {
        UiNode::Text { content, style } => {
            spans.push((content.clone(), resolve_style(style, theme)));
        }
        UiNode::Column { children } | UiNode::Row { children } => {
            for child in children {
                collect_row_spans(spans, child, theme);
            }
        }
        _ => {}
    }
}

fn resolve_style(ts: &TextStyle, theme: &Theme) -> Style {
    let mut s = Style::default();
    s = s.fg(match ts.fg.as_deref() {
        Some("red") => theme.error,
        Some("green") => theme.success,
        Some("yellow") => theme.warning,
        Some("blue") => theme.info,
        Some("cyan") => theme.accent,
        Some("dim") | Some("gray") | Some("grey") => theme.dim(),
        Some("primary") => theme.primary,
        _ => theme.foreground,
    });
    if ts.bold {
        s = s.add_modifier(Modifier::BOLD);
    }
    if ts.italic {
        s = s.add_modifier(Modifier::ITALIC);
    }
    if ts.dim {
        s = s.fg(theme.dim());
    }
    s
}

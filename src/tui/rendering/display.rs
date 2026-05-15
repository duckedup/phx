use ratatui::prelude::*;
use ratatui::text::{Line, Span};

use crate::session::message::Role;
use crate::tui::rendering::diff::{is_diff_content, render_side_by_side_diff};
use crate::tui::rendering::helpers::wrap_text;
use crate::tui::rendering::markdown::render_markdown;
use crate::tui::tabs::{AssistantLine, ChatItem, ChatLine, WidgetKind};
use crate::tui::theme::Theme;

pub struct DisplayLine {
    pub spans: Vec<(String, Style)>,
}

impl DisplayLine {
    pub fn styled(text: &str, style: Style) -> Self {
        Self {
            spans: vec![(text.to_string(), style)],
        }
    }

    pub fn multi(parts: Vec<(String, Style)>) -> Self {
        Self { spans: parts }
    }

    pub fn empty() -> Self {
        Self { spans: vec![] }
    }

    pub fn to_line(&self) -> Line<'static> {
        Line::from(
            self.spans
                .iter()
                .map(|(t, s)| Span::styled(t.clone(), *s))
                .collect::<Vec<_>>(),
        )
    }
}

pub fn build_item_display_lines(
    lines: &mut Vec<DisplayLine>,
    item: &ChatItem,
    theme: &Theme,
    content_width: usize,
    pad: u16,
) {
    match item {
        ChatItem::Line(cl) => build_chat_display_lines(lines, cl, theme, content_width, pad),
        ChatItem::Assistant(al) => {
            build_assistant_display_lines(lines, al, theme, content_width, pad)
        }
        ChatItem::Widget(w) => build_widget_display_lines(lines, w, theme, content_width, pad),
        ChatItem::ContextLoaded(names) => build_context_loaded_lines(lines, names, theme, pad),
    }
}

fn build_widget_display_lines(
    lines: &mut Vec<DisplayLine>,
    widget: &WidgetKind,
    theme: &Theme,
    content_width: usize,
    pad: u16,
) {
    crate::tui::rendering::plugin_ui::render_ui_json(
        lines,
        &widget.json,
        theme,
        content_width,
        pad,
    );
}

fn build_context_loaded_lines(
    lines: &mut Vec<DisplayLine>,
    names: &[String],
    theme: &Theme,
    pad: u16,
) {
    let indent = " ".repeat(pad as usize);
    let dim = Style::default().fg(theme.dim());
    let accent = Style::default().fg(theme.accent);
    let info_style = Style::default().fg(theme.info);

    lines.push(DisplayLine::empty());
    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        ("◆ ".to_string(), accent),
        ("context resolved".to_string(), dim),
    ]));

    for (i, name) in names.iter().enumerate() {
        let is_last = i + 1 == names.len();
        let connector = if is_last { "╰" } else { "├" };
        lines.push(DisplayLine::multi(vec![
            (format!("{indent}    "), Style::default()),
            (format!("{connector} "), dim),
            (name.clone(), info_style),
        ]));
    }

    lines.push(DisplayLine::empty());
}

fn build_assistant_display_lines(
    lines: &mut Vec<DisplayLine>,
    al: &AssistantLine,
    theme: &Theme,
    content_width: usize,
    pad: u16,
) {
    let indent = " ".repeat(pad as usize);
    let body_indent = format!("{}  ", indent);

    let mut label_spans = vec![
        (format!("{}  ", indent), Style::default()),
        ("✦ ".to_string(), Style::default().fg(theme.warning)),
        (
            "phx".to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if al.turn > 0 {
        label_spans.push((format!(" · T{}", al.turn), Style::default().fg(theme.dim())));
    }
    lines.push(DisplayLine::multi(label_spans));
    let md_lines = render_markdown(
        &al.content,
        theme,
        &body_indent,
        content_width.saturating_sub(2),
    );
    lines.extend(md_lines);
    lines.push(DisplayLine::empty());
}

fn build_chat_display_lines(
    lines: &mut Vec<DisplayLine>,
    cl: &ChatLine,
    theme: &Theme,
    content_width: usize,
    pad: u16,
) {
    let indent = " ".repeat(pad as usize);
    let body_indent = format!("{}  ", indent);

    match cl.role {
        Role::User => {
            let full_width = content_width + (pad as usize) * 2;
            let bg = theme.user_msg_bg();
            let border_style = Style::default().fg(theme.user_msg_border()).bg(bg);
            let text_style = Style::default().fg(theme.foreground).bg(bg);
            let header_style = Style::default()
                .fg(theme.primary)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            let fill_style = Style::default().bg(bg);

            let header_text = " you";
            let header_pad = full_width.saturating_sub(1 + header_text.len());
            lines.push(DisplayLine::multi(vec![
                ("▎".to_string(), border_style),
                (header_text.to_string(), header_style),
                (" ".repeat(header_pad), fill_style),
            ]));

            let wrap_width = full_width.saturating_sub(3);
            for wl in wrap_text(&cl.content, wrap_width) {
                let line_pad = full_width.saturating_sub(1 + 1 + wl.len());
                lines.push(DisplayLine::multi(vec![
                    ("▎".to_string(), border_style),
                    (" ".to_string(), fill_style),
                    (wl, text_style),
                    (" ".repeat(line_pad), fill_style),
                ]));
            }

            let bottom_pad = full_width.saturating_sub(1);
            lines.push(DisplayLine::multi(vec![
                ("▎".to_string(), border_style),
                (" ".repeat(bottom_pad), fill_style),
            ]));
            lines.push(DisplayLine::empty());
        }
        Role::Assistant => {
            // Assistant messages should use ChatItem::Assistant; this is a fallback.
            let body_md = render_markdown(
                &cl.content,
                theme,
                &body_indent,
                content_width.saturating_sub(2),
            );
            lines.extend(body_md);
            lines.push(DisplayLine::empty());
        }
        Role::ToolCall => {
            lines.push(DisplayLine::multi(vec![
                (format!("{}  ", indent), Style::default()),
                (
                    "▶ ".to_string(),
                    Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
                ),
                (
                    cl.content.clone(),
                    Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        Role::ToolResult => {
            let is_error = cl.content.starts_with("Error:")
                || cl.content.starts_with("error:")
                || cl.content.starts_with("unknown tool:");

            if !is_error && is_diff_content(&cl.content) {
                render_side_by_side_diff(lines, &cl.content, theme, &indent, content_width);
            } else {
                let border_color = if is_error {
                    theme.error
                } else {
                    theme.tool_border()
                };
                let result_width = content_width.saturating_sub(6);

                let output_lines: Vec<&str> = cl.content.lines().collect();
                let max_display = 20;
                let truncated = output_lines.len() > max_display;

                let show_lines: Vec<&str> = if truncated {
                    let mut v: Vec<&str> = output_lines[..10].to_vec();
                    v.push("");
                    v.extend_from_slice(&output_lines[output_lines.len().saturating_sub(5)..]);
                    v
                } else {
                    output_lines.clone()
                };

                let hidden_count = if truncated {
                    output_lines.len().saturating_sub(15)
                } else {
                    0
                };

                for (i, line_text) in show_lines.iter().enumerate() {
                    if truncated && i == 10 {
                        lines.push(DisplayLine::multi(vec![
                            (format!("{}  │ ", indent), Style::default().fg(border_color)),
                            (
                                format!("... ({hidden_count} more lines)"),
                                Style::default()
                                    .fg(theme.dim())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                        continue;
                    }

                    let wrapped = wrap_text(line_text, result_width);
                    for wl in wrapped {
                        let text_style = diff_line_style(&wl, theme, is_error);
                        lines.push(DisplayLine::multi(vec![
                            (format!("{}  │ ", indent), Style::default().fg(border_color)),
                            (wl, text_style),
                        ]));
                    }
                }

                let (icon, icon_color) = if is_error {
                    ("✗", theme.error)
                } else {
                    ("✓", theme.success)
                };
                lines.push(DisplayLine::multi(vec![
                    (format!("{}  ╰ ", indent), Style::default().fg(border_color)),
                    (format!("{icon} done"), Style::default().fg(icon_color)),
                ]));
            }
            lines.push(DisplayLine::empty());
        }
        Role::System => {
            let sys_style = Style::default()
                .fg(theme.dim())
                .add_modifier(Modifier::ITALIC);
            let wrap_width = content_width.saturating_sub(4);
            let content_lines: Vec<&str> = cl.content.split('\n').collect();
            if content_lines.len() <= 1 {
                let text = format!("{indent}  ── {} ──", cl.content);
                lines.push(DisplayLine::styled(&text, sys_style));
            } else {
                lines.push(DisplayLine::styled(&format!("{indent}  ──────"), sys_style));
                for content_line in &content_lines {
                    if content_line.is_empty() {
                        lines.push(DisplayLine::empty());
                    } else {
                        for wl in wrap_text(content_line, wrap_width) {
                            lines.push(DisplayLine::styled(
                                &format!("{body_indent}{wl}"),
                                sys_style,
                            ));
                        }
                    }
                }
                lines.push(DisplayLine::styled(&format!("{indent}  ──────"), sys_style));
            }
            lines.push(DisplayLine::empty());
        }
    }
}

fn diff_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error {
        return Style::default().fg(theme.error);
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Style::default().fg(theme.diff_add)
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Style::default().fg(theme.diff_delete)
    } else if trimmed.starts_with("@@") {
        Style::default()
            .fg(theme.info)
            .add_modifier(Modifier::ITALIC)
    } else {
        Style::default().fg(theme.dim())
    }
}

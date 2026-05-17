use std::path::PathBuf;

use ratatui::prelude::*;
use ratatui::text::{Line, Span};

use crate::session::message::Role;
use crate::tui::rendering::diff::{is_diff_content, render_diff};
use crate::tui::rendering::helpers::wrap_text;
use crate::tui::rendering::markdown::render_markdown;
use crate::tui::rendering::measure::{display_width, expand_tabs};
use crate::tui::tabs::{AssistantLine, ChatItem, ChatLine, WidgetKind};
use crate::tui::theme::Theme;

pub struct DisplayLine {
    pub spans: Vec<(String, Style)>,
    pub file_path: Option<PathBuf>,
}

impl DisplayLine {
    pub fn styled(text: &str, style: Style) -> Self {
        Self {
            spans: vec![(text.to_string(), style)],
            file_path: None,
        }
    }

    pub fn multi(parts: Vec<(String, Style)>) -> Self {
        Self {
            spans: parts,
            file_path: None,
        }
    }

    pub fn empty() -> Self {
        Self {
            spans: vec![],
            file_path: None,
        }
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
    full_content_width: usize,
    pad: u16,
) {
    match item {
        ChatItem::Line(cl) => {
            build_chat_display_lines(lines, cl, theme, content_width, full_content_width, pad)
        }
        ChatItem::Assistant(al) => {
            build_assistant_display_lines(lines, al, theme, content_width, pad)
        }
        ChatItem::Widget(w) => build_widget_display_lines(lines, w, theme, content_width, pad),
        ChatItem::ContextLoaded(names) => build_context_loaded_lines(lines, names, theme, pad),
        ChatItem::FileSummary(paths) => build_file_summary_lines(lines, paths, theme, pad),
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

fn build_file_summary_lines(
    lines: &mut Vec<DisplayLine>,
    paths: &[PathBuf],
    theme: &Theme,
    pad: u16,
) {
    if paths.is_empty() {
        return;
    }
    let indent = " ".repeat(pad as usize);
    let dim = Style::default().fg(theme.dim());
    let accent = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let file_style = Style::default().fg(theme.info);

    lines.push(DisplayLine::empty());
    lines.push(DisplayLine::multi(vec![
        (indent.clone(), Style::default()),
        ("\u{25c6} ".to_string(), accent),
        (
            format!(
                "{} file{} changed",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" }
            ),
            dim,
        ),
    ]));

    for (i, path) in paths.iter().enumerate() {
        let is_last = i + 1 == paths.len();
        let connector = if is_last { "\u{2570}" } else { "\u{251c}" };
        let display = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");
        let dir = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|d| format!("{d}/"))
            .unwrap_or_default();
        let mut dl = DisplayLine::multi(vec![
            (format!("{indent}  "), Style::default()),
            (format!("{connector} "), dim),
            (dir, dim),
            (display.to_string(), file_style),
        ]);
        dl.file_path = Some(path.clone());
        lines.push(dl);
    }

    lines.push(DisplayLine::empty());
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
    let body_indent = " ".repeat(pad as usize);

    let md_lines = render_markdown(&al.content, theme, &body_indent, content_width);
    lines.extend(md_lines);
    lines.push(DisplayLine::empty());
}

fn build_chat_display_lines(
    lines: &mut Vec<DisplayLine>,
    cl: &ChatLine,
    theme: &Theme,
    content_width: usize,
    full_content_width: usize,
    pad: u16,
) {
    let indent = " ".repeat(pad as usize);

    match cl.role {
        Role::User => {
            let full_width = full_content_width;
            let bg = theme.user_msg_bg();
            let border_style = Style::default().fg(theme.user_msg_border()).bg(bg);
            let text_style = Style::default().fg(theme.foreground).bg(bg);
            let header_style = Style::default()
                .fg(theme.primary)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            let fill_style = Style::default().bg(bg);

            let header_text = " you";
            let header_pad = full_width.saturating_sub(1 + display_width(header_text));
            lines.push(DisplayLine::multi(vec![
                ("▎".to_string(), border_style),
                (header_text.to_string(), header_style),
                (" ".repeat(header_pad), fill_style),
            ]));

            let wrap_width = full_width.saturating_sub(3);
            for wl in wrap_text(&cl.content, wrap_width) {
                let line_pad = full_width.saturating_sub(1 + 1 + display_width(&wl));
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
            let body_md = render_markdown(&cl.content, theme, &indent, content_width);
            lines.extend(body_md);
            lines.push(DisplayLine::empty());
        }
        Role::ToolCall => {
            let (tool_name, detail) = cl.content.split_once(" > ").unwrap_or((&cl.content, ""));
            let clickable_path = match tool_name {
                "write" | "read" | "edit" if !detail.is_empty() => {
                    Some(PathBuf::from(detail.trim()))
                }
                _ => None,
            };

            let mut header = DisplayLine::multi(vec![
                (indent.to_string(), Style::default()),
                (
                    "▶ ".to_string(),
                    Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
                ),
                (
                    tool_name.to_string(),
                    Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
                ),
            ]);
            header.file_path = clickable_path.clone();
            lines.push(header);

            if !detail.is_empty() {
                let result_width = content_width.saturating_sub(4);
                for wl in wrap_text(detail, result_width) {
                    let mut dl = DisplayLine::multi(vec![
                        (
                            format!("{indent}│ "),
                            Style::default().fg(theme.tool_border()),
                        ),
                        (wl, Style::default().fg(theme.dim())),
                    ]);
                    dl.file_path = clickable_path.clone();
                    lines.push(dl);
                }
            }
        }
        Role::ToolResult => {
            let is_error = cl.content.starts_with("Error:")
                || cl.content.starts_with("error:")
                || cl.content.starts_with("unknown tool:");

            if !is_error && is_diff_content(&cl.content) {
                render_diff(lines, &cl.content, theme, &indent, content_width);
            } else {
                let border_color = if is_error {
                    theme.error
                } else {
                    theme.tool_border()
                };
                let result_width = content_width.saturating_sub(4);

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
                            (format!("{indent}│ "), Style::default().fg(border_color)),
                            (
                                format!("... ({hidden_count} more lines)"),
                                Style::default()
                                    .fg(theme.dim())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                        continue;
                    }

                    let expanded = expand_tabs(line_text);
                    let wrapped = wrap_text(&expanded, result_width);
                    for wl in wrapped {
                        let text_style = diff_line_style(&wl, theme, is_error);
                        lines.push(DisplayLine::multi(vec![
                            (format!("{indent}│ "), Style::default().fg(border_color)),
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
                    (format!("{indent}╰ "), Style::default().fg(border_color)),
                    (format!("{icon} done"), Style::default().fg(icon_color)),
                ]));
            }
            lines.push(DisplayLine::empty());
        }
        Role::System => {
            let sys_style = Style::default()
                .fg(theme.dim())
                .add_modifier(Modifier::ITALIC);
            let wrap_width = content_width.saturating_sub(2);
            let content_lines: Vec<&str> = cl.content.split('\n').collect();
            if content_lines.len() <= 1 {
                let text = format!("{indent}── {} ──", cl.content);
                lines.push(DisplayLine::styled(&text, sys_style));
            } else {
                lines.push(DisplayLine::styled(&format!("{indent}──────"), sys_style));
                for content_line in &content_lines {
                    if content_line.is_empty() {
                        lines.push(DisplayLine::empty());
                    } else {
                        for wl in wrap_text(content_line, wrap_width) {
                            lines.push(DisplayLine::styled(&format!("{indent}{wl}"), sys_style));
                        }
                    }
                }
                lines.push(DisplayLine::styled(&format!("{indent}──────"), sys_style));
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

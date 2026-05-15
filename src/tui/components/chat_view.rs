use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::session::message::Role;
use crate::tui::layout::CHAT_PADDING;
use crate::tui::rendering::display::{DisplayLine, build_item_display_lines};
use crate::tui::rendering::helpers::{spinner_color, spinner_frame};
use crate::tui::rendering::markdown::render_markdown;
use crate::tui::tabs::{ChatItem, ChatLine, Tab};
use crate::tui::theme::Theme;

fn is_check_agents_call(item: &ChatItem) -> bool {
    matches!(item, ChatItem::Line(ChatLine { role: Role::ToolCall, content }) if content.contains("check_agents"))
}

fn is_tool_result(item: &ChatItem) -> bool {
    matches!(
        item,
        ChatItem::Line(ChatLine {
            role: Role::ToolResult,
            ..
        })
    )
}

pub fn render_chat(
    frame: &mut Frame,
    area: Rect,
    display_lines: &[DisplayLine],
    effective_scroll: usize,
    theme: &Theme,
) {
    render_chat_with_panel(frame, area, display_lines, effective_scroll, theme, None);
}

pub fn render_chat_with_panel(
    frame: &mut Frame,
    area: Rect,
    display_lines: &[DisplayLine],
    effective_scroll: usize,
    theme: &Theme,
    panel: Option<Rect>,
) {
    let visible = area.height as usize;
    let full_width = area.width as usize;

    for (i, dl) in display_lines
        .iter()
        .skip(effective_scroll)
        .take(visible)
        .enumerate()
    {
        let y = area.y + i as u16;
        let row_width = if let Some(p) = panel {
            if y >= p.y && y < p.y + p.height {
                (p.x.saturating_sub(area.x)) as usize
            } else {
                full_width
            }
        } else {
            full_width
        };

        let line = if row_width == 0 {
            Line::from("")
        } else {
            let mut l = dl.to_line();
            let text_width: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            if text_width > row_width {
                l = truncate_line(&l, row_width);
            }
            let actual: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            if actual < row_width {
                l.spans.push(Span::styled(
                    " ".repeat(row_width - actual),
                    Style::default().bg(theme.background),
                ));
            }
            l
        };

        let row_rect = Rect {
            x: area.x,
            y,
            width: full_width as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(theme.background)),
            row_rect,
        );
    }

    let rendered = display_lines
        .len()
        .saturating_sub(effective_scroll)
        .min(visible);
    for i in rendered..visible {
        let y = area.y + i as u16;
        let row_width = if let Some(p) = panel {
            if y >= p.y && y < p.y + p.height {
                (p.x.saturating_sub(area.x)) as usize
            } else {
                full_width
            }
        } else {
            full_width
        };
        let row_rect = Rect {
            x: area.x,
            y,
            width: row_width as u16,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new("").style(Style::default().bg(theme.background)),
            row_rect,
        );
    }
}

fn truncate_line(line: &Line<'static>, max_chars: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut remaining = max_chars;
    for span in &line.spans {
        let count = span.content.chars().count();
        if remaining == 0 {
            break;
        }
        if count <= remaining {
            spans.push(span.clone());
            remaining -= count;
        } else {
            let truncated: String = span.content.chars().take(remaining).collect();
            spans.push(Span::styled(truncated, span.style));
            remaining = 0;
        }
    }
    Line::from(spans)
}

pub fn compute_display_lines(
    tab: Option<&Tab>,
    theme: &Theme,
    is_running: bool,
    frame_tick: u64,
    width: u16,
    _turn_count: u32,
) -> Vec<DisplayLine> {
    let pad = CHAT_PADDING;
    let content_width = (width as usize).saturating_sub(pad as usize * 2);
    if content_width == 0 {
        return Vec::new();
    }

    let tab = match tab {
        Some(t) => t,
        None => {
            return vec![
                DisplayLine::empty(),
                DisplayLine::styled(
                    "  Press Enter to start a new session.",
                    Style::default().fg(theme.dim()),
                ),
            ];
        }
    };

    let mut lines = Vec::new();

    let items = &tab.chat_lines;
    let mut i = 0;
    while i < items.len() {
        if is_check_agents_call(&items[i]) {
            let mut last_call = i;
            let mut last_result = None;
            let mut j = i;
            while j < items.len() {
                if is_check_agents_call(&items[j]) {
                    last_call = j;
                    if j + 1 < items.len() && is_tool_result(&items[j + 1]) {
                        last_result = Some(j + 1);
                        j += 2;
                    } else {
                        j += 1;
                    }
                } else {
                    break;
                }
            }
            build_item_display_lines(&mut lines, &items[last_call], theme, content_width, pad);
            if let Some(ri) = last_result {
                build_item_display_lines(&mut lines, &items[ri], theme, content_width, pad);
            }
            i = j;
        } else {
            build_item_display_lines(&mut lines, &items[i], theme, content_width, pad);
            i += 1;
        }
    }

    if !tab.streaming_text.is_empty() {
        let body_indent = format!("{}  ", " ".repeat(pad as usize));
        let md_lines = render_markdown(
            &tab.streaming_text,
            theme,
            &body_indent,
            content_width.saturating_sub(2),
        );
        lines.extend(md_lines);
    }

    let is_thinking = is_running && tab.streaming_text.is_empty() && tab.stream_buffer.is_empty();
    if is_thinking {
        let frame_idx = (frame_tick / 4) as usize;
        let spin = spinner_frame(frame_idx);
        let color = spinner_color(frame_idx, theme);

        let last_was_check = items
            .iter()
            .rev()
            .find(|item| {
                matches!(
                    item,
                    ChatItem::Line(ChatLine {
                        role: Role::ToolCall,
                        ..
                    })
                )
            })
            .is_some_and(is_check_agents_call);

        if last_was_check {
            let dots = &["   ", ".  ", ".. ", "..."][(frame_tick / 8) as usize % 4];
            lines.push(DisplayLine::multi(vec![
                (format!("{}  ", " ".repeat(pad as usize)), Style::default()),
                (
                    format!("{spin} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                (
                    format!("monitoring agents{dots}"),
                    Style::default().fg(theme.dim()),
                ),
            ]));
        } else {
            let thinking_msgs = ["thinking", "thinking.", "thinking..", "thinking..."];
            let msg = thinking_msgs[(frame_tick / 12) as usize % thinking_msgs.len()];
            lines.push(DisplayLine::multi(vec![
                (format!("{}  ", " ".repeat(pad as usize)), Style::default()),
                (
                    format!("{spin} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                (msg.to_string(), Style::default().fg(theme.dim())),
            ]));
        }
    }

    lines
}

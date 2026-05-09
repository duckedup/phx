use ratatui::prelude::*;
use ratatui::widgets::{Clear, Paragraph};

use crate::tui::layout::CHAT_PADDING;
use crate::tui::rendering::display::{DisplayLine, build_item_display_lines};
use crate::tui::rendering::helpers::spinner_frame;
use crate::tui::rendering::markdown::render_markdown;
use crate::tui::tabs::Tab;
use crate::tui::theme::Theme;

pub fn render_chat(
    frame: &mut Frame,
    area: Rect,
    display_lines: &[DisplayLine],
    effective_scroll: usize,
    theme: &Theme,
) {
    frame.render_widget(Clear, area);

    let visible = area.height as usize;
    let width = area.width as usize;

    let visible_lines: Vec<Line<'static>> = display_lines
        .iter()
        .skip(effective_scroll)
        .take(visible)
        .map(|dl| {
            let mut line = dl.to_line();
            let text_width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
            if text_width < width {
                line.spans.push(Span::styled(
                    " ".repeat(width - text_width),
                    Style::default().bg(theme.background),
                ));
            }
            line
        })
        .collect();

    let chat = Paragraph::new(visible_lines).style(Style::default().bg(theme.background));
    frame.render_widget(chat, area);
}

pub fn compute_display_lines(
    tab: Option<&Tab>,
    theme: &Theme,
    is_running: bool,
    frame_tick: u64,
    width: u16,
    turn_count: u32,
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

    for item in &tab.chat_lines {
        build_item_display_lines(&mut lines, item, theme, content_width, pad);
    }

    if !tab.streaming_text.is_empty() {
        let mut label_spans = vec![
            (format!("{}  ", " ".repeat(pad as usize)), Style::default()),
            ("✦ ".to_string(), Style::default().fg(theme.warning)),
            (
                "phoenix".to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        if turn_count > 0 {
            label_spans.push((
                format!(" · T{turn_count}"),
                Style::default().fg(theme.dim()),
            ));
        }
        lines.push(DisplayLine::multi(label_spans));
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
        let thinking_msgs = ["thinking", "thinking.", "thinking..", "thinking..."];
        let msg = thinking_msgs[(frame_tick / 12) as usize % thinking_msgs.len()];
        lines.push(DisplayLine::multi(vec![
            (format!("{}  ", " ".repeat(pad as usize)), Style::default()),
            (
                format!("{spin} "),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            (msg.to_string(), Style::default().fg(theme.dim())),
        ]));
    }

    lines
}

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::rendering::helpers::{format_tokens, spinner_frame};
use crate::tui::theme::Theme;

pub struct SessionTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
}

pub struct StatusState<'a> {
    pub tokens: Option<SessionTokens>,
    pub is_running: bool,
    pub provider_info: &'a str,
    pub frame_tick: u64,
}

pub fn render_status(frame: &mut Frame, area: Rect, state: &StatusState<'_>, theme: &Theme) {
    let bg = theme.status_bar_bg();
    let fg = theme.status_bar_fg();
    let dim = theme.status_bar_dim();

    let spinner = if state.is_running {
        spinner_frame((state.frame_tick / 4) as usize)
    } else {
        "✦"
    };

    let left = format!(" {spinner} phoenix");

    let mut right_parts: Vec<String> = Vec::new();
    if let Some(ref t) = state.tokens {
        let mut tok = format!("{}↓ {}↑", format_tokens(t.input), format_tokens(t.output));
        if t.cache_read > 0 {
            tok.push_str(&format!(" {}⚡", format_tokens(t.cache_read)));
        }
        right_parts.push(tok);
    }
    if !state.provider_info.is_empty() {
        right_parts.push(state.provider_info.to_string());
    }
    let right = format!("{} ", right_parts.join(" · "));

    let pad_w = (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let padding = " ".repeat(pad_w);

    let line = Line::from(vec![
        Span::styled(
            left,
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(padding, Style::default().bg(bg)),
        Span::styled(right, Style::default().fg(dim).bg(bg)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

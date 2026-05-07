use std::time::{Duration, Instant};

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::theme::Theme;

pub struct Toast {
    pub message: String,
    pub created: Instant,
    pub ttl: Duration,
}

impl Toast {
    pub fn new(message: String, ttl: Duration) -> Self {
        Self {
            message,
            created: Instant::now(),
            ttl,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created.elapsed() >= self.ttl
    }
}

pub fn render_toast(frame: &mut Frame, anchor_area: Rect, toast: &Toast, theme: &Theme) {
    let msg = &toast.message;
    let text_len = msg.chars().count();
    let width = (text_len + 4).min(anchor_area.width as usize) as u16;
    let x = anchor_area.x + (anchor_area.width.saturating_sub(width)) / 2;
    let y = anchor_area.y.saturating_sub(1);

    let bg = theme.status_bar_bg();
    let fg = theme.status_bar_fg();

    let padded = format!("  {msg}  ");
    let line = Line::from(Span::styled(
        padded,
        Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
    ));

    let area = Rect {
        x,
        y,
        width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(line), area);
}

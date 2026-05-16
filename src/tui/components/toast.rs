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
    use ratatui::widgets::{Block, BorderType, Borders, Clear};

    let msg = &toast.message;
    let text_len = msg.chars().count();
    let inner_w = text_len + 2;
    let box_w = (inner_w + 2).min(anchor_area.width as usize) as u16;
    let box_h = 3u16;
    let x = anchor_area.x + (anchor_area.width.saturating_sub(box_w)) / 2;
    let y = anchor_area.y.saturating_sub(box_h);

    let elapsed = toast.created.elapsed().as_millis() as f64;
    let fade_in = (elapsed / 120.0).min(1.0);
    let remaining = toast.ttl.as_millis() as f64 - elapsed;
    let fade_out = if remaining < 400.0 {
        (remaining / 400.0).max(0.0)
    } else {
        1.0
    };
    let opacity = fade_in * fade_out;

    let fg = Theme::blend(theme.foreground, theme.background, 1.0 - opacity);
    let border_fg = Theme::blend(theme.accent, theme.background, 1.0 - opacity);

    let area = Rect {
        x,
        y,
        width: box_w,
        height: box_h,
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_fg))
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let padded = format!(" {msg} ");
    let line = Line::from(Span::styled(
        padded,
        Style::default().fg(fg).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(
        Paragraph::new(line)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::default().bg(theme.background)),
        inner,
    );
}

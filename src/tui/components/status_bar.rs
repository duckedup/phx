use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::rendering::helpers::{format_tokens, spinner_frame};
use crate::tui::theme::Theme;

pub struct SessionTokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
}

pub struct ContextUsage {
    pub used: u64,
    pub capacity: u32,
}

pub struct StatusState<'a> {
    pub tokens: Option<SessionTokens>,
    pub cost: Option<f64>,
    pub context: Option<ContextUsage>,
    pub is_running: bool,
    pub provider_info: &'a str,
    pub frame_tick: u64,
}

fn format_cost(cost: f64) -> String {
    if cost < 0.01 {
        format!("${:.4}", cost)
    } else if cost < 1.0 {
        format!("${:.3}", cost)
    } else {
        format!("${:.2}", cost)
    }
}

fn format_context(ctx: &ContextUsage) -> String {
    let pct = if ctx.capacity > 0 {
        (ctx.used as f64 / ctx.capacity as f64 * 100.0) as u32
    } else {
        0
    };
    format!(
        "{}% ({}/{})",
        pct,
        format_tokens(ctx.used),
        format_tokens(ctx.capacity as u64)
    )
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

    let left = format!(" {spinner} phx");

    let mut right_parts: Vec<String> = Vec::new();
    if let Some(ref t) = state.tokens {
        let mut tok = format!("{}↑ {}↓", format_tokens(t.input), format_tokens(t.output));
        if t.cache_read > 0 {
            tok.push_str(&format!(" {}⚡", format_tokens(t.cache_read)));
        }
        right_parts.push(tok);
    }
    if let Some(ref ctx) = state.context {
        right_parts.push(format_context(ctx));
    }
    if let Some(cost) = state.cost {
        right_parts.push(format_cost(cost));
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

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::rendering::helpers::{format_tokens, spinner_color, spinner_frame};
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
    pub conductor_mode: bool,
    pub conductor_panel_hidden: bool,
    pub agent_count: usize,
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
    let bg = theme.background;
    let dim = theme.dim();

    let frame_idx = (state.frame_tick / 4) as usize;
    let (spinner, spin_color) = if state.is_running {
        (spinner_frame(frame_idx), spinner_color(frame_idx, theme))
    } else {
        ("◆", theme.accent)
    };

    let mut left_spans: Vec<Span> = vec![
        Span::styled("    ", Style::default()),
        Span::styled(
            spinner,
            Style::default().fg(spin_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" phx", Style::default().fg(dim)),
    ];

    if state.conductor_mode {
        if state.conductor_panel_hidden {
            left_spans.push(Span::styled(" · conductor ", Style::default().fg(dim)));
            left_spans.push(Span::styled("◇", Style::default().fg(theme.warning)));
        } else {
            left_spans.push(Span::styled(" · conductor", Style::default().fg(dim)));
        }
    }

    let mut right_parts: Vec<String> = Vec::new();
    if state.conductor_mode && state.agent_count > 0 {
        right_parts.push(format!("{} agents", state.agent_count));
    }
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
    let right = format!("{}    ", right_parts.join(" · "));

    let left_len: usize = left_spans.iter().map(|s| s.content.chars().count()).sum();
    let pad_w = (area.width as usize).saturating_sub(left_len + right.chars().count());

    left_spans.push(Span::styled(" ".repeat(pad_w), Style::default()));
    left_spans.push(Span::styled(right, Style::default().fg(dim)));

    let line = Line::from(left_spans);
    frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), area);
}

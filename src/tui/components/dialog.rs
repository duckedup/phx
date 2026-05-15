use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::theme::Theme;

// ── Centered popup ──────────────────────────────────────────────

pub fn centered(term: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(term.width.saturating_sub(2));
    let h = h.min(term.height.saturating_sub(2));
    Rect {
        x: term.x + (term.width.saturating_sub(w)) / 2,
        y: term.y + (term.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

pub fn render_backdrop(frame: &mut Frame, theme: &Theme) {
    let bg = Theme::blend(theme.background, Color::Rgb(0, 0, 0), 0.4);
    frame.render_widget(
        Block::default().style(Style::default().bg(bg)),
        frame.area(),
    );
}

pub fn dialog_block(theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.background))
        .padding(Padding::new(2, 1, 1, 0))
}

// ── Step indicator ──────────────────────────────────────────────

pub fn step_indicator<'a>(labels: &[&'a str], current: usize, theme: &Theme) -> Line<'a> {
    let mut spans = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let (dot, style) = if i < current {
            ("●", Style::default().fg(theme.success))
        } else if i == current {
            (
                "◉",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
        } else {
            ("○", Style::default().fg(theme.dim()))
        };
        let label_style = if i == current {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim())
        };
        spans.push(Span::styled(format!("{dot} "), style));
        spans.push(Span::styled(*label, label_style));
        if i + 1 < labels.len() {
            spans.push(Span::styled("   ", Style::default()));
        }
    }
    Line::from(spans)
}

// ── Selectable list item ────────────────────────────────────────

pub fn list_item<'a>(
    label: &'a str,
    desc: Option<&'a str>,
    selected: bool,
    theme: &Theme,
) -> Vec<Line<'a>> {
    let bg = if selected {
        Theme::blend(theme.accent, theme.background, 0.75)
    } else {
        theme.background
    };
    let name_style = if selected {
        Style::default()
            .fg(theme.accent)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.foreground).bg(bg)
    };
    let desc_style = Style::default().fg(theme.dim()).bg(bg);
    let pad = Style::default().bg(bg);

    let mut lines = vec![Line::from(vec![
        Span::styled(if selected { "▸ " } else { "  " }, name_style),
        Span::styled(label, name_style),
        // fill rest of line with bg
        Span::styled("", pad),
    ])];

    if let Some(d) = desc {
        lines.push(Line::from(vec![
            Span::styled("  ", desc_style),
            Span::styled(d, desc_style),
            Span::styled("", pad),
        ]));
    }
    lines
}

// ── Text input field ────────────────────────────────────────────

pub fn input_field<'a>(value: &'a str, area_width: u16, theme: &Theme) -> Line<'a> {
    let bg = theme.input_bg();
    let field_w = (area_width as usize).saturating_sub(1);
    let display = if value.is_empty() { " " } else { value };
    let padded = if display.len() < field_w {
        format!("{display}{}", " ".repeat(field_w - display.len()))
    } else {
        display[display.len() - field_w..].to_string()
    };
    Line::from(Span::styled(
        padded,
        Style::default().fg(theme.primary).bg(bg),
    ))
}

pub fn masked_input<'a>(value: &str, area_width: u16, theme: &Theme) -> Line<'a> {
    let bg = theme.input_bg();
    let field_w = (area_width as usize).saturating_sub(1);
    let masked = if value.is_empty() {
        String::new()
    } else {
        let tail = &value[value.len().saturating_sub(4)..];
        let stars = value.len().saturating_sub(4);
        format!("{}{}", "*".repeat(stars), tail)
    };
    let display = if masked.is_empty() {
        " ".to_string()
    } else {
        masked
    };
    let padded = if display.len() < field_w {
        format!("{display}{}", " ".repeat(field_w - display.len()))
    } else {
        display[display.len() - field_w..].to_string()
    };
    Line::from(Span::styled(
        padded,
        Style::default().fg(theme.primary).bg(bg),
    ))
}

// ── Section label ───────────────────────────────────────────────

pub fn heading<'a>(text: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(theme.foreground)
            .add_modifier(Modifier::BOLD),
    ))
}

pub fn hint<'a>(text: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(Span::styled(text, Style::default().fg(theme.dim())))
}

pub fn hint_owned(text: String, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(text, Style::default().fg(theme.dim())))
}

pub fn hint_italic(text: String, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default()
            .fg(theme.dim())
            .add_modifier(Modifier::ITALIC),
    ))
}

pub fn warning_line<'a>(text: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(vec![
        Span::styled("▲ ", Style::default().fg(theme.warning)),
        Span::styled(text, Style::default().fg(theme.warning)),
    ])
}

pub fn success_line(text: String, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled("● ", Style::default().fg(theme.success)),
        Span::styled(text, Style::default().fg(theme.success)),
    ])
}

pub fn footer_hints<'a>(text: &'a str, theme: &Theme) -> Line<'a> {
    Line::from(Span::styled(text, Style::default().fg(theme.dim())))
}

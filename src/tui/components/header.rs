use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::rendering::helpers::{spinner_color, spinner_frame};
use crate::tui::theme::Theme;

pub fn render_header(
    frame: &mut Frame,
    area: Rect,
    tab_name: &str,
    provider_info: &str,
    is_running: bool,
    frame_tick: u64,
    theme: &Theme,
) {
    let frame_idx = (frame_tick / 4) as usize;
    let (spinner, spin_color) = if is_running {
        (spinner_frame(frame_idx), spinner_color(frame_idx, theme))
    } else {
        ("◆", theme.accent)
    };

    let label = format!(" phx · {tab_name}");
    let right = format!("{}  ", provider_info);
    let pad_width = (area.width as usize)
        .saturating_sub(1 + spinner.len() + 1 + label.chars().count() + right.chars().count());
    let padding = " ".repeat(pad_width);

    let header_line = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            spinner,
            Style::default().fg(spin_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            label,
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(padding, Style::default()),
        Span::styled(right, Style::default().fg(theme.dim())),
    ]);

    let header = Paragraph::new(header_line)
        .style(Style::default().fg(theme.foreground).bg(theme.header_bg()));
    frame.render_widget(header, area);
}

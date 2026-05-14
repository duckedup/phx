use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::rendering::helpers::spinner_frame;
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
    let spinner = if is_running {
        let frame_idx = (frame_tick / 4) as usize;
        spinner_frame(frame_idx)
    } else {
        "✦"
    };

    let left = format!(" {spinner} phx · {tab_name}");
    let right = format!("{}  ", provider_info);
    let pad_width =
        (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
    let padding = " ".repeat(pad_width);

    let header_line = Line::from(vec![
        Span::styled(
            left,
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

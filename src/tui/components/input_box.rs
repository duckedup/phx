use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::input::InputState;
use crate::tui::theme::Theme;

pub fn render_input(frame: &mut Frame, area: Rect, input: &InputState, theme: &Theme) {
    let bg = theme.background;
    let border_fg = theme.separator();
    let sep = "─".repeat(area.width as usize);

    let top = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(sep).style(Style::default().fg(border_fg).bg(bg)),
        top,
    );

    let input_area = Rect {
        x: area.x,
        y: area.y + 1,
        width: area.width,
        height: area.height.saturating_sub(1),
    };

    let prompt_str = "  > ";
    let prompt_len = prompt_str.len();
    let prompt_style = Style::default()
        .fg(theme.primary)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.foreground);
    let cont_str = "  … ";

    if input.is_empty() {
        let content = Line::from(vec![
            Span::styled(prompt_str, prompt_style),
            Span::styled("Send a message...", Style::default().fg(theme.dim())),
        ]);
        frame.render_widget(
            Paragraph::new(content).style(Style::default().bg(bg)),
            input_area,
        );
        let cursor_x = input_area.x + prompt_len as u16;
        frame.set_cursor_position((cursor_x, input_area.y));
        return;
    }

    let visible_rows = input_area.height as usize;
    let text_width = (input_area.width as usize).saturating_sub(prompt_len);
    if text_width == 0 || visible_rows == 0 {
        return;
    }

    // Build wrapped rows: (line_idx, row_text)
    let mut rows: Vec<(usize, String)> = Vec::new();
    let mut cursor_row = 0usize;
    let mut cursor_x_in_row = 0usize;

    for (li, line) in input.lines.iter().enumerate() {
        if line.is_empty() {
            if li == input.cursor_line {
                cursor_row = rows.len();
                cursor_x_in_row = 0;
            }
            rows.push((li, String::new()));
            continue;
        }

        let chars: Vec<char> = line.chars().collect();
        let mut offset = 0;
        while offset < chars.len() {
            let end = (offset + text_width).min(chars.len());
            let chunk: String = chars[offset..end].iter().collect();

            if li == input.cursor_line && input.cursor_col >= offset && input.cursor_col <= end {
                if input.cursor_col == end && end < chars.len() {
                    // cursor is at the wrap boundary — it'll be at start of next row
                } else {
                    cursor_row = rows.len();
                    cursor_x_in_row = input.cursor_col - offset;
                }
            }

            rows.push((li, chunk));
            offset = end;
        }

        // Handle cursor at end-of-line that wrapped exactly to boundary
        if li == input.cursor_line
            && input.cursor_col == chars.len()
            && chars.len().is_multiple_of(text_width)
            && !chars.is_empty()
        {
            cursor_row = rows.len();
            cursor_x_in_row = 0;
            rows.push((li, String::new()));
        }
    }

    // Scroll so cursor is visible
    let scroll = if cursor_row >= visible_rows {
        cursor_row - visible_rows + 1
    } else {
        0
    };

    for (i, (li, text)) in rows.iter().skip(scroll).take(visible_rows).enumerate() {
        let row_y = input_area.y + i as u16;
        let prefix = if i == 0 && scroll == 0 {
            if *li == 0 { prompt_str } else { cont_str }
        } else if scroll + i > 0 {
            let prev_li = if scroll + i > 0 {
                rows.get(scroll + i - 1).map(|(l, _)| *l).unwrap_or(0)
            } else {
                0
            };
            if *li != prev_li { cont_str } else { "    " }
        } else {
            prompt_str
        };

        // first row of first line always gets prompt
        let is_first_of_first_line = *li == 0 && (scroll + i == 0);
        let pref = if is_first_of_first_line {
            prompt_str
        } else if scroll + i > 0
            && *li
                != rows
                    .get(scroll + i - 1)
                    .map(|(l, _)| *l)
                    .unwrap_or(usize::MAX)
        {
            cont_str
        } else if scroll + i == 0 {
            prompt_str
        } else {
            let _ = prefix;
            "    "
        };

        let line = Line::from(vec![
            Span::styled(pref, prompt_style),
            Span::styled(text.clone(), text_style),
        ]);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(bg)),
            Rect {
                x: input_area.x,
                y: row_y,
                width: input_area.width,
                height: 1,
            },
        );
    }

    let cursor_screen_row = cursor_row.saturating_sub(scroll);
    if cursor_screen_row < visible_rows {
        let cx = input_area.x + prompt_len as u16 + cursor_x_in_row as u16;
        let cy = input_area.y + cursor_screen_row as u16;
        frame.set_cursor_position((cx.min(input_area.right().saturating_sub(1)), cy));
    }
}

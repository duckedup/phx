use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::input::InputState;
use crate::tui::theme::Theme;

fn is_selected(line: usize, col: usize, sel: &((usize, usize), (usize, usize))) -> bool {
    let &((sr, sc), (er, ec)) = sel;
    if line < sr || line > er {
        return false;
    }
    if line == sr && line == er {
        return col >= sc && col < ec;
    }
    if line == sr {
        return col >= sc;
    }
    if line == er {
        return col < ec;
    }
    true
}

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
    let select_style = Style::default().fg(theme.background).bg(theme.foreground);
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

    let lines = input.lines();
    let cursor_line = input.cursor_line();
    let cursor_col = input.cursor_col();
    let selection = input.textarea.selection_range();

    // Build wrapped rows: (line_idx, char_start, row_text)
    let mut rows: Vec<(usize, usize, String)> = Vec::new();
    let mut cursor_row = 0usize;
    let mut cursor_x_in_row = 0usize;

    for (li, line) in lines.iter().enumerate() {
        if line.is_empty() {
            if li == cursor_line {
                cursor_row = rows.len();
                cursor_x_in_row = 0;
            }
            rows.push((li, 0, String::new()));
            continue;
        }

        let chars: Vec<char> = line.chars().collect();
        let mut offset = 0;
        while offset < chars.len() {
            let end = (offset + text_width).min(chars.len());
            let chunk: String = chars[offset..end].iter().collect();

            if li == cursor_line && cursor_col >= offset && cursor_col <= end {
                if cursor_col == end && end < chars.len() {
                    // cursor is at the wrap boundary — it'll be at start of next row
                } else {
                    cursor_row = rows.len();
                    cursor_x_in_row = cursor_col - offset;
                }
            }

            rows.push((li, offset, chunk));
            offset = end;
        }

        // Handle cursor at end-of-line that wrapped exactly to boundary
        if li == cursor_line
            && cursor_col == chars.len()
            && chars.len().is_multiple_of(text_width)
            && !chars.is_empty()
        {
            cursor_row = rows.len();
            cursor_x_in_row = 0;
            rows.push((li, chars.len(), String::new()));
        }
    }

    // Scroll so cursor is visible
    let scroll = if cursor_row >= visible_rows {
        cursor_row - visible_rows + 1
    } else {
        0
    };

    for (i, (li, char_start, text)) in rows.iter().skip(scroll).take(visible_rows).enumerate() {
        let row_y = input_area.y + i as u16;

        // first row of first line always gets prompt
        let is_first_of_first_line = *li == 0 && (scroll + i == 0);
        let pref = if is_first_of_first_line {
            prompt_str
        } else if scroll + i > 0
            && *li
                != rows
                    .get(scroll + i - 1)
                    .map(|(l, _, _)| *l)
                    .unwrap_or(usize::MAX)
        {
            cont_str
        } else if scroll + i == 0 {
            prompt_str
        } else {
            "    "
        };

        let text_spans = if let Some(ref sel) = selection {
            build_selected_spans(text, *li, *char_start, sel, text_style, select_style)
        } else {
            vec![Span::styled(text.clone(), text_style)]
        };

        let mut spans = vec![Span::styled(pref, prompt_style)];
        spans.extend(text_spans);
        let line = Line::from(spans);
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

fn build_selected_spans<'a>(
    text: &str,
    line_idx: usize,
    char_start: usize,
    sel: &((usize, usize), (usize, usize)),
    normal: Style,
    highlight: Style,
) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut current_selected = false;

    for (i, ch) in text.chars().enumerate() {
        let col = char_start + i;
        let selected = is_selected(line_idx, col, sel);
        if selected != current_selected && !current.is_empty() {
            let style = if current_selected { highlight } else { normal };
            spans.push(Span::styled(std::mem::take(&mut current), style));
        }
        current_selected = selected;
        current.push(ch);
    }

    if !current.is_empty() {
        let style = if current_selected { highlight } else { normal };
        spans.push(Span::styled(current, style));
    } else if spans.is_empty() {
        spans.push(Span::styled(String::new(), normal));
    }

    spans
}

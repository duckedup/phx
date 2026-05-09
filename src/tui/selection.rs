use base64::Engine;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::rendering::display::DisplayLine;
use crate::tui::theme::Theme;

#[derive(Clone)]
pub struct Selection {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
    pub active: bool,
}

impl Selection {
    pub fn is_set(&self) -> bool {
        self.start_row != self.end_row || self.start_col != self.end_col
    }

    fn ordered(&self) -> (u16, u16, u16, u16) {
        if self.start_row < self.end_row
            || (self.start_row == self.end_row && self.start_col <= self.end_col)
        {
            (self.start_row, self.start_col, self.end_row, self.end_col)
        } else {
            (self.end_row, self.end_col, self.start_row, self.start_col)
        }
    }
}

pub fn render_selection_overlay(
    frame: &mut Frame,
    area: Rect,
    selection: &Selection,
    display_lines: &[DisplayLine],
    scroll: usize,
    theme: &Theme,
) {
    if !selection.is_set() {
        return;
    }
    let (sr, sc, er, ec) = selection.ordered();
    let visible = area.height;
    let sel_bg = theme.selection_bg();
    let sel_fg = theme.selection_fg();

    for screen_row in 0..visible {
        let abs_row = screen_row + area.y;
        if abs_row < sr || abs_row > er {
            continue;
        }
        if abs_row < area.y || abs_row >= area.y + area.height {
            continue;
        }

        let col_start = if abs_row == sr { sc } else { area.x };
        let col_end = if abs_row == er { ec } else { area.width };
        let col_start = col_start.max(area.x);
        let col_end = col_end.min(area.x + area.width);
        if col_start >= col_end {
            continue;
        }

        let line_idx = scroll + screen_row as usize;
        let line_text = if line_idx < display_lines.len() {
            let dl = &display_lines[line_idx];
            let full: String = dl.spans.iter().map(|(t, _)| t.as_str()).collect();
            full
        } else {
            String::new()
        };

        let chars: Vec<char> = line_text.chars().collect();
        let start = (col_start - area.x) as usize;
        let end = (col_end - area.x) as usize;
        let selected: String = chars
            .get(start..end.min(chars.len()))
            .unwrap_or_default()
            .iter()
            .collect();
        let pad = end.saturating_sub(end.min(chars.len()));
        let text = format!("{selected}{}", " ".repeat(pad));

        let highlight = Paragraph::new(Line::from(Span::styled(
            text,
            Style::default().fg(sel_fg).bg(sel_bg),
        )));

        frame.render_widget(
            highlight,
            Rect {
                x: col_start,
                y: abs_row,
                width: col_end - col_start,
                height: 1,
            },
        );
    }
}

pub fn extract_selected_text(
    display_lines: &[DisplayLine],
    scroll: usize,
    chat_area: Rect,
    selection: &Selection,
) -> String {
    if !selection.is_set() {
        return String::new();
    }
    let (sr, sc, er, ec) = selection.ordered();
    let mut result = String::new();

    for abs_row in sr..=er {
        if abs_row < chat_area.y || abs_row >= chat_area.y + chat_area.height {
            continue;
        }
        let screen_row = (abs_row - chat_area.y) as usize;
        let line_idx = scroll + screen_row;
        let line_text = if line_idx < display_lines.len() {
            let dl = &display_lines[line_idx];
            let full: String = dl.spans.iter().map(|(t, _)| t.as_str()).collect();
            full
        } else {
            String::new()
        };

        let chars: Vec<char> = line_text.chars().collect();
        let col_start = if abs_row == sr {
            (sc - chat_area.x) as usize
        } else {
            0
        };
        let col_end = if abs_row == er {
            (ec - chat_area.x) as usize
        } else {
            chars.len()
        };

        if abs_row > sr {
            result.push('\n');
        }
        let slice: String = chars
            .get(col_start..col_end.min(chars.len()))
            .unwrap_or_default()
            .iter()
            .collect();
        let trimmed = slice.trim_end();
        result.push_str(trimmed);
    }
    result
}

pub fn copy_to_clipboard_osc52(text: &str) {
    if copy_to_clipboard_native(text) {
        return;
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let _ = std::io::Write::write_all(
        &mut std::io::stdout(),
        format!("\x1b]52;c;{encoded}\x07").as_bytes(),
    );
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

pub fn paste_from_clipboard() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        Command::new("pbpaste")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn copy_to_clipboard_native(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};
        if let Ok(mut child) = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            if let Some(ref mut stdin) = child.stdin {
                let _ = stdin.write_all(text.as_bytes());
            }
            return child.wait().is_ok();
        }
        false
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        false
    }
}

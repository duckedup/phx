use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui_textarea::{CursorMove, Key, TextArea};

use crate::tui::input::key_to_input;
use crate::tui::theme::Theme;

pub struct TextField {
    pub value: String,
    pub cursor: usize,
    pub placeholder: String,
}

impl TextField {
    pub fn new(placeholder: impl Into<String>) -> Self {
        Self {
            value: String::new(),
            cursor: 0,
            placeholder: placeholder.into(),
        }
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        let val = value.into();
        self.cursor = val.len();
        self.value = val;
        self
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(c) => {
                self.value.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                true
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = self.value[..self.cursor]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(1);
                    self.cursor -= prev;
                    self.value.remove(self.cursor);
                }
                true
            }
            KeyCode::Delete => {
                if self.cursor < self.value.len() {
                    self.value.remove(self.cursor);
                }
                true
            }
            KeyCode::Left => {
                if self.cursor > 0 {
                    let prev = self.value[..self.cursor]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(1);
                    self.cursor -= prev;
                }
                true
            }
            KeyCode::Right => {
                if self.cursor < self.value.len() {
                    let next = self.value[self.cursor..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(1);
                    self.cursor += next;
                }
                true
            }
            KeyCode::Home => {
                self.cursor = 0;
                true
            }
            KeyCode::End => {
                self.cursor = self.value.len();
                true
            }
            _ => false,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool, theme: &Theme) {
        let bg = theme.background;
        let fg = if focused {
            theme.foreground
        } else {
            theme.dim()
        };

        if self.value.is_empty() {
            let line = Line::from(Span::styled(
                &self.placeholder,
                Style::default().fg(theme.dim()),
            ));
            frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), area);
        } else {
            let line = Line::from(Span::styled(&self.value, Style::default().fg(fg)));
            frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), area);
        }

        if focused {
            let display_col = self.value[..self.cursor].chars().count() as u16;
            let cx = area.x + display_col;
            let cy = area.y;
            if cx < area.right() {
                frame.set_cursor_position((cx, cy));
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }
}

pub struct TextAreaField {
    pub textarea: TextArea<'static>,
    pub placeholder: String,
}

impl TextAreaField {
    pub fn new(placeholder: impl Into<String>) -> Self {
        Self {
            textarea: TextArea::default(),
            placeholder: placeholder.into(),
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Enter
                if key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT) =>
            {
                self.textarea.insert_newline();
                true
            }
            KeyCode::Up => {
                if self.textarea.cursor().0 > 0 {
                    self.textarea.move_cursor(CursorMove::Up);
                    true
                } else {
                    false
                }
            }
            KeyCode::Down => {
                let lines = self.textarea.lines().len();
                if self.textarea.cursor().0 + 1 < lines {
                    self.textarea.move_cursor(CursorMove::Down);
                    true
                } else {
                    false
                }
            }
            _ => {
                let input = key_to_input(key);
                if input.key != Key::Null {
                    self.textarea.input(input);
                    true
                } else {
                    false
                }
            }
        }
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                self.textarea.insert_newline();
            } else {
                self.textarea.insert_char(c);
            }
        }
    }

    pub fn value(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    pub fn selected_text(&self) -> String {
        let Some(((sr, sc), (er, ec))) = self.textarea.selection_range() else {
            return String::new();
        };
        let lines = self.textarea.lines();
        if sr == er {
            return lines[sr].chars().skip(sc).take(ec - sc).collect();
        }
        let mut result = String::new();
        for row in sr..=er {
            if row >= lines.len() {
                break;
            }
            let line = &lines[row];
            if row == sr {
                result.extend(line.chars().skip(sc));
            } else if row == er {
                result.push('\n');
                result.extend(line.chars().take(ec));
            } else {
                result.push('\n');
                result.push_str(line);
            }
        }
        result
    }

    pub fn line_count(&self) -> usize {
        self.textarea.lines().len()
    }
}

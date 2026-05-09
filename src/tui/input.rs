use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui_textarea::{CursorMove, Input, Key, TextArea};

pub struct InputState {
    pub textarea: TextArea<'static>,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
    pub paste_count: u32,
    pub pending_pastes: Vec<String>,
    history_file: PathBuf,
}

impl InputState {
    pub fn new(history_file: PathBuf) -> Self {
        let history = std::fs::read_to_string(&history_file)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect();
        Self {
            textarea: TextArea::default(),
            history,
            history_idx: None,
            paste_count: 0,
            pending_pastes: Vec::new(),
            history_file,
        }
    }

    pub fn empty() -> Self {
        Self {
            textarea: TextArea::default(),
            history: Vec::new(),
            history_idx: None,
            paste_count: 0,
            pending_pastes: Vec::new(),
            history_file: PathBuf::new(),
        }
    }

    pub fn lines(&self) -> &[String] {
        self.textarea.lines()
    }

    pub fn line_count(&self) -> usize {
        self.textarea.lines().len()
    }

    pub fn cursor_line(&self) -> usize {
        self.textarea.cursor().0
    }

    pub fn cursor_col(&self) -> usize {
        self.textarea.cursor().1
    }

    pub fn buffer_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    pub fn clear(&mut self) {
        self.textarea = TextArea::default();
        self.paste_count = 0;
        self.pending_pastes.clear();
        self.history_idx = None;
    }

    pub fn set_single_line(&mut self, text: &str) {
        self.textarea = TextArea::from([text]);
        self.textarea.move_cursor(CursorMove::End);
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.textarea.insert_newline();
        } else {
            self.textarea.insert_char(c);
        }
        self.history_idx = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
        self.history_idx = None;
    }

    pub fn insert_newline(&mut self) {
        self.textarea.insert_newline();
        self.history_idx = None;
    }

    pub fn insert_paste(&mut self, text: &str) {
        let line_count = text.lines().count().max(1);
        if line_count <= 2 {
            self.insert_str(text);
            return;
        }

        self.paste_count += 1;
        self.pending_pastes.push(text.to_string());
        let label = format!("[Pasted text #{} +{} lines]", self.paste_count, line_count);
        self.insert_str(&label);
    }

    pub fn expand_pastes(&self, text: &str) -> String {
        if self.pending_pastes.is_empty() {
            return text.to_string();
        }
        let mut result = String::with_capacity(text.len());
        let mut paste_idx = 0;
        let mut i = 0;
        let bytes = text.as_bytes();
        while i < bytes.len() {
            if bytes[i] == b'['
                && text[i..].starts_with("[Pasted text #")
                && let Some(end) = text[i..].find(']')
            {
                if paste_idx < self.pending_pastes.len() {
                    result.push_str(&self.pending_pastes[paste_idx]);
                    paste_idx += 1;
                }
                i += end + 1;
                continue;
            }
            result.push(bytes[i] as char);
            i += 1;
        }
        result
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

    /// Map a screen position (relative to the text area, after separator and prompt)
    /// to a (line_idx, char_col) in the text buffer.
    /// `text_width` is the available width for text (area width minus prompt prefix).
    pub fn screen_to_text_pos(
        &self,
        screen_row: usize,
        screen_col: usize,
        text_width: usize,
    ) -> (usize, usize) {
        if text_width == 0 {
            return (0, 0);
        }
        let lines = self.textarea.lines();
        let cursor_line = self.cursor_line();
        let cursor_col = self.cursor_col();

        // Build the same wrapped-row map as the renderer
        let mut rows: Vec<(usize, usize)> = Vec::new(); // (line_idx, char_start)
        let mut cursor_row = 0usize;

        for (li, line) in lines.iter().enumerate() {
            if line.is_empty() {
                if li == cursor_line {
                    cursor_row = rows.len();
                }
                rows.push((li, 0));
                continue;
            }
            let char_count = line.chars().count();
            let mut offset = 0;
            while offset < char_count {
                let end = (offset + text_width).min(char_count);
                if li == cursor_line
                    && cursor_col >= offset
                    && cursor_col <= end
                    && !(cursor_col == end && end < char_count)
                {
                    cursor_row = rows.len();
                }
                rows.push((li, offset));
                offset = end;
            }
            if li == cursor_line
                && cursor_col == char_count
                && char_count.is_multiple_of(text_width)
                && !line.is_empty()
            {
                cursor_row = rows.len();
                rows.push((li, char_count));
            }
        }

        let visible_rows = 9; // MAX_INPUT_HEIGHT - 1 (separator)
        let scroll = if cursor_row >= visible_rows {
            cursor_row - visible_rows + 1
        } else {
            0
        };

        let abs_row = scroll + screen_row;
        if abs_row >= rows.len() {
            let last_line = lines.len().saturating_sub(1);
            let last_col = lines.last().map(|l| l.chars().count()).unwrap_or(0);
            return (last_line, last_col);
        }

        let (li, char_start) = rows[abs_row];
        let line_char_count = lines[li].chars().count();
        let chunk_end = (char_start + text_width).min(line_char_count);
        let col = (char_start + screen_col).min(chunk_end);
        (li, col)
    }

    /// Place the cursor at a screen position (used for mouse click).
    pub fn click_at(&mut self, screen_row: usize, screen_col: usize, text_width: usize) {
        let (line, col) = self.screen_to_text_pos(screen_row, screen_col, text_width);
        self.textarea.cancel_selection();
        self.textarea
            .move_cursor(CursorMove::Jump(line as u16, col as u16));
        self.history_idx = None;
    }

    /// Start or extend a selection to a screen position (used for mouse drag).
    pub fn drag_to(&mut self, screen_row: usize, screen_col: usize, text_width: usize) {
        let (line, col) = self.screen_to_text_pos(screen_row, screen_col, text_width);
        if !self.textarea.is_selecting() {
            self.textarea.start_selection();
        }
        self.textarea
            .move_cursor(CursorMove::Jump(line as u16, col as u16));
    }

    /// Pass a key event to the textarea for standard text editing.
    /// Returns true if the event was handled.
    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        let input = key_to_input(key);
        if input.key == Key::Null {
            return false;
        }
        self.history_idx = None;
        self.textarea.input(input)
    }

    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.history_idx {
            Some(i) if i > 0 => i - 1,
            Some(i) => i,
            None => self.history.len() - 1,
        };
        self.history_idx = Some(idx);
        self.set_buffer(&self.history[idx].clone());
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            Some(i) if i + 1 < self.history.len() => {
                let idx = i + 1;
                self.history_idx = Some(idx);
                self.set_buffer(&self.history[idx].clone());
            }
            Some(_) => {
                self.history_idx = None;
                self.textarea = TextArea::default();
            }
            None => {}
        }
    }

    pub fn submit(&mut self) -> String {
        let raw = self.buffer_text();
        let text = self.expand_pastes(&raw);
        self.textarea = TextArea::default();
        self.history_idx = None;
        self.paste_count = 0;
        self.pending_pastes.clear();
        if !text.trim().is_empty() {
            self.history.push(text.clone());
            self.save_history();
        }
        text
    }

    fn set_buffer(&mut self, text: &str) {
        let lines: Vec<String> = text.lines().map(String::from).collect();
        self.textarea = if lines.is_empty() {
            TextArea::default()
        } else {
            TextArea::new(lines)
        };
        self.textarea.move_cursor(CursorMove::Bottom);
        self.textarea.move_cursor(CursorMove::End);
    }

    fn save_history(&self) {
        if let Some(parent) = self.history_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content: String = self.history.iter().map(|l| format!("{l}\n")).collect();
        let _ = std::fs::write(&self.history_file, content);
    }
}

fn key_to_input(key: KeyEvent) -> Input {
    if key.kind == crossterm::event::KeyEventKind::Release {
        return Input::default();
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    if key.code == KeyCode::BackTab {
        return Input {
            key: Key::Tab,
            shift: true,
            ctrl,
            alt,
        };
    }

    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let k = match key.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Enter => Key::Enter,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Tab => Key::Tab,
        KeyCode::Delete => Key::Delete,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Esc => Key::Esc,
        KeyCode::F(n) => Key::F(n),
        _ => Key::Null,
    };

    Input {
        key: k,
        ctrl,
        alt,
        shift,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_input() -> InputState {
        InputState {
            textarea: TextArea::default(),
            history: vec!["first".into(), "second".into()],
            history_idx: None,
            paste_count: 0,
            pending_pastes: Vec::new(),
            history_file: PathBuf::from("/dev/null"),
        }
    }

    #[test]
    fn insert_and_backspace() {
        let mut input = test_input();
        input.insert_char('h');
        input.insert_char('i');
        assert_eq!(input.buffer_text(), "hi");
        assert_eq!(input.cursor_col(), 2);
        input.textarea.delete_char();
        assert_eq!(input.buffer_text(), "h");
        assert_eq!(input.cursor_col(), 1);
    }

    #[test]
    fn multiline_insert() {
        let mut input = test_input();
        input.insert_str("hello");
        input.insert_newline();
        input.insert_str("world");
        assert_eq!(input.line_count(), 2);
        assert_eq!(input.buffer_text(), "hello\nworld");
        assert_eq!(input.cursor_line(), 1);
        assert_eq!(input.cursor_col(), 5);
    }

    #[test]
    fn backspace_merges_lines() {
        let mut input = test_input();
        input.insert_str("ab");
        input.insert_newline();
        input.insert_str("cd");
        input.textarea.move_cursor(CursorMove::Head);
        input.textarea.delete_char();
        assert_eq!(input.line_count(), 1);
        assert_eq!(input.buffer_text(), "abcd");
        assert_eq!(input.cursor_col(), 2);
    }

    #[test]
    fn history_navigation() {
        let mut input = test_input();
        input.history_up();
        assert_eq!(input.buffer_text(), "second");
        input.history_up();
        assert_eq!(input.buffer_text(), "first");
        input.history_down();
        assert_eq!(input.buffer_text(), "second");
        input.history_down();
        assert_eq!(input.buffer_text(), "");
    }

    #[test]
    fn submit_adds_to_history() {
        let mut input = test_input();
        input.insert_str("third");
        let text = input.submit();
        assert_eq!(text, "third");
        assert_eq!(input.history.last().unwrap(), "third");
        assert!(input.is_empty());
    }

    #[test]
    fn home_end() {
        let mut input = test_input();
        input.insert_str("hello");
        input.textarea.move_cursor(CursorMove::Head);
        assert_eq!(input.cursor_col(), 0);
        input.textarea.move_cursor(CursorMove::End);
        assert_eq!(input.cursor_col(), 5);
    }

    #[test]
    fn paste_label() {
        let mut input = test_input();
        input.insert_paste("line1\nline2\nline3\nline4");
        assert!(input.buffer_text().contains("[Pasted text #1 +4 lines]"));
        assert_eq!(input.pending_pastes.len(), 1);
        let submitted = input.submit();
        assert_eq!(submitted, "line1\nline2\nline3\nline4");
    }

    #[test]
    fn small_paste_inline() {
        let mut input = test_input();
        input.insert_paste("ab\ncd");
        assert_eq!(input.buffer_text(), "ab\ncd");
        assert!(input.pending_pastes.is_empty());
    }
}

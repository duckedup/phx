use std::path::PathBuf;

pub struct InputState {
    pub lines: Vec<String>,
    pub cursor_line: usize,
    pub cursor_col: usize,
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
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            history,
            history_idx: None,
            paste_count: 0,
            pending_pastes: Vec::new(),
            history_file,
        }
    }

    pub fn empty() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
            history: Vec::new(),
            history_idx: None,
            paste_count: 0,
            pending_pastes: Vec::new(),
            history_file: PathBuf::new(),
        }
    }

    pub fn buffer_text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn set_single_line(&mut self, text: &str) {
        self.lines = vec![text.to_string()];
        self.cursor_line = 0;
        self.cursor_col = text.chars().count();
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            self.insert_newline();
            return;
        }
        let line = &mut self.lines[self.cursor_line];
        let byte_pos = char_to_byte(line, self.cursor_col);
        line.insert(byte_pos, c);
        self.cursor_col += 1;
        self.history_idx = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert_char(c);
        }
        self.history_idx = None;
    }

    pub fn insert_newline(&mut self) {
        let line = &mut self.lines[self.cursor_line];
        let byte_pos = char_to_byte(line, self.cursor_col);
        let rest = line[byte_pos..].to_string();
        line.truncate(byte_pos);
        self.cursor_line += 1;
        self.cursor_col = 0;
        self.lines.insert(self.cursor_line, rest);
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

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_line];
            let byte_pos = char_to_byte(line, self.cursor_col - 1);
            line.remove(byte_pos);
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            let removed = self.lines.remove(self.cursor_line);
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
            self.lines[self.cursor_line].push_str(&removed);
        }
    }

    pub fn delete(&mut self) {
        let line = &self.lines[self.cursor_line];
        let char_count = line.chars().count();
        if self.cursor_col < char_count {
            let byte_pos = char_to_byte(&self.lines[self.cursor_line], self.cursor_col);
            self.lines[self.cursor_line].remove(byte_pos);
        } else if self.cursor_line + 1 < self.lines.len() {
            let next = self.lines.remove(self.cursor_line + 1);
            self.lines[self.cursor_line].push_str(&next);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_line > 0 {
            self.cursor_line -= 1;
            self.cursor_col = self.lines[self.cursor_line].chars().count();
        }
    }

    pub fn move_right(&mut self) {
        let char_count = self.lines[self.cursor_line].chars().count();
        if self.cursor_col < char_count {
            self.cursor_col += 1;
        } else if self.cursor_line + 1 < self.lines.len() {
            self.cursor_line += 1;
            self.cursor_col = 0;
        }
    }

    pub fn home(&mut self) {
        self.cursor_col = 0;
    }

    pub fn end(&mut self) {
        self.cursor_col = self.lines[self.cursor_line].chars().count();
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
                self.lines = vec![String::new()];
                self.cursor_line = 0;
                self.cursor_col = 0;
            }
            None => {}
        }
    }

    pub fn submit(&mut self) -> String {
        let raw = self.buffer_text();
        let text = self.expand_pastes(&raw);
        self.lines = vec![String::new()];
        self.cursor_line = 0;
        self.cursor_col = 0;
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
        self.lines = text.lines().map(String::from).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor_line = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_line].chars().count();
    }

    fn save_history(&self) {
        if let Some(parent) = self.history_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content: String = self.history.iter().map(|l| format!("{l}\n")).collect();
        let _ = std::fs::write(&self.history_file, content);
    }
}

fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_input() -> InputState {
        InputState {
            lines: vec![String::new()],
            cursor_line: 0,
            cursor_col: 0,
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
        assert_eq!(input.cursor_col, 2);
        input.backspace();
        assert_eq!(input.buffer_text(), "h");
        assert_eq!(input.cursor_col, 1);
    }

    #[test]
    fn multiline_insert() {
        let mut input = test_input();
        input.insert_str("hello");
        input.insert_newline();
        input.insert_str("world");
        assert_eq!(input.lines.len(), 2);
        assert_eq!(input.buffer_text(), "hello\nworld");
        assert_eq!(input.cursor_line, 1);
        assert_eq!(input.cursor_col, 5);
    }

    #[test]
    fn backspace_merges_lines() {
        let mut input = test_input();
        input.insert_str("ab");
        input.insert_newline();
        input.insert_str("cd");
        input.home();
        input.backspace();
        assert_eq!(input.lines.len(), 1);
        assert_eq!(input.buffer_text(), "abcd");
        assert_eq!(input.cursor_col, 2);
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
        input.home();
        assert_eq!(input.cursor_col, 0);
        input.end();
        assert_eq!(input.cursor_col, 5);
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

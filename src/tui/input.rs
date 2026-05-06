use std::path::PathBuf;

pub struct InputState {
    pub buffer: String,
    pub cursor: usize,
    pub history: Vec<String>,
    pub history_idx: Option<usize>,
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
            buffer: String::new(),
            cursor: 0,
            history,
            history_idx: None,
            history_file,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.history_idx = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        self.buffer.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.history_idx = None;
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let prev = self.buffer[..self.cursor]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor -= prev;
            self.buffer.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.buffer.len() {
            self.buffer.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            let prev = self.buffer[..self.cursor]
                .chars()
                .last()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor -= prev;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor < self.buffer.len() {
            let next = self.buffer[self.cursor..]
                .chars()
                .next()
                .map(|c| c.len_utf8())
                .unwrap_or(0);
            self.cursor += next;
        }
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.buffer.len();
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
        self.buffer = self.history[idx].clone();
        self.cursor = self.buffer.len();
    }

    pub fn history_down(&mut self) {
        match self.history_idx {
            Some(i) if i + 1 < self.history.len() => {
                let idx = i + 1;
                self.history_idx = Some(idx);
                self.buffer = self.history[idx].clone();
                self.cursor = self.buffer.len();
            }
            Some(_) => {
                self.history_idx = None;
                self.buffer.clear();
                self.cursor = 0;
            }
            None => {}
        }
    }

    pub fn submit(&mut self) -> String {
        let text = std::mem::take(&mut self.buffer);
        self.cursor = 0;
        self.history_idx = None;
        if !text.trim().is_empty() {
            self.history.push(text.clone());
            self.save_history();
        }
        text
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    fn save_history(&self) {
        if let Some(parent) = self.history_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content: String = self.history.iter().map(|l| format!("{l}\n")).collect();
        let _ = std::fs::write(&self.history_file, content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_input() -> InputState {
        InputState {
            buffer: String::new(),
            cursor: 0,
            history: vec!["first".into(), "second".into()],
            history_idx: None,
            history_file: PathBuf::from("/dev/null"),
        }
    }

    #[test]
    fn insert_and_backspace() {
        let mut input = test_input();
        input.insert_char('h');
        input.insert_char('i');
        assert_eq!(input.buffer, "hi");
        assert_eq!(input.cursor, 2);
        input.backspace();
        assert_eq!(input.buffer, "h");
        assert_eq!(input.cursor, 1);
    }

    #[test]
    fn history_navigation() {
        let mut input = test_input();
        input.history_up();
        assert_eq!(input.buffer, "second");
        input.history_up();
        assert_eq!(input.buffer, "first");
        input.history_down();
        assert_eq!(input.buffer, "second");
        input.history_down();
        assert_eq!(input.buffer, "");
    }

    #[test]
    fn submit_adds_to_history() {
        let mut input = test_input();
        input.insert_str("third");
        let text = input.submit();
        assert_eq!(text, "third");
        assert_eq!(input.history.last().unwrap(), "third");
        assert!(input.buffer.is_empty());
    }

    #[test]
    fn home_end() {
        let mut input = test_input();
        input.insert_str("hello");
        input.home();
        assert_eq!(input.cursor, 0);
        input.end();
        assert_eq!(input.cursor, 5);
    }
}

use crate::session::agent_loop::SessionEvent;
use crate::session::message::Role;
use crate::tui::input::InputState;
use tokio::sync::broadcast;

#[derive(Debug, Clone)]
pub struct ChatLine {
    pub role: Role,
    pub content: String,
}

pub struct Tab {
    pub id: String,
    pub title: String,
    pub events_rx: broadcast::Receiver<SessionEvent>,
    pub input: InputState,
    pub chat_lines: Vec<ChatLine>,
    pub scroll_offset: usize,
    pub auto_scroll: bool,
    pub streaming_text: String,
    pub stream_buffer: String,
}

impl Tab {
    pub fn new(
        id: String,
        events_rx: broadcast::Receiver<SessionEvent>,
        history_file: std::path::PathBuf,
    ) -> Self {
        Self {
            id,
            title: "New Session".into(),
            events_rx,
            input: InputState::new(history_file),
            chat_lines: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
            streaming_text: String::new(),
            stream_buffer: String::new(),
        }
    }

    pub fn apply_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Token(text) => {
                self.streaming_text.push_str(&text);
            }
            SessionEvent::ToolCallStart { name, .. } => {
                self.chat_lines.push(ChatLine {
                    role: Role::ToolCall,
                    content: format!("Running: {name}"),
                });
            }
            SessionEvent::ToolCallEnd { output, .. } => {
                self.chat_lines.push(ChatLine {
                    role: Role::ToolResult,
                    content: output,
                });
            }
            SessionEvent::Done => {
                if !self.streaming_text.is_empty() {
                    self.chat_lines.push(ChatLine {
                        role: Role::Assistant,
                        content: std::mem::take(&mut self.streaming_text),
                    });
                }
            }
            SessionEvent::Error(e) => {
                self.chat_lines.push(ChatLine {
                    role: Role::System,
                    content: format!("Error: {e}"),
                });
            }
        }
        self.auto_scroll = true;
    }

    pub fn add_user_message(&mut self, content: String) {
        self.chat_lines.push(ChatLine {
            role: Role::User,
            content,
        });
        self.auto_scroll = true;
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.auto_scroll = false;
    }

    pub fn scroll_down(&mut self, lines: usize, total: usize, visible: usize) {
        let max = total.saturating_sub(visible);
        self.scroll_offset = (self.scroll_offset + lines).min(max);
        if self.scroll_offset >= max {
            self.auto_scroll = true;
        }
    }
}

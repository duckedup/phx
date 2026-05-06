use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::config::schema::ProviderKind;
use crate::tui::theme::Theme;

pub struct ProviderPreset {
    pub name: &'static str,
    pub display: &'static str,
    pub kind: ProviderKind,
    pub default_model: &'static str,
    pub env_hint: &'static str,
}

pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        name: "claude",
        display: "Anthropic (Claude)",
        kind: ProviderKind::Claude,
        default_model: "claude-opus-4-7",
        env_hint: "ANTHROPIC_API_KEY",
    },
    ProviderPreset {
        name: "openai",
        display: "OpenAI",
        kind: ProviderKind::OpenAI,
        default_model: "gpt-4.1",
        env_hint: "OPENAI_API_KEY",
    },
    ProviderPreset {
        name: "gemini",
        display: "Google (Gemini)",
        kind: ProviderKind::Gemini,
        default_model: "gemini-2.5-flash",
        env_hint: "GOOGLE_API_KEY",
    },
    ProviderPreset {
        name: "nvidia",
        display: "Nvidia NIM",
        kind: ProviderKind::Nvidia,
        default_model: "meta/llama-3.3-70b-instruct",
        env_hint: "NVIDIA_API_KEY",
    },
    ProviderPreset {
        name: "ollama",
        display: "Ollama (local)",
        kind: ProviderKind::Ollama,
        default_model: "llama3",
        env_hint: "",
    },
];

enum Step {
    Provider,
    ApiKey,
    Model,
}

pub struct OnboardingState {
    step: Step,
    selected: usize,
    key_buf: String,
    key_cursor: usize,
    model_buf: String,
    model_cursor: usize,
}

pub enum Action {
    None,
    Cancelled,
    Complete {
        name: String,
        kind: ProviderKind,
        model: String,
        api_key: Option<String>,
        env_hint: String,
    },
}

impl OnboardingState {
    pub fn new() -> Self {
        Self {
            step: Step::Provider,
            selected: 0,
            key_buf: String::new(),
            key_cursor: 0,
            model_buf: String::new(),
            model_cursor: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        match self.step {
            Step::Provider => self.handle_provider_key(key),
            Step::ApiKey => self.handle_apikey_key(key),
            Step::Model => self.handle_model_key(key),
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        match self.step {
            Step::ApiKey => {
                self.key_buf.insert_str(self.key_cursor, text);
                self.key_cursor += text.len();
            }
            Step::Model => {
                self.model_buf.insert_str(self.model_cursor, text);
                self.model_cursor += text.len();
            }
            _ => {}
        }
    }

    fn advance_to_model(&mut self) {
        let preset = &PRESETS[self.selected];
        self.model_buf = preset.default_model.to_string();
        self.model_cursor = self.model_buf.len();
        self.step = Step::Model;
    }

    fn finish(&self) -> Action {
        let preset = &PRESETS[self.selected];
        let model = if self.model_buf.trim().is_empty() {
            preset.default_model.to_string()
        } else {
            self.model_buf.trim().to_string()
        };
        let api_key = if !self.key_buf.trim().is_empty() {
            Some(self.key_buf.trim().to_string())
        } else {
            None
        };
        Action::Complete {
            name: preset.name.to_string(),
            kind: preset.kind,
            model,
            api_key,
            env_hint: if preset.env_hint.is_empty() {
                String::new()
            } else {
                preset.env_hint.to_string()
            },
        }
    }

    fn select_current_provider(&mut self) -> Action {
        let preset = &PRESETS[self.selected];
        if preset.kind.is_local() {
            self.advance_to_model();
            return Action::None;
        }
        if !preset.env_hint.is_empty()
            && let Ok(val) = std::env::var(preset.env_hint)
            && !val.is_empty()
        {
            self.advance_to_model();
            return Action::None;
        }
        self.step = Step::ApiKey;
        self.key_buf.clear();
        self.key_cursor = 0;
        Action::None
    }

    fn handle_provider_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up if self.selected > 0 => {
                self.selected -= 1;
            }
            KeyCode::Down if self.selected + 1 < PRESETS.len() => {
                self.selected += 1;
            }
            KeyCode::Enter => {
                return self.select_current_provider();
            }
            KeyCode::Esc => {
                return Action::Cancelled;
            }
            KeyCode::Char(c) if ('1'..='5').contains(&c) => {
                let idx = (c as u8 - b'1') as usize;
                if idx < PRESETS.len() {
                    self.selected = idx;
                    return self.select_current_provider();
                }
            }
            _ => {}
        }
        Action::None
    }

    fn handle_apikey_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Enter if !self.key_buf.trim().is_empty() => {
                self.advance_to_model();
            }
            KeyCode::Char(c) => {
                self.key_buf.insert(self.key_cursor, c);
                self.key_cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.key_cursor > 0 => {
                let prev = self.key_buf[..self.key_cursor]
                    .chars()
                    .last()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                self.key_cursor -= prev;
                self.key_buf.remove(self.key_cursor);
            }
            KeyCode::Esc => {
                self.step = Step::Provider;
            }
            _ => {}
        }
        Action::None
    }

    fn handle_model_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Enter => {
                return self.finish();
            }
            KeyCode::Char(c) => {
                self.model_buf.insert(self.model_cursor, c);
                self.model_cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.model_cursor > 0 => {
                let prev = self.model_buf[..self.model_cursor]
                    .chars()
                    .last()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                self.model_cursor -= prev;
                self.model_buf.remove(self.model_cursor);
            }
            KeyCode::Left if self.model_cursor > 0 => {
                let prev = self.model_buf[..self.model_cursor]
                    .chars()
                    .last()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                self.model_cursor -= prev;
            }
            KeyCode::Right if self.model_cursor < self.model_buf.len() => {
                let next = self.model_buf[self.model_cursor..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(0);
                self.model_cursor += next;
            }
            KeyCode::Esc => {
                self.step = Step::ApiKey;
                let preset = &PRESETS[self.selected];
                if preset.kind.is_local() {
                    self.step = Step::Provider;
                }
            }
            _ => {}
        }
        Action::None
    }

    pub fn render(&self, frame: &mut Frame, theme: &Theme) {
        let area = frame.area();
        let popup_width = 60u16.min(area.width.saturating_sub(4));
        let popup_height = 16u16.min(area.height.saturating_sub(4));
        let popup_area = Rect {
            x: (area.width.saturating_sub(popup_width)) / 2,
            y: (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        frame.render_widget(Clear, popup_area);

        match self.step {
            Step::Provider => self.render_provider(frame, popup_area, theme),
            Step::ApiKey => self.render_apikey(frame, popup_area, theme),
            Step::Model => self.render_model(frame, popup_area, theme),
        }
    }

    fn render_provider(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent))
            .style(Style::default().bg(theme.background))
            .title(" Welcome to Phoenix ");

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Select a provider:",
                Style::default().fg(theme.foreground),
            )),
            Line::from(""),
        ];

        for (i, preset) in PRESETS.iter().enumerate() {
            let style = if i == self.selected {
                Style::default().fg(theme.background).bg(theme.accent)
            } else {
                Style::default().fg(theme.foreground)
            };
            let marker = if i == self.selected { ">" } else { " " };
            lines.push(Line::from(Span::styled(
                format!("  {marker} {}. {}", i + 1, preset.display),
                style,
            )));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Enter to select · Esc to cancel",
            Style::default().fg(theme.dim()),
        )));

        let para = Paragraph::new(lines);
        frame.render_widget(para, inner);
    }

    fn render_apikey(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let preset = &PRESETS[self.selected];
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent))
            .style(Style::default().bg(theme.background))
            .title(format!(" {} ", preset.display));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let masked = if self.key_buf.is_empty() {
            String::new()
        } else if self.key_buf.len() <= 4 {
            "*".repeat(self.key_buf.len())
        } else {
            format!(
                "{}{}",
                "*".repeat(self.key_buf.len() - 4),
                &self.key_buf[self.key_buf.len() - 4..],
            )
        };

        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("  Enter your API key for {}:", preset.display),
                Style::default().fg(theme.foreground),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  > {masked}"),
                Style::default().fg(theme.primary),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  Hint: set {} and restart to skip this", preset.env_hint),
                Style::default().fg(theme.dim()),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Enter to confirm, Esc to go back",
                Style::default().fg(theme.dim()),
            )),
        ];

        let para = Paragraph::new(lines);
        frame.render_widget(para, inner);
    }

    fn render_model(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let preset = &PRESETS[self.selected];
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.accent))
            .style(Style::default().bg(theme.background))
            .title(format!(" {} - Model ", preset.display));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Model name (edit or Enter to accept):",
                Style::default().fg(theme.foreground),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  > {}", self.model_buf),
                Style::default().fg(theme.primary),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  Default: {}", preset.default_model),
                Style::default().fg(theme.dim()),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Enter to confirm, Esc to go back",
                Style::default().fg(theme.dim()),
            )),
        ];

        let para = Paragraph::new(lines);
        frame.render_widget(para, inner);
    }
}

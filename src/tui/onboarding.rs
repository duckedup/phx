use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::config::schema::ProviderKind;
use crate::providers::model_info;
use crate::tui::components::dialog as d;
use crate::tui::theme::Theme;

// ── Presets ─────────────────────────────────────────────────────

pub struct ProviderPreset {
    pub name: &'static str,
    pub display: &'static str,
    pub kind: ProviderKind,
    pub default_model: &'static str,
    pub env_hint: &'static str,
    desc: &'static str,
}

pub const PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        name: "claude",
        display: "Anthropic",
        kind: ProviderKind::Claude,
        default_model: "claude-opus-4-7",
        env_hint: "ANTHROPIC_API_KEY",
        desc: "Claude models",
    },
    ProviderPreset {
        name: "openai",
        display: "OpenAI",
        kind: ProviderKind::OpenAI,
        default_model: "gpt-4.1",
        env_hint: "OPENAI_API_KEY",
        desc: "GPT & o-series",
    },
    ProviderPreset {
        name: "gemini",
        display: "Google Gemini",
        kind: ProviderKind::Gemini,
        default_model: "gemini-2.5-flash",
        env_hint: "GOOGLE_API_KEY",
        desc: "Gemini models",
    },
    ProviderPreset {
        name: "nvidia",
        display: "NVIDIA NIM",
        kind: ProviderKind::Nvidia,
        default_model: "meta/llama-3.3-70b-instruct",
        env_hint: "NVIDIA_API_KEY",
        desc: "Open models via NIM",
    },
    ProviderPreset {
        name: "ollama",
        display: "Ollama",
        kind: ProviderKind::Ollama,
        default_model: "llama3",
        env_hint: "",
        desc: "Run models locally",
    },
];

const STEPS_CLOUD: &[&str] = &["Provider", "Auth", "Model"];
const STEPS_LOCAL: &[&str] = &["Provider", "Server", "Model"];

// ── State ───────────────────────────────────────────────────────

enum Step {
    Provider,
    BaseUrl,
    ApiKey,
    Fetching,
    Model,
    CustomModel,
}

impl Step {
    fn index(&self) -> usize {
        match self {
            Step::Provider => 0,
            Step::BaseUrl | Step::ApiKey => 1,
            Step::Fetching | Step::Model | Step::CustomModel => 2,
        }
    }
}

pub struct OnboardingState {
    step: Step,
    selected: usize,
    key_buf: String,
    key_cursor: usize,
    url_buf: String,
    url_cursor: usize,
    model_buf: String,
    model_cursor: usize,
    skip_model: bool,
    env_key_detected: bool,
    models: Vec<String>,
    model_selected: usize,
    fetch_error: Option<String>,
    model_rx: Option<mpsc::Receiver<Result<Vec<String>, String>>>,
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
        base_url: Option<String>,
    },
}

impl OnboardingState {
    pub fn new() -> Self {
        Self {
            step: Step::Provider,
            selected: 0,
            key_buf: String::new(),
            key_cursor: 0,
            url_buf: String::new(),
            url_cursor: 0,
            model_buf: String::new(),
            model_cursor: 0,
            skip_model: false,
            env_key_detected: false,
            models: Vec::new(),
            model_selected: 0,
            fetch_error: None,
            model_rx: None,
        }
    }

    pub fn new_for_api_key(provider_idx: usize) -> Self {
        let preset = &PRESETS[provider_idx];
        Self {
            step: Step::ApiKey,
            selected: provider_idx,
            key_buf: String::new(),
            key_cursor: 0,
            url_buf: String::new(),
            url_cursor: 0,
            model_buf: preset.default_model.to_string(),
            model_cursor: preset.default_model.len(),
            skip_model: true,
            env_key_detected: false,
            models: Vec::new(),
            model_selected: 0,
            fetch_error: None,
            model_rx: None,
        }
    }

    pub fn poll_models(&mut self) {
        let rx = match self.model_rx.take() {
            Some(rx) => rx,
            None => return,
        };
        match rx.try_recv() {
            Ok(result) => self.apply_fetch_result(result),
            Err(mpsc::TryRecvError::Empty) => {
                self.model_rx = Some(rx);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.apply_fetch_result(Err("model fetch failed".into()));
            }
        }
    }

    fn apply_fetch_result(&mut self, result: Result<Vec<String>, String>) {
        let preset = &PRESETS[self.selected];
        match result {
            Ok(m) if !m.is_empty() => {
                self.models = m;
                self.step = Step::Model;
            }
            Ok(_) => {
                self.fetch_error = Some(if preset.kind == ProviderKind::Ollama {
                    "No models found — run: ollama pull <model>".into()
                } else {
                    "No models returned by API".into()
                });
                self.models = static_models_for(preset.kind);
                self.transition_to_model_or_custom();
            }
            Err(e) => {
                self.fetch_error = Some(e);
                self.models = static_models_for(preset.kind);
                self.transition_to_model_or_custom();
            }
        }
    }

    fn transition_to_model_or_custom(&mut self) {
        let preset = &PRESETS[self.selected];
        if self.models.is_empty() {
            self.model_buf = preset.default_model.to_string();
            self.model_cursor = self.model_buf.len();
            self.step = Step::CustomModel;
        } else {
            self.step = Step::Model;
        }
    }

    // ── Input dispatch ──────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        match self.step {
            Step::Provider => self.handle_provider_key(key),
            Step::BaseUrl => self.handle_baseurl_key(key),
            Step::ApiKey => self.handle_apikey_key(key),
            Step::Fetching => self.handle_fetching_key(key),
            Step::Model => self.handle_model_key(key),
            Step::CustomModel => self.handle_custom_model_key(key),
        }
    }

    pub fn handle_paste(&mut self, text: &str) {
        match self.step {
            Step::BaseUrl => {
                self.url_buf.insert_str(self.url_cursor, text);
                self.url_cursor += text.len();
            }
            Step::ApiKey => {
                self.key_buf.insert_str(self.key_cursor, text);
                self.key_cursor += text.len();
            }
            Step::CustomModel => {
                self.model_buf.insert_str(self.model_cursor, text);
                self.model_cursor += text.len();
            }
            _ => {}
        }
    }

    // ── Logic ───────────────────────────────────────────────────

    fn resolve_api_key(&self) -> Option<String> {
        if !self.key_buf.trim().is_empty() {
            return Some(self.key_buf.trim().to_string());
        }
        let preset = &PRESETS[self.selected];
        if !preset.env_hint.is_empty() {
            std::env::var(preset.env_hint)
                .ok()
                .filter(|v| !v.is_empty())
        } else {
            None
        }
    }

    fn resolve_base_url(&self) -> Option<String> {
        let t = self.url_buf.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    }

    fn advance_to_model(&mut self) {
        let preset = &PRESETS[self.selected];
        self.model_selected = 0;
        self.model_buf.clear();
        self.model_cursor = 0;
        self.models.clear();
        self.fetch_error = None;

        let api_key = self.resolve_api_key();
        let base_url = self.resolve_base_url();

        let (tx, rx) = mpsc::channel();
        let kind = preset.kind;
        std::thread::spawn(move || {
            let result = fetch_models_blocking(kind, api_key.as_deref(), base_url.as_deref());
            let _ = tx.send(result);
        });
        self.model_rx = Some(rx);
        self.step = Step::Fetching;
    }

    fn finish(&self) -> Action {
        let preset = &PRESETS[self.selected];
        let model = match self.step {
            Step::CustomModel => {
                let t = self.model_buf.trim();
                if t.is_empty() {
                    preset.default_model
                } else {
                    t
                }
                .to_string()
            }
            _ => {
                if self.model_selected < self.models.len() {
                    self.models[self.model_selected].clone()
                } else {
                    preset.default_model.to_string()
                }
            }
        };
        Action::Complete {
            name: preset.name.to_string(),
            kind: preset.kind,
            model,
            api_key: if self.key_buf.trim().is_empty() {
                None
            } else {
                Some(self.key_buf.trim().to_string())
            },
            env_hint: if preset.env_hint.is_empty() {
                String::new()
            } else {
                preset.env_hint.to_string()
            },
            base_url: self.resolve_base_url(),
        }
    }

    // ── Key handlers ────────────────────────────────────────────

    fn select_current_provider(&mut self) -> Action {
        let preset = &PRESETS[self.selected];
        if preset.kind.is_local() {
            self.url_buf = "http://localhost:11434".to_string();
            self.url_cursor = self.url_buf.len();
            self.step = Step::BaseUrl;
            return Action::None;
        }
        self.env_key_detected = !preset.env_hint.is_empty()
            && std::env::var(preset.env_hint)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        self.step = Step::ApiKey;
        self.key_buf.clear();
        self.key_cursor = 0;
        Action::None
    }

    fn handle_provider_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if self.selected > 0 => self.selected -= 1,
            KeyCode::Down | KeyCode::Char('j') if self.selected + 1 < PRESETS.len() => {
                self.selected += 1
            }
            KeyCode::Enter => return self.select_current_provider(),
            KeyCode::Esc => return Action::Cancelled,
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

    fn handle_baseurl_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Enter => self.advance_to_model(),
            KeyCode::Char(c) => {
                self.url_buf.insert(self.url_cursor, c);
                self.url_cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.url_cursor > 0 => {
                prev_delete(&mut self.url_buf, &mut self.url_cursor);
            }
            KeyCode::Left if self.url_cursor > 0 => {
                self.url_cursor -= prev_char_len(&self.url_buf, self.url_cursor);
            }
            KeyCode::Right if self.url_cursor < self.url_buf.len() => {
                self.url_cursor += next_char_len(&self.url_buf, self.url_cursor);
            }
            KeyCode::Esc => self.step = Step::Provider,
            _ => {}
        }
        Action::None
    }

    fn handle_fetching_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Esc {
            self.model_rx = None;
            let preset = &PRESETS[self.selected];
            self.step = if preset.kind.is_local() {
                Step::BaseUrl
            } else {
                Step::ApiKey
            };
        }
        Action::None
    }

    fn handle_apikey_key(&mut self, key: KeyEvent) -> Action {
        let can_skip = self.env_key_detected && self.key_buf.trim().is_empty();
        match key.code {
            KeyCode::Enter if !self.key_buf.trim().is_empty() || can_skip => {
                if self.skip_model {
                    return self.finish();
                }
                self.advance_to_model();
            }
            KeyCode::Char(c) => {
                self.key_buf.insert(self.key_cursor, c);
                self.key_cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.key_cursor > 0 => {
                prev_delete(&mut self.key_buf, &mut self.key_cursor);
            }
            KeyCode::Esc => self.step = Step::Provider,
            _ => {}
        }
        Action::None
    }

    fn handle_model_key(&mut self, key: KeyEvent) -> Action {
        let total = self.models.len() + 1;
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if self.model_selected > 0 => self.model_selected -= 1,
            KeyCode::Down | KeyCode::Char('j') if self.model_selected + 1 < total => {
                self.model_selected += 1
            }
            KeyCode::Enter => {
                if self.model_selected == self.models.len() {
                    self.model_buf.clear();
                    self.model_cursor = 0;
                    self.step = Step::CustomModel;
                } else {
                    return self.finish();
                }
            }
            KeyCode::Esc => {
                let preset = &PRESETS[self.selected];
                self.step = if preset.kind.is_local() {
                    Step::BaseUrl
                } else {
                    Step::ApiKey
                };
            }
            _ => {}
        }
        Action::None
    }

    fn handle_custom_model_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Enter if !self.model_buf.trim().is_empty() => return self.finish(),
            KeyCode::Char(c) => {
                self.model_buf.insert(self.model_cursor, c);
                self.model_cursor += c.len_utf8();
            }
            KeyCode::Backspace if self.model_cursor > 0 => {
                prev_delete(&mut self.model_buf, &mut self.model_cursor);
            }
            KeyCode::Left if self.model_cursor > 0 => {
                self.model_cursor -= prev_char_len(&self.model_buf, self.model_cursor);
            }
            KeyCode::Right if self.model_cursor < self.model_buf.len() => {
                self.model_cursor += next_char_len(&self.model_buf, self.model_cursor);
            }
            KeyCode::Esc => {
                if self.models.is_empty() {
                    let preset = &PRESETS[self.selected];
                    self.step = if preset.kind.is_local() {
                        Step::BaseUrl
                    } else {
                        Step::ApiKey
                    };
                } else {
                    self.step = Step::Model;
                }
            }
            _ => {}
        }
        Action::None
    }

    // ── Rendering ───────────────────────────────────────────────

    pub fn render(&self, frame: &mut Frame, theme: &Theme) {
        d::render_backdrop(frame, theme);

        let popup = d::centered(frame.area(), 72, 28);
        frame.render_widget(Clear, popup);

        let block = d::dialog_block(theme);
        let content = block.inner(popup);
        frame.render_widget(block, popup);

        let rows = Layout::vertical([
            Constraint::Length(2), // header
            Constraint::Min(1),    // body
            Constraint::Length(1), // footer
        ])
        .split(content);

        self.render_header(frame, rows[0], theme);
        match self.step {
            Step::Provider => self.render_provider(frame, rows[1], theme),
            Step::BaseUrl => self.render_baseurl(frame, rows[1], theme),
            Step::ApiKey => self.render_apikey(frame, rows[1], theme),
            Step::Fetching => self.render_fetching(frame, rows[1], theme),
            Step::Model => self.render_model(frame, rows[1], theme),
            Step::CustomModel => self.render_custom_model(frame, rows[1], theme),
        }
        self.render_footer(frame, rows[2], theme);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let preset = &PRESETS[self.selected];
        let labels = if preset.kind.is_local() {
            STEPS_LOCAL
        } else {
            STEPS_CLOUD
        };

        let steps = d::step_indicator(labels, self.step.index(), theme);
        frame.render_widget(
            Paragraph::new(steps).alignment(ratatui::layout::Alignment::Center),
            area,
        );
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let hints = match self.step {
            Step::Provider => "↑↓ navigate  ⏎ select  esc quit",
            Step::BaseUrl | Step::ApiKey | Step::CustomModel => "⏎ confirm  esc back",
            Step::Fetching => "esc cancel",
            Step::Model => "↑↓ navigate  ⏎ select  esc back",
        };
        let dim = Style::default().fg(theme.dim());

        if matches!(self.step, Step::Provider) {
            let note = "* requires API key";
            let gap = (area.width as usize).saturating_sub(hints.len() + note.len());
            let line = Line::from(vec![
                Span::styled(hints, dim),
                Span::raw(" ".repeat(gap)),
                Span::styled(note, dim),
            ]);
            frame.render_widget(Paragraph::new(line), area);
        } else {
            frame.render_widget(Paragraph::new(d::footer_hints(hints, theme)), area);
        }
    }

    fn render_provider(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut lines: Vec<Line> = vec![Line::from("")];
        for (i, preset) in PRESETS.iter().enumerate() {
            let sel = i == self.selected;
            let needs_key = !preset.env_hint.is_empty();

            let bg = if sel {
                Theme::blend(theme.accent, theme.background, 0.75)
            } else {
                theme.background
            };
            let name_style = if sel {
                Style::default()
                    .fg(theme.accent)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.foreground).bg(bg)
            };
            let mark_style = Style::default().fg(theme.dim()).bg(bg);
            let desc_style = Style::default().fg(theme.dim()).bg(bg);

            let mut name_spans = vec![
                Span::styled(if sel { "▸ " } else { "  " }, name_style),
                Span::styled(preset.display, name_style),
            ];
            if needs_key {
                name_spans.push(Span::styled("*", mark_style));
            }
            lines.push(Line::from(name_spans));
            lines.push(Line::from(vec![
                Span::styled("  ", desc_style),
                Span::styled(preset.desc, desc_style),
            ]));

            if i + 1 < PRESETS.len() {
                lines.push(Line::from(""));
            }
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_baseurl(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let lines = vec![
            Line::from(""),
            d::heading("Server URL", theme),
            d::hint("Where is Ollama running?", theme),
            Line::from(""),
            d::input_field(&self.url_buf, area.width, theme),
        ];
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_apikey(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let preset = &PRESETS[self.selected];
        let mut lines: Vec<Line> = vec![Line::from("")];

        if self.env_key_detected {
            lines.push(d::success_line(
                format!("{} found in environment", preset.env_hint),
                theme,
            ));
            lines.push(d::hint(
                "Press Enter to use it, or paste a different key",
                theme,
            ));
        } else {
            lines.push(d::heading("API Key", theme));
            lines.push(d::hint_owned(
                format!("Paste your {} API key", preset.display),
                theme,
            ));
        }

        lines.push(Line::from(""));
        lines.push(d::masked_input(&self.key_buf, area.width, theme));

        if !self.env_key_detected {
            lines.push(Line::from(""));
            lines.push(d::hint_italic(
                format!("tip: export {} to skip this step", preset.env_hint),
                theme,
            ));
        }

        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_fetching(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let preset = &PRESETS[self.selected];
        let lines = vec![
            Line::from(""),
            d::heading("Fetching models", theme),
            d::hint_owned(format!("Connecting to {}...", preset.display), theme),
        ];
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_model(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let has_error = self.fetch_error.is_some();
        let overhead: usize = if has_error { 5 } else { 3 };
        let max_visible = (area.height as usize).saturating_sub(overhead);
        let total = self.models.len() + 1;
        let scroll = self
            .model_selected
            .saturating_sub(max_visible.saturating_sub(1));

        let mut lines: Vec<Line> = vec![Line::from("")];

        if let Some(err) = &self.fetch_error {
            lines.push(d::warning_line(err, theme));
            lines.push(Line::from(""));
        }

        lines.push(d::heading("Choose a model", theme));
        lines.push(Line::from(""));

        let end = total.min(scroll + max_visible);
        for i in scroll..end {
            let sel = i == self.model_selected;
            let is_custom = i == self.models.len();
            let label = if is_custom {
                "Enter custom model..."
            } else {
                &self.models[i]
            };
            let item = d::list_item(label, None, sel && !is_custom, theme);
            if is_custom {
                let style = if sel {
                    Style::default()
                        .fg(theme.dim())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.dim())
                };
                lines.push(Line::from(vec![
                    Span::styled(if sel { "▸ " } else { "  " }, style),
                    Span::styled(label, style),
                ]));
            } else {
                lines.extend(item);
            }
        }

        if end < total {
            lines.push(d::hint_owned(format!("  ↓ {} more", total - end), theme));
        }

        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_custom_model(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut lines: Vec<Line> = vec![Line::from("")];

        if let Some(err) = &self.fetch_error {
            lines.push(d::warning_line(err, theme));
            lines.push(Line::from(""));
        }

        lines.push(d::heading("Custom Model", theme));
        lines.push(d::hint("Enter the model identifier", theme));
        lines.push(Line::from(""));
        lines.push(d::input_field(&self.model_buf, area.width, theme));

        frame.render_widget(Paragraph::new(lines), area);
    }
}

// ── Helpers ─────────────────────────────────────────────────────

fn prev_char_len(s: &str, cursor: usize) -> usize {
    s[..cursor]
        .chars()
        .last()
        .map(|c| c.len_utf8())
        .unwrap_or(0)
}

fn next_char_len(s: &str, cursor: usize) -> usize {
    s[cursor..]
        .chars()
        .next()
        .map(|c| c.len_utf8())
        .unwrap_or(0)
}

fn prev_delete(buf: &mut String, cursor: &mut usize) {
    let len = prev_char_len(buf, *cursor);
    *cursor -= len;
    buf.remove(*cursor);
}

fn static_models_for(kind: ProviderKind) -> Vec<String> {
    model_info::models_for_provider(kind)
        .into_iter()
        .map(|m| m.id.to_string())
        .collect()
}

// ── Dynamic model fetching ──────────────────────────────────────

fn fetch_models_blocking(
    kind: ProviderKind,
    api_key: Option<&str>,
    base_url: Option<&str>,
) -> Result<Vec<String>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    match kind {
        ProviderKind::Ollama => {
            let url = base_url.unwrap_or("http://localhost:11434");
            let resp = client
                .get(format!("{url}/api/tags"))
                .send()
                .map_err(|e| format!("Cannot reach Ollama at {url}: {e}"))?;
            let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
            Ok(json
                .get("models")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default())
        }
        ProviderKind::Claude => {
            let key = api_key.ok_or("API key required")?;
            let resp = client
                .get("https://api.anthropic.com/v1/models")
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .send()
                .map_err(|e| format!("Anthropic API error: {e}"))?;
            parse_openai_style_models(resp)
        }
        ProviderKind::OpenAI => {
            let key = api_key.ok_or("API key required")?;
            let resp = client
                .get("https://api.openai.com/v1/models")
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .map_err(|e| format!("OpenAI API error: {e}"))?;
            let mut models = parse_openai_style_models(resp)?;
            models.retain(|m| {
                m.starts_with("gpt-")
                    || m.starts_with("o1")
                    || m.starts_with("o3")
                    || m.starts_with("o4")
            });
            models.sort();
            models.dedup();
            Ok(models)
        }
        ProviderKind::Nvidia => {
            let key = api_key.ok_or("API key required")?;
            let url = base_url.unwrap_or("https://integrate.api.nvidia.com");
            let resp = client
                .get(format!("{url}/v1/models"))
                .header("Authorization", format!("Bearer {key}"))
                .send()
                .map_err(|e| format!("NIM API error: {e}"))?;
            parse_openai_style_models(resp)
        }
        ProviderKind::Gemini => {
            let key = api_key.ok_or("API key required")?;
            let resp = client
                .get("https://generativelanguage.googleapis.com/v1beta/models")
                .header("x-goog-api-key", key)
                .send()
                .map_err(|e| format!("Gemini API error: {e}"))?;
            let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
            Ok(json
                .get("models")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            m.get("name")
                                .and_then(|n| n.as_str())
                                .map(|n| n.strip_prefix("models/").unwrap_or(n).to_string())
                        })
                        .filter(|n| n.starts_with("gemini-"))
                        .collect()
                })
                .unwrap_or_default())
        }
        _ => Ok(Vec::new()),
    }
}

fn parse_openai_style_models(resp: reqwest::blocking::Response) -> Result<Vec<String>, String> {
    let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    Ok(json
        .get("data")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default())
}

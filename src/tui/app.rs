use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self as ct_event, Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::*;
use tokio::sync::broadcast;

use crate::commands::dispatcher::ModelChoice;
use crate::config::schema::Config;
use crate::providers;
use crate::providers::traits::Provider;
use crate::session::agent_loop::{Session, SessionEvent};
use crate::session::message::Message;
use crate::store::session_store::{SessionId, SessionStore};
use crate::tools;
use crate::tools::traits::ToolRegistry;
use crate::tui::onboarding;
use crate::tui::picker::{PickerItem, PickerMode, PickerState};
use crate::tui::tabs::{ChatLine, Tab};
use crate::tui::theme::{self, Theme};

// ─── Display line ────────────────────────────────────────────────────────────

struct DisplayLine {
    spans: Vec<(String, Style)>,
}

impl DisplayLine {
    fn styled(text: &str, style: Style) -> Self {
        Self {
            spans: vec![(text.to_string(), style)],
        }
    }

    fn multi(parts: Vec<(String, Style)>) -> Self {
        Self { spans: parts }
    }

    fn empty() -> Self {
        Self { spans: vec![] }
    }

    fn to_line(&self) -> Line<'static> {
        Line::from(
            self.spans
                .iter()
                .map(|(t, s)| Span::styled(t.clone(), *s))
                .collect::<Vec<_>>(),
        )
    }
}

// ─── App ─────────────────────────────────────────────────────────────────────

pub struct App {
    pub config: Config,
    pub theme: Theme,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub should_quit: bool,
    pub ctrl_c_count: u8,
    pub ctrl_c_time: Option<std::time::Instant>,
    pub provider: Option<Arc<dyn Provider>>,
    pub tools: ToolRegistry,
    pub store: SessionStore,
    pub project: std::path::PathBuf,
    pub session: Option<Session>,
    pub events_tx: broadcast::Sender<SessionEvent>,
    pub is_running: bool,
    pub picker: Option<PickerState>,
    pub saved_theme: Option<Theme>,
    pub model_choices: Vec<ModelChoice>,
    pub onboarding: Option<onboarding::OnboardingState>,
    pub pending_session_resume: Option<String>,
    command_items: Vec<PickerItem>,
    display_lines: Vec<DisplayLine>,
    chat_area_height: u16,
    frame_tick: u64,
}

impl App {
    pub fn new(config: Config) -> Self {
        let theme = config
            .theme
            .as_deref()
            .and_then(theme::get_by_name)
            .unwrap_or_else(theme::default_theme);
        let (events_tx, _) = broadcast::channel(1024);
        let store = SessionStore::new(crate::config::paths::sessions_dir());
        let tool_registry = tools::build_registry_all();
        let project = std::env::current_dir().unwrap_or_default();

        let provider: Option<Arc<dyn Provider>> = crate::config::loader::active_provider(&config)
            .and_then(
                |(name, profile)| match providers::create_provider(name, profile) {
                    Ok(p) => Some(Arc::from(p)),
                    Err(e) => {
                        tracing::warn!("failed to create provider: {e}");
                        None
                    }
                },
            );

        let skills = crate::session::skills::discover_layered(
            Some(&project),
            &crate::config::paths::user_home(),
            &config.skills.dirs,
        );
        let command_list = crate::commands::dispatcher::list_commands(&skills);
        let command_items: Vec<PickerItem> = command_list
            .iter()
            .map(|cmd| PickerItem {
                id: cmd.name.clone(),
                label: cmd.name.clone(),
                description: cmd.summary.clone(),
            })
            .collect();

        let needs_onboarding = provider.is_none();

        Self {
            config,
            theme,
            tabs: Vec::new(),
            active_tab: 0,
            should_quit: false,
            ctrl_c_count: 0,
            ctrl_c_time: None,
            provider,
            tools: tool_registry,
            store,
            project,
            session: None,
            events_tx,
            is_running: false,
            picker: None,
            saved_theme: None,
            model_choices: Vec::new(),
            onboarding: if needs_onboarding {
                Some(onboarding::OnboardingState::new())
            } else {
                None
            },
            pending_session_resume: None,
            command_items,
            display_lines: Vec::new(),
            chat_area_height: 0,
            frame_tick: 0,
        }
    }

    fn current_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    fn current_tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active_tab)
    }

    fn submit_message(&mut self) -> Option<String> {
        let tab = self.current_tab_mut()?;
        let text = tab.input.submit();
        if text.trim().is_empty() {
            return None;
        }
        tab.add_user_message(text.clone());
        Some(text)
    }

    fn effective_scroll(&self) -> usize {
        let total = self.display_lines.len();
        let visible = self.chat_area_height as usize;
        if self.current_tab().map(|t| t.auto_scroll).unwrap_or(true) {
            total.saturating_sub(visible)
        } else {
            let scroll = self.current_tab().map(|t| t.scroll_offset).unwrap_or(0);
            scroll.min(total.saturating_sub(visible))
        }
    }

    // ─── Key handling ────────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Ctrl+C: cancel running / quit
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.onboarding.is_some() {
                self.should_quit = true;
                return true;
            }
            if self.picker.is_some() {
                self.restore_theme();
                self.picker = None;
                return true;
            }
            // Reset count if >2s since last Ctrl+C
            if let Some(t) = self.ctrl_c_time
                && t.elapsed() > Duration::from_secs(2)
            {
                self.ctrl_c_count = 0;
            }
            self.ctrl_c_count += 1;
            self.ctrl_c_time = Some(std::time::Instant::now());
            if self.ctrl_c_count >= 2 {
                self.should_quit = true;
            }
            return false;
        }

        // Any other key resets ctrl_c count
        self.ctrl_c_count = 0;
        self.ctrl_c_time = None;

        if self.onboarding.is_some() {
            let action = self.onboarding.as_mut().unwrap().handle_key(key);
            match action {
                onboarding::Action::Complete {
                    name,
                    kind,
                    model,
                    api_key,
                    env_hint,
                } => {
                    self.complete_onboarding(name, kind, model, api_key, env_hint);
                }
                onboarding::Action::Cancelled => {
                    self.onboarding = None;
                }
                onboarding::Action::None => {}
            }
            return true;
        }

        if let Some(ref picker) = self.picker {
            match picker.mode {
                PickerMode::Theme | PickerMode::Model | PickerMode::Session => {
                    self.handle_modal_picker_key(key);
                    return true;
                }
                PickerMode::CommandComplete => {}
            }
        }

        if self
            .picker
            .as_ref()
            .is_some_and(|p| p.mode == PickerMode::CommandComplete)
        {
            match key.code {
                KeyCode::Up => {
                    self.picker.as_mut().unwrap().move_up();
                    return true;
                }
                KeyCode::Down => {
                    self.picker.as_mut().unwrap().move_down();
                    return true;
                }
                KeyCode::Tab => {
                    self.complete_command();
                    return true;
                }
                KeyCode::Esc => {
                    self.picker = None;
                    return true;
                }
                KeyCode::Enter
                    if !key.modifiers.contains(KeyModifiers::SHIFT)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    self.complete_command();
                    return false;
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Tab
                if key.modifiers.contains(KeyModifiers::CONTROL) && !self.tabs.is_empty() =>
            {
                self.active_tab = (self.active_tab + 1) % self.tabs.len();
            }
            KeyCode::Char('w')
                if key.modifiers.contains(KeyModifiers::CONTROL) && !self.tabs.is_empty() =>
            {
                self.tabs.remove(self.active_tab);
                if self.active_tab >= self.tabs.len() && self.active_tab > 0 {
                    self.active_tab -= 1;
                }
            }
            KeyCode::Up => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.history_up();
                }
            }
            KeyCode::Down => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.history_down();
                }
            }
            KeyCode::Left => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.move_left();
                }
            }
            KeyCode::Right => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.move_right();
                }
            }
            KeyCode::Home => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.home();
                }
            }
            KeyCode::End => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.end();
                }
            }
            KeyCode::Backspace => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.backspace();
                }
            }
            KeyCode::Delete => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.delete();
                }
            }
            KeyCode::Enter => {
                if (key.modifiers.contains(KeyModifiers::SHIFT)
                    || key.modifiers.contains(KeyModifiers::ALT))
                    && let Some(tab) = self.current_tab_mut()
                {
                    tab.input.insert_newline();
                }
            }
            KeyCode::PageUp => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.scroll_up(10);
                }
            }
            KeyCode::PageDown => {
                let total = self.display_lines.len();
                let visible = self.chat_area_height as usize;
                if let Some(tab) = self.current_tab_mut() {
                    tab.scroll_down(10, total, visible);
                }
            }
            // Vim-style half-page scroll
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let half = (self.chat_area_height as usize) / 2;
                if let Some(tab) = self.current_tab_mut() {
                    tab.scroll_up(half);
                }
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let total = self.display_lines.len();
                let visible = self.chat_area_height as usize;
                let half = visible / 2;
                if let Some(tab) = self.current_tab_mut() {
                    tab.scroll_down(half, total, visible);
                }
            }
            KeyCode::Char(c) => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.insert_char(c);
                }
            }
            _ => {}
        }

        self.update_command_completion();
        false
    }

    // ─── Display line computation ────────────────────────────────────────

    pub fn compute_display_lines(&mut self, width: u16) {
        let pad = 2u16;
        let content_width = (width as usize).saturating_sub(pad as usize * 2);
        if content_width == 0 {
            self.display_lines = Vec::new();
            return;
        }

        let tab = match self.current_tab() {
            Some(t) => t,
            None => {
                self.display_lines = vec![
                    DisplayLine::empty(),
                    DisplayLine::styled(
                        "  Press Enter to start a new session.",
                        Style::default().fg(self.theme.dim()),
                    ),
                ];
                return;
            }
        };

        let mut lines = Vec::new();
        let theme = &self.theme;

        for cl in &tab.chat_lines {
            build_chat_display_lines(&mut lines, cl, theme, content_width, pad);
        }

        if !tab.streaming_text.is_empty() {
            lines.push(DisplayLine::multi(vec![
                (format!("{}  ", " ".repeat(pad as usize)), Style::default()),
                ("✦ ".to_string(), Style::default().fg(theme.warning)),
                (
                    "phoenix".to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            let body_indent = format!("{}  ", " ".repeat(pad as usize));
            for wl in wrap_text(&tab.streaming_text, content_width.saturating_sub(2)) {
                lines.push(DisplayLine::styled(
                    &format!("{body_indent}{wl}"),
                    Style::default().fg(theme.foreground),
                ));
            }
        }

        if self.is_running {
            let frame_idx = (self.frame_tick / 4) as usize;
            let spin = spinner_frame(frame_idx);
            let thinking_msgs = ["thinking", "thinking.", "thinking..", "thinking..."];
            let msg = thinking_msgs[(self.frame_tick / 12) as usize % thinking_msgs.len()];
            lines.push(DisplayLine::multi(vec![
                (format!("{}  ", " ".repeat(pad as usize)), Style::default()),
                (
                    format!("{spin} "),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                (msg.to_string(), Style::default().fg(theme.dim())),
            ]));
        }

        self.display_lines = lines;
    }

    // ─── Rendering ───────────────────────────────────────────────────────

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        frame.render_widget(
            Block::default().style(Style::default().bg(self.theme.background)),
            area,
        );

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(4),
                Constraint::Length(1),
            ])
            .split(area);

        self.render_header(frame, chunks[0]);
        self.render_chat(frame, chunks[1]);
        self.render_input(frame, chunks[2]);
        self.render_status(frame, chunks[3]);

        if let Some(ref picker) = self.picker {
            match picker.mode {
                PickerMode::CommandComplete => {
                    self.render_command_completion(frame, chunks[2]);
                }
                PickerMode::Theme | PickerMode::Model | PickerMode::Session => {
                    self.render_modal_picker(frame);
                }
            }
        }

        if let Some(ref ob) = self.onboarding {
            ob.render(frame, &self.theme);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let tab_name = self
            .current_tab()
            .map(|t| t.title.as_str())
            .unwrap_or("phoenix");

        let provider_info = crate::config::loader::active_provider(&self.config)
            .map(|(name, p)| format!("{name}/{}", p.model))
            .unwrap_or_else(|| "no provider".into());

        let spinner = if self.is_running {
            let frame_idx = (self.frame_tick / 4) as usize; // slow it down: change every 4 ticks
            spinner_frame(frame_idx)
        } else {
            "✦"
        };

        let left = format!(" {spinner} phoenix · {tab_name}");
        let right = format!("{}  ", provider_info);
        let pad_width =
            (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
        let padding = " ".repeat(pad_width);

        let header_line = Line::from(vec![
            Span::styled(
                left,
                Style::default()
                    .fg(self.theme.primary)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(padding, Style::default()),
            Span::styled(right, Style::default().fg(self.theme.dim())),
        ]);

        let header = Paragraph::new(header_line).style(
            Style::default()
                .fg(self.theme.foreground)
                .bg(self.theme.header_bg()),
        );
        frame.render_widget(header, area);
    }

    fn render_chat(&self, frame: &mut Frame, area: Rect) {
        let effective_scroll = self.effective_scroll();
        let visible = area.height as usize;

        let visible_lines: Vec<Line<'static>> = self
            .display_lines
            .iter()
            .skip(effective_scroll)
            .take(visible)
            .map(|dl| dl.to_line())
            .collect();

        let chat = Paragraph::new(visible_lines).style(Style::default().bg(self.theme.background));
        frame.render_widget(chat, area);
    }

    fn render_input(&self, frame: &mut Frame, area: Rect) {
        let bg = self.theme.background;
        let border_fg = self.theme.separator();
        let sep = "─".repeat(area.width as usize);

        // Top border
        let top = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(sep.clone()).style(Style::default().fg(border_fg).bg(bg)),
            top,
        );

        // Input content
        let input_area = Rect {
            x: area.x,
            y: area.y + 1,
            width: area.width,
            height: area.height.saturating_sub(2),
        };

        let input_text = self
            .current_tab()
            .map(|t| t.input.buffer.as_str())
            .unwrap_or("");

        let prompt_str = "  > ";
        let prompt_len = prompt_str.len();

        let content = if input_text.is_empty() {
            Line::from(vec![
                Span::styled(
                    prompt_str,
                    Style::default()
                        .fg(self.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("Send a message...", Style::default().fg(self.theme.dim())),
            ])
        } else {
            Line::from(vec![
                Span::styled(
                    prompt_str,
                    Style::default()
                        .fg(self.theme.primary)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(input_text, Style::default().fg(self.theme.foreground)),
            ])
        };

        frame.render_widget(
            Paragraph::new(content).style(Style::default().bg(bg)),
            input_area,
        );

        // Bottom border
        let bot = Rect {
            x: area.x,
            y: area.y + area.height.saturating_sub(1),
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(sep).style(Style::default().fg(border_fg).bg(bg)),
            bot,
        );

        // Cursor
        if let Some(tab) = self.current_tab() {
            let cursor_x = input_area.x + prompt_len as u16 + tab.input.cursor as u16;
            let cursor_y = input_area.y;
            frame.set_cursor_position((
                cursor_x.min(input_area.right().saturating_sub(1)),
                cursor_y,
            ));
        }
    }

    fn render_status(&self, frame: &mut Frame, area: Rect) {
        let tokens = self
            .session
            .as_ref()
            .map(|s| {
                let mut tok = format!(
                    "{}↓ {}↑",
                    format_tokens(s.token_input),
                    format_tokens(s.token_output)
                );
                if s.cache_read_tokens > 0 {
                    tok.push_str(&format!(" {}⚡", format_tokens(s.cache_read_tokens)));
                }
                tok
            })
            .unwrap_or_default();

        let hint = if self.ctrl_c_count == 1 {
            "Press Ctrl+C again to quit"
        } else if self.is_running {
            "Ctrl+C cancel"
        } else {
            "Ctrl+C×2 quit · PageUp/Dn scroll"
        };

        let left = if tokens.is_empty() {
            String::new()
        } else {
            format!("  {tokens}")
        };
        let right = format!("{hint}  ");
        let pad_width =
            (area.width as usize).saturating_sub(left.chars().count() + right.chars().count());
        let padding = " ".repeat(pad_width);

        let status_line = Line::from(vec![
            Span::styled(&left, Style::default().fg(self.theme.dim())),
            Span::styled(padding, Style::default()),
            Span::styled(right, Style::default().fg(self.theme.dim())),
        ]);

        let status = Paragraph::new(status_line).style(
            Style::default()
                .fg(self.theme.dim())
                .bg(self.theme.header_bg()),
        );
        frame.render_widget(status, area);
    }

    fn render_command_completion(&self, frame: &mut Frame, input_area: Rect) {
        let picker = match &self.picker {
            Some(p) if p.mode == PickerMode::CommandComplete => p,
            _ => return,
        };

        let max_visible = 8usize;
        let count = picker.visible_count().min(max_visible);
        if count == 0 {
            return;
        }

        let popup_height = count as u16 + 2;
        let popup_width = input_area.width.min(50);
        let popup_area = Rect {
            x: input_area.x,
            y: input_area.y.saturating_sub(popup_height),
            width: popup_width,
            height: popup_height,
        };

        frame.render_widget(Clear, popup_area);

        let items: Vec<ListItem> = picker
            .filtered
            .iter()
            .take(max_visible)
            .enumerate()
            .map(|(i, &idx)| {
                let item = &picker.items[idx];
                let style = if i == picker.cursor {
                    Style::default()
                        .fg(self.theme.background)
                        .bg(self.theme.accent)
                } else {
                    Style::default().fg(self.theme.foreground)
                };
                ListItem::new(format!(" /{:<14} {}", item.label, item.description)).style(style)
            })
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.primary))
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(self.theme.background));

        let list = List::new(items).block(block);
        frame.render_widget(list, popup_area);
    }

    fn render_modal_picker(&self, frame: &mut Frame) {
        let picker = match &self.picker {
            Some(p) => p,
            None => return,
        };

        let area = frame.area();
        let popup_width = 50u16.min(area.width.saturating_sub(4));
        let popup_height = 20u16.min(area.height.saturating_sub(4));
        let popup_area = Rect {
            x: (area.width.saturating_sub(popup_width)) / 2,
            y: (area.height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };

        frame.render_widget(Clear, popup_area);

        let title = match picker.mode {
            PickerMode::Theme => " Theme ",
            PickerMode::Model => " Model ",
            PickerMode::Session => " Session ",
            PickerMode::CommandComplete => "",
        };

        let inner_height = popup_height.saturating_sub(2) as usize;
        let scroll = picker.cursor.saturating_sub(inner_height.saturating_sub(1));

        let items: Vec<ListItem> = picker
            .filtered
            .iter()
            .skip(scroll)
            .take(inner_height)
            .enumerate()
            .map(|(i, &idx)| {
                let item = &picker.items[idx];
                let display_idx = scroll + i;
                let style = if display_idx == picker.cursor {
                    Style::default()
                        .fg(self.theme.background)
                        .bg(self.theme.accent)
                } else {
                    Style::default().fg(self.theme.foreground)
                };
                let text = if item.description.is_empty() {
                    format!("  {}", item.label)
                } else {
                    format!("  {:<20} {}", item.label, item.description)
                };
                ListItem::new(text).style(style)
            })
            .collect();

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.accent))
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(self.theme.background))
            .title(title);

        let list = List::new(items).block(block);
        frame.render_widget(list, popup_area);
    }

    // ─── Picker / theme / model / onboarding ─────────────────────────────

    fn handle_modal_picker_key(&mut self, key: KeyEvent) {
        let mode = match self.picker.as_ref() {
            Some(p) => p.mode.clone(),
            None => return,
        };

        match key.code {
            KeyCode::Up => {
                self.picker.as_mut().unwrap().move_up();
                if mode == PickerMode::Theme {
                    self.preview_selected_theme();
                }
            }
            KeyCode::Down => {
                self.picker.as_mut().unwrap().move_down();
                if mode == PickerMode::Theme {
                    self.preview_selected_theme();
                }
            }
            KeyCode::Enter => {
                if let Some(selected) = self.picker.as_ref().and_then(|p| p.selected()).cloned() {
                    match mode {
                        PickerMode::Theme => {
                            if let Some(t) = theme::get_by_name(&selected.id) {
                                self.theme = t;
                            }
                            self.saved_theme = None;
                            let config_path = crate::config::paths::user_config_file();
                            let _ = crate::config::writer::save_theme(&config_path, &selected.id);
                            if let Some(tab) = self.current_tab_mut() {
                                tab.chat_lines.push(ChatLine {
                                    role: crate::session::message::Role::System,
                                    content: format!("Theme: {}", selected.label),
                                });
                            }
                        }
                        PickerMode::Model => {
                            self.apply_model_selection(&selected.id);
                        }
                        PickerMode::Session => {
                            self.pending_session_resume = Some(selected.id.clone());
                        }
                        _ => {}
                    }
                }
                self.picker = None;
            }
            KeyCode::Esc => {
                self.restore_theme();
                self.picker = None;
            }
            KeyCode::Char(c) => {
                let picker = self.picker.as_mut().unwrap();
                let mut filter = picker.filter.clone();
                filter.push(c);
                picker.set_filter(&filter);
                if mode == PickerMode::Theme {
                    self.preview_selected_theme();
                }
            }
            KeyCode::Backspace => {
                let picker = self.picker.as_mut().unwrap();
                let mut filter = picker.filter.clone();
                filter.pop();
                picker.set_filter(&filter);
                if mode == PickerMode::Theme {
                    self.preview_selected_theme();
                }
            }
            _ => {}
        }
    }

    fn preview_selected_theme(&mut self) {
        if let Some(selected) = self.picker.as_ref().and_then(|p| p.selected())
            && let Some(t) = theme::get_by_name(&selected.id)
        {
            self.theme = t;
        }
    }

    fn restore_theme(&mut self) {
        if let Some(saved) = self.saved_theme.take() {
            self.theme = saved;
        }
    }

    fn complete_command(&mut self) {
        if let Some(selected) = self.picker.as_ref().and_then(|p| p.selected()).cloned() {
            let cmd = format!("/{}", selected.id);
            if let Some(tab) = self.current_tab_mut() {
                tab.input.buffer = cmd.clone();
                tab.input.cursor = cmd.len();
            }
        }
        self.picker = None;
    }

    fn apply_model_selection(&mut self, id: &str) {
        if let Ok(idx) = id.parse::<usize>()
            && let Some(choice) = self.model_choices.get(idx)
        {
            match providers::create_provider(&choice.provider_name, &choice.profile) {
                Ok(p) => {
                    self.provider = Some(Arc::from(p));
                    let display = choice.display.clone();
                    if let Some(tab) = self.current_tab_mut() {
                        tab.chat_lines.push(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Model: {display}"),
                        });
                    }
                }
                Err(e) => {
                    if let Some(tab) = self.current_tab_mut() {
                        tab.chat_lines.push(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Error: {e}"),
                        });
                    }
                }
            }
        }
    }

    fn complete_onboarding(
        &mut self,
        name: String,
        kind: crate::config::schema::ProviderKind,
        model: String,
        api_key: Option<String>,
        env_hint: String,
    ) {
        use crate::config::schema::{AuthEntry, ProviderProfile};

        let auth = api_key.map(AuthEntry::InlineValue).or({
            if !env_hint.is_empty() {
                Some(AuthEntry::EnvVar(env_hint))
            } else {
                None
            }
        });

        let profile = ProviderProfile {
            kind,
            model: model.clone(),
            active: true,
            auth,
            ..Default::default()
        };

        let config_path = crate::config::paths::user_config_file();
        if let Err(e) = crate::config::writer::save_provider(&config_path, &name, &profile) {
            if let Some(tab) = self.current_tab_mut() {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Failed to save config: {e}"),
                });
            }
            self.onboarding = None;
            return;
        }

        if let Ok(cfg) = crate::config::loader::load(None) {
            self.config = cfg;
        }

        if let Some((pname, pprofile)) = crate::config::loader::active_provider(&self.config) {
            match providers::create_provider(pname, pprofile) {
                Ok(p) => self.provider = Some(Arc::from(p)),
                Err(e) => {
                    tracing::warn!("failed to create provider after onboarding: {e}");
                }
            }
        }

        self.onboarding = None;

        if let Some(tab) = self.current_tab_mut() {
            tab.chat_lines.push(ChatLine {
                role: crate::session::message::Role::System,
                content: format!("Connected to {name} ({model})."),
            });
        }
    }

    fn update_command_completion(&mut self) {
        if let Some(ref picker) = self.picker
            && picker.mode != PickerMode::CommandComplete
        {
            return;
        }

        let buffer = match self.current_tab() {
            Some(tab) => tab.input.buffer.clone(),
            None => {
                self.picker = None;
                return;
            }
        };

        if !buffer.starts_with('/') || buffer.contains(' ') || buffer.contains('\n') {
            self.picker = None;
            return;
        }

        let filter = &buffer[1..];
        let mut picker = PickerState::new(self.command_items.clone(), PickerMode::CommandComplete);
        picker.set_filter(filter);

        if picker.visible_count() == 0 {
            self.picker = None;
            return;
        }

        self.picker = Some(picker);
    }

    pub fn handle_paste(&mut self, text: &str) {
        if let Some(ref mut ob) = self.onboarding {
            ob.handle_paste(text);
            return;
        }
        if let Some(tab) = self.current_tab_mut() {
            tab.input.insert_str(text);
        }
        self.update_command_completion();
    }
}

// ─── Chat display line building ──────────────────────────────────────────────

fn build_chat_display_lines(
    lines: &mut Vec<DisplayLine>,
    cl: &ChatLine,
    theme: &Theme,
    content_width: usize,
    pad: u16,
) {
    use crate::session::message::Role;
    let indent = " ".repeat(pad as usize);
    let body_indent = format!("{}  ", indent);

    match cl.role {
        Role::User => {
            let header = format!("{}  you", indent);
            lines.push(DisplayLine::styled(
                &header,
                Style::default()
                    .fg(theme.primary)
                    .add_modifier(Modifier::BOLD),
            ));
            for wl in wrap_text(&cl.content, content_width.saturating_sub(2)) {
                lines.push(DisplayLine::styled(
                    &format!("{body_indent}{wl}"),
                    Style::default().fg(theme.foreground),
                ));
            }
            lines.push(DisplayLine::empty());
        }
        Role::Assistant => {
            lines.push(DisplayLine::multi(vec![
                (format!("{}  ", indent), Style::default()),
                ("✦ ".to_string(), Style::default().fg(theme.warning)),
                (
                    "phoenix".to_string(),
                    Style::default()
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
            let md_lines = render_markdown(
                &cl.content,
                theme,
                &body_indent,
                content_width.saturating_sub(2),
            );
            lines.extend(md_lines);
            lines.push(DisplayLine::empty());
        }
        Role::ToolCall => {
            lines.push(DisplayLine::multi(vec![
                (format!("{}  ", indent), Style::default()),
                (
                    "▶ ".to_string(),
                    Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
                ),
                (
                    cl.content.clone(),
                    Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        Role::ToolResult => {
            let is_error = cl.content.starts_with("Error:")
                || cl.content.starts_with("error:")
                || cl.content.starts_with("unknown tool:");

            if !is_error && is_diff_content(&cl.content) {
                render_side_by_side_diff(lines, &cl.content, theme, &indent, content_width);
            } else {
                let border_color = if is_error {
                    theme.error
                } else {
                    theme.tool_border()
                };
                let result_width = content_width.saturating_sub(6);

                let output_lines: Vec<&str> = cl.content.lines().collect();
                let max_display = 20;
                let truncated = output_lines.len() > max_display;

                let show_lines: Vec<&str> = if truncated {
                    let mut v: Vec<&str> = output_lines[..10].to_vec();
                    v.push("");
                    v.extend_from_slice(&output_lines[output_lines.len().saturating_sub(5)..]);
                    v
                } else {
                    output_lines.clone()
                };

                let hidden_count = if truncated {
                    output_lines.len().saturating_sub(15)
                } else {
                    0
                };

                for (i, line_text) in show_lines.iter().enumerate() {
                    if truncated && i == 10 {
                        lines.push(DisplayLine::multi(vec![
                            (format!("{}  │ ", indent), Style::default().fg(border_color)),
                            (
                                format!("... ({hidden_count} more lines)"),
                                Style::default()
                                    .fg(theme.dim())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                        continue;
                    }

                    let wrapped = wrap_text(line_text, result_width);
                    for wl in wrapped {
                        let text_style = diff_line_style(&wl, theme, is_error);
                        lines.push(DisplayLine::multi(vec![
                            (format!("{}  │ ", indent), Style::default().fg(border_color)),
                            (wl, text_style),
                        ]));
                    }
                }

                let (icon, icon_color) = if is_error {
                    ("✗", theme.error)
                } else {
                    ("✓", theme.success)
                };
                lines.push(DisplayLine::multi(vec![
                    (format!("{}  ╰ ", indent), Style::default().fg(border_color)),
                    (format!("{icon} done"), Style::default().fg(icon_color)),
                ]));
            }
            lines.push(DisplayLine::empty());
        }
        Role::System => {
            let msg = &cl.content;
            let text = format!("{indent}  ── {msg} ──");
            lines.push(DisplayLine::styled(
                &text,
                Style::default()
                    .fg(theme.dim())
                    .add_modifier(Modifier::ITALIC),
            ));
            lines.push(DisplayLine::empty());
        }
    }
}

fn diff_line_style(line: &str, theme: &Theme, is_error: bool) -> Style {
    if is_error {
        return Style::default().fg(theme.error);
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        Style::default().fg(theme.diff_add)
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        Style::default().fg(theme.diff_delete)
    } else if trimmed.starts_with("@@") {
        Style::default()
            .fg(theme.info)
            .add_modifier(Modifier::ITALIC)
    } else {
        Style::default().fg(theme.dim())
    }
}

// ─── Side-by-side diff ───────────────────────────────────────────────────────

fn is_diff_content(content: &str) -> bool {
    let first = content.lines().next().unwrap_or("");
    if !(first.contains("edited") && first.contains("replaced")) {
        return false;
    }
    content
        .lines()
        .any(|l| l.starts_with("- ") || l.starts_with("+ "))
}

fn render_side_by_side_diff(
    lines: &mut Vec<DisplayLine>,
    content: &str,
    theme: &Theme,
    indent: &str,
    content_width: usize,
) {
    let mut header = String::new();
    let mut old_lines: Vec<&str> = Vec::new();
    let mut new_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        if line.starts_with("edited ") {
            header = line.to_string();
        } else if let Some(rest) = line.strip_prefix("- ") {
            old_lines.push(rest);
        } else if let Some(rest) = line.strip_prefix("+ ") {
            new_lines.push(rest);
        }
    }

    // Header
    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        (header, Style::default().fg(theme.dim())),
    ]));

    // Each side gets half the width minus line numbers and separator
    let gutter = 4; // "nn│ "
    let sep_w = 3; // " │ "
    let usable = content_width.saturating_sub(gutter * 2 + sep_w);
    let half = usable / 2;
    if half < 8 {
        // Too narrow for side-by-side, fall back to inline
        for ol in &old_lines {
            lines.push(DisplayLine::styled(
                &format!("{indent}  - {ol}"),
                Style::default().fg(theme.diff_delete),
            ));
        }
        for nl in &new_lines {
            lines.push(DisplayLine::styled(
                &format!("{indent}  + {nl}"),
                Style::default().fg(theme.diff_add),
            ));
        }
        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  "), Style::default()),
            ("✓ done".to_string(), Style::default().fg(theme.success)),
        ]));
        return;
    }

    // Column headers
    let lh = pad_to_width("removed", half);
    let rh = pad_to_width("added", half);
    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        (" ".repeat(gutter), Style::default()),
        (
            lh,
            Style::default()
                .fg(theme.diff_delete)
                .add_modifier(Modifier::BOLD),
        ),
        (" │ ".to_string(), Style::default().fg(theme.dim())),
        (" ".repeat(gutter), Style::default()),
        (
            rh,
            Style::default()
                .fg(theme.diff_add)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    // Separator line
    let dash_l = "─".repeat(half + gutter);
    let dash_r = "─".repeat(half + gutter);
    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        (dash_l, Style::default().fg(theme.dim())),
        ("─┼─".to_string(), Style::default().fg(theme.dim())),
        (dash_r, Style::default().fg(theme.dim())),
    ]));

    // Rows
    let max_rows = old_lines.len().max(new_lines.len());
    for i in 0..max_rows {
        let left = old_lines.get(i).copied().unwrap_or("");
        let right = new_lines.get(i).copied().unwrap_or("");

        let ln = if i < old_lines.len() {
            format!("{:>2}│ ", i + 1)
        } else {
            "  │ ".to_string()
        };
        let rn = if i < new_lines.len() {
            format!("{:>2}│ ", i + 1)
        } else {
            "  │ ".to_string()
        };

        let lt = pad_to_width(left, half);
        let rt = pad_to_width(right, half);

        let left_style = if left.is_empty() && i >= old_lines.len() {
            Style::default().fg(theme.dim())
        } else {
            Style::default().fg(theme.diff_delete)
        };
        let right_style = if right.is_empty() && i >= new_lines.len() {
            Style::default().fg(theme.dim())
        } else {
            Style::default().fg(theme.diff_add)
        };

        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  "), Style::default()),
            (ln, Style::default().fg(theme.dim())),
            (lt, left_style),
            (" │ ".to_string(), Style::default().fg(theme.dim())),
            (rn, Style::default().fg(theme.dim())),
            (rt, right_style),
        ]));
    }

    // Status
    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        ("✓ done".to_string(), Style::default().fg(theme.success)),
    ]));
}

fn pad_to_width(s: &str, width: usize) -> String {
    let char_count = s.chars().count();
    if char_count > width {
        let truncated: String = s.chars().take(width.saturating_sub(1)).collect();
        format!("{truncated}…")
    } else {
        let padding = " ".repeat(width - char_count);
        format!("{s}{padding}")
    }
}

// ─── Markdown rendering ──────────────────────────────────────────────────────

fn render_markdown(text: &str, theme: &Theme, indent: &str, max_width: usize) -> Vec<DisplayLine> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let code_bg = Theme::darken(theme.background, 0.75);

    for line in text.split('\n') {
        let trimmed = line.trim_start();

        // Code fence toggle
        if trimmed.starts_with("```") {
            if !in_code_block {
                in_code_block = true;
                let lang = trimmed.trim_start_matches('`').trim();
                let label = if lang.is_empty() {
                    String::new()
                } else {
                    format!(" {lang} ")
                };
                lines.push(DisplayLine::multi(vec![(
                    format!("{indent}┌{label}"),
                    Style::default().fg(theme.dim()),
                )]));
            } else {
                in_code_block = false;
                lines.push(DisplayLine::multi(vec![(
                    format!("{indent}└"),
                    Style::default().fg(theme.dim()),
                )]));
            }
            continue;
        }

        if in_code_block {
            lines.push(DisplayLine::multi(vec![
                (format!("{indent}│ "), Style::default().fg(theme.dim())),
                (
                    line.to_string(),
                    Style::default().fg(theme.code_fg()).bg(code_bg),
                ),
            ]));
            continue;
        }

        // Headers
        if let Some(header) = trimmed.strip_prefix("### ") {
            let header = header.trim();
            lines.push(DisplayLine::styled(
                &format!("{indent}{header}"),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if let Some(header) = trimmed.strip_prefix("## ") {
            let header = header.trim();
            lines.push(DisplayLine::styled(
                &format!("{indent}{header}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if let Some(header) = trimmed.strip_prefix("# ") {
            let header = header.trim();
            lines.push(DisplayLine::styled(
                &format!("{indent}{header}"),
                Style::default()
                    .fg(theme.primary)
                    .add_modifier(Modifier::BOLD),
            ));
            continue;
        }

        // Bullet lists
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let item = &trimmed[2..];
            let mut parts = vec![(format!("{indent}  • "), Style::default().fg(theme.dim()))];
            parts.extend(parse_inline_md(item, theme));
            lines.push(DisplayLine::multi(parts));
            continue;
        }

        // Numbered lists
        if let Some(rest) = strip_numbered_list(trimmed) {
            let mut parts = vec![(format!("{indent}  "), Style::default())];
            let prefix_end = trimmed.len() - rest.len();
            parts.push((
                trimmed[..prefix_end].to_string(),
                Style::default().fg(theme.dim()),
            ));
            parts.extend(parse_inline_md(rest, theme));
            lines.push(DisplayLine::multi(parts));
            continue;
        }

        // Empty line
        if trimmed.is_empty() {
            lines.push(DisplayLine::empty());
            continue;
        }

        // Regular paragraph — wrap then apply inline formatting
        for wl in wrap_text(line, max_width) {
            let mut parts = vec![(indent.to_string(), Style::default())];
            parts.extend(parse_inline_md(&wl, theme));
            lines.push(DisplayLine::multi(parts));
        }
    }

    lines
}

fn strip_numbered_list(s: &str) -> Option<&str> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if !first.is_ascii_digit() {
        return None;
    }
    for ch in chars.by_ref() {
        if ch == '.' {
            let rest = chars.as_str();
            if let Some(stripped) = rest.strip_prefix(' ') {
                return Some(stripped.trim_start());
            }
            return None;
        }
        if !ch.is_ascii_digit() {
            return None;
        }
    }
    None
}

fn parse_inline_md(text: &str, theme: &Theme) -> Vec<(String, Style)> {
    let normal = Style::default().fg(theme.foreground);
    let bold = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::BOLD);
    let italic = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::ITALIC);
    let code = Style::default().fg(theme.code_fg());

    let mut spans: Vec<(String, Style)> = Vec::new();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '`' => {
                if !buf.is_empty() {
                    spans.push((std::mem::take(&mut buf), normal));
                }
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '`' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if closed {
                    spans.push((inner, code));
                } else {
                    buf.push('`');
                    buf.push_str(&inner);
                }
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if !buf.is_empty() {
                    spans.push((std::mem::take(&mut buf), normal));
                }
                let mut inner = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if closed {
                    spans.push((inner, bold));
                } else {
                    buf.push_str("**");
                    buf.push_str(&inner);
                }
            }
            '*' => {
                if !buf.is_empty() {
                    spans.push((std::mem::take(&mut buf), normal));
                }
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '*' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if closed {
                    spans.push((inner, italic));
                } else {
                    buf.push('*');
                    buf.push_str(&inner);
                }
            }
            _ => buf.push(ch),
        }
    }

    if !buf.is_empty() {
        spans.push((buf, normal));
    }

    spans
}

// ─── Tool call summary ──────────────────────────────────────────────────────

fn tool_call_summary(name: &str, args_json: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(args_json).unwrap_or_default();
    match name {
        "bash" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let display = if cmd.len() > 120 {
                format!("{}...", &cmd[..117])
            } else {
                cmd.to_string()
            };
            format!("bash > {display}")
        }
        "read" => {
            let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            format!("read > {path}")
        }
        "edit" => {
            let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            format!("edit > {path}")
        }
        "write" => {
            let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            format!("write > {path}")
        }
        _ => name.to_string(),
    }
}

// ─── Stream buffer drain ─────────────────────────────────────────────────────

fn drain_stream_buffer(tab: &mut Tab) {
    if tab.stream_buffer.is_empty() {
        return;
    }
    let buf_chars = tab.stream_buffer.chars().count();
    let n = if buf_chars > 200 {
        20
    } else if buf_chars > 50 {
        8
    } else {
        4
    }
    .min(buf_chars);

    let byte_pos = tab
        .stream_buffer
        .char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(tab.stream_buffer.len());
    let chunk = tab.stream_buffer[..byte_pos].to_string();
    tab.stream_buffer = tab.stream_buffer[byte_pos..].to_string();
    tab.streaming_text.push_str(&chunk);
}

// ─── Text wrapping ───────────────────────────────────────────────────────────

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return text.lines().map(String::from).collect();
    }

    let mut result = Vec::new();

    for line in text.split('\n') {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }

        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            result.push(String::new());
            continue;
        }

        let mut current = String::new();
        let mut col = 0usize;

        for word in &words {
            let wlen = word.chars().count();
            if col == 0 {
                current.push_str(word);
                col = wlen;
            } else if col + 1 + wlen <= max_width {
                current.push(' ');
                current.push_str(word);
                col += 1 + wlen;
            } else {
                result.push(std::mem::take(&mut current));
                current.push_str(word);
                col = wlen;
            }
        }

        if !current.is_empty() {
            result.push(current);
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }

    result
}

// ─── Token formatting ────────────────────────────────────────────────────────

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn spinner_frame(idx: usize) -> &'static str {
    SPINNER_FRAMES[idx % SPINNER_FRAMES.len()]
}

fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ─── System prompt ──────────────────────────────────────────────────────────

fn default_system_prompt() -> &'static str {
    "You are Phoenix, a fast and capable coding assistant running in a terminal.\n\
     \n\
     You have access to tools for reading files, writing files, editing files, and \
     running shell commands. Use them to help the user with software engineering tasks.\n\
     \n\
     Guidelines:\n\
     - Be concise. The user is in a terminal — respect their screen space.\n\
     - When editing code, preserve existing style and conventions.\n\
     - Prefer editing existing files over creating new ones.\n\
     - Use the bash tool for commands; use read/write/edit tools for files.\n\
     - Show your work: explain what you're doing briefly, then do it.\n\
     - If a task is ambiguous, make a reasonable assumption and proceed.\n\
     - When you encounter errors, diagnose the root cause before retrying."
}

// ─── Main loop ───────────────────────────────────────────────────────────────

pub async fn run(config: Config, _needs_onboarding: bool) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableMouseCapture,
    )?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config);

    let history_file = crate::config::paths::history_file();
    let rx = app.events_tx.subscribe();
    app.tabs.push(Tab::new("default".into(), rx, history_file));

    let result = run_loop(&mut app, &mut terminal).await;

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen,
        crossterm::event::DisableBracketedPaste,
        crossterm::event::DisableMouseCapture,
    )?;
    terminal.show_cursor()?;

    result
}

async fn run_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) -> anyhow::Result<()> {
    loop {
        let size = terminal.size()?;
        let area = Rect::new(0, 0, size.width, size.height);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(4),
                Constraint::Length(1),
            ])
            .split(area);
        app.chat_area_height = chunks[1].height;
        app.frame_tick = app.frame_tick.wrapping_add(1);
        app.compute_display_lines(size.width);

        terminal.draw(|f| app.render(f))?;

        if ct_event::poll(Duration::from_millis(16))? {
            match ct_event::read()? {
                CEvent::Key(key) => {
                    let is_plain_enter = key.code == KeyCode::Enter
                        && !key.modifiers.contains(KeyModifiers::SHIFT)
                        && !key.modifiers.contains(KeyModifiers::ALT);

                    let consumed = app.handle_key(key);

                    if is_plain_enter && !app.is_running && !consumed {
                        let input_text = app
                            .current_tab()
                            .map(|t| t.input.buffer.clone())
                            .unwrap_or_default();

                        if !input_text.trim().is_empty()
                            && crate::commands::dispatcher::is_command(input_text.trim())
                        {
                            if let Some(tab) = app.current_tab_mut() {
                                tab.input.submit();
                            }
                            handle_command(app, input_text.trim());
                        } else if let Some(text) = app.submit_message() {
                            send_message(app, text, terminal).await;
                        }
                    }
                }
                CEvent::Paste(text) => app.handle_paste(&text),
                CEvent::Mouse(mouse) => {
                    use crossterm::event::MouseEventKind;
                    match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            if let Some(tab) = app.current_tab_mut() {
                                tab.scroll_up(3);
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            let total = app.display_lines.len();
                            let visible = app.chat_area_height as usize;
                            if let Some(tab) = app.current_tab_mut() {
                                tab.scroll_down(3, total, visible);
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if let Some(tab) = app.tabs.get_mut(app.active_tab) {
            while let Ok(event) = tab.events_rx.try_recv() {
                tab.apply_event(event);
            }
        }

        if let Some(session_id) = app.pending_session_resume.take() {
            resume_session(app, &session_id).await;
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

async fn resume_session(app: &mut App, session_id: &str) {
    let sid = SessionId::from(session_id.to_string());
    match app.store.load_messages(&app.project, &sid).await {
        Ok(raw_messages) => {
            let mut session = Session::new(sid, crate::config::schema::SessionProfile::default());
            if let Some((name, profile)) = crate::config::loader::active_provider(&app.config) {
                session.provider_name = name.to_string();
                session.model_name = profile.model.clone();
            }

            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.clear();
            }

            for val in &raw_messages {
                if let Ok(msg) = serde_json::from_value::<Message>(val.clone()) {
                    match msg.role {
                        crate::session::message::Role::User
                        | crate::session::message::Role::Assistant => {
                            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                tab.chat_lines.push(ChatLine {
                                    role: msg.role.clone(),
                                    content: msg.content.clone(),
                                });
                            }
                        }
                        crate::session::message::Role::ToolCall => {
                            if let Some(tc) = &msg.tool_call {
                                let summary = tool_call_summary(&tc.name, &tc.args_json);
                                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                    tab.chat_lines.push(ChatLine {
                                        role: crate::session::message::Role::ToolCall,
                                        content: summary,
                                    });
                                }
                            }
                        }
                        crate::session::message::Role::ToolResult => {
                            if let Some(tr) = &msg.tool_result {
                                let output = if tr.output.len() > 2000 {
                                    format!("{}...", &tr.output[..2000])
                                } else {
                                    tr.output.clone()
                                };
                                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                    tab.chat_lines.push(ChatLine {
                                        role: crate::session::message::Role::ToolResult,
                                        content: output,
                                    });
                                }
                            }
                        }
                        _ => {}
                    }
                    session.add_message(msg);
                }
            }

            let msg_count = session.messages.len();
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Resumed session ({msg_count} messages)"),
                });
            }
            app.session = Some(session);
        }
        Err(e) => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Failed to resume: {e}"),
                });
            }
        }
    }
}

fn handle_command(app: &mut App, input: &str) {
    let skills = crate::session::skills::discover_layered(
        Some(&app.project),
        &crate::config::paths::user_home(),
        &app.config.skills.dirs,
    );
    let result = crate::commands::dispatcher::dispatch(
        input,
        &app.config,
        &skills,
        &app.store,
        &app.project,
    );

    match result {
        crate::commands::CommandResult::Message(msg) => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: msg,
                });
            }
        }
        crate::commands::CommandResult::Error(err) => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Error: {err}"),
                });
            }
        }
        crate::commands::CommandResult::ClearSession => {
            app.session = None;
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.clear();
                tab.streaming_text.clear();
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "Session cleared.".into(),
                });
            }
        }
        crate::commands::CommandResult::InjectContext { name, content } => {
            if app.session.is_none() {
                app.session = Some(Session::new(
                    SessionId::new(),
                    crate::config::schema::SessionProfile::default(),
                ));
            }
            if let Some(session) = &mut app.session {
                if !session.context_state.activated_skills.insert(name.clone()) {
                    if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                        tab.chat_lines.push(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Skill '{name}' already loaded in this session."),
                        });
                    }
                } else {
                    session.add_message(Message::system(&content));
                    let preview = if content.len() > 80 {
                        format!("{}...", &content[..77])
                    } else {
                        content.clone()
                    };
                    if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                        tab.chat_lines.push(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Skill loaded: {preview}"),
                        });
                    }
                }
            }
        }
        crate::commands::CommandResult::ThemePicker(themes) => {
            if themes.len() == 1 {
                let entry = &themes[0];
                if let Some(t) = theme::get_by_name(&entry.id) {
                    app.theme = t;
                }
                let config_path = crate::config::paths::user_config_file();
                let _ = crate::config::writer::save_theme(&config_path, &entry.id);
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    tab.chat_lines.push(ChatLine {
                        role: crate::session::message::Role::System,
                        content: format!("Theme: {}", entry.name),
                    });
                }
            } else {
                app.saved_theme = Some(app.theme.clone());
                let items: Vec<PickerItem> = themes
                    .iter()
                    .map(|t| PickerItem {
                        id: t.id.clone(),
                        label: t.name.clone(),
                        description: String::new(),
                    })
                    .collect();
                app.picker = Some(PickerState::new(items, PickerMode::Theme));
            }
        }
        crate::commands::CommandResult::ModelPicker(choices) => {
            if choices.len() == 1 {
                app.model_choices = choices;
                app.apply_model_selection("0");
            } else {
                let items: Vec<PickerItem> = choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| PickerItem {
                        id: i.to_string(),
                        label: c.display.clone(),
                        description: c.provider_name.clone(),
                    })
                    .collect();
                app.model_choices = choices;
                app.picker = Some(PickerState::new(items, PickerMode::Model));
            }
        }
        crate::commands::CommandResult::ModelsPage => {
            let choices = crate::commands::model::list_model_entries(&app.config);
            if choices.is_empty() {
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    tab.chat_lines.push(ChatLine {
                        role: crate::session::message::Role::System,
                        content: "No models configured. Use /connect to add a provider.".into(),
                    });
                }
            } else {
                let items: Vec<PickerItem> = choices
                    .iter()
                    .enumerate()
                    .map(|(i, c)| PickerItem {
                        id: i.to_string(),
                        label: c.display.clone(),
                        description: c.provider_name.clone(),
                    })
                    .collect();
                app.model_choices = choices;
                app.picker = Some(PickerState::new(items, PickerMode::Model));
            }
        }
        crate::commands::CommandResult::SessionPicker(choices) => {
            if choices.is_empty() {
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    tab.chat_lines.push(ChatLine {
                        role: crate::session::message::Role::System,
                        content: "No sessions to resume.".into(),
                    });
                }
            } else {
                let items: Vec<PickerItem> = choices
                    .iter()
                    .map(|c| PickerItem {
                        id: c.id.clone(),
                        label: c.display_name.clone(),
                        description: if c.model.is_empty() {
                            c.provider.clone()
                        } else {
                            format!("{}/{}", c.provider, c.model)
                        },
                    })
                    .collect();
                app.picker = Some(PickerState::new(items, PickerMode::Session));
            }
        }
        crate::commands::CommandResult::CompactSession => {
            if let Some(session) = &mut app.session {
                let before = session.messages.len();
                let force_limits = crate::session::context::ContextLimits {
                    context_window: 200_000,
                    max_output: 16_384,
                    threshold: 0.0,
                };
                let result =
                    crate::session::context::compact_messages(&mut session.messages, &force_limits);
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    if result.was_compacted {
                        tab.chat_lines.push(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!(
                                "Compacted session: removed {} messages ({before} → {})",
                                result.removed_count, result.remaining_count
                            ),
                        });
                    } else {
                        tab.chat_lines.push(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Session has {before} messages — too few to compact."),
                        });
                    }
                }
            } else if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "No active session to compact.".into(),
                });
            }
        }
        crate::commands::CommandResult::ConnectWizard => {
            app.onboarding = Some(onboarding::OnboardingState::new());
        }
        other => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("{other:?}"),
                });
            }
        }
    }
}

async fn send_message(
    app: &mut App,
    text: String,
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
) {
    let provider = match &app.provider {
        Some(p) => Arc::clone(p),
        None => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "No provider configured. Use /connect to add one.".into(),
                });
            }
            return;
        }
    };

    let mut session = app.session.take().unwrap_or_else(|| {
        let mut s = Session::new(
            SessionId::new(),
            crate::config::schema::SessionProfile::default(),
        );
        if let Some((name, profile)) = crate::config::loader::active_provider(&app.config) {
            s.provider_name = name.to_string();
            s.model_name = profile.model.clone();
        }
        s
    });

    let user_msg = Message::user(&text);
    session
        .persist_message(&app.store, &app.project, &user_msg)
        .await;
    session.add_message(user_msg);
    app.is_running = true;

    // Show user message + thinking indicator immediately, before network call
    redraw(app, terminal);

    use crate::providers::traits::{
        Event, ProviderMessage, ProviderRole, ProviderToolCall, ProviderToolResult, SendOptions,
        StopReason, ToolSchema,
    };
    use crossterm::event::EventStream;
    use futures::StreamExt;

    let mut term_events = EventStream::new();

    loop {
        let tool_schemas: Vec<ToolSchema> = app
            .tools
            .list_schemas()
            .into_iter()
            .map(|s| ToolSchema {
                name: s.name.to_string(),
                description: s.description.to_string(),
                parameters: s.parameters,
            })
            .collect();

        let base_prompt = session
            .profile
            .system_prompt_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .or_else(|| Some(default_system_prompt().to_string()));

        // Context injection: rules + AGENTS.md + skills catalog
        let home = crate::config::paths::config_dir()
            .parent()
            .unwrap_or(std::path::Path::new("/"))
            .to_path_buf();
        let skills = crate::session::skills::discover_layered(
            Some(&app.project),
            &crate::config::paths::user_home(),
            &app.config.skills.dirs,
        );
        let ctx = crate::session::context::build_context(
            &home,
            &app.project,
            &session.messages,
            &mut session.context_state,
            &skills,
        );

        let system_prompt = base_prompt.map(|base| {
            if ctx.system_prompt_suffix.is_empty() {
                base
            } else {
                format!("{base}\n\n{}", ctx.system_prompt_suffix)
            }
        });

        if !ctx.newly_loaded.is_empty() {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Context loaded: {}", ctx.newly_loaded.join(", ")),
                });
            }
            redraw(app, terminal);
        }

        // Auto-compaction: enforce context limits
        let active_provider_profile = crate::config::loader::active_provider(&app.config)
            .map(|(_, p)| p.clone())
            .unwrap_or_default();
        let limits = crate::session::context::resolve_context_limits(
            &session.model_name,
            &active_provider_profile,
            &session.profile,
        );
        let prompt_ref = system_prompt.as_deref().unwrap_or("");
        let compaction =
            crate::session::context::enforce_limits(&mut session.messages, prompt_ref, &limits);
        if compaction.was_compacted {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!(
                        "Context compacted: removed {} messages ({} remaining) to stay within {} token limit",
                        compaction.removed_count,
                        compaction.remaining_count,
                        limits.context_window,
                    ),
                });
            }
            redraw(app, terminal);
        }

        let provider_messages: Vec<ProviderMessage> = session
            .messages
            .iter()
            .map(|m| ProviderMessage {
                role: match m.role {
                    crate::session::message::Role::System => ProviderRole::System,
                    crate::session::message::Role::User => ProviderRole::User,
                    crate::session::message::Role::Assistant => ProviderRole::Assistant,
                    crate::session::message::Role::ToolCall => ProviderRole::Assistant,
                    crate::session::message::Role::ToolResult => ProviderRole::Tool,
                },
                content: m.content.clone(),
                tool_call: m.tool_call.as_ref().map(|tc| ProviderToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    args_json: tc.args_json.clone(),
                }),
                tool_result: m.tool_result.as_ref().map(|tr| ProviderToolResult {
                    id: tr.id.clone(),
                    output: tr.output.clone(),
                    is_error: tr.is_error,
                }),
            })
            .collect();

        let opts = SendOptions {
            messages: provider_messages,
            tools: tool_schemas,
            system_prompt,
        };

        let stream = match provider.send(opts).await {
            Ok(s) => s,
            Err(e) => {
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    tab.chat_lines.push(ChatLine {
                        role: crate::session::message::Role::System,
                        content: format!("Provider error: {e}"),
                    });
                }
                app.is_running = false;
                app.session = Some(session);
                return;
            }
        };

        futures::pin_mut!(stream);

        let mut assistant_text = String::new();
        let mut pending_tool_calls: Vec<crate::session::message::ToolCall> = vec![];
        let mut got_tool_use_stop = false;
        let mut cancelled = false;
        let mut tick = tokio::time::interval(Duration::from_millis(16));

        loop {
            tokio::select! {
                maybe_event = stream.next() => {
                    match maybe_event {
                        Some(Event::Token(t)) => {
                            assistant_text.push_str(&t);
                            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                tab.stream_buffer.push_str(&t);
                            }
                        }
                        Some(Event::ToolCall { id, name, args_json }) => {
                            pending_tool_calls.push(crate::session::message::ToolCall {
                                id,
                                name,
                                args_json,
                            });
                        }
                        Some(Event::Done { stop_reason, usage }) => {
                            session.token_input += usage.input_tokens;
                            session.token_output += usage.output_tokens;
                            session.cache_creation_tokens += usage.cache_creation_tokens;
                            session.cache_read_tokens += usage.cache_read_tokens;
                            if stop_reason == StopReason::ToolUse {
                                got_tool_use_stop = true;
                            }
                            break;
                        }
                        Some(Event::Error(e)) => {
                            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                tab.streaming_text.clear();
                                tab.stream_buffer.clear();
                                tab.chat_lines.push(ChatLine {
                                    role: crate::session::message::Role::System,
                                    content: format!("Error: {e}"),
                                });
                            }
                            app.is_running = false;
                            app.session = Some(session);
                            return;
                        }
                        None => break,
                    }
                }
                maybe_term = term_events.next() => {
                    if let Some(Ok(CEvent::Key(key))) = maybe_term
                        && key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                        {
                            cancelled = true;
                            break;
                        }
                }
                _ = tick.tick() => {
                    // Smooth streaming: drain buffer gradually
                    let has_buffer = app.tabs.get(app.active_tab)
                        .map(|t| !t.stream_buffer.is_empty())
                        .unwrap_or(false);
                    if has_buffer {
                        if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                            drain_stream_buffer(tab);
                        }
                        redraw(app, terminal);
                    }
                }
            }
        }

        // Flush remaining buffer
        if let Some(tab) = app.tabs.get_mut(app.active_tab) {
            tab.streaming_text.push_str(&tab.stream_buffer);
            tab.stream_buffer.clear();
        }

        if cancelled {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.streaming_text.clear();
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "Cancelled.".into(),
                });
            }
            app.is_running = false;
            app.session = Some(session);
            return;
        }

        if !assistant_text.is_empty() {
            let asst_msg = Message::assistant(std::mem::take(&mut assistant_text));
            session
                .persist_message(&app.store, &app.project, &asst_msg)
                .await;
            session.add_message(asst_msg);
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                let text = std::mem::take(&mut tab.streaming_text);
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::Assistant,
                    content: text,
                });
            }
        }

        if !pending_tool_calls.is_empty() {
            for tc in &pending_tool_calls {
                let tc_msg = Message::tool_call(tc.clone());
                session
                    .persist_message(&app.store, &app.project, &tc_msg)
                    .await;
                session.add_message(tc_msg);
                let summary = tool_call_summary(&tc.name, &tc.args_json);
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    tab.chat_lines.push(ChatLine {
                        role: crate::session::message::Role::ToolCall,
                        content: summary,
                    });
                }

                redraw(app, terminal);

                let tr = if let Some(tool) = app.tools.get(&tc.name) {
                    let args: serde_json::Value =
                        serde_json::from_str(&tc.args_json).unwrap_or_default();
                    match tool.invoke(args).await {
                        Ok(r) => crate::session::message::ToolResult {
                            id: tc.id.clone(),
                            output: r.output,
                            is_error: r.is_error,
                        },
                        Err(e) => crate::session::message::ToolResult {
                            id: tc.id.clone(),
                            output: e.to_string(),
                            is_error: true,
                        },
                    }
                } else {
                    crate::session::message::ToolResult {
                        id: tc.id.clone(),
                        output: format!("unknown tool: {}", tc.name),
                        is_error: true,
                    }
                };

                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    let output = if tr.output.len() > 2000 {
                        format!("{}...", &tr.output[..2000])
                    } else {
                        tr.output.clone()
                    };
                    tab.chat_lines.push(ChatLine {
                        role: crate::session::message::Role::ToolResult,
                        content: output,
                    });
                }

                redraw(app, terminal);
                let tr_msg = Message::tool_result(tr);
                session
                    .persist_message(&app.store, &app.project, &tr_msg)
                    .await;
                session.add_message(tr_msg);
            }

            if got_tool_use_stop {
                continue;
            }
        }

        break;
    }

    app.is_running = false;
    session.persist_state(&app.store, &app.project).await;
    app.session = Some(session);
}

fn redraw(app: &mut App, terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) {
    let sz = terminal.size().unwrap_or_default();
    let sz_rect = Rect::new(0, 0, sz.width, sz.height);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(4),
            Constraint::Length(1),
        ])
        .split(sz_rect);
    app.chat_area_height = chunks[1].height;
    app.compute_display_lines(sz.width);
    let _ = terminal.draw(|f| app.render(f));
}

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self as ct_event, Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::*;
use tokio::sync::broadcast;

use crate::commands::dispatcher::ModelChoice;
use crate::config::schema::Config;
use crate::plugin::manager::PluginManager;
use crate::providers;
use crate::providers::traits::Provider;
use crate::session::agent_loop::Session;
use crate::store::session_store::SessionStore;
use crate::tools;
use crate::tools::traits::ToolRegistry;
use crate::tui::components::{
    chat_view, command_completion, input_box, modal_picker, status_bar, toast,
};
use crate::tui::layout;
use crate::tui::message_handler;
use crate::tui::onboarding;
use crate::tui::picker::{PickerItem, PickerMode, PickerState};
use crate::tui::rendering::display::DisplayLine;
use crate::tui::selection::Selection;
use crate::tui::tabs::{ChatLine, Tab};
use crate::tui::theme::{self, Theme};

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
    pub events_tx: broadcast::Sender<crate::session::agent_loop::SessionEvent>,
    pub is_running: bool,
    pub picker: Option<PickerState>,
    pub saved_theme: Option<Theme>,
    pub model_choices: Vec<ModelChoice>,
    pub onboarding: Option<onboarding::OnboardingState>,
    pub pending_session_resume: Option<String>,
    pub plugin_manager: PluginManager,
    pub command_items: Vec<PickerItem>,
    pub display_lines: Vec<DisplayLine>,
    pub chat_area_height: u16,
    pub chat_area: Rect,
    pub frame_tick: u64,
    pub selection: Option<Selection>,
    pub toast: Option<toast::Toast>,
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
            plugin_manager: PluginManager::new(),
            command_items,
            display_lines: Vec::new(),
            chat_area_height: 0,
            chat_area: Rect::default(),
            frame_tick: 0,
            selection: None,
            toast: None,
        }
    }

    pub fn current_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active_tab)
    }

    pub fn current_tab_mut(&mut self) -> Option<&mut Tab> {
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

    pub fn show_toast(&mut self, message: impl Into<String>) {
        self.toast = Some(toast::Toast::new(message.into(), Duration::from_secs(3)));
    }

    pub fn recompute_display_lines(&mut self, width: u16) {
        self.display_lines = chat_view::compute_display_lines(
            self.current_tab(),
            &self.theme,
            self.is_running,
            self.frame_tick,
            width,
        );
    }

    // ─── Key handling ────────────────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
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
            if let Some(t) = self.ctrl_c_time
                && t.elapsed() > Duration::from_secs(2)
            {
                self.ctrl_c_count = 0;
            }
            self.ctrl_c_count += 1;
            self.ctrl_c_time = Some(std::time::Instant::now());
            if self.ctrl_c_count == 1 {
                self.show_toast("Press Ctrl+C again to quit");
            } else if self.ctrl_c_count >= 2 {
                self.should_quit = true;
            }
            return false;
        }

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
                    let action = modal_picker::handle_key(self.picker.as_mut().unwrap(), key);
                    self.apply_picker_action(action);
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
            let action = command_completion::handle_key(self.picker.as_mut().unwrap(), key);
            match action {
                command_completion::CompletionAction::None => {}
                command_completion::CompletionAction::Dismiss => {
                    self.picker = None;
                    return true;
                }
                command_completion::CompletionAction::Complete(cmd) => {
                    if let Some(tab) = self.current_tab_mut() {
                        tab.input.set_single_line(&cmd);
                    }
                    self.picker = None;
                    return true;
                }
                command_completion::CompletionAction::Accept(cmd) => {
                    if let Some(tab) = self.current_tab_mut() {
                        tab.input.set_single_line(&cmd);
                    }
                    self.picker = None;
                    return false;
                }
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

    // ─── Rendering ───────────────────────────────────────────────────────

    fn input_line_count(&self) -> u16 {
        self.current_tab()
            .map(|t| t.input.lines.len() as u16)
            .unwrap_or(1)
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        frame.render_widget(
            Block::default().style(Style::default().bg(self.theme.background)),
            area,
        );

        let chunks = layout::main_layout(area, self.input_line_count());

        let provider_info = crate::config::loader::active_provider(&self.config)
            .map(|(name, p)| format!("{name}/{}", p.model))
            .unwrap_or_else(|| "no provider".into());

        chat_view::render_chat(
            frame,
            chunks[0],
            &self.display_lines,
            self.effective_scroll(),
            &self.theme,
        );

        if let Some(ref sel) = self.selection {
            crate::tui::selection::render_selection_overlay(
                frame,
                chunks[0],
                sel,
                &self.display_lines,
                self.effective_scroll(),
                &self.theme,
            );
        }

        if let Some(tab) = self.current_tab() {
            input_box::render_input(frame, chunks[1], &tab.input, &self.theme);
        } else {
            let empty = crate::tui::input::InputState::empty();
            input_box::render_input(frame, chunks[1], &empty, &self.theme);
        }

        let (session_tokens, session_cost, session_context) = if let Some(ref s) = self.session {
            let tokens = status_bar::SessionTokens {
                input: s.token_input + s.cache_read_tokens + s.cache_creation_tokens,
                output: s.token_output,
                cache_read: s.cache_read_tokens,
            };
            let cost = crate::providers::model_info::cost_for_model(
                &s.model_name,
                s.token_input,
                s.token_output,
                s.cache_read_tokens,
                s.cache_creation_tokens,
            );
            let context = if s.last_turn_input > 0 {
                crate::providers::model_info::context_window_for_model(&s.model_name).map(|cap| {
                    status_bar::ContextUsage {
                        used: s.last_turn_input,
                        capacity: cap,
                    }
                })
            } else {
                None
            };
            (Some(tokens), cost, context)
        } else {
            (None, None, None)
        };
        let status_state = status_bar::StatusState {
            tokens: session_tokens,
            cost: session_cost,
            context: session_context,
            is_running: self.is_running,
            provider_info: &provider_info,
            frame_tick: self.frame_tick,
        };

        status_bar::render_status(frame, chunks[2], &status_state, &self.theme);

        if let Some(ref picker) = self.picker {
            match picker.mode {
                PickerMode::CommandComplete => {
                    command_completion::render_command_completion(
                        frame,
                        chunks[1],
                        picker,
                        &self.theme,
                    );
                }
                PickerMode::Theme | PickerMode::Model | PickerMode::Session => {
                    modal_picker::render_modal_picker(frame, picker, &self.theme);
                }
            }
        }

        if let Some(ref t) = self.toast
            && !t.is_expired()
        {
            toast::render_toast(frame, chunks[1], t, &self.theme);
        }

        if let Some(ref ob) = self.onboarding {
            ob.render(frame, &self.theme);
        }
    }

    // ─── Picker / theme / model / onboarding helpers ─────────────────────

    fn apply_picker_action(&mut self, action: modal_picker::PickerAction) {
        match action {
            modal_picker::PickerAction::None => {}
            modal_picker::PickerAction::Dismiss => {
                self.restore_theme();
                self.picker = None;
            }
            modal_picker::PickerAction::Select(selected) => {
                let mode = self.picker.as_ref().map(|p| p.mode.clone());
                match mode {
                    Some(PickerMode::Theme) => {
                        if let Some(t) = theme::get_by_name(&selected.id) {
                            self.theme = t;
                        }
                        self.saved_theme = None;
                        let config_path = crate::config::paths::user_config_file();
                        let _ = crate::config::writer::save_theme(&config_path, &selected.id);
                        self.show_toast(format!("Theme: {}", selected.label));
                    }
                    Some(PickerMode::Model) => {
                        self.apply_model_selection(&selected.id);
                    }
                    Some(PickerMode::Session) => {
                        self.pending_session_resume = Some(selected.id.clone());
                    }
                    _ => {}
                }
                self.picker = None;
            }
            modal_picker::PickerAction::PreviewTheme => {
                self.preview_selected_theme();
            }
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

    pub fn apply_model_selection(&mut self, id: &str) {
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
            Some(tab) => tab.input.buffer_text(),
            None => {
                self.picker = None;
                return;
            }
        };

        self.picker = command_completion::update_completion(&buffer, &self.command_items);
    }

    pub fn handle_paste(&mut self, text: &str) {
        if let Some(ref mut ob) = self.onboarding {
            ob.handle_paste(text);
            return;
        }
        if let Some(tab) = self.current_tab_mut() {
            tab.input.insert_paste(text);
        }
        self.update_command_completion();
    }
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

    let plugin_dirs = crate::plugin::discover_plugin_dirs(
        Some(&app.project),
        &crate::config::paths::user_home(),
        &app.config.plugins.dirs,
    );
    if !plugin_dirs.is_empty() {
        app.plugin_manager
            .load_and_start(plugin_dirs, &app.project, &mut app.tools)
            .await;
    }

    let history_file = crate::config::paths::history_file();
    let rx = app.events_tx.subscribe();
    app.tabs.push(Tab::new("default".into(), rx, history_file));

    let result = run_loop(&mut app, &mut terminal).await;

    app.plugin_manager.shutdown_all().await;

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
        let input_lines = app
            .current_tab()
            .map(|t| t.input.lines.len() as u16)
            .unwrap_or(1);
        let chunks = layout::main_layout(area, input_lines);
        app.chat_area_height = chunks[0].height;
        app.chat_area = chunks[0];
        app.frame_tick = app.frame_tick.wrapping_add(1);
        if app.toast.as_ref().is_some_and(|t| t.is_expired()) {
            app.toast = None;
        }
        app.recompute_display_lines(size.width);

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
                            .map(|t| t.input.buffer_text())
                            .unwrap_or_default();

                        if !input_text.trim().is_empty()
                            && crate::commands::dispatcher::is_command(input_text.trim())
                        {
                            if let Some(tab) = app.current_tab_mut() {
                                tab.input.submit();
                            }
                            message_handler::handle_command(app, input_text.trim());
                        } else if let Some(text) = app.submit_message() {
                            message_handler::send_message(app, text, terminal).await;
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
                        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                            let r = mouse.row;
                            let c = mouse.column;
                            app.selection = Some(Selection {
                                start_row: r,
                                start_col: c,
                                end_row: r,
                                end_col: c,
                                active: true,
                            });
                        }
                        MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                            if let Some(ref mut sel) = app.selection {
                                sel.end_row = mouse.row;
                                sel.end_col = mouse.column;
                            }
                        }
                        MouseEventKind::Up(crossterm::event::MouseButton::Left) => {
                            if let Some(ref mut sel) = app.selection {
                                sel.active = false;
                                sel.end_row = mouse.row;
                                sel.end_col = mouse.column;
                            }
                            let should_copy = app.selection.as_ref().is_some_and(|s| s.is_set());
                            if should_copy {
                                let scroll = app.effective_scroll();
                                let area = app.chat_area;
                                let sel = app.selection.as_ref().unwrap();
                                let text = crate::tui::selection::extract_selected_text(
                                    &app.display_lines,
                                    scroll,
                                    area,
                                    sel,
                                );
                                if !text.is_empty() {
                                    crate::tui::selection::copy_to_clipboard_osc52(&text);
                                }
                            } else {
                                app.selection = None;
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
            message_handler::resume_session(app, &session_id).await;
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

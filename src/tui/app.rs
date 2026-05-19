use std::sync::Arc;
use std::time::Duration;

use ratatui::prelude::*;
use ratatui::widgets::*;
use tokio::sync::broadcast;

use crate::commands::dispatcher::ModelChoice;
use crate::config::schema::Config;
use crate::plugin::manager::PluginManager;
use crate::plugin::plugin_runtime::PluginRuntime;
use crate::providers;
use crate::providers::traits::Provider;
use crate::session::agent_loop::Session;
use crate::session::orchestration::SessionPool;
use crate::store::session_store::SessionStore;
use crate::tools;
use crate::tools::traits::ToolRegistry;
use crate::tui::components::{
    chat_view, command_completion, input_box, modal_picker, sidebar, status_bar, toast,
};
use crate::tui::file_viewer;
use crate::tui::layout;
use crate::tui::models_page::{self, ModelsPageState};
use crate::tui::onboarding;
use crate::tui::picker::{PickerItem, PickerMode, PickerState};
use crate::tui::rendering::display::DisplayLine;
use crate::tui::selection::Selection;
use crate::tui::tabs::{ChatItem, ChatLine, Tab};
use crate::tui::theme::{self, Theme};
use crate::tui::ui::tool_form;

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
    pub tools: Arc<parking_lot::RwLock<ToolRegistry>>,
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
    pub pending_skill_message: Option<String>,
    pub plugin_manager: PluginManager,
    pub extra_plugin_dirs: Vec<std::path::PathBuf>,
    pub plugin_runtime: Option<Arc<parking_lot::Mutex<PluginRuntime>>>,
    pub command_items: Vec<PickerItem>,
    pub display_lines: Vec<DisplayLine>,
    pub chat_area_height: u16,
    pub chat_area: Rect,
    pub input_area: Rect,
    pub frame_tick: u64,
    pub selection: Option<Selection>,
    pub toast: Option<toast::Toast>,
    pub is_reloading: bool,
    pub reload_task: Option<tokio::task::JoinHandle<ReloadOutput>>,
    pub conductor_mode: bool,
    pub panel_focused: bool,
    pub session_pool: Arc<SessionPool>,
    pub orch_ctx: Arc<crate::tools::orchestration::OrchestrationContext>,
    pub sidebar_state: sidebar::SidebarState,
    pub sidebar_area: Option<Rect>,
    pub panels: std::collections::HashMap<String, crate::shared::ui_types::PanelState>,
    pub panel_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<crate::plugin::host_handler::PanelUpdate>>,
    pub agent_receivers: Vec<AgentReceiver>,
    pub pending_model_selection: Option<usize>,
    pub models_page: Option<ModelsPageState>,
    pub tool_form: Option<tool_form::ToolFormState>,
    pub interactive_response_tx: Option<tokio::sync::oneshot::Sender<Option<String>>>,
    pub file_viewer: file_viewer::FileViewerState,
    pub tab_bar_area: Option<Rect>,
    pub hovered_line: Option<usize>,
}

pub struct AgentReceiver {
    pub tab_index: usize,
    pub session_id: Option<String>,
    pub rx: tokio::sync::mpsc::UnboundedReceiver<crate::session::conversation::ConvEvent>,
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Debug)]
pub struct ReloadOutput {
    pub plugin_result: Option<crate::plugin::plugin_runtime::ReloadResult>,
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

        let max_agents = config.conductor.max_agents;
        let worktree_mgr = crate::worktree::WorktreeManager::new(project.clone()).ok();
        let pool = Arc::new(SessionPool::new(max_agents, worktree_mgr));
        let parent_provider = crate::config::loader::active_provider(&config)
            .map(|(name, _)| name.to_string())
            .unwrap_or_default();

        let skills = crate::session::skills::discover_layered(
            Some(&project),
            &crate::config::paths::user_home(),
            &config.skills.dirs,
        );
        let mut tool_registry = tool_registry;
        crate::tools::skill_tool::register_skill_tools(&skills, &mut tool_registry);

        let orch_ctx = Arc::new(crate::tools::orchestration::OrchestrationContext {
            pool: Arc::clone(&pool),
            config: Arc::new(parking_lot::RwLock::new(config.clone())),
            store: Arc::new(store.clone()),
            project: project.clone(),
            parent_provider: parking_lot::RwLock::new(parent_provider),
            parent_tools: parking_lot::RwLock::new(tool_registry.clone()),
        });
        crate::tools::orchestration::register_orchestration_tools(
            &mut tool_registry,
            Arc::clone(&orch_ctx),
        );
        let command_list = crate::commands::dispatcher::list_commands(&skills);
        let command_items: Vec<PickerItem> = command_list
            .iter()
            .map(|cmd| PickerItem {
                id: cmd.name.clone(),
                label: cmd.name.clone(),
                description: cmd.summary.clone(),
                source_tag: command_source_tag(&cmd.source),
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
            tools: Arc::new(parking_lot::RwLock::new(tool_registry)),
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
            pending_skill_message: None,
            plugin_manager: PluginManager::new(),
            extra_plugin_dirs: Vec::new(),
            plugin_runtime: None,
            command_items,
            display_lines: Vec::new(),
            chat_area_height: 0,
            chat_area: Rect::default(),
            input_area: Rect::default(),
            frame_tick: 0,
            selection: None,
            toast: None,
            is_reloading: false,
            reload_task: None,
            conductor_mode: true,
            panel_focused: false,
            session_pool: pool,
            orch_ctx,
            sidebar_state: sidebar::SidebarState::new(),
            sidebar_area: None,
            panels: std::collections::HashMap::new(),
            panel_rx: None,
            agent_receivers: Vec::new(),
            pending_model_selection: None,
            models_page: None,
            tool_form: None,
            interactive_response_tx: None,
            file_viewer: file_viewer::FileViewerState::new(),
            tab_bar_area: None,
            hovered_line: None,
        }
    }

    pub fn show_sidebar(&self) -> bool {
        self.conductor_mode && !self.sidebar_state.agents.is_empty()
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

    pub(crate) fn effective_scroll(&self) -> usize {
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

    pub fn apply_tool_result(&mut self, result: crate::plugin::plugin_runtime::ToolExecResult) {
        if !result.toast.is_empty() {
            self.show_toast(result.toast);
        }
        if !result.widget.is_empty()
            && let Some(tab) = self.current_tab_mut()
        {
            tab.chat_lines
                .push(ChatItem::Widget(crate::tui::tabs::WidgetKind {
                    json: result.widget,
                }));
        }
        if !result.output.is_empty() {
            if let Some(tab) = self.current_tab_mut() {
                tab.chat_lines
                    .push(ChatItem::Assistant(crate::tui::tabs::AssistantLine {
                        content: result.output.clone(),
                        turn: 0,
                    }));
            }
            self.pending_skill_message = Some(result.output);
        }
    }

    pub fn recompute_display_lines(&mut self, width: u16, full_width: u16) {
        let has_active_agent = self
            .agent_receivers
            .iter()
            .any(|a| a.tab_index == self.active_tab);
        let turn_count = self.session.as_ref().map_or(0, |s| s.turn_count);
        self.display_lines = chat_view::compute_display_lines(
            self.current_tab(),
            &self.theme,
            self.is_running || has_active_agent,
            self.frame_tick,
            (width, full_width),
            turn_count,
            &self.sidebar_state.agents,
        );
    }

    pub fn drain_panels(&mut self) {
        if let Some(rx) = &mut self.panel_rx {
            while let Ok(update) = rx.try_recv() {
                match update {
                    crate::plugin::host_handler::PanelUpdate::Set {
                        panel_id,
                        position,
                        content,
                    } => {
                        self.panels.insert(
                            panel_id.clone(),
                            crate::shared::ui_types::PanelState {
                                panel_id,
                                position,
                                content,
                                selected_index: 0,
                            },
                        );
                    }
                    crate::plugin::host_handler::PanelUpdate::Remove { panel_id } => {
                        self.panels.remove(&panel_id);
                    }
                }
            }
        }
    }

    pub fn drain_conversations(&mut self) {
        use crate::session::conversation::ConvEvent;
        use crate::tui::msg::Msg;

        let mut messages: Vec<Msg> = Vec::new();

        for (i, agent) in self.agent_receivers.iter_mut().enumerate() {
            let tab_idx = agent
                .session_id
                .as_ref()
                .and_then(|sid| self.tabs.iter().position(|t| t.id == *sid))
                .unwrap_or(agent.tab_index);
            while let Ok(event) = agent.rx.try_recv() {
                match event {
                    ConvEvent::StreamToken(t) => {
                        messages.push(Msg::ConvStreamToken { tab_idx, text: t });
                    }
                    ConvEvent::AssistantMessage(text) => {
                        messages.push(Msg::ConvAssistantMessage { tab_idx, text });
                    }
                    ConvEvent::ToolCall(summary) => {
                        messages.push(Msg::ConvToolCall { tab_idx, summary });
                    }
                    ConvEvent::ToolResult { output, .. } => {
                        messages.push(Msg::ConvToolResult { tab_idx, output });
                    }
                    ConvEvent::InteractiveUi {
                        fields,
                        response_tx,
                        tool_name,
                        ..
                    } => {
                        messages.push(Msg::ConvInteractiveUi {
                            tool_name,
                            fields,
                            response_tx,
                        });
                        break;
                    }
                    ConvEvent::ContextLoaded(names) => {
                        messages.push(Msg::ConvContextLoaded { tab_idx, names });
                    }
                    ConvEvent::ContextCompacted { removed, remaining } => {
                        messages.push(Msg::ConvContextCompacted {
                            tab_idx,
                            removed,
                            remaining,
                        });
                    }
                    ConvEvent::Error(e) => {
                        messages.push(Msg::ConvError {
                            tab_idx,
                            message: e,
                        });
                    }
                    ConvEvent::Cancelled(session) => {
                        if tab_idx == 0 {
                            self.session = Some(session);
                        }
                        messages.push(Msg::ConvCancelled {
                            tab_idx,
                            agent_idx: i,
                        });
                        break;
                    }
                    ConvEvent::Done(session) => {
                        if tab_idx == 0 {
                            self.session = Some(session);
                        }
                        messages.push(Msg::ConvDone {
                            tab_idx,
                            agent_idx: i,
                        });
                        break;
                    }
                }
            }
        }

        for msg in messages {
            crate::tui::update::update(self, msg);
        }
    }

    // ─── Key handling ────────────────────────────────────────────────────

    pub async fn invoke_tool_command(&mut self, tool_name: &str, args_json: &str) {
        if tool_name == "conductor" {
            crate::tui::commands::handle_command(self, "/conductor").await;
            return;
        }
        if let Some(rt) = self.plugin_runtime.as_ref().map(Arc::clone) {
            let toggle_info = rt.lock().prepare_toggle(tool_name);
            match toggle_info {
                Ok((is_exit, resolved, exec_kind, project_dir)) => {
                    let result = if is_exit {
                        crate::plugin::plugin_runtime::exit_tool_async(
                            Some(exec_kind),
                            &resolved,
                            &project_dir,
                        )
                        .await
                    } else {
                        crate::plugin::plugin_runtime::invoke_tool_async(
                            exec_kind,
                            &resolved,
                            args_json,
                            &project_dir,
                        )
                        .await
                    };
                    match result {
                        Ok(result) => self.apply_tool_result(result),
                        Err(e) => self.show_toast(format!("Plugin error: {e}")),
                    }
                }
                Err(e) => self.show_toast(format!("Plugin error: {e}")),
            }
        }
    }

    // handle_key and handle_paste moved to event_handler.rs

    // ─── Rendering ───────────────────────────────────────────────────────

    pub(crate) fn input_text_rect(&self) -> Rect {
        let pad_x = 2u16;
        let border = 1u16;
        let inner_pad = 1u16;
        let prompt_len = 2u16; // "> "
        let x_offset = pad_x + border + inner_pad + prompt_len;
        Rect {
            x: self.input_area.x + x_offset,
            y: self.input_area.y + border,
            width: self
                .input_area
                .width
                .saturating_sub(x_offset + pad_x + border + inner_pad),
            height: self.input_area.height.saturating_sub(border * 2 + 1),
        }
    }

    fn input_line_count(&self, area_width: u16) -> u16 {
        let text_width = layout::input_text_width(area_width);
        self.current_tab()
            .map(|t| t.input.wrapped_line_count(text_width) as u16)
            .unwrap_or(1)
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        frame.render_widget(ratatui::widgets::Clear, area);
        frame.render_widget(
            Block::default().style(Style::default().bg(self.theme.background)),
            area,
        );

        let viewing_file = self.file_viewer.is_viewing_file();
        let has_file_tabs = self.file_viewer.has_tabs();

        let chunks = if let Some(ref form) = self.tool_form {
            let fh = tool_form::form_height(form);
            layout::main_layout_with_form(area, fh)
        } else if viewing_file {
            layout::file_viewer_layout(area)
        } else {
            layout::main_layout(area, self.input_line_count(area.width))
        };

        let provider_info = crate::config::loader::active_provider(&self.config)
            .map(|(name, p)| format!("{name}/{}", p.model))
            .unwrap_or_else(|| "no provider".into());

        // Split main area for tab bar when file tabs exist
        let (tab_bar_rect, content_rect) = if has_file_tabs {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(file_viewer::TAB_BAR_HEIGHT),
                    Constraint::Min(1),
                ])
                .split(chunks[0]);
            (Some(split[0]), split[1])
        } else {
            (None, chunks[0])
        };

        // Tab bar
        if let Some(tb) = tab_bar_rect {
            file_viewer::render_tab_bar(frame, tb, &self.file_viewer, &self.theme);
        }

        let panel_rect = if !viewing_file {
            agent_panel_rect(
                self.conductor_mode,
                self.sidebar_state.agents.len(),
                padded_chat_area(content_rect),
            )
        } else {
            None
        };

        if viewing_file {
            frame.render_widget(
                Paragraph::new("").style(Style::default().bg(self.theme.background)),
                content_rect,
            );
            if let Some(ft) = self.file_viewer.active_tab() {
                file_viewer::render_file_content(frame, content_rect, ft, &self.theme);
            }

            if let Some(ft) = self.file_viewer.active_tab() {
                file_viewer::render_file_status(frame, chunks[1], ft, &self.theme);
            }
        } else {
            let chat_area = padded_chat_area(content_rect);

            frame.render_widget(
                Paragraph::new("").style(Style::default().bg(self.theme.background)),
                content_rect,
            );

            chat_view::render_chat_with_panel(
                frame,
                chat_area,
                &self.display_lines,
                self.effective_scroll(),
                &self.theme,
                panel_rect,
                self.hovered_line,
            );

            if let Some(ref sel) = self.selection {
                crate::tui::selection::render_selection_overlay(
                    frame,
                    chat_area,
                    sel,
                    &self.display_lines,
                    self.effective_scroll(),
                    &self.theme,
                );
            }

            if let Some(pa) = panel_rect {
                let active_sid = if self.active_tab > 0 {
                    self.tabs.get(self.active_tab).map(|t| t.id.as_str())
                } else {
                    None
                };
                sidebar::render_agent_panel(
                    frame,
                    pa,
                    &self.sidebar_state,
                    &self.theme,
                    self.panel_focused,
                    active_sid,
                );
            }

            if let Some(ref form) = self.tool_form {
                tool_form::render_tool_form(frame, chunks[1], form, &self.theme);
            } else if let Some(tab) = self.current_tab() {
                input_box::render_input(frame, chunks[1], &tab.input, &self.theme);
            } else {
                let empty = crate::tui::input::InputState::empty();
                input_box::render_input(frame, chunks[1], &empty, &self.theme);
            }
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
            conductor_mode: self.conductor_mode,
            agent_count: self.sidebar_state.agents.len(),
            provider_info: &provider_info,
            frame_tick: self.frame_tick,
        };

        status_bar::render_status(frame, chunks[2], &status_state, &self.theme);

        if let Some(ref picker) = self.picker {
            match picker.mode {
                PickerMode::CommandComplete => {
                    let reserve = panel_rect.map_or(0, |p| p.width + 4);
                    command_completion::render_command_completion(
                        frame,
                        chunks[1],
                        picker,
                        &self.theme,
                        reserve,
                    );
                }
                PickerMode::Theme | PickerMode::Model | PickerMode::Session => {
                    modal_picker::render_modal_picker(frame, picker, &self.theme);
                }
            }
        }

        if self.is_reloading {
            use crate::tui::rendering::helpers::{spinner_color, spinner_frame};
            let frame_idx = (self.frame_tick / 4) as usize;
            let spin = spinner_frame(frame_idx);
            let color = spinner_color(frame_idx, &self.theme);
            let msg = "reloading";
            let text = format!("  {spin} {msg}  ");
            let width = text.chars().count().min(chunks[1].width as usize) as u16;
            let x = chunks[1].x + (chunks[1].width.saturating_sub(width)) / 2;
            let y = chunks[1].y.saturating_sub(1);
            let bg = self.theme.status_bar_bg();
            let line = ratatui::text::Line::from(vec![
                ratatui::text::Span::styled("  ", Style::default().bg(bg)),
                ratatui::text::Span::styled(
                    spin,
                    Style::default()
                        .fg(color)
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
                ratatui::text::Span::styled(
                    format!(" {msg}  "),
                    Style::default()
                        .fg(self.theme.status_bar_fg())
                        .bg(bg)
                        .add_modifier(Modifier::BOLD),
                ),
            ]);
            let area = Rect {
                x,
                y,
                width,
                height: 1,
            };
            frame.render_widget(ratatui::widgets::Paragraph::new(line), area);
        } else if let Some(ref t) = self.toast
            && !t.is_expired()
        {
            toast::render_toast(frame, chunks[1], t, &self.theme);
        }

        if let Some(ref page) = self.models_page {
            models_page::render_models_page(frame, page, &self.theme);
        }

        if let Some(ref ob) = self.onboarding {
            ob.render(frame, &self.theme);
        }
    }

    // ─── Picker / theme / model / onboarding helpers ─────────────────────

    pub(crate) fn apply_picker_action(&mut self, action: modal_picker::PickerAction) {
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

    pub(crate) fn apply_models_page_action(&mut self, action: models_page::ModelsPageAction) {
        match action {
            models_page::ModelsPageAction::None => {}
            models_page::ModelsPageAction::Close => {
                self.models_page = None;
            }
            models_page::ModelsPageAction::AddProvider => {
                self.onboarding = Some(onboarding::OnboardingState::new());
            }
            models_page::ModelsPageAction::DeleteProvider { name } => {
                let config_path = crate::config::paths::user_config_file();
                if let Err(e) = crate::config::writer::delete_provider(&config_path, &name) {
                    self.show_toast(format!("Failed to delete: {e}"));
                } else {
                    self.config.providers.remove(&name);
                    if let Some(page) = &mut self.models_page {
                        page.refresh(&self.config);
                    }
                    let was_active = !self.config.providers.values().any(|p| p.active);
                    if was_active {
                        if let Some((pname, pprofile)) =
                            crate::config::loader::active_provider(&self.config)
                        {
                            if let Ok(p) = providers::create_provider(pname, pprofile) {
                                self.provider = Some(Arc::from(p));
                            }
                        } else {
                            self.provider = None;
                        }
                    }
                    self.show_toast(format!("Deleted provider '{name}'"));
                }
            }
            models_page::ModelsPageAction::SwitchTo { name, model } => {
                if let Some(cfg_profile) = self.config.providers.get(&name) {
                    let mut profile = cfg_profile.clone();
                    profile.model.clone_from(&model);
                    match providers::create_provider(&name, &profile) {
                        Ok(p) => {
                            self.provider = Some(Arc::from(p));
                            for (_, pp) in self.config.providers.iter_mut() {
                                pp.active = false;
                            }
                            if let Some(cp) = self.config.providers.get_mut(&name) {
                                cp.active = true;
                            }
                            if let Some(session) = &mut self.session {
                                session.provider_name.clone_from(&name);
                                session.model_name.clone_from(&model);
                            }
                            let config_path = crate::config::paths::user_config_file();
                            let _ = crate::config::writer::save_active_model(
                                &config_path,
                                &name,
                                &model,
                            );
                            if let Some(page) = &mut self.models_page {
                                page.refresh(&self.config);
                            }
                            self.show_toast(format!("Switched to {name}/{model}"));
                        }
                        Err(crate::providers::traits::ProviderError::MissingCredential) => {
                            let kind = profile.kind;
                            if let Some(preset_idx) =
                                onboarding::PRESETS.iter().position(|p| p.kind == kind)
                            {
                                self.onboarding =
                                    Some(onboarding::OnboardingState::new_for_api_key(preset_idx));
                            } else {
                                self.show_toast("API key required. Use 'e' to edit.");
                            }
                        }
                        Err(e) => {
                            self.show_toast(format!("Error: {e}"));
                        }
                    }
                }
            }
            models_page::ModelsPageAction::EditApiKey { name: _, kind } => {
                if let Some(preset_idx) = onboarding::PRESETS.iter().position(|p| p.kind == kind) {
                    self.onboarding =
                        Some(onboarding::OnboardingState::new_for_api_key(preset_idx));
                } else {
                    self.show_toast("No preset for this provider type.");
                }
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

    pub(crate) fn restore_theme(&mut self) {
        if let Some(saved) = self.saved_theme.take() {
            self.theme = saved;
        }
    }

    pub fn apply_model_selection(&mut self, id: &str) {
        if let Ok(idx) = id.parse::<usize>()
            && let Some(choice) = self.model_choices.get(idx)
        {
            let mut profile = choice.profile.clone();
            if profile.resolve_credential().is_none()
                && let Some(cfg_profile) = self.config.providers.get(&choice.provider_name)
            {
                profile.auth = cfg_profile.auth.clone();
            }
            match providers::create_provider(&choice.provider_name, &profile) {
                Ok(p) => {
                    self.provider = Some(Arc::from(p));

                    let provider_name = choice.provider_name.clone();
                    let new_model = profile.model.clone();
                    let display = choice.display.clone();

                    for (_, p) in self.config.providers.iter_mut() {
                        p.active = false;
                    }
                    if let Some(cfg_profile) = self.config.providers.get_mut(&provider_name) {
                        cfg_profile.model.clone_from(&new_model);
                        cfg_profile.active = true;
                    }

                    if let Some(session) = &mut self.session {
                        session.provider_name.clone_from(&provider_name);
                        session.model_name.clone_from(&new_model);
                    }

                    let config_path = crate::config::paths::user_config_file();
                    if let Err(e) = crate::config::writer::save_active_model(
                        &config_path,
                        &provider_name,
                        &new_model,
                    ) {
                        tracing::warn!("failed to persist model choice: {e}");
                    }

                    self.show_toast(format!("Model: {display}"));
                }
                Err(crate::providers::traits::ProviderError::MissingCredential) => {
                    let kind = profile.kind;
                    if let Some(preset_idx) =
                        onboarding::PRESETS.iter().position(|p| p.kind == kind)
                    {
                        self.pending_model_selection = Some(idx);
                        self.onboarding =
                            Some(onboarding::OnboardingState::new_for_api_key(preset_idx));
                    } else {
                        if let Some(tab) = self.current_tab_mut() {
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::System,
                                content:
                                    "API key required. Use /connect to configure this provider."
                                        .into(),
                            }));
                        }
                    }
                }
                Err(e) => {
                    if let Some(tab) = self.current_tab_mut() {
                        tab.chat_lines.push(ChatItem::Line(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Error: {e}"),
                        }));
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn complete_onboarding(
        &mut self,
        name: String,
        kind: crate::config::schema::ProviderKind,
        model: String,
        api_key: Option<String>,
        env_hint: String,
        base_url: Option<String>,
        subagent_model: Option<String>,
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
            base_url,
            ..Default::default()
        };

        let config_path = crate::config::paths::user_config_file();
        if let Err(e) = crate::config::writer::save_provider(&config_path, &name, &profile) {
            if let Some(tab) = self.current_tab_mut() {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Failed to save config: {e}"),
                }));
            }
            self.onboarding = None;
            return;
        }

        if let Some(agent_model) = &subagent_model {
            self.config.conductor.agent_provider = Some(name.clone());
            self.config.conductor.agent_model = Some(agent_model.clone());
            let _ =
                crate::config::writer::save_conductor_config(&config_path, &self.config.conductor);
        }

        if let Ok(cfg) = crate::config::loader::load(None) {
            self.config = cfg;
        }

        *self.orch_ctx.config.write() = self.config.clone();
        let parent_provider = crate::config::loader::active_provider(&self.config)
            .map(|(n, _)| n.to_string())
            .unwrap_or_default();
        *self.orch_ctx.parent_provider.write() = parent_provider;
        *self.orch_ctx.parent_tools.write() = self.tools.read().clone();

        if let Some((pname, pprofile)) = crate::config::loader::active_provider(&self.config) {
            match providers::create_provider(pname, pprofile) {
                Ok(p) => self.provider = Some(Arc::from(p)),
                Err(e) => {
                    tracing::warn!("failed to create provider after onboarding: {e}");
                }
            }
        }

        self.onboarding = None;

        if let Some(page) = &mut self.models_page {
            page.refresh(&self.config);
        }

        if let Some(tab) = self.current_tab_mut() {
            tab.chat_lines.push(ChatItem::Line(ChatLine {
                role: crate::session::message::Role::System,
                content: format!("Connected to {name} ({model})."),
            }));
        }
    }

    pub(crate) fn update_command_completion(&mut self) {
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
}

// ─── Main loop ───────────────────────────────────────────────────────────────

pub fn command_source_tag(source: &crate::commands::dispatcher::CommandSource) -> Option<String> {
    use crate::commands::dispatcher::CommandSource;
    match source {
        CommandSource::Builtin => None,
        CommandSource::Skill => Some("skill".into()),
        CommandSource::Plugin => Some("plugin".into()),
        CommandSource::NativePlugin => Some("plugin".into()),
    }
}

pub fn resolve_extra_plugin_dirs(
    extras: &[std::path::PathBuf],
    project: &std::path::Path,
    out: &mut Vec<std::path::PathBuf>,
) {
    for extra in extras {
        let abs = if extra.is_absolute() {
            extra.clone()
        } else {
            project.join(extra)
        };
        if abs.join("Cargo.toml").exists() {
            out.push(abs);
        } else if abs.is_dir()
            && let Ok(entries) = std::fs::read_dir(&abs)
        {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() && p.join("Cargo.toml").exists() {
                    out.push(p);
                }
            }
        }
    }
}

use crate::tui::layout::padded_chat_area;

const AGENT_PANEL_HEIGHT: u16 = 10;

pub(crate) fn agent_panel_rect(
    conductor_mode: bool,
    _agent_count: usize,
    chat: Rect,
) -> Option<Rect> {
    if !conductor_mode {
        return None;
    }
    if chat.width < 20 || chat.height < 8 {
        return None;
    }
    let h = AGENT_PANEL_HEIGHT.min(chat.height / 3).max(5);
    let panel_w = 56u16.min(chat.width.saturating_sub(4));
    if panel_w == 0 || h == 0 {
        return None;
    }
    Some(Rect {
        x: chat.x + chat.width - panel_w,
        y: chat.y + chat.height.saturating_sub(h),
        width: panel_w,
        height: h,
    })
}

pub(crate) fn append_file_summary(tabs: &mut [Tab], tab_idx: usize) {
    let paths: Vec<std::path::PathBuf> = if let Some(tab) = tabs.get(tab_idx) {
        let mut seen = std::collections::HashSet::new();
        tab.chat_lines
            .iter()
            .filter_map(|item| {
                if let ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::ToolCall,
                    content,
                }) = item
                {
                    let (tool, detail) = content.split_once(" > ")?;
                    if matches!(tool, "write" | "edit") {
                        let p = std::path::PathBuf::from(detail.trim());
                        if seen.insert(p.clone()) {
                            return Some(p);
                        }
                    }
                }
                None
            })
            .collect()
    } else {
        Vec::new()
    };

    if !paths.is_empty()
        && let Some(tab) = tabs.get_mut(tab_idx)
    {
        tab.chat_lines.push(ChatItem::FileSummary(paths));
    }
}

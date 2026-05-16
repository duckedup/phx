use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self as ct_event, Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
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
use crate::tui::message_handler;
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

    pub fn recompute_display_lines(&mut self, width: u16) {
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
            width,
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

        let mut finished = Vec::new();

        for (i, agent) in self.agent_receivers.iter_mut().enumerate() {
            let tab_idx = agent
                .session_id
                .as_ref()
                .and_then(|sid| self.tabs.iter().position(|t| t.id == *sid))
                .unwrap_or(agent.tab_index);
            while let Ok(event) = agent.rx.try_recv() {
                match event {
                    ConvEvent::StreamToken(t) => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.stream_buffer.push_str(&t);
                        }
                    }
                    ConvEvent::AssistantMessage(text) => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.streaming_text.clear();
                            tab.stream_buffer.clear();
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::Assistant,
                                content: text,
                            }));
                        }
                    }
                    ConvEvent::ToolCall(summary) => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            crate::tui::rendering::helpers::drain_stream_buffer(tab);
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::ToolCall,
                                content: summary,
                            }));
                        }
                    }
                    ConvEvent::ToolResult { output, .. } => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::ToolResult,
                                content: output,
                            }));
                        }
                    }
                    ConvEvent::InteractiveUi {
                        fields,
                        response_tx,
                        tool_name,
                        ..
                    } => {
                        let config = crate::shared::ui_field_types::ToolUiConfig::new(fields);
                        self.tool_form = Some(tool_form::ToolFormState::from_ui(
                            tool_name,
                            String::new(),
                            &config,
                        ));
                        self.interactive_response_tx = Some(response_tx);
                        break;
                    }
                    ConvEvent::ContextLoaded(names) => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.chat_lines.push(ChatItem::ContextLoaded(names));
                        }
                    }
                    ConvEvent::ContextCompacted { removed, remaining } => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::System,
                                content: format!(
                                    "Context compacted: removed {removed} messages ({remaining} remaining)"
                                ),
                            }));
                        }
                    }
                    ConvEvent::Error(e) => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.streaming_text.clear();
                            tab.stream_buffer.clear();
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::System,
                                content: e,
                            }));
                        }
                    }
                    ConvEvent::Cancelled(session) => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.streaming_text.clear();
                            tab.stream_buffer.clear();
                        }
                        append_file_summary(&mut self.tabs, tab_idx);
                        self.toast = Some(toast::Toast::new(
                            "Cancelled".into(),
                            std::time::Duration::from_secs(3),
                        ));
                        if tab_idx == 0 {
                            self.session = Some(session);
                            self.is_running = false;
                        } else if let Some(sid) = &agent.session_id {
                            self.session_pool.mark_done(sid, false);
                        }
                        finished.push(i);
                        break;
                    }
                    ConvEvent::Done(session) => {
                        append_file_summary(&mut self.tabs, tab_idx);
                        if tab_idx == 0 {
                            self.session = Some(session);
                            self.is_running = false;
                        } else if let Some(sid) = &agent.session_id {
                            self.session_pool.mark_done(sid, true);
                        }
                        finished.push(i);
                        break;
                    }
                }
            }
        }

        for i in finished.into_iter().rev() {
            self.agent_receivers.remove(i);
        }
    }

    // ─── Key handling ────────────────────────────────────────────────────

    pub async fn invoke_tool_command(&mut self, tool_name: &str, args_json: &str) {
        if tool_name == "conductor" {
            message_handler::handle_command(self, "/conductor").await;
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

    pub async fn handle_key(&mut self, key: KeyEvent) -> bool {
        if let Some(ref mut form) = self.tool_form
            && key.modifiers.contains(KeyModifiers::SUPER)
        {
            match key.code {
                KeyCode::Char('c') => {
                    if let Some(text) = tool_form::handle_copy(form) {
                        crate::tui::selection::copy_to_clipboard_osc52(&text);
                    }
                    tool_form::cancel_selection(form);
                    return true;
                }
                KeyCode::Char('x') => {
                    if let Some(text) = tool_form::cut_selection(form) {
                        crate::tui::selection::copy_to_clipboard_osc52(&text);
                    }
                    return true;
                }
                KeyCode::Char('v') => {
                    if let Some(text) = crate::tui::selection::paste_from_clipboard() {
                        tool_form::handle_paste(form, &text);
                    }
                    return true;
                }
                _ => {}
            }
        }

        if let Some(ref mut form) = self.tool_form {
            let action = tool_form::handle_key(form, key);
            match action {
                tool_form::FormAction::Submit(_) => {
                    if let Some(response_tx) = self.interactive_response_tx.take() {
                        let answers = tool_form::format_answers(form);
                        let _ = response_tx.send(Some(answers));
                        self.tool_form = None;
                    } else {
                        let name = form.tool_name.clone();
                        let args = form.collect_json();
                        let args_str = args.to_string();
                        self.tool_form = None;
                        self.invoke_tool_command(&name, &args_str).await;
                    }
                }
                tool_form::FormAction::Cancel => {
                    if let Some(response_tx) = self.interactive_response_tx.take() {
                        let _ = response_tx.send(None);
                    }
                    self.tool_form = None;
                }
                tool_form::FormAction::None => {}
            }
            return true;
        }

        // Cmd+C (macOS) — copy only, never clear/quit
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::SUPER) {
            if let Some(tab) = self.current_tab_mut()
                && tab.input.textarea.is_selecting()
            {
                let selected = tab.input.selected_text();
                if !selected.is_empty() {
                    crate::tui::selection::copy_to_clipboard_osc52(&selected);
                }
                tab.input.textarea.cancel_selection();
            }
            return true;
        }

        // Cmd+X (macOS) — cut selection to clipboard
        if key.code == KeyCode::Char('x') && key.modifiers.contains(KeyModifiers::SUPER) {
            if let Some(tab) = self.current_tab_mut()
                && tab.input.textarea.is_selecting()
            {
                let selected = tab.input.selected_text();
                if !selected.is_empty() {
                    crate::tui::selection::copy_to_clipboard_osc52(&selected);
                }
                tab.input.textarea.cut();
            }
            return true;
        }

        // Cmd+V (macOS) — paste from clipboard
        if key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::SUPER) {
            if let Some(text) = crate::tui::selection::paste_from_clipboard()
                && let Some(tab) = self.current_tab_mut()
            {
                tab.input.insert_paste(&text);
            }
            return true;
        }

        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.onboarding.is_some() {
                self.should_quit = true;
                return true;
            }
            if self.models_page.is_some() {
                self.models_page = None;
                return true;
            }
            if self.picker.is_some() {
                self.restore_theme();
                self.picker = None;
                return true;
            }
            if let Some(tab) = self.current_tab_mut() {
                if tab.input.textarea.is_selecting() {
                    let selected = tab.input.selected_text();
                    if !selected.is_empty() {
                        crate::tui::selection::copy_to_clipboard_osc52(&selected);
                    }
                    tab.input.textarea.cancel_selection();
                    self.ctrl_c_count = 0;
                    self.ctrl_c_time = None;
                    return true;
                }
                if !tab.input.is_empty() {
                    tab.input.clear();
                    self.ctrl_c_count = 0;
                    self.ctrl_c_time = None;
                    return true;
                }
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
                    base_url,
                    subagent_model,
                } => {
                    self.complete_onboarding(
                        name,
                        kind,
                        model,
                        api_key,
                        env_hint,
                        base_url,
                        subagent_model,
                    );
                    if let Some(idx) = self.pending_model_selection.take() {
                        self.apply_model_selection(&idx.to_string());
                    }
                }
                onboarding::Action::Cancelled => {
                    self.onboarding = None;
                    self.pending_model_selection = None;
                }
                onboarding::Action::None => {}
            }
            return true;
        }

        if self.models_page.is_some() {
            let action = self.models_page.as_mut().unwrap().handle_key(key);
            self.apply_models_page_action(action);
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
                command_completion::CompletionAction::Handled => {
                    return true;
                }
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

        if self.panel_focused && self.conductor_mode {
            let panel_visible = self
                .sidebar_area
                .map(|r| r.height.saturating_sub(2) as usize)
                .unwrap_or(10);
            match key.code {
                KeyCode::Up => {
                    let agents = &self.sidebar_state.agents;
                    if let sidebar::SidebarSelection::Agent(ref id) = self.sidebar_state.selected
                        && let Some(pos) = agents.iter().position(|a| &a.session_id == id)
                    {
                        if pos > 0 {
                            self.sidebar_state.selected = sidebar::SidebarSelection::Agent(
                                agents[pos - 1].session_id.clone(),
                            );
                        } else {
                            self.sidebar_state.selected = sidebar::SidebarSelection::Conductor;
                        }
                    }
                    self.sidebar_state.ensure_selected_visible(panel_visible);
                    return true;
                }
                KeyCode::Down => {
                    let agents = &self.sidebar_state.agents;
                    match &self.sidebar_state.selected {
                        sidebar::SidebarSelection::Conductor if !agents.is_empty() => {
                            self.sidebar_state.selected =
                                sidebar::SidebarSelection::Agent(agents[0].session_id.clone());
                        }
                        sidebar::SidebarSelection::Agent(id)
                            if let Some(pos) = agents.iter().position(|a| &a.session_id == id)
                                && pos + 1 < agents.len() =>
                        {
                            self.sidebar_state.selected = sidebar::SidebarSelection::Agent(
                                agents[pos + 1].session_id.clone(),
                            );
                        }
                        _ => {}
                    }
                    self.sidebar_state.ensure_selected_visible(panel_visible);
                    return true;
                }
                KeyCode::Delete | KeyCode::Backspace => {
                    if let sidebar::SidebarSelection::Agent(ref id) = self.sidebar_state.selected {
                        let id = id.clone();
                        let agents = &self.sidebar_state.agents;
                        let is_finished =
                            agents.iter().find(|a| a.session_id == id).is_some_and(|a| {
                                matches!(
                                    a.status,
                                    crate::session::orchestration::ChildStatus::Done
                                        | crate::session::orchestration::ChildStatus::Error(_)
                                        | crate::session::orchestration::ChildStatus::Cancelled
                                )
                            });
                        if is_finished {
                            let was_active =
                                self.tabs.get(self.active_tab).is_some_and(|t| t.id == id);
                            self.sidebar_state.dismiss(&id);
                            self.sidebar_state.selected = sidebar::SidebarSelection::Conductor;
                            if was_active {
                                self.active_tab = 0;
                            }
                        }
                    }
                    return true;
                }
                KeyCode::Enter => {
                    match &self.sidebar_state.selected {
                        sidebar::SidebarSelection::Conductor => {
                            self.active_tab = 0;
                        }
                        sidebar::SidebarSelection::Agent(id) => {
                            if let Some(idx) = self.tabs.iter().position(|t| t.id == *id) {
                                self.active_tab = idx;
                            } else if let Some(agent) = self
                                .agent_receivers
                                .iter()
                                .find(|a| a.session_id.as_deref() == Some(id.as_str()))
                            {
                                self.active_tab = agent.tab_index;
                            }
                        }
                    }
                    self.panel_focused = false;
                    return true;
                }
                KeyCode::Left | KeyCode::Esc => {
                    self.panel_focused = false;
                    return true;
                }
                _ => {
                    self.panel_focused = false;
                }
            }
        }

        if self.conductor_mode && key.code == KeyCode::Right {
            let input_empty = self
                .current_tab()
                .map(|t| t.input.buffer_text().is_empty())
                .unwrap_or(true);
            if input_empty {
                self.panel_focused = true;
                return true;
            }
        }

        if self.file_viewer.is_viewing_file() {
            match key.code {
                KeyCode::Esc => {
                    self.file_viewer.switch_to_chat();
                    return true;
                }
                KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(idx) = self.file_viewer.active_idx {
                        self.file_viewer.close_tab(idx);
                    }
                    return true;
                }
                KeyCode::PageUp | KeyCode::Up => {
                    let n = if key.code == KeyCode::PageUp { 10 } else { 1 };
                    if let Some(tab) = self.file_viewer.active_tab_mut() {
                        tab.scroll_up(n);
                    }
                    return true;
                }
                KeyCode::PageDown | KeyCode::Down => {
                    let visible = self.chat_area_height as usize;
                    let n = if key.code == KeyCode::PageDown { 10 } else { 1 };
                    if let Some(tab) = self.file_viewer.active_tab_mut() {
                        tab.scroll_down(n, visible);
                    }
                    return true;
                }
                KeyCode::Home => {
                    if let Some(tab) = self.file_viewer.active_tab_mut() {
                        tab.scroll_offset = 0;
                    }
                    return true;
                }
                KeyCode::End => {
                    let visible = self.chat_area_height as usize;
                    if let Some(tab) = self.file_viewer.active_tab_mut() {
                        tab.scroll_offset = tab.total_lines.saturating_sub(visible);
                    }
                    return true;
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    let visible = self.chat_area_height as usize;
                    let half = visible / 2;
                    if let Some(tab) = self.file_viewer.active_tab_mut() {
                        tab.scroll_down(half, visible);
                    }
                    return true;
                }
                KeyCode::Tab if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(idx) = self.file_viewer.active_idx {
                        let next = (idx + 1) % self.file_viewer.tabs.len();
                        self.file_viewer.switch_to_tab(next);
                    }
                    return true;
                }
                _ => {
                    self.file_viewer.switch_to_chat();
                }
            }
        }

        match key.code {
            KeyCode::BackTab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                let rt_clone = self.plugin_runtime.as_ref().map(Arc::clone);
                let tool_name = rt_clone.as_ref().and_then(|rt| {
                    rt.lock()
                        .tool_for_keybind("shift+tab")
                        .map(|s| s.to_string())
                });
                if let Some(tool_name) = tool_name
                    && let Some(rt) = rt_clone
                {
                    let toggle_info = rt.lock().prepare_toggle(&tool_name);
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
                                    "{}",
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
            KeyCode::Tab
                if key.modifiers.contains(KeyModifiers::CONTROL) && !self.tabs.is_empty() =>
            {
                self.active_tab = (self.active_tab + 1) % self.tabs.len();
            }
            KeyCode::Char('1') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.conductor_mode {
                    message_handler::handle_command(self, "/solo").await;
                }
            }
            KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.conductor_mode {
                    message_handler::handle_command(self, "/conductor").await;
                }
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
                    if tab.input.history_idx.is_some() || tab.input.line_count() == 1 {
                        tab.input.history_up();
                        self.picker = None;
                        return false;
                    } else {
                        tab.input.handle_key_event(key);
                    }
                }
            }
            KeyCode::Down => {
                if let Some(tab) = self.current_tab_mut() {
                    if tab.input.history_idx.is_some() || tab.input.line_count() == 1 {
                        tab.input.history_down();
                        self.picker = None;
                        return false;
                    } else {
                        tab.input.handle_key_event(key);
                    }
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
            KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(text) = crate::tui::selection::paste_from_clipboard()
                    && let Some(tab) = self.current_tab_mut()
                {
                    tab.input.insert_paste(&text);
                }
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.clear();
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
            _ => {
                if let Some(tab) = self.current_tab_mut() {
                    tab.input.handle_key_event(key);
                }
            }
        }

        self.update_command_completion();
        false
    }

    // ─── Rendering ───────────────────────────────────────────────────────

    fn input_text_rect(&self) -> Rect {
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

    fn input_line_count(&self) -> u16 {
        self.current_tab()
            .map(|t| t.input.line_count() as u16)
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
            layout::main_layout(area, self.input_line_count())
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

    fn apply_models_page_action(&mut self, action: models_page::ModelsPageAction) {
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

    fn restore_theme(&mut self) {
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
    fn complete_onboarding(
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
        if let Some(ref mut form) = self.tool_form {
            tool_form::handle_paste(form, text);
            return;
        }
        if let Some(tab) = self.current_tab_mut() {
            tab.input.insert_paste(text);
        }
        self.update_command_completion();
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

fn padded_chat_area(raw: Rect) -> Rect {
    let pad = 2u16;
    let top_pad = 1u16;
    Rect {
        x: raw.x + pad,
        y: raw.y + top_pad,
        width: raw.width.saturating_sub(pad * 2),
        height: raw.height.saturating_sub(top_pad),
    }
}

const AGENT_PANEL_HEIGHT: u16 = 10;

fn agent_panel_rect(conductor_mode: bool, _agent_count: usize, chat: Rect) -> Option<Rect> {
    if !conductor_mode {
        return None;
    }
    let h = AGENT_PANEL_HEIGHT.min(chat.height / 3).max(5);
    let panel_w = 56u16.min(chat.width.saturating_sub(4));
    Some(Rect {
        x: chat.x + chat.width - panel_w,
        y: chat.y + chat.height - h,
        width: panel_w,
        height: h,
    })
}

fn append_file_summary(tabs: &mut [Tab], tab_idx: usize) {
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

pub async fn run(
    config: Config,
    _needs_onboarding: bool,
    extra_plugin_dirs: Vec<std::path::PathBuf>,
) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(
        stdout,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableBracketedPaste,
        crossterm::event::EnableMouseCapture,
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        ),
    )?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config);
    app.extra_plugin_dirs = extra_plugin_dirs;

    let plugin_dirs = crate::plugin::discover_plugin_dirs(
        Some(&app.project),
        &crate::config::paths::user_home(),
        &app.config.plugins.dirs,
    );
    if !plugin_dirs.is_empty() {
        let mut tools_snapshot = app.tools.read().clone();
        app.plugin_manager
            .load_and_start(plugin_dirs, &app.project, &mut tools_snapshot)
            .await;
        *app.tools.write() = tools_snapshot;
    }

    {
        let mut rt = PluginRuntime::new(std::env::current_dir().unwrap_or_default());
        rt.load_bundled();
        let plugin_dirs =
            PluginRuntime::discover_dirs(Some(&app.project), &crate::config::paths::user_home());
        for dir in &plugin_dirs {
            let _ = rt.load_from_dir(dir);
        }
        let rt_arc = Arc::new(parking_lot::Mutex::new(rt));
        crate::plugin::plugin_tool_adapter::register_plugin_tools(&rt_arc, &mut app.tools.write());
        app.plugin_runtime = Some(rt_arc);
    }

    if app
        .plugin_runtime
        .as_ref()
        .is_some_and(|rt| rt.lock().tool_count() > 0)
        || app.plugin_manager.plugin_count() > 0
    {
        let skills = crate::session::skills::discover_layered(
            Some(&app.project),
            &crate::config::paths::user_home(),
            &app.config.skills.dirs,
        );
        let rt_ref = app.plugin_runtime.as_ref().map(|rt| rt.lock());
        let command_list = crate::commands::dispatcher::list_commands_with_plugins(
            &skills,
            Some(&app.plugin_manager),
            rt_ref.as_deref(),
        );
        app.command_items = command_list
            .iter()
            .map(|cmd| PickerItem {
                id: cmd.name.clone(),
                label: cmd.name.clone(),
                description: cmd.summary.clone(),
                source_tag: command_source_tag(&cmd.source),
            })
            .collect();
    }

    let history_file = crate::config::paths::history_file();
    let rx = app.events_tx.subscribe();
    app.tabs.push(Tab::new("default".into(), rx, history_file));

    let result = run_loop(&mut app, &mut terminal).await;

    app.plugin_manager.shutdown_all().await;

    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::event::PopKeyboardEnhancementFlags,
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
            .map(|t| t.input.line_count() as u16)
            .unwrap_or(1);
        app.sidebar_area = None;
        app.tab_bar_area = None;
        let viewing_file = app.file_viewer.is_viewing_file();
        let has_file_tabs = app.file_viewer.has_tabs();
        let chunks = if let Some(ref form) = app.tool_form {
            let fh = tool_form::form_height(form);
            layout::main_layout_with_form(area, fh)
        } else if viewing_file {
            layout::file_viewer_layout(area)
        } else {
            layout::main_layout(area, input_lines)
        };

        // Compute tab bar and content rects
        let content_rect = if has_file_tabs {
            let split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(file_viewer::TAB_BAR_HEIGHT),
                    Constraint::Min(1),
                ])
                .split(chunks[0]);
            app.tab_bar_area = Some(split[0]);
            split[1]
        } else {
            chunks[0]
        };
        let padded = padded_chat_area(content_rect);
        app.chat_area_height = padded.height;
        app.chat_area = padded;
        if !viewing_file {
            app.input_area = chunks[1];
        }

        if !viewing_file {
            app.sidebar_area = agent_panel_rect(
                app.conductor_mode,
                app.sidebar_state.agents.len(),
                padded_chat_area(content_rect),
            );
        }
        app.frame_tick = app.frame_tick.wrapping_add(1);

        // Pick up newly spawned agents and create tabs for them
        for spawned in app.session_pool.drain_spawned() {
            let rx = app.events_tx.subscribe();
            let history = crate::config::paths::history_file();
            let mut tab = Tab::new(spawned.session_id.clone(), rx, history);
            tab.title = spawned.task.clone();
            let tab_index = app.tabs.len();
            app.tabs.push(tab);
            app.agent_receivers.push(crate::tui::app::AgentReceiver {
                tab_index,
                session_id: Some(spawned.session_id),
                rx: spawned.conv_rx,
                cancel: None,
            });
        }

        app.drain_panels();
        app.drain_conversations();
        if let Some(ref mut ob) = app.onboarding {
            ob.poll_models();
        }
        for tab in &mut app.tabs {
            crate::tui::rendering::helpers::drain_stream_buffer(tab);
        }

        if let Some(agents) = app.session_pool.try_check() {
            app.sidebar_state.update(agents);
        }
        if app.toast.as_ref().is_some_and(|t| t.is_expired()) {
            app.toast = None;
        }
        app.recompute_display_lines(size.width);

        terminal.draw(|f| app.render(f))?;

        if ct_event::poll(Duration::from_millis(16))? {
            match ct_event::read()? {
                CEvent::Key(key) => {
                    if key.code == KeyCode::Esc {
                        if app.conductor_mode
                            && app.active_tab == 0
                            && let Some(agents) = app.session_pool.try_check()
                        {
                            let has_running = agents.iter().any(|a| {
                                a.status == crate::session::orchestration::ChildStatus::Running
                                    || a.status
                                        == crate::session::orchestration::ChildStatus::Queued
                            });
                            if has_running {
                                app.session_pool.cancel_all().await;
                                // Also cancel the conductor conversation
                                for agent in &app.agent_receivers {
                                    if agent.tab_index == 0
                                        && let Some(cancel) = &agent.cancel
                                    {
                                        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                                    }
                                }
                                app.show_toast("All agents cancelled");
                                continue;
                            }
                        }
                        let has_running = app
                            .agent_receivers
                            .iter()
                            .any(|a| a.tab_index == app.active_tab);
                        if has_running {
                            for agent in &app.agent_receivers {
                                if agent.tab_index == app.active_tab
                                    && let Some(cancel) = &agent.cancel
                                {
                                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                            continue;
                        }
                        if app.active_tab > 0 {
                            app.active_tab = 0;
                            continue;
                        }
                    }

                    let is_plain_enter = key.code == KeyCode::Enter
                        && !key.modifiers.contains(KeyModifiers::SHIFT)
                        && !key.modifiers.contains(KeyModifiers::ALT);

                    let consumed = app.handle_key(key).await;

                    let is_agent_tab = app.active_tab > 0;
                    let input_is_command = app
                        .current_tab()
                        .map(|t| {
                            crate::commands::dispatcher::is_command(t.input.buffer_text().trim())
                        })
                        .unwrap_or(false);
                    let can_submit = is_plain_enter
                        && !consumed
                        && (input_is_command || !app.is_running || is_agent_tab);

                    if can_submit {
                        let input_text = app
                            .current_tab()
                            .map(|t| t.input.buffer_text())
                            .unwrap_or_default();

                        if !input_text.trim().is_empty() {
                            if let Some(tab) = app.current_tab_mut() {
                                tab.input.submit();
                            }

                            if is_agent_tab {
                                if let Some(agent) = app
                                    .agent_receivers
                                    .iter()
                                    .find(|a| a.tab_index == app.active_tab)
                                    && let Some(id) = &agent.session_id
                                {
                                    app.session_pool.try_send_message(id, &input_text);
                                }
                            } else if crate::commands::dispatcher::is_command(input_text.trim()) {
                                app.picker = None;
                                message_handler::handle_command(app, input_text.trim()).await;
                            } else {
                                message_handler::start_conversation(app, input_text);
                            }
                        }
                    }
                }
                CEvent::Paste(text) => app.handle_paste(&text),
                CEvent::Mouse(mouse) => {
                    use crossterm::event::MouseEventKind;
                    let in_panel = app.sidebar_area.is_some_and(|sb| {
                        mouse.column >= sb.x
                            && mouse.column < sb.x + sb.width
                            && mouse.row >= sb.y
                            && mouse.row < sb.y + sb.height
                    });

                    let in_tab_bar = app.tab_bar_area.is_some_and(|tb| {
                        mouse.row >= tb.y
                            && mouse.row < tb.y + tb.height
                            && mouse.column >= tb.x
                            && mouse.column < tb.x + tb.width
                    });

                    match mouse.kind {
                        MouseEventKind::ScrollUp if in_panel => {
                            app.sidebar_state.scroll = app.sidebar_state.scroll.saturating_sub(1);
                        }
                        MouseEventKind::ScrollDown if in_panel => {
                            app.sidebar_state.scroll += 1;
                        }
                        MouseEventKind::ScrollUp if app.file_viewer.is_viewing_file() => {
                            if let Some(ft) = app.file_viewer.active_tab_mut() {
                                ft.scroll_up(3);
                            }
                        }
                        MouseEventKind::ScrollDown if app.file_viewer.is_viewing_file() => {
                            let visible = app.chat_area_height as usize;
                            if let Some(ft) = app.file_viewer.active_tab_mut() {
                                ft.scroll_down(3, visible);
                            }
                        }
                        MouseEventKind::ScrollUp => {
                            if let Some(tab) = app.current_tab_mut() {
                                tab.scroll_up(1);
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            let total = app.display_lines.len();
                            let visible = app.chat_area_height as usize;
                            if let Some(tab) = app.current_tab_mut() {
                                tab.scroll_down(1, total, visible);
                            }
                        }
                        MouseEventKind::Down(crossterm::event::MouseButton::Left) if in_tab_bar => {
                            if let Some(tb_area) = app.tab_bar_area
                                && let Some(hit) = file_viewer::tab_bar_hit_test(
                                    tb_area,
                                    mouse.row,
                                    mouse.column,
                                    &app.file_viewer,
                                )
                            {
                                match hit {
                                    file_viewer::TabBarHit::Chat => {
                                        app.file_viewer.switch_to_chat();
                                    }
                                    file_viewer::TabBarHit::FileTab(idx) => {
                                        app.file_viewer.switch_to_tab(idx);
                                    }
                                    file_viewer::TabBarHit::CloseTab(idx) => {
                                        app.file_viewer.close_tab(idx);
                                    }
                                }
                            }
                            continue;
                        }
                        MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                            let r = mouse.row;
                            let c = mouse.column;

                            // Click on hovered file path opens in viewer
                            if !app.file_viewer.is_viewing_file() {
                                let chat_area = padded_chat_area(app.chat_area);
                                if r >= chat_area.y
                                    && r < chat_area.y + chat_area.height
                                    && c >= chat_area.x
                                    && c < chat_area.x + chat_area.width
                                {
                                    let scroll = app.effective_scroll();
                                    let line_idx = scroll + (r - chat_area.y) as usize;
                                    if let Some(dl) = app.display_lines.get(line_idx)
                                        && let Some(path) = &dl.file_path
                                        && path.exists()
                                    {
                                        let path = path.clone();
                                        let _ = app.file_viewer.open_file(&path, &app.theme);
                                        continue;
                                    }
                                }
                            }

                            if let Some(sb_area) = app.sidebar_area
                                && let Some(hit) =
                                    sidebar::hit_test(sb_area, r, c, &app.sidebar_state)
                            {
                                match hit {
                                    sidebar::HitResult::Dismiss(id) => {
                                        let was_active = app
                                            .tabs
                                            .get(app.active_tab)
                                            .is_some_and(|t| t.id == id);
                                        app.sidebar_state.dismiss(&id);
                                        if was_active {
                                            app.active_tab = 0;
                                        }
                                    }
                                    sidebar::HitResult::Select(ref sel) => {
                                        match sel {
                                            sidebar::SidebarSelection::Conductor => {
                                                app.active_tab = 0;
                                            }
                                            sidebar::SidebarSelection::Agent(id) => {
                                                if let Some(idx) =
                                                    app.tabs.iter().position(|t| t.id == *id)
                                                {
                                                    app.active_tab = idx;
                                                } else if let Some(agent) =
                                                    app.agent_receivers.iter().find(|a| {
                                                        a.session_id.as_deref() == Some(id.as_str())
                                                    })
                                                {
                                                    app.active_tab = agent.tab_index;
                                                }
                                            }
                                        }
                                        app.sidebar_state.selected = sel.clone();
                                    }
                                }
                                continue;
                            }

                            app.selection = Some(Selection {
                                start_row: r,
                                start_col: c,
                                end_row: r,
                                end_col: c,
                                active: true,
                            });
                            if let Some(ref picker) = app.picker {
                                match picker.mode {
                                    PickerMode::Theme | PickerMode::Model | PickerMode::Session => {
                                        let action = modal_picker::handle_click(
                                            picker,
                                            area,
                                            mouse.row,
                                            mouse.column,
                                        );
                                        app.apply_picker_action(action);
                                        continue;
                                    }
                                    PickerMode::CommandComplete => {}
                                }
                            }
                            let input_text_area = app.input_text_rect();
                            if mouse.row >= input_text_area.y
                                && mouse.row < input_text_area.y + input_text_area.height
                                && mouse.column >= input_text_area.x
                                && mouse.column < input_text_area.x + input_text_area.width
                            {
                                let row = (mouse.row - input_text_area.y) as usize;
                                let col = (mouse.column - input_text_area.x) as usize;
                                let tw = input_text_area.width as usize;
                                if let Some(tab) = app.current_tab_mut() {
                                    tab.input.click_at(row, col, tw);
                                }
                                app.selection = None;
                            } else {
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
                        }
                        MouseEventKind::Drag(crossterm::event::MouseButton::Left) => {
                            let input_text_area = app.input_text_rect();
                            if app.selection.is_none()
                                && mouse.row >= input_text_area.y
                                && mouse.row < input_text_area.y + input_text_area.height
                            {
                                let row = (mouse.row - input_text_area.y) as usize;
                                let col = (mouse.column.saturating_sub(input_text_area.x)) as usize;
                                let tw = input_text_area.width as usize;
                                if let Some(tab) = app.current_tab_mut() {
                                    tab.input.drag_to(row, col, tw);
                                }
                            } else if let Some(ref mut sel) = app.selection {
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
                        MouseEventKind::Moved
                            if app.picker.as_ref().is_some_and(|p| {
                                matches!(
                                    p.mode,
                                    PickerMode::Theme | PickerMode::Model | PickerMode::Session
                                )
                            }) =>
                        {
                            let action = modal_picker::handle_hover(
                                app.picker.as_mut().unwrap(),
                                area,
                                mouse.row,
                                mouse.column,
                            );
                            app.apply_picker_action(action);
                        }
                        MouseEventKind::Moved => {
                            // Tab bar close button hover
                            if let Some(tb_area) = app.tab_bar_area {
                                if let Some(file_viewer::TabBarHit::CloseTab(idx)) =
                                    file_viewer::tab_bar_hit_test(
                                        tb_area,
                                        mouse.row,
                                        mouse.column,
                                        &app.file_viewer,
                                    )
                                {
                                    app.file_viewer.hovered_close = Some(idx);
                                } else {
                                    app.file_viewer.hovered_close = None;
                                }
                            } else {
                                app.file_viewer.hovered_close = None;
                            }

                            // Chat file path hover
                            if !app.file_viewer.is_viewing_file() {
                                let chat_area = padded_chat_area(app.chat_area);
                                if mouse.row >= chat_area.y
                                    && mouse.row < chat_area.y + chat_area.height
                                    && mouse.column >= chat_area.x
                                    && mouse.column < chat_area.x + chat_area.width
                                {
                                    let scroll = app.effective_scroll();
                                    let line_idx = scroll + (mouse.row - chat_area.y) as usize;
                                    if app
                                        .display_lines
                                        .get(line_idx)
                                        .and_then(|dl| dl.file_path.as_ref())
                                        .is_some()
                                    {
                                        app.hovered_line = Some(line_idx);
                                    } else {
                                        app.hovered_line = None;
                                    }
                                } else {
                                    app.hovered_line = None;
                                }
                            } else {
                                app.hovered_line = None;
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

        if let Some(ref task) = app.reload_task
            && task.is_finished()
        {
            let task = app.reload_task.take().unwrap();
            if let Ok(output) = task.await {
                message_handler::apply_reload(app, output);
            }
            app.is_reloading = false;
        }

        if let Some(session_id) = app.pending_session_resume.take() {
            message_handler::resume_session(app, &session_id).await;
        }

        if let Some(text) = app.pending_skill_message.take() {
            message_handler::start_conversation(app, text);
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

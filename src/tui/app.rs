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
    pub session_pool: Arc<SessionPool>,
    pub orch_ctx: Arc<crate::tools::orchestration::OrchestrationContext>,
    pub sidebar_state: sidebar::SidebarState,
    pub sidebar_area: Option<Rect>,
    pub panels: std::collections::HashMap<String, phoenix_shared::ui_types::PanelState>,
    pub panel_rx:
        Option<tokio::sync::mpsc::UnboundedReceiver<crate::plugin::host_handler::PanelUpdate>>,
    pub pending_conductor_activate: bool,
    pub agent_receivers: Vec<AgentReceiver>,
    pub pending_model_selection: Option<usize>,
    pub models_page: Option<ModelsPageState>,
    pub tool_form: Option<tool_form::ToolFormState>,
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
            agents: parking_lot::RwLock::new(Vec::new()),
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
            conductor_mode: false,
            session_pool: pool,
            orch_ctx,
            sidebar_state: sidebar::SidebarState::new(),
            sidebar_area: None,
            panels: std::collections::HashMap::new(),
            panel_rx: None,
            pending_conductor_activate: false,
            agent_receivers: Vec::new(),
            pending_model_selection: None,
            models_page: None,
            tool_form: None,
        }
    }

    pub fn show_sidebar(&self) -> bool {
        if self.conductor_mode {
            return true;
        }
        self.session_pool.try_check().is_some_and(|agents| {
            agents.iter().any(|a| {
                a.status == crate::session::orchestration::ChildStatus::Running
                    || a.status == crate::session::orchestration::ChildStatus::Queued
            })
        })
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
                            phoenix_shared::ui_types::PanelState {
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
            let tab_idx = agent.tab_index;
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
                    ConvEvent::ContextLoaded(names) => {
                        if let Some(tab) = self.tabs.get_mut(tab_idx) {
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::System,
                                content: crate::tui::rendering::helpers::format_context_tree(
                                    &names,
                                ),
                            }));
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
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::System,
                                content: "Cancelled.".into(),
                            }));
                        }
                        if tab_idx == 0 {
                            self.session = Some(session);
                            self.is_running = false;
                        }
                        finished.push(i);
                        break;
                    }
                    ConvEvent::Done(session) => {
                        if tab_idx == 0 {
                            self.session = Some(session);
                            self.is_running = false;
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

    pub fn invoke_tool_command(&mut self, tool_name: &str, args_json: &str) {
        if tool_name == "conductor" {
            message_handler::handle_command(self, "/conductor");
            return;
        }
        if let Some(rt) = self.plugin_runtime.as_ref().map(Arc::clone) {
            match rt.lock().toggle_tool(tool_name, args_json) {
                Ok(result) => self.apply_tool_result(result),
                Err(e) => self.show_toast(format!("Plugin error: {e}")),
            }
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
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
                tool_form::FormAction::Submit(args) => {
                    let name = form.tool_name.clone();
                    let args_str = args.to_string();
                    self.tool_form = None;
                    self.invoke_tool_command(&name, &args_str);
                }
                tool_form::FormAction::Cancel => {
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
                } => {
                    self.complete_onboarding(name, kind, model, api_key, env_hint);
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
                PickerMode::Theme
                | PickerMode::Model
                | PickerMode::Session
                | PickerMode::ConductorModelPick
                | PickerMode::ConductorAgent
                | PickerMode::ConductorTracker => {
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
                    match rt.lock().toggle_tool(&tool_name, "{}") {
                        Ok(result) => self.apply_tool_result(result),
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
                    message_handler::handle_command(self, "/solo");
                }
            }
            KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.conductor_mode {
                    message_handler::handle_command(self, "/conductor");
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
                    } else {
                        tab.input.handle_key_event(key);
                    }
                }
            }
            KeyCode::Down => {
                if let Some(tab) = self.current_tab_mut() {
                    if tab.input.history_idx.is_some() || tab.input.line_count() == 1 {
                        tab.input.history_down();
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
        let prompt_len = 4u16; // "  > "
        Rect {
            x: self.input_area.x + prompt_len,
            y: self.input_area.y + 1, // skip separator
            width: self.input_area.width.saturating_sub(prompt_len),
            height: self.input_area.height.saturating_sub(1),
        }
    }

    fn input_line_count(&self) -> u16 {
        self.current_tab()
            .map(|t| t.input.line_count() as u16)
            .unwrap_or(1)
    }

    pub fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        frame.render_widget(
            Block::default().style(Style::default().bg(self.theme.background)),
            area,
        );

        let content_area = if self.show_sidebar() {
            let (sb_area, content) = layout::split_sidebar(area);
            if sb_area.width > 0 {
                sidebar::render_sidebar(frame, sb_area, &self.sidebar_state, &self.theme);
            }
            content
        } else {
            area
        };

        let chunks = if let Some(ref form) = self.tool_form {
            let fh = tool_form::form_height(form);
            layout::main_layout_with_form(content_area, fh)
        } else {
            layout::main_layout(content_area, self.input_line_count())
        };

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

        if let Some(ref form) = self.tool_form {
            tool_form::render_tool_form(frame, chunks[1], form, &self.theme);
        } else if let Some(tab) = self.current_tab() {
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
                PickerMode::Theme
                | PickerMode::Model
                | PickerMode::Session
                | PickerMode::ConductorModelPick
                | PickerMode::ConductorAgent
                | PickerMode::ConductorTracker => {
                    modal_picker::render_modal_picker(frame, picker, &self.theme);
                }
            }
        }

        if self.is_reloading {
            use crate::tui::rendering::helpers::spinner_frame;
            let frame_idx = (self.frame_tick / 4) as usize;
            let spin = spinner_frame(frame_idx);
            let msg = "reloading";
            let text = format!("  {spin} {msg}  ");
            let width = text.chars().count().min(chunks[1].width as usize) as u16;
            let x = chunks[1].x + (chunks[1].width.saturating_sub(width)) / 2;
            let y = chunks[1].y.saturating_sub(1);
            let bg = self.theme.status_bar_bg();
            let fg = self.theme.status_bar_fg();
            let line = ratatui::text::Line::from(ratatui::text::Span::styled(
                text,
                Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
            ));
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
                    Some(PickerMode::ConductorModelPick) => {
                        if let Some((provider, model)) = selected.id.split_once('/') {
                            self.config.conductor.conductor_provider = Some(provider.to_string());
                            self.config.conductor.conductor_model = Some(model.to_string());
                            self.show_toast(format!("Conductor: {}", selected.label));
                        }
                        let items = message_handler::build_conductor_picker_items(&self.config);
                        let picker_items: Vec<PickerItem> = items
                            .into_iter()
                            .map(|(id, label, desc)| PickerItem {
                                id,
                                label,
                                description: desc,
                                source_tag: None,
                            })
                            .collect();
                        self.picker =
                            Some(PickerState::new(picker_items, PickerMode::ConductorAgent));
                        return;
                    }
                    Some(PickerMode::ConductorAgent) => {
                        if let Some((provider, model)) = selected.id.split_once('/') {
                            self.config.conductor.agent_provider = Some(provider.to_string());
                            self.config.conductor.agent_model = Some(model.to_string());
                            self.show_toast(format!("Sub-agents: {}", selected.label));
                        }
                        let tracker_items = vec![
                            PickerItem {
                                id: "beads".into(),
                                label: "Beads (bd)".into(),
                                description: "Local git-native issue tracking".into(),
                                source_tag: None,
                            },
                            PickerItem {
                                id: "linear".into(),
                                label: "Linear".into(),
                                description: "Linear project management (MCP)".into(),
                                source_tag: None,
                            },
                            PickerItem {
                                id: "jira".into(),
                                label: "Jira".into(),
                                description: "Atlassian Jira".into(),
                                source_tag: None,
                            },
                            PickerItem {
                                id: "none".into(),
                                label: "None".into(),
                                description: "No issue tracker — tasks given directly".into(),
                                source_tag: None,
                            },
                        ];
                        self.picker = Some(PickerState::new(
                            tracker_items,
                            PickerMode::ConductorTracker,
                        ));
                        return;
                    }
                    Some(PickerMode::ConductorTracker) => {
                        let tracker = if selected.id == "none" {
                            None
                        } else {
                            Some(selected.id.clone())
                        };
                        self.config.conductor.tracker = tracker;

                        let config_path = crate::config::paths::user_config_file();
                        let _ = crate::config::writer::save_conductor_config(
                            &config_path,
                            &self.config.conductor,
                        );
                        self.show_toast(format!("Tracker: {}", selected.label));
                        self.pending_conductor_activate = true;
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

                    if let Some(tab) = self.current_tab_mut() {
                        tab.chat_lines.push(ChatItem::Line(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Model: {display}"),
                        }));
                    }
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
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Failed to save config: {e}"),
                }));
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
        app.plugin_manager
            .load_and_start(plugin_dirs, &app.project, &mut app.tools)
            .await;
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
        crate::plugin::plugin_tool_adapter::register_plugin_tools(&rt_arc, &mut app.tools);
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
        let content_area = if app.show_sidebar() {
            let (sb_area, content) = layout::split_sidebar(area);
            app.sidebar_area = if sb_area.width > 0 {
                Some(sb_area)
            } else {
                None
            };
            content
        } else {
            app.sidebar_area = None;
            area
        };
        let chunks = if let Some(ref form) = app.tool_form {
            let fh = tool_form::form_height(form);
            layout::main_layout_with_form(content_area, fh)
        } else {
            layout::main_layout(content_area, input_lines)
        };
        app.chat_area_height = chunks[0].height;
        app.chat_area = chunks[0];
        app.input_area = chunks[1];
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
                                futures::executor::block_on(app.session_pool.cancel_all());
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

                    let consumed = app.handle_key(key);

                    let is_agent_tab = app.active_tab > 0;
                    let can_submit =
                        is_plain_enter && !consumed && (!app.is_running || is_agent_tab);

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
                                message_handler::handle_command(app, input_text.trim());
                            } else {
                                message_handler::start_conversation(app, input_text);
                            }
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

                            if let Some(sb_area) = app.sidebar_area
                                && let Some(sel) =
                                    sidebar::hit_test(sb_area, r, c, &app.sidebar_state)
                            {
                                match &sel {
                                    sidebar::SidebarSelection::Conductor => {
                                        app.active_tab = 0;
                                    }
                                    sidebar::SidebarSelection::Agent(id) => {
                                        if let Some(agent) = app
                                            .agent_receivers
                                            .iter()
                                            .find(|a| a.session_id.as_deref() == Some(id.as_str()))
                                        {
                                            app.active_tab = agent.tab_index;
                                        }
                                    }
                                }
                                app.sidebar_state.selected = sel;
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
                                    PickerMode::Theme
                                    | PickerMode::Model
                                    | PickerMode::Session
                                    | PickerMode::ConductorModelPick
                                    | PickerMode::ConductorAgent
                                    | PickerMode::ConductorTracker => {
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

        if app.pending_conductor_activate {
            app.pending_conductor_activate = false;
            message_handler::activate_conductor(app);
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

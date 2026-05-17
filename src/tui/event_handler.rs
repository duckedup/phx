use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self as ct_event, Event as CEvent, KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::App;
use crate::tui::components::sidebar::SidebarSelection;
use crate::tui::components::{command_completion, modal_picker, sidebar};
use crate::tui::picker::PickerMode;
use crate::tui::ui::tool_form;
use crate::tui::{commands, onboarding};

pub fn drain_pending_events(app: &mut App) {
    while ct_event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        if let Ok(event) = ct_event::read() {
            match event {
                CEvent::Mouse(mouse) => handle_sidebar_click(app, mouse),
                CEvent::Key(key)
                    if key.code == KeyCode::Esc
                        || (key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)) =>
                {
                    app.ctrl_c_count += 1;
                }
                _ => {}
            }
        }
    }
}

pub fn handle_sidebar_click(app: &mut App, mouse: crossterm::event::MouseEvent) {
    use crossterm::event::MouseEventKind;
    if mouse.kind != MouseEventKind::Down(crossterm::event::MouseButton::Left) {
        return;
    }
    if let Some(sb_area) = app.sidebar_area
        && let Some(hit) = crate::tui::components::sidebar::hit_test(
            sb_area,
            mouse.row,
            mouse.column,
            &app.sidebar_state,
        )
    {
        use crate::tui::components::sidebar::HitResult;
        match hit {
            HitResult::Dismiss(id) => {
                let was_active = app.tabs.get(app.active_tab).is_some_and(|t| t.id == id);
                app.sidebar_state.dismiss(&id);
                if was_active {
                    app.active_tab = 0;
                }
            }
            HitResult::Select(ref sel) => {
                match sel {
                    SidebarSelection::Conductor => {
                        app.active_tab = 0;
                    }
                    SidebarSelection::Agent(id) => {
                        if let Some(agent) = app
                            .agent_receivers
                            .iter()
                            .find(|a| a.session_id.as_deref() == Some(id.as_str()))
                        {
                            app.active_tab = agent.tab_index;
                        }
                    }
                }
                app.sidebar_state.selected = sel.clone();
            }
        }
    }
}

// ─── Methods moved from app.rs ──────────────────────────────────────────────

impl App {
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
                crate::tui::update::update(self, crate::tui::msg::Msg::Quit);
                return true;
            }
            if self.models_page.is_some() {
                self.models_page = None;
                return true;
            }
            if self.picker.is_some() {
                crate::tui::update::update(self, crate::tui::msg::Msg::PickerClose);
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
                crate::tui::update::update(self, crate::tui::msg::Msg::Quit);
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
                    crate::tui::update::update(self, crate::tui::msg::Msg::PickerClear);
                    return true;
                }
                command_completion::CompletionAction::Complete(cmd) => {
                    if let Some(tab) = self.current_tab_mut() {
                        tab.input.set_single_line(&cmd);
                    }
                    crate::tui::update::update(self, crate::tui::msg::Msg::PickerClear);
                    return true;
                }
                command_completion::CompletionAction::Accept(cmd) => {
                    if let Some(tab) = self.current_tab_mut() {
                        tab.input.set_single_line(&cmd);
                    }
                    crate::tui::update::update(self, crate::tui::msg::Msg::PickerClear);
                    return false;
                }
            }
        }

        if self.panel_focused && self.conductor_mode {
            use crate::tui::msg::Msg;
            match key.code {
                KeyCode::Up => {
                    crate::tui::update::update(self, Msg::SidebarNavigateUp);
                    return true;
                }
                KeyCode::Down => {
                    crate::tui::update::update(self, Msg::SidebarNavigateDown);
                    return true;
                }
                KeyCode::Delete | KeyCode::Backspace => {
                    if let sidebar::SidebarSelection::Agent(ref id) = self.sidebar_state.selected {
                        let id = id.clone();
                        let is_finished = self
                            .sidebar_state
                            .agents
                            .iter()
                            .find(|a| a.session_id == id)
                            .is_some_and(|a| {
                                matches!(
                                    a.status,
                                    crate::session::orchestration::ChildStatus::Done
                                        | crate::session::orchestration::ChildStatus::Error(_)
                                        | crate::session::orchestration::ChildStatus::Cancelled
                                )
                            });
                        if is_finished {
                            crate::tui::update::update(self, Msg::SidebarDismissSelected);
                        }
                    }
                    return true;
                }
                KeyCode::Enter => {
                    crate::tui::update::update(self, Msg::SidebarActivateSelected);
                    return true;
                }
                KeyCode::Left | KeyCode::Esc => {
                    crate::tui::update::update(self, Msg::PanelFocusSet(false));
                    return true;
                }
                _ => {
                    crate::tui::update::update(self, Msg::PanelFocusSet(false));
                }
            }
        }

        if self.conductor_mode && key.code == KeyCode::Right {
            let input_empty = self
                .current_tab()
                .map(|t| t.input.buffer_text().is_empty())
                .unwrap_or(true);
            if input_empty {
                crate::tui::update::update(self, crate::tui::msg::Msg::PanelFocusSet(true));
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
                let next = (self.active_tab + 1) % self.tabs.len();
                crate::tui::update::update(self, crate::tui::msg::Msg::TabSwitch(next));
            }
            KeyCode::Char('1') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.conductor_mode {
                    commands::handle_command(self, "/solo").await;
                }
            }
            KeyCode::Char('2') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if !self.conductor_mode {
                    commands::handle_command(self, "/conductor").await;
                }
            }
            KeyCode::Char('w')
                if key.modifiers.contains(KeyModifiers::CONTROL) && !self.tabs.is_empty() =>
            {
                crate::tui::update::update(self, crate::tui::msg::Msg::TabClose(self.active_tab));
            }
            KeyCode::Up => {
                if let Some(tab) = self.current_tab_mut() {
                    if tab.input.history_idx.is_some() || tab.input.line_count() == 1 {
                        tab.input.history_up();
                        crate::tui::update::update(self, crate::tui::msg::Msg::PickerClear);
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
                        crate::tui::update::update(self, crate::tui::msg::Msg::PickerClear);
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
                crate::tui::update::update(self, crate::tui::msg::Msg::ScrollUp(10));
            }
            KeyCode::PageDown => {
                crate::tui::update::update(self, crate::tui::msg::Msg::ScrollDown(10));
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
                let half = self.chat_area_height as usize / 2;
                crate::tui::update::update(self, crate::tui::msg::Msg::ScrollDown(half));
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

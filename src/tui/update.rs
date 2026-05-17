use crate::tui::app::App;
use crate::tui::cmd::Cmd;
use crate::tui::components::sidebar::SidebarSelection;
use crate::tui::msg::Msg;

pub fn update(app: &mut App, msg: Msg) -> Cmd {
    match msg {
        // ── Scroll ──────────────────────────────────────────────────────
        Msg::ScrollUp(n) => {
            if let Some(tab) = app.current_tab_mut() {
                tab.scroll_up(n);
            }
        }
        Msg::ScrollDown(n) => {
            let total = app.display_lines.len();
            let visible = app.chat_area_height as usize;
            if let Some(tab) = app.current_tab_mut() {
                tab.scroll_down(n, total, visible);
            }
        }
        Msg::ScrollToBottom => {
            if let Some(tab) = app.current_tab_mut() {
                tab.auto_scroll = true;
                tab.scroll_offset = 0;
            }
        }

        // ── Tab management ──────────────────────────────────────────────
        Msg::TabSwitch(idx) => {
            if idx < app.tabs.len() {
                app.active_tab = idx;
            }
        }
        Msg::TabClose(idx) => {
            if idx > 0 && idx < app.tabs.len() {
                app.tabs.remove(idx);
                if app.active_tab >= app.tabs.len() {
                    app.active_tab = app.tabs.len().saturating_sub(1);
                }
                if app.active_tab >= idx && app.active_tab > 0 {
                    app.active_tab -= 1;
                }
            }
        }

        // ── Focus ───────────────────────────────────────────────────────
        Msg::PanelFocusSet(focused) => {
            app.panel_focused = focused;
        }

        // ── Sidebar ─────────────────────────────────────────────────────
        Msg::SidebarScrollUp => {
            app.sidebar_state.scroll = app.sidebar_state.scroll.saturating_sub(1);
        }
        Msg::SidebarScrollDown => {
            app.sidebar_state.scroll += 1;
        }
        Msg::SidebarNavigateUp => {
            let agents = &app.sidebar_state.agents;
            if let SidebarSelection::Agent(ref id) = app.sidebar_state.selected
                && let Some(pos) = agents.iter().position(|a| a.session_id == *id)
            {
                if pos > 0 {
                    app.sidebar_state.selected =
                        SidebarSelection::Agent(agents[pos - 1].session_id.clone());
                } else {
                    app.sidebar_state.selected = SidebarSelection::Conductor;
                }
            }
            let visible = app.sidebar_area.map(|a| a.height as usize).unwrap_or(10);
            app.sidebar_state.ensure_selected_visible(visible);
        }
        Msg::SidebarNavigateDown => {
            let agents = &app.sidebar_state.agents;
            match &app.sidebar_state.selected {
                SidebarSelection::Conductor => {
                    if let Some(first) = agents.first() {
                        app.sidebar_state.selected =
                            SidebarSelection::Agent(first.session_id.clone());
                    }
                }
                SidebarSelection::Agent(id) => {
                    if let Some(pos) = agents.iter().position(|a| a.session_id == *id)
                        && pos + 1 < agents.len()
                    {
                        app.sidebar_state.selected =
                            SidebarSelection::Agent(agents[pos + 1].session_id.clone());
                    }
                }
            }
            let visible = app.sidebar_area.map(|a| a.height as usize).unwrap_or(10);
            app.sidebar_state.ensure_selected_visible(visible);
        }
        Msg::SidebarDismissAgent(id) => {
            let was_active = app.tabs.get(app.active_tab).is_some_and(|t| t.id == id);
            app.sidebar_state.dismiss(&id);
            if was_active {
                app.active_tab = 0;
            }
        }
        Msg::SidebarDismissSelected => {
            if let SidebarSelection::Agent(ref id) = app.sidebar_state.selected {
                let id = id.clone();
                let was_active = app.tabs.get(app.active_tab).is_some_and(|t| t.id == id);
                app.sidebar_state.dismiss(&id);
                app.sidebar_state.selected = SidebarSelection::Conductor;
                if was_active {
                    app.active_tab = 0;
                }
            }
        }
        Msg::SidebarSelect(sel) => {
            match &sel {
                SidebarSelection::Conductor => {
                    app.active_tab = 0;
                }
                SidebarSelection::Agent(id) => {
                    if let Some(idx) = app.tabs.iter().position(|t| t.id == *id) {
                        app.active_tab = idx;
                    } else if let Some(agent) = app
                        .agent_receivers
                        .iter()
                        .find(|a| a.session_id.as_deref() == Some(id.as_str()))
                    {
                        app.active_tab = agent.tab_index;
                    }
                }
            }
            app.sidebar_state.selected = sel;
        }
        Msg::SidebarActivateSelected => {
            match &app.sidebar_state.selected {
                SidebarSelection::Conductor => {
                    app.active_tab = 0;
                }
                SidebarSelection::Agent(id) => {
                    if let Some(idx) = app.tabs.iter().position(|t| t.id == *id) {
                        app.active_tab = idx;
                    } else if let Some(agent) = app
                        .agent_receivers
                        .iter()
                        .find(|a| a.session_id.as_deref() == Some(id.as_str()))
                    {
                        app.active_tab = agent.tab_index;
                    }
                }
            }
            app.panel_focused = false;
        }

        // ── File viewer ─────────────────────────────────────────────────
        Msg::FileViewerScrollUp(n) => {
            if let Some(ft) = app.file_viewer.active_tab_mut() {
                ft.scroll_up(n);
            }
        }
        Msg::FileViewerScrollDown(n) => {
            let visible = app.chat_area_height as usize;
            if let Some(ft) = app.file_viewer.active_tab_mut() {
                ft.scroll_down(n, visible);
            }
        }
        Msg::FileViewerSwitchToChat => {
            app.file_viewer.switch_to_chat();
        }
        Msg::FileViewerSwitchTab(idx) => {
            app.file_viewer.switch_to_tab(idx);
        }
        Msg::FileViewerCloseTab(idx) => {
            app.file_viewer.close_tab(idx);
        }
        Msg::FileViewerOpenFile(path) => {
            let _ = app.file_viewer.open_file(&path, &app.theme);
        }
        Msg::FileViewerHoverClose(idx) => {
            app.file_viewer.hovered_close = idx;
        }

        // ── Picker ──────────────────────────────────────────────────────
        Msg::PickerClose => {
            app.restore_theme();
            app.picker = None;
        }
        Msg::PickerClear => {
            app.picker = None;
        }

        // ── Selection & hover ───────────────────────────────────────────
        Msg::SelectionClear => {
            app.selection = None;
        }
        Msg::HoverLine(line) => {
            app.hovered_line = line;
        }

        // ── Modals ───────────────────────────────────────────────────────
        Msg::ToolFormSubmit {
            answers,
            tool_name,
            args_json,
        } => {
            if let Some(response_tx) = app.interactive_response_tx.take() {
                let _ = response_tx.send(Some(answers));
                app.tool_form = None;
            } else {
                app.tool_form = None;
                return Cmd::RunToolCommand {
                    tool_name,
                    args_json,
                };
            }
        }
        Msg::ToolFormDismiss => {
            if let Some(response_tx) = app.interactive_response_tx.take() {
                let _ = response_tx.send(None);
            }
            app.tool_form = None;
        }
        Msg::ModelsPageDismiss => {
            app.models_page = None;
        }
        Msg::OnboardingDismiss => {
            app.onboarding = None;
        }

        // ── Conversation events ──────────────────────────────────────────
        Msg::ConvStreamToken { tab_idx, text } => {
            if let Some(tab) = app.tabs.get_mut(tab_idx) {
                tab.stream_buffer.push_str(&text);
            }
        }
        Msg::ConvAssistantMessage { tab_idx, text } => {
            if let Some(tab) = app.tabs.get_mut(tab_idx) {
                tab.streaming_text.clear();
                tab.stream_buffer.clear();
                tab.chat_lines.push(crate::tui::tabs::ChatItem::Line(
                    crate::tui::tabs::ChatLine {
                        role: crate::session::message::Role::Assistant,
                        content: text,
                    },
                ));
            }
        }
        Msg::ConvToolCall { tab_idx, summary } => {
            if let Some(tab) = app.tabs.get_mut(tab_idx) {
                crate::tui::rendering::helpers::drain_stream_buffer(tab);
                tab.chat_lines.push(crate::tui::tabs::ChatItem::Line(
                    crate::tui::tabs::ChatLine {
                        role: crate::session::message::Role::ToolCall,
                        content: summary,
                    },
                ));
            }
        }
        Msg::ConvToolResult { tab_idx, output } => {
            if let Some(tab) = app.tabs.get_mut(tab_idx) {
                tab.chat_lines.push(crate::tui::tabs::ChatItem::Line(
                    crate::tui::tabs::ChatLine {
                        role: crate::session::message::Role::ToolResult,
                        content: output,
                    },
                ));
            }
        }
        Msg::ConvContextLoaded { tab_idx, names } => {
            if let Some(tab) = app.tabs.get_mut(tab_idx) {
                tab.chat_lines
                    .push(crate::tui::tabs::ChatItem::ContextLoaded(names));
            }
        }
        Msg::ConvContextCompacted {
            tab_idx,
            removed,
            remaining,
        } => {
            if let Some(tab) = app.tabs.get_mut(tab_idx) {
                tab.chat_lines.push(crate::tui::tabs::ChatItem::Line(
                    crate::tui::tabs::ChatLine {
                        role: crate::session::message::Role::System,
                        content: format!(
                            "Context compacted: removed {removed} messages ({remaining} remaining)"
                        ),
                    },
                ));
            }
        }
        Msg::ConvError { tab_idx, message } => {
            if let Some(tab) = app.tabs.get_mut(tab_idx) {
                tab.streaming_text.clear();
                tab.stream_buffer.clear();
                tab.chat_lines.push(crate::tui::tabs::ChatItem::Line(
                    crate::tui::tabs::ChatLine {
                        role: crate::session::message::Role::System,
                        content: message,
                    },
                ));
            }
        }
        Msg::ConvCancelled { tab_idx, agent_idx } => {
            if let Some(tab) = app.tabs.get_mut(tab_idx) {
                tab.streaming_text.clear();
                tab.stream_buffer.clear();
            }
            crate::tui::app::append_file_summary(&mut app.tabs, tab_idx);
            app.toast = Some(crate::tui::components::toast::Toast::new(
                "Cancelled".into(),
                std::time::Duration::from_secs(3),
            ));
            if tab_idx == 0 {
                app.is_running = false;
            } else if let Some(sid) = app
                .agent_receivers
                .get(agent_idx)
                .and_then(|a| a.session_id.as_deref())
            {
                app.session_pool.mark_done(sid, false);
            }
            if agent_idx < app.agent_receivers.len() {
                app.agent_receivers.remove(agent_idx);
            }
        }
        Msg::ConvDone { tab_idx, agent_idx } => {
            crate::tui::app::append_file_summary(&mut app.tabs, tab_idx);
            if tab_idx == 0 {
                app.is_running = false;
            } else if let Some(sid) = app
                .agent_receivers
                .get(agent_idx)
                .and_then(|a| a.session_id.as_deref())
            {
                app.session_pool.mark_done(sid, true);
            }
            if agent_idx < app.agent_receivers.len() {
                app.agent_receivers.remove(agent_idx);
            }
        }
        Msg::ConvInteractiveUi {
            tool_name,
            fields,
            response_tx,
        } => {
            let config = crate::shared::ui_field_types::ToolUiConfig::new(fields);
            app.tool_form = Some(crate::tui::ui::tool_form::ToolFormState::from_ui(
                tool_name,
                String::new(),
                &config,
            ));
            app.interactive_response_tx = Some(response_tx);
        }

        // ── Input ────────────────────────────────────────────────────────
        Msg::InputSubmit => {
            let input_text = app
                .current_tab()
                .map(|t| t.input.buffer_text())
                .unwrap_or_default();

            if input_text.trim().is_empty() {
                return Cmd::None;
            }

            if let Some(tab) = app.current_tab_mut() {
                tab.input.submit();
            }

            let is_agent_tab = app.active_tab > 0;

            if is_agent_tab {
                if let Some(agent) = app
                    .agent_receivers
                    .iter()
                    .find(|a| a.tab_index == app.active_tab)
                    && let Some(id) = &agent.session_id
                {
                    return Cmd::SendAgentMessage {
                        session_id: id.clone(),
                        text: input_text,
                    };
                }
                return Cmd::None;
            }

            if crate::commands::dispatcher::is_command(input_text.trim()) {
                app.picker = None;
                return Cmd::RunCommand(input_text.trim().to_string());
            }

            return Cmd::StartConversation(input_text);
        }

        // ── Toast ───────────────────────────────────────────────────────
        Msg::ToastExpire => {
            app.toast = None;
        }

        // ── Quit ────────────────────────────────────────────────────────
        Msg::Quit => {
            app.should_quit = true;
        }
    }
    Cmd::None
}

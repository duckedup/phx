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

        // ── Tool detail ─────────────────────────────────────────────────
        Msg::ToolDetailOpen { tool_name, content } => {
            let tab_name = format!("⚙ {tool_name}");
            app.file_viewer
                .open_content(&tab_name, &content, &app.theme);
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

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use ratatui::prelude::Rect;

    fn test_app() -> App {
        let config = crate::config::schema::Config::default();
        let mut app = App::new(config);
        app.onboarding = None;
        app.conductor_mode = false;

        let history = std::env::temp_dir().join("phx-test-history");
        let rx = app.events_tx.subscribe();
        app.tabs
            .push(crate::tui::tabs::Tab::new("test".into(), rx, history));
        app.chat_area = Rect::new(2, 1, 120, 40);
        app.chat_area_height = 40;
        app
    }

    // ── Scroll ──────────────────────────────────────────────────────

    #[test]
    fn scroll_up_decreases_offset() {
        let mut app = test_app();
        app.tabs[0].scroll_offset = 10;
        app.tabs[0].auto_scroll = false;
        update(&mut app, Msg::ScrollUp(3));
        assert_eq!(app.tabs[0].scroll_offset, 7);
    }

    #[test]
    fn scroll_up_saturates_at_zero() {
        let mut app = test_app();
        app.tabs[0].scroll_offset = 2;
        app.tabs[0].auto_scroll = false;
        update(&mut app, Msg::ScrollUp(10));
        assert_eq!(app.tabs[0].scroll_offset, 0);
    }

    #[test]
    fn scroll_up_disables_auto_scroll() {
        let mut app = test_app();
        app.tabs[0].auto_scroll = true;
        update(&mut app, Msg::ScrollUp(1));
        assert!(!app.tabs[0].auto_scroll);
    }

    #[test]
    fn scroll_to_bottom_enables_auto_scroll() {
        let mut app = test_app();
        app.tabs[0].auto_scroll = false;
        app.tabs[0].scroll_offset = 50;
        update(&mut app, Msg::ScrollToBottom);
        assert!(app.tabs[0].auto_scroll);
        assert_eq!(app.tabs[0].scroll_offset, 0);
    }

    // ── Tab management ──────────────────────────────────────────────

    #[test]
    fn tab_switch_changes_active() {
        let mut app = test_app();
        let rx2 = app.events_tx.subscribe();
        let h = std::env::temp_dir().join("phx-test-h2");
        app.tabs
            .push(crate::tui::tabs::Tab::new("t2".into(), rx2, h));
        update(&mut app, Msg::TabSwitch(1));
        assert_eq!(app.active_tab, 1);
    }

    #[test]
    fn tab_switch_out_of_bounds_is_noop() {
        let mut app = test_app();
        update(&mut app, Msg::TabSwitch(99));
        assert_eq!(app.active_tab, 0);
    }

    #[test]
    fn tab_close_removes_tab() {
        let mut app = test_app();
        let rx2 = app.events_tx.subscribe();
        let h = std::env::temp_dir().join("phx-test-h3");
        app.tabs
            .push(crate::tui::tabs::Tab::new("t2".into(), rx2, h));
        assert_eq!(app.tabs.len(), 2);
        update(&mut app, Msg::TabClose(1));
        assert_eq!(app.tabs.len(), 1);
    }

    #[test]
    fn tab_close_index_zero_is_noop() {
        let mut app = test_app();
        update(&mut app, Msg::TabClose(0));
        assert_eq!(app.tabs.len(), 1);
    }

    // ── Focus ───────────────────────────────────────────────────────

    #[test]
    fn panel_focus_set() {
        let mut app = test_app();
        update(&mut app, Msg::PanelFocusSet(true));
        assert!(app.panel_focused);
        update(&mut app, Msg::PanelFocusSet(false));
        assert!(!app.panel_focused);
    }

    // ── Sidebar ─────────────────────────────────────────────────────

    #[test]
    fn sidebar_scroll_saturates_at_zero() {
        let mut app = test_app();
        app.sidebar_state.scroll = 0;
        update(&mut app, Msg::SidebarScrollUp);
        assert_eq!(app.sidebar_state.scroll, 0);
    }

    #[test]
    fn sidebar_scroll_down_increments() {
        let mut app = test_app();
        update(&mut app, Msg::SidebarScrollDown);
        assert_eq!(app.sidebar_state.scroll, 1);
    }

    // ── Picker ──────────────────────────────────────────────────────

    #[test]
    fn picker_clear_removes_picker() {
        let mut app = test_app();
        app.picker = Some(crate::tui::picker::PickerState::new(
            vec![crate::tui::picker::PickerItem {
                id: "a".into(),
                label: "test".into(),
                description: "".into(),
                source_tag: None,
            }],
            crate::tui::picker::PickerMode::Theme,
        ));
        update(&mut app, Msg::PickerClear);
        assert!(app.picker.is_none());
    }

    // ── Selection & hover ───────────────────────────────────────────

    #[test]
    fn selection_clear() {
        let mut app = test_app();
        app.selection = Some(crate::tui::selection::Selection {
            start_row: 0,
            start_col: 0,
            end_row: 5,
            end_col: 10,
            active: false,
        });
        update(&mut app, Msg::SelectionClear);
        assert!(app.selection.is_none());
    }

    #[test]
    fn hover_line_set_and_clear() {
        let mut app = test_app();
        update(&mut app, Msg::HoverLine(Some(42)));
        assert_eq!(app.hovered_line, Some(42));
        update(&mut app, Msg::HoverLine(None));
        assert_eq!(app.hovered_line, None);
    }

    // ── Modals ──────────────────────────────────────────────────────

    #[test]
    fn tool_form_dismiss_clears_form() {
        let mut app = test_app();
        app.tool_form = Some(crate::tui::ui::tool_form::ToolFormState::from_ui(
            "test".into(),
            "".into(),
            &crate::shared::ui_field_types::ToolUiConfig::new(vec![]),
        ));
        update(&mut app, Msg::ToolFormDismiss);
        assert!(app.tool_form.is_none());
    }

    #[test]
    fn models_page_dismiss() {
        let mut app = test_app();
        app.models_page = Some(crate::tui::models_page::ModelsPageState::new(&app.config));
        update(&mut app, Msg::ModelsPageDismiss);
        assert!(app.models_page.is_none());
    }

    #[test]
    fn onboarding_dismiss() {
        let mut app = test_app();
        app.onboarding = Some(crate::tui::onboarding::OnboardingState::new());
        update(&mut app, Msg::OnboardingDismiss);
        assert!(app.onboarding.is_none());
    }

    // ── Toast ───────────────────────────────────────────────────────

    #[test]
    fn toast_expire_clears() {
        let mut app = test_app();
        app.show_toast("hello");
        assert!(app.toast.is_some());
        update(&mut app, Msg::ToastExpire);
        assert!(app.toast.is_none());
    }

    // ── Quit ────────────────────────────────────────────────────────

    #[test]
    fn quit_sets_flag() {
        let mut app = test_app();
        update(&mut app, Msg::Quit);
        assert!(app.should_quit);
    }

    // ── InputSubmit ─────────────────────────────────────────────────

    #[test]
    fn input_submit_empty_is_noop() {
        let mut app = test_app();
        let cmd = update(&mut app, Msg::InputSubmit);
        assert!(matches!(cmd, Cmd::None));
    }

    #[test]
    fn input_submit_command_returns_run_command() {
        let mut app = test_app();
        if let Some(tab) = app.current_tab_mut() {
            tab.input.set_single_line("/help");
        }
        let cmd = update(&mut app, Msg::InputSubmit);
        assert!(matches!(cmd, Cmd::RunCommand(ref s) if s == "/help"));
    }

    #[test]
    fn input_submit_text_returns_start_conversation() {
        let mut app = test_app();
        if let Some(tab) = app.current_tab_mut() {
            tab.input.set_single_line("hello world");
        }
        let cmd = update(&mut app, Msg::InputSubmit);
        assert!(matches!(cmd, Cmd::StartConversation(ref s) if s == "hello world"));
    }

    // ── ConvEvent ───────────────────────────────────────────────────

    #[test]
    fn conv_stream_token_appends() {
        let mut app = test_app();
        update(
            &mut app,
            Msg::ConvStreamToken {
                tab_idx: 0,
                text: "hello".into(),
            },
        );
        assert_eq!(app.tabs[0].stream_buffer, "hello");
        update(
            &mut app,
            Msg::ConvStreamToken {
                tab_idx: 0,
                text: " world".into(),
            },
        );
        assert_eq!(app.tabs[0].stream_buffer, "hello world");
    }

    #[test]
    fn conv_assistant_message_clears_streaming() {
        let mut app = test_app();
        app.tabs[0].streaming_text = "partial".into();
        app.tabs[0].stream_buffer = "buf".into();
        update(
            &mut app,
            Msg::ConvAssistantMessage {
                tab_idx: 0,
                text: "full".into(),
            },
        );
        assert!(app.tabs[0].streaming_text.is_empty());
        assert!(app.tabs[0].stream_buffer.is_empty());
        assert!(!app.tabs[0].chat_lines.is_empty());
    }

    #[test]
    fn conv_error_clears_streaming() {
        let mut app = test_app();
        app.tabs[0].streaming_text = "partial".into();
        update(
            &mut app,
            Msg::ConvError {
                tab_idx: 0,
                message: "broke".into(),
            },
        );
        assert!(app.tabs[0].streaming_text.is_empty());
    }

    #[test]
    fn conv_event_invalid_tab_is_noop() {
        let mut app = test_app();
        update(
            &mut app,
            Msg::ConvStreamToken {
                tab_idx: 99,
                text: "ignored".into(),
            },
        );
        assert!(app.tabs[0].stream_buffer.is_empty());
    }

    // ── File viewer ─────────────────────────────────────────────────

    // ── Tool detail ──────────────────────────────────────────────────

    #[test]
    fn tool_detail_open_creates_viewer_tab() {
        let mut app = test_app();
        update(
            &mut app,
            Msg::ToolDetailOpen {
                tool_name: "bash".into(),
                content: "test output\nline 2".into(),
            },
        );
        assert!(app.file_viewer.is_viewing_file());
        let tab = app.file_viewer.active_tab().unwrap();
        assert!(tab.is_virtual);
        assert_eq!(tab.display_name, "⚙ bash");
        assert_eq!(tab.total_lines, 2);
    }

    #[test]
    fn tool_detail_open_reuses_same_tab() {
        let mut app = test_app();
        update(
            &mut app,
            Msg::ToolDetailOpen {
                tool_name: "bash".into(),
                content: "output 1".into(),
            },
        );
        update(
            &mut app,
            Msg::ToolDetailOpen {
                tool_name: "bash".into(),
                content: "output 2".into(),
            },
        );
        assert_eq!(app.file_viewer.tabs.len(), 1);
    }

    #[test]
    fn tool_detail_open_different_tools_create_separate_tabs() {
        let mut app = test_app();
        update(
            &mut app,
            Msg::ToolDetailOpen {
                tool_name: "bash".into(),
                content: "output".into(),
            },
        );
        update(
            &mut app,
            Msg::ToolDetailOpen {
                tool_name: "read".into(),
                content: "file content".into(),
            },
        );
        assert_eq!(app.file_viewer.tabs.len(), 2);
    }

    // ── File viewer ─────────────────────────────────────────────────

    #[test]
    fn file_viewer_hover_close() {
        let mut app = test_app();
        update(&mut app, Msg::FileViewerHoverClose(Some(2)));
        assert_eq!(app.file_viewer.hovered_close, Some(2));
        update(&mut app, Msg::FileViewerHoverClose(None));
        assert_eq!(app.file_viewer.hovered_close, None);
    }
}

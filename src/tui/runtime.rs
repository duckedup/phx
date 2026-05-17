use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self as ct_event, Event as CEvent, KeyCode, KeyModifiers};
use ratatui::prelude::*;

use crate::config::schema::Config;
use crate::plugin::plugin_runtime::PluginRuntime;
use crate::tui::app::{App, command_source_tag};
use crate::tui::components::{modal_picker, sidebar};
use crate::tui::file_viewer;
use crate::tui::layout::{self, padded_chat_area};
use crate::tui::msg::Msg;
use crate::tui::picker::{PickerItem, PickerMode};
use crate::tui::selection::Selection;
use crate::tui::tabs::Tab;
use crate::tui::ui::tool_form;

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
            app.sidebar_area = crate::tui::app::agent_panel_rect(
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
            crate::tui::update::update(app, Msg::ToastExpire);
        }
        app.recompute_display_lines(app.chat_area.width, app.chat_area.width);

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
                            crate::tui::update::update(app, Msg::TabSwitch(0));
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
                                crate::tui::update::update(app, Msg::PickerClear);
                                crate::tui::commands::handle_command(app, input_text.trim()).await;
                            } else {
                                crate::tui::conversation::start_conversation(app, input_text);
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
                            crate::tui::update::update(app, Msg::SidebarScrollUp);
                        }
                        MouseEventKind::ScrollDown if in_panel => {
                            crate::tui::update::update(app, Msg::SidebarScrollDown);
                        }
                        MouseEventKind::ScrollUp if app.file_viewer.is_viewing_file() => {
                            crate::tui::update::update(app, Msg::FileViewerScrollUp(3));
                        }
                        MouseEventKind::ScrollDown if app.file_viewer.is_viewing_file() => {
                            crate::tui::update::update(app, Msg::FileViewerScrollDown(3));
                        }
                        MouseEventKind::ScrollUp => {
                            crate::tui::update::update(app, crate::tui::msg::Msg::ScrollUp(1));
                        }
                        MouseEventKind::ScrollDown => {
                            crate::tui::update::update(app, crate::tui::msg::Msg::ScrollDown(1));
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
                                        crate::tui::update::update(
                                            app,
                                            Msg::FileViewerSwitchToChat,
                                        );
                                    }
                                    file_viewer::TabBarHit::FileTab(idx) => {
                                        crate::tui::update::update(
                                            app,
                                            Msg::FileViewerSwitchTab(idx),
                                        );
                                    }
                                    file_viewer::TabBarHit::CloseTab(idx) => {
                                        crate::tui::update::update(
                                            app,
                                            Msg::FileViewerCloseTab(idx),
                                        );
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
                                        crate::tui::update::update(
                                            app,
                                            Msg::FileViewerOpenFile(path.clone()),
                                        );
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
                                        crate::tui::update::update(
                                            app,
                                            Msg::SidebarDismissAgent(id),
                                        );
                                    }
                                    sidebar::HitResult::Select(sel) => {
                                        crate::tui::update::update(app, Msg::SidebarSelect(sel));
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
                            let hover_close = if let Some(tb_area) = app.tab_bar_area {
                                if let Some(file_viewer::TabBarHit::CloseTab(idx)) =
                                    file_viewer::tab_bar_hit_test(
                                        tb_area,
                                        mouse.row,
                                        mouse.column,
                                        &app.file_viewer,
                                    )
                                {
                                    Some(idx)
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            crate::tui::update::update(app, Msg::FileViewerHoverClose(hover_close));

                            let hovered = if !app.file_viewer.is_viewing_file() {
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
                                        Some(line_idx)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            crate::tui::update::update(app, Msg::HoverLine(hovered));
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
                crate::tui::reload::apply_reload(app, output);
            }
            app.is_reloading = false;
        }

        if let Some(session_id) = app.pending_session_resume.take() {
            crate::tui::conversation::resume_session(app, &session_id).await;
        }

        if let Some(text) = app.pending_skill_message.take() {
            crate::tui::conversation::start_conversation(app, text);
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

pub fn redraw(app: &mut App, terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) {
    if let Some(agents) = app.session_pool.try_check() {
        app.sidebar_state.update(agents);
    }
    let sz = terminal.size().unwrap_or_default();
    let sz_rect = Rect::new(0, 0, sz.width, sz.height);
    let input_lines = app
        .current_tab()
        .map(|t| t.input.line_count() as u16)
        .unwrap_or(1);
    let content_area = if app.show_sidebar() {
        layout::split_sidebar(sz_rect).1
    } else {
        sz_rect
    };
    let chunks = layout::main_layout(content_area, input_lines);
    let padded = layout::padded_chat_area(chunks[0]);
    app.chat_area_height = padded.height;
    app.chat_area = padded;
    app.frame_tick = app.frame_tick.wrapping_add(1);
    app.recompute_display_lines(padded.width, padded.width);
    let _ = terminal.draw(|f| app.render(f));
}

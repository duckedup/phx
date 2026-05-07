use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event as CEvent, KeyCode, KeyModifiers};
use ratatui::prelude::*;

use crate::session::agent_loop::Session;
use crate::session::message::Message;
use crate::store::session_store::SessionId;
use crate::tui::app::App;
use crate::tui::layout;
use crate::tui::rendering::helpers::tool_call_summary;
use crate::tui::tabs::ChatLine;

pub fn default_system_prompt() -> &'static str {
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

pub async fn resume_session(app: &mut App, session_id: &str) {
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
            app.session = Some(session);
            app.show_toast(format!("Resumed session ({msg_count} messages)"));
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

pub fn handle_command(app: &mut App, input: &str) {
    let skills = crate::session::skills::discover_layered(
        Some(&app.project),
        &crate::config::paths::user_home(),
        &app.config.skills.dirs,
    );
    let result = crate::commands::dispatcher::dispatch_with_plugins(
        input,
        &app.config,
        &skills,
        &app.store,
        &app.project,
        Some(&app.plugin_manager),
    );

    use crate::tui::picker::{PickerItem, PickerMode, PickerState};
    use crate::tui::theme;

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
            app.onboarding = Some(crate::tui::onboarding::OnboardingState::new());
        }
        crate::commands::CommandResult::PluginCommand {
            plugin_command,
            args,
        } => {
            if let Some(handle) = app.plugin_manager.get_command_handler(&plugin_command) {
                let result =
                    futures::executor::block_on(handle.execute_command(&plugin_command, &args));
                match result {
                    Ok(value) => {
                        let msg = value
                            .get("text")
                            .or_else(|| value.get("message"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| value.to_string());
                        if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                            tab.chat_lines.push(ChatLine {
                                role: crate::session::message::Role::System,
                                content: msg,
                            });
                        }
                    }
                    Err(e) => {
                        if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                            tab.chat_lines.push(ChatLine {
                                role: crate::session::message::Role::System,
                                content: format!("Plugin error: {e}"),
                            });
                        }
                    }
                }
            }
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

pub async fn send_message(
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

        let mut tick = tokio::time::interval(Duration::from_millis(16));
        let send_fut = provider.send(opts);
        futures::pin_mut!(send_fut);

        let mut cancelled = false;
        let stream = loop {
            tokio::select! {
                result = &mut send_fut => {
                    match result {
                        Ok(s) => break s,
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
                    }
                }
                maybe_term = term_events.next() => {
                    if let Some(Ok(CEvent::Key(key))) = maybe_term
                        && key.code == KeyCode::Char('c')
                            && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        cancelled = true;
                        break futures::stream::empty().boxed();
                    }
                }
                _ = tick.tick() => {
                    redraw(app, terminal);
                }
            }
        };

        if cancelled {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "Cancelled.".into(),
                });
            }
            app.is_running = false;
            app.session = Some(session);
            return;
        }

        futures::pin_mut!(stream);

        let mut assistant_text = String::new();
        let mut pending_tool_calls: Vec<crate::session::message::ToolCall> = vec![];
        let mut got_tool_use_stop = false;

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
                            session.last_turn_input = usage.input_tokens;
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
                    if let Some(tab) = app.tabs.get_mut(app.active_tab)
                        && !tab.stream_buffer.is_empty()
                    {
                        crate::tui::rendering::helpers::drain_stream_buffer(tab);
                    }
                    redraw(app, terminal);
                }
            }
        }

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

                use crate::plugin::{HookAction, HookEvent};
                let hook_data = serde_json::json!({
                    "name": tc.name,
                    "args": serde_json::from_str::<serde_json::Value>(&tc.args_json).unwrap_or_default(),
                    "call_id": tc.id,
                });
                let hook_action = app
                    .plugin_manager
                    .hooks
                    .hook(HookEvent::ToolCallStart, hook_data)
                    .await;

                let tr = match hook_action {
                    HookAction::Block { reason } => crate::session::message::ToolResult {
                        id: tc.id.clone(),
                        output: format!("blocked by plugin: {reason}"),
                        is_error: true,
                    },
                    _ => {
                        if let Some(tool) = app.tools.get(&tc.name) {
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
                        }
                    }
                };

                app.plugin_manager
                    .hooks
                    .notify(
                        HookEvent::ToolCallEnd,
                        serde_json::json!({
                            "call_id": tc.id,
                            "name": tc.name,
                            "output": tr.output,
                            "is_error": tr.is_error,
                        }),
                    )
                    .await;

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

pub fn redraw(app: &mut App, terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>) {
    let sz = terminal.size().unwrap_or_default();
    let sz_rect = Rect::new(0, 0, sz.width, sz.height);
    let input_lines = app
        .current_tab()
        .map(|t| t.input.lines.len() as u16)
        .unwrap_or(1);
    let chunks = layout::main_layout(sz_rect, input_lines);
    app.chat_area_height = chunks[0].height;
    app.frame_tick = app.frame_tick.wrapping_add(1);
    app.recompute_display_lines(sz.width);
    let _ = terminal.draw(|f| app.render(f));
}

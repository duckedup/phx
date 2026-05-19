use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event as CEvent, KeyCode, KeyModifiers};
use ratatui::prelude::*;

use crate::session::agent_loop::Session;
use crate::session::message::Message;
use crate::store::session_store::SessionId;
use crate::tui::app::App;
use crate::tui::rendering::helpers::tool_call_summary;
use crate::tui::tabs::{AssistantLine, ChatItem, ChatLine};

pub fn start_conversation(app: &mut App, text: String) {
    let provider = match &app.provider {
        Some(p) => Arc::clone(p),
        None => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "No provider configured. Use /connect to add one.".into(),
                }));
            }
            return;
        }
    };

    let session = app.session.take().unwrap_or_else(|| {
        let mut s = Session::new(SessionId::new(), crate::config::SessionProfile::default());
        if let Some((name, profile)) = crate::config::loader::active_provider(&app.config) {
            s.provider_name = name.to_string();
            s.model_name = profile.model.clone();
        }
        s
    });

    if let Some(tab) = app.current_tab_mut() {
        tab.add_user_message(text.clone());
    }

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let tool_router = crate::session::tool_router::ToolRouter::from_config(&app.config);
    let cfg = crate::session::conversation::ConvParams {
        provider,
        tools: app.tools.clone(),
        store: app.store.clone(),
        project: app.project.clone(),
        config: app.config.clone(),
        system_prompt_override: None,
        plugin_runtime: app.plugin_runtime.clone(),
        tool_router,
    };

    let rx =
        crate::session::conversation::spawn_conversation(session, text, cfg, Arc::clone(&cancel));

    app.is_running = true;
    app.agent_receivers.push(crate::tui::app::AgentReceiver {
        tab_index: app.active_tab,
        session_id: None,
        rx,
        cancel: Some(cancel),
    });
}

pub fn default_system_prompt() -> &'static str {
    "You are phx, a fast and capable coding assistant running in a terminal.\n\
     \n\
     You have access to tools for reading files, writing files, editing files, \
     running shell commands, and spawning sub-agents. Use them to help the user \
     with software engineering tasks.\n\
     \n\
     Guidelines:\n\
     - Be concise. The user is in a terminal — respect their screen space.\n\
     - When editing code, preserve existing style and conventions.\n\
     - Prefer editing existing files over creating new ones.\n\
     - Use the bash tool for commands; use read/write/edit tools for files.\n\
     - Show your work: explain what you're doing briefly, then do it.\n\
     - If a task is ambiguous, make a reasonable assumption and proceed.\n\
     - When you encounter errors, diagnose the root cause before retrying.\n\
     - For large tasks, use spawn_agent to delegate sub-tasks to parallel agents.\n\
     - Use check_agents to see live status. Use collect_agent to get results when done.\n\
     - Use merge_agent to merge a completed agent's worktree branch back."
}

pub async fn resume_session(app: &mut App, session_id: &str) {
    let sid = SessionId::from(session_id.to_string());
    match app.store.load_messages(&app.project, &sid).await {
        Ok(raw_messages) => {
            let mut session = Session::new(sid, crate::config::SessionProfile::default());
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
                        crate::session::message::Role::User => {
                            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                tab.chat_lines.push(ChatItem::Line(ChatLine {
                                    role: msg.role.clone(),
                                    content: msg.content.clone(),
                                }));
                            }
                        }
                        crate::session::message::Role::Assistant => {
                            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                tab.chat_lines.push(ChatItem::Assistant(AssistantLine {
                                    content: msg.content.clone(),
                                    turn: 0,
                                }));
                            }
                        }
                        crate::session::message::Role::ToolCall => {
                            if let Some(tc) = &msg.tool_call {
                                let summary = tool_call_summary(&tc.name, &tc.args_json);
                                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                    tab.chat_lines.push(ChatItem::Line(ChatLine {
                                        role: crate::session::message::Role::ToolCall,
                                        content: summary,
                                    }));
                                }
                            }
                        }
                        crate::session::message::Role::ToolResult => {
                            if let Some(tr) = &msg.tool_result {
                                let output = crate::tui::rendering::helpers::truncate_output(
                                    &tr.output, 2000,
                                );
                                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                    tab.chat_lines.push(ChatItem::Line(ChatLine {
                                        role: crate::session::message::Role::ToolResult,
                                        content: output,
                                    }));
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
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Failed to resume: {e}"),
                }));
            }
        }
    }
}

pub async fn activate_conductor(app: &mut App) {
    toggle_conductor_mode(app, true);
}

pub async fn deactivate_conductor_mode(app: &mut App) {
    toggle_conductor_mode(app, false);
}

pub fn toggle_conductor_mode(app: &mut App, activate: bool) {
    app.conductor_mode = activate;

    if activate {
        *app.orch_ctx.config.write() = app.config.clone();
        let parent_provider = crate::config::loader::active_provider(&app.config)
            .map(|(name, _)| name.to_string())
            .unwrap_or_default();
        *app.orch_ctx.parent_provider.write() = parent_provider;
        *app.orch_ctx.parent_tools.write() = app.tools.read().clone();

        app.show_toast("Conductor mode");
    } else {
        let running_count = app
            .session_pool
            .try_check()
            .map(|agents| {
                agents
                    .iter()
                    .filter(|a| {
                        a.status == crate::session::orchestration::ChildStatus::Running
                            || a.status == crate::session::orchestration::ChildStatus::Queued
                    })
                    .count()
            })
            .unwrap_or(0);

        if running_count > 0 {
            app.show_toast(format!(
                "Solo mode ({running_count} agent{} still running)",
                if running_count == 1 { "" } else { "s" }
            ));
        } else {
            app.sidebar_area = None;
            app.show_toast("Solo mode");
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
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "No provider configured. Use /connect to add one.".into(),
                }));
            }
            return;
        }
    };

    let mut session = app.session.take().unwrap_or_else(|| {
        let mut s = Session::new(SessionId::new(), crate::config::SessionProfile::default());
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

    crate::tui::runtime::redraw(app, terminal);

    use crate::providers::traits::{
        Event, ProviderMessage, ProviderRole, ProviderToolCall, ProviderToolResult, SendOptions,
        StopReason, ToolSchema,
    };
    use crossterm::event::EventStream;
    use futures::StreamExt;

    let mut term_events = EventStream::new();

    let cached_tool_schemas: Vec<ToolSchema> = app
        .tools
        .read()
        .list_schemas()
        .into_iter()
        .map(|s| ToolSchema {
            name: s.name.to_string(),
            description: s.description.to_string(),
            parameters: s.parameters,
        })
        .collect();

    loop {
        session.turn_count += 1;

        let tool_schemas = cached_tool_schemas.clone();

        let base_prompt = match &session.profile.system_prompt_path {
            Some(p) => tokio::fs::read_to_string(p).await.ok(),
            None => None,
        }
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

        let mut system_prompt: Vec<String> = Vec::new();
        if let Some(base) = base_prompt {
            system_prompt.push(base);
        }
        if !ctx.system_prompt_suffix.is_empty() {
            system_prompt.push(ctx.system_prompt_suffix);
        }

        if !ctx.newly_loaded.is_empty() {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: crate::tui::rendering::helpers::format_context_tree(&ctx.newly_loaded),
                }));
            }
            crate::tui::runtime::redraw(app, terminal);
        }

        let active_provider_profile = crate::config::loader::active_provider(&app.config)
            .map(|(_, p)| p.clone())
            .unwrap_or_default();
        let limits = crate::session::context::resolve_context_limits(
            &session.model_name,
            &active_provider_profile,
            &session.profile,
        );
        let prompt_combined = system_prompt.join("\n\n");
        let prompt_ref = prompt_combined.as_str();
        let compaction =
            crate::session::context::enforce_limits(&mut session.messages, prompt_ref, &limits);
        if compaction.was_compacted {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!(
                        "Context compacted: removed {} messages ({} remaining) to stay within {} token limit",
                        compaction.removed_count,
                        compaction.remaining_count,
                        limits.context_window,
                    ),
                }));
            }
            crate::tui::runtime::redraw(app, terminal);
        }

        // TODO: compression disabled pending settings support
        // let compressed = crate::session::compress::compress_for_provider(&session.messages);
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
                                tab.chat_lines.push(ChatItem::Line(ChatLine {
                                    role: crate::session::message::Role::System,
                                    content: format!("Provider error: {e}"),
                                }));
                            }
                            app.is_running = false;
                            app.session = Some(session);
                            return;
                        }
                    }
                }
                maybe_term = term_events.next() => {
                    match maybe_term {
                        Some(Ok(CEvent::Key(key)))
                            if key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            app.handle_key(key).await;
                            cancelled = true;
                            break futures::stream::empty().boxed();
                        }
                        Some(Ok(CEvent::Key(key)))
                            if key.code == KeyCode::Esc =>
                        {
                            cancelled = true;
                            break futures::stream::empty().boxed();
                        }
                        Some(Ok(CEvent::Mouse(mouse))) => {
                            crate::tui::event_handler::handle_sidebar_click(app, mouse);
                        }
                        _ => {}
                    }
                }
                _ = tick.tick() => {
                    crate::tui::runtime::redraw(app, terminal);
                }
            }
        };

        if cancelled {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "Cancelled.".into(),
                }));
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
                            session.last_turn_input = usage.input_tokens
                                + usage.cache_read_tokens
                                + usage.cache_creation_tokens;
                            if stop_reason == StopReason::ToolUse {
                                got_tool_use_stop = true;
                            }
                            break;
                        }
                        Some(Event::Error(e)) => {
                            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                tab.streaming_text.clear();
                                tab.stream_buffer.clear();
                                tab.chat_lines.push(ChatItem::Line(ChatLine {
                                    role: crate::session::message::Role::System,
                                    content: format!("Error: {e}"),
                                }));
                            }
                            app.is_running = false;
                            app.session = Some(session);
                            return;
                        }
                        None => break,
                    }
                }
                maybe_term = term_events.next() => {
                    match maybe_term {
                        Some(Ok(CEvent::Key(key)))
                            if key.code == KeyCode::Char('c')
                                && key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            app.handle_key(key).await;
                            cancelled = true;
                            break;
                        }
                        Some(Ok(CEvent::Key(key)))
                            if key.code == KeyCode::Esc =>
                        {
                            cancelled = true;
                            break;
                        }
                        Some(Ok(CEvent::Mouse(mouse))) => {
                            crate::tui::event_handler::handle_sidebar_click(app, mouse);
                        }
                        _ => {}
                    }
                }
                _ = tick.tick() => {
                    if let Some(tab) = app.tabs.get_mut(app.active_tab)
                        && !tab.stream_buffer.is_empty()
                    {
                        crate::tui::rendering::helpers::drain_stream_buffer(tab);
                    }
                    crate::tui::runtime::redraw(app, terminal);
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
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "Cancelled.".into(),
                }));
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
                tab.chat_lines.push(ChatItem::Assistant(AssistantLine {
                    content: text,
                    turn: session.turn_count,
                }));
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
                    tab.chat_lines.push(ChatItem::Line(ChatLine {
                        role: crate::session::message::Role::ToolCall,
                        content: summary,
                    }));
                }

                crate::tui::event_handler::drain_pending_events(app);
                crate::tui::runtime::redraw(app, terminal);

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
                        let args: serde_json::Value =
                            serde_json::from_str(&tc.args_json).unwrap_or_default();

                        let dynamic_ui_info = app
                            .plugin_runtime
                            .as_ref()
                            .and_then(|rt| rt.lock().dynamic_ui_info(&tc.name));
                        let dynamic_ui =
                            if let Some((path, bin_args, project_dir)) = dynamic_ui_info {
                                crate::plugin::plugin_runtime::request_dynamic_ui_async(
                                    &path,
                                    &bin_args,
                                    &tc.name,
                                    &tc.args_json,
                                    &project_dir,
                                )
                                .await
                            } else {
                                None
                            };

                        if let Some(fields) = dynamic_ui {
                            let config = crate::shared::ui_field_types::ToolUiConfig::new(fields);
                            let form_state = crate::tui::ui::tool_form::ToolFormState::from_ui(
                                tc.name.clone(),
                                String::new(),
                                &config,
                            );
                            match run_interactive_form(app, terminal, form_state).await {
                                Some(state) => {
                                    let answers = crate::tui::ui::tool_form::format_answers(&state);
                                    crate::session::message::ToolResult {
                                        id: tc.id.clone(),
                                        output: answers,
                                        is_error: false,
                                    }
                                }
                                None => crate::session::message::ToolResult {
                                    id: tc.id.clone(),
                                    output: "User cancelled.".into(),
                                    is_error: true,
                                },
                            }
                        } else {
                            let maybe_tool = app.tools.read().get(&tc.name);
                            if let Some(tool) = maybe_tool {
                                let noop = crate::tools::traits::NoopInputRequester;
                                match tool.invoke(args, &noop).await {
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
                    let output = crate::tui::rendering::helpers::truncate_output(&tr.output, 2000);
                    tab.chat_lines.push(ChatItem::Line(ChatLine {
                        role: crate::session::message::Role::ToolResult,
                        content: output,
                    }));
                }

                crate::tui::event_handler::drain_pending_events(app);
                crate::tui::runtime::redraw(app, terminal);
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

async fn run_interactive_form(
    app: &mut App,
    terminal: &mut ratatui::Terminal<ratatui::prelude::CrosstermBackend<std::io::Stdout>>,
    form_state: crate::tui::ui::tool_form::ToolFormState,
) -> Option<crate::tui::ui::tool_form::ToolFormState> {
    use crate::tui::ui::tool_form;
    use crossterm::event::EventStream;
    use futures::StreamExt;

    app.tool_form = Some(form_state);
    crate::tui::runtime::redraw(app, terminal);

    let mut term_events = EventStream::new();
    let result = loop {
        let maybe = term_events.next().await;
        match maybe {
            Some(Ok(crossterm::event::Event::Key(key))) => {
                if let Some(ref mut form) = app.tool_form {
                    let action = tool_form::handle_key(form, key);
                    match action {
                        tool_form::FormAction::Submit(_) => {
                            break Some(());
                        }
                        tool_form::FormAction::Cancel => {
                            break None;
                        }
                        tool_form::FormAction::None => {}
                    }
                }
            }
            Some(Ok(crossterm::event::Event::Paste(text))) => {
                if let Some(ref mut form) = app.tool_form {
                    tool_form::handle_paste(form, &text);
                }
            }
            Some(Ok(crossterm::event::Event::Mouse(mouse))) => {
                crate::tui::event_handler::handle_sidebar_click(app, mouse);
            }
            _ => {}
        }
        crate::tui::runtime::redraw(app, terminal);
    };

    let form = app.tool_form.take();
    crate::tui::runtime::redraw(app, terminal);

    match result {
        Some(()) => form,
        None => None,
    }
}

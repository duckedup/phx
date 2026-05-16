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

    if let Some(tab) = app.current_tab_mut() {
        tab.add_user_message(text.clone());
    }

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let tool_router = crate::session::tool_router::ToolRouter::from_config(&app.config);
    let cfg = crate::session::conversation::ConvConfig {
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

pub async fn handle_command(app: &mut App, input: &str) {
    let skills = crate::session::skills::discover_layered(
        Some(&app.project),
        &crate::config::paths::user_home(),
        &app.config.skills.dirs,
    );
    #[allow(clippy::await_holding_lock)]
    let result = {
        let rt_guard = app.plugin_runtime.as_ref().map(|rt| rt.lock());
        let r = crate::commands::dispatcher::dispatch_with_plugins(
            input,
            &app.config,
            &skills,
            &app.store,
            &app.project,
            Some(&app.plugin_manager),
            rt_guard.as_deref(),
        )
        .await;
        drop(rt_guard);
        r
    };

    use crate::tui::picker::{PickerItem, PickerMode, PickerState};
    use crate::tui::theme;

    match result {
        crate::commands::CommandResult::Message(msg) => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: msg,
                }));
            }
        }
        crate::commands::CommandResult::Error(err) => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Error: {err}"),
                }));
            }
        }
        crate::commands::CommandResult::ClearSession => {
            app.session = None;
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.clear();
                tab.streaming_text.clear();
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "Session cleared.".into(),
                }));
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
                        tab.chat_lines.push(ChatItem::Line(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Skill '{name}' already loaded in this session."),
                        }));
                    }
                } else {
                    session.add_message(Message::system(&content));
                    let preview = if content.chars().count() > 80 {
                        let truncated: String = content.chars().take(77).collect();
                        format!("{truncated}...")
                    } else {
                        content.clone()
                    };
                    if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                        tab.chat_lines.push(ChatItem::Line(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Skill loaded: {preview}"),
                        }));
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
                    tab.chat_lines.push(ChatItem::Line(ChatLine {
                        role: crate::session::message::Role::System,
                        content: format!("Theme: {}", entry.name),
                    }));
                }
            } else {
                app.saved_theme = Some(app.theme.clone());
                let items: Vec<PickerItem> = themes
                    .iter()
                    .map(|t| PickerItem {
                        id: t.id.clone(),
                        label: t.name.clone(),
                        description: String::new(),
                        source_tag: None,
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
                        source_tag: None,
                    })
                    .collect();
                app.model_choices = choices;
                app.picker = Some(PickerState::new(items, PickerMode::Model));
            }
        }
        crate::commands::CommandResult::ModelsPage => {
            app.models_page = Some(crate::tui::models_page::ModelsPageState::new(&app.config));
        }
        crate::commands::CommandResult::SessionPicker(choices) => {
            if choices.is_empty() {
                if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                    tab.chat_lines.push(ChatItem::Line(ChatLine {
                        role: crate::session::message::Role::System,
                        content: "No sessions to resume.".into(),
                    }));
                }
            } else {
                let items: Vec<PickerItem> = choices
                    .iter()
                    .map(|c| PickerItem {
                        id: c.id.clone(),
                        label: c.display_name.clone(),
                        source_tag: None,
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
                        tab.chat_lines.push(ChatItem::Line(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!(
                                "Compacted session: removed {} messages ({before} → {})",
                                result.removed_count, result.remaining_count
                            ),
                        }));
                    } else {
                        tab.chat_lines.push(ChatItem::Line(ChatLine {
                            role: crate::session::message::Role::System,
                            content: format!("Session has {before} messages — too few to compact."),
                        }));
                    }
                }
            } else if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: "No active session to compact.".into(),
                }));
            }
        }
        crate::commands::CommandResult::ConnectWizard => {
            app.onboarding = Some(crate::tui::onboarding::OnboardingState::new());
        }
        crate::commands::CommandResult::Route(result) => {
            use crate::commands::route::{RouteResult, apply};
            match &result {
                RouteResult::Table(entries) => {
                    if entries.is_empty() {
                        app.show_toast("No tool routes configured");
                    } else {
                        let lines: Vec<String> = entries
                            .iter()
                            .map(|e| {
                                if let Some(ref m) = e.model {
                                    format!("  {} → {}/{}", e.tool, e.provider, m)
                                } else {
                                    format!("  {} → {}", e.tool, e.provider)
                                }
                            })
                            .collect();
                        if let Some(tab) = app.current_tab_mut() {
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::System,
                                content: format!("Tool routes:\n{}", lines.join("\n")),
                            }));
                        }
                    }
                }
                RouteResult::Error(msg) => {
                    app.show_toast(msg.clone());
                }
                _ => {
                    if let Some(toast_msg) = apply(&result, &mut app.config) {
                        let config_path = crate::config::paths::user_config_file();
                        let _ = crate::config::writer::save_tool_routing(
                            &config_path,
                            &app.config.tool_routing,
                        );
                        app.show_toast(toast_msg);
                    }
                }
            }
        }
        crate::commands::CommandResult::Conductor => {
            handle_conductor_command(app).await;
        }
        crate::commands::CommandResult::Solo => {
            if app.conductor_mode {
                deactivate_conductor_mode(app).await;
            } else {
                app.show_toast("Already in solo mode");
            }
        }
        crate::commands::CommandResult::PluginCommand {
            plugin_command,
            args,
        } => {
            if let Some(handle) = app.plugin_manager.get_command_handler(&plugin_command) {
                let result = handle.execute_command(&plugin_command, &args).await;
                match result {
                    Ok(value) => {
                        let msg = value
                            .get("text")
                            .or_else(|| value.get("message"))
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| value.to_string());
                        if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::System,
                                content: msg,
                            }));
                        }
                    }
                    Err(e) => {
                        if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                            tab.chat_lines.push(ChatItem::Line(ChatLine {
                                role: crate::session::message::Role::System,
                                content: format!("Plugin error: {e}"),
                            }));
                        }
                    }
                }
            }
        }
        crate::commands::CommandResult::PluginToolCommand { command, args } => {
            if let Some(rt) = app.plugin_runtime.as_ref().map(Arc::clone) {
                let (ui_config, description) = {
                    let rt_guard = rt.lock();
                    let ui = rt_guard.tool_ui(&command).cloned();
                    let desc = rt_guard
                        .command_tools()
                        .iter()
                        .find(|t| t.command == command)
                        .map(|t| t.description.clone())
                        .unwrap_or_default();
                    (ui, desc)
                };

                if let Some(config) = ui_config
                    && args.is_empty()
                {
                    app.tool_form = Some(crate::tui::ui::tool_form::ToolFormState::from_ui(
                        command,
                        description,
                        &config,
                    ));
                } else {
                    let args_json = if args.is_empty() {
                        "{}".to_string()
                    } else {
                        serde_json::json!({"arguments": args}).to_string()
                    };
                    let toggle_info = rt.lock().prepare_toggle(&command);
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
                                    &args_json,
                                    &project_dir,
                                )
                                .await
                            };
                            match result {
                                Ok(result) => {
                                    app.apply_tool_result(result);
                                }
                                Err(e) => {
                                    if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                        tab.chat_lines.push(ChatItem::Line(ChatLine {
                                            role: crate::session::message::Role::System,
                                            content: format!("Plugin tool error: {e}"),
                                        }));
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                                tab.chat_lines.push(ChatItem::Line(ChatLine {
                                    role: crate::session::message::Role::System,
                                    content: format!("Plugin tool error: {e}"),
                                }));
                            }
                        }
                    }
                }
            }
        }
        crate::commands::CommandResult::ReloadPlugins => {
            if app.is_reloading {
                return;
            }
            app.is_reloading = true;

            if let Some(rt_arc) = app.plugin_runtime.clone() {
                use crate::plugin::plugin_runtime::PluginRuntime;

                let mut plugin_dirs = PluginRuntime::discover_dirs(
                    Some(&app.project),
                    &crate::config::paths::user_home(),
                );
                let project_plugin_dir = app.project.join(".phx/plugins");
                if !plugin_dirs.contains(&project_plugin_dir) {
                    plugin_dirs.push(project_plugin_dir);
                }
                let mut source_dirs = PluginRuntime::discover_source_dirs(Some(&app.project));
                crate::tui::app::resolve_extra_plugin_dirs(
                    &app.extra_plugin_dirs,
                    &app.project,
                    &mut source_dirs,
                );

                // Reload uses sync subprocess calls (cargo build) — run on blocking pool
                let handle = tokio::task::spawn_blocking(move || {
                    let result = rt_arc.lock().reload(&plugin_dirs, &source_dirs);
                    crate::tui::app::ReloadOutput {
                        plugin_result: Some(result),
                    }
                });
                app.reload_task = Some(handle);
            } else {
                app.is_reloading = false;
                app.show_toast("Plugin runtime not available.".to_string());
            }
        }
        crate::commands::CommandResult::ContextInfo => {
            let builtin_tools: std::collections::HashSet<&str> = [
                "bash",
                "read",
                "write",
                "edit",
                "spawn_agent",
                "check_agents",
                "collect_agent",
                "cancel_agent",
                "merge_agent",
            ]
            .into();

            let mut lines = Vec::new();
            let mut schemas = app.tools.read().list_schemas();
            schemas.sort_by(|a, b| a.name.cmp(&b.name));

            let (core_tools, plugin_tools): (Vec<_>, Vec<_>) = schemas
                .iter()
                .partition(|s| builtin_tools.contains(s.name.as_str()));

            lines.push("### Tools".to_string());
            lines.push(String::new());
            for schema in &core_tools {
                format_bullet(&mut lines, &schema.name, &schema.description, "");
            }

            if !plugin_tools.is_empty() {
                lines.push(String::new());
                lines.push("### Plugin tools".to_string());
                lines.push(String::new());
                for schema in &plugin_tools {
                    format_bullet(&mut lines, &schema.name, &schema.description, "");
                }
            }

            let skills = crate::session::skills::discover_layered(
                Some(&app.project),
                &crate::config::paths::user_home(),
                &app.config.skills.dirs,
            );
            if !skills.is_empty() {
                lines.push(String::new());
                lines.push("### Skills".to_string());
                lines.push(String::new());
                for skill in &skills {
                    let tag = if skill.is_tool { " [tool]" } else { "" };
                    format_bullet(&mut lines, &skill.name, &skill.description, tag);
                }
            }

            {
                let rt_guard2 = app.plugin_runtime.as_ref().map(|rt| rt.lock());
                let plugin_commands: Vec<(&str, &str)> = rt_guard2
                    .as_ref()
                    .map(|rt| rt.commands())
                    .unwrap_or_default();
                let process_cmds = app.plugin_manager.plugin_commands();

                if !plugin_commands.is_empty() || !process_cmds.is_empty() {
                    lines.push(String::new());
                    lines.push("### Plugin commands".to_string());
                    lines.push(String::new());
                    for (name, desc) in &plugin_commands {
                        format_bullet(&mut lines, name, desc, "");
                    }
                    for (name, summary) in &process_cmds {
                        format_bullet(&mut lines, name, summary, "");
                    }
                }
            }

            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Assistant(AssistantLine {
                    content: lines.join("\n"),
                    turn: 0,
                }));
            }
        }
        other => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("{other:?}"),
                }));
            }
        }
    }
}

async fn handle_conductor_command(app: &mut App) {
    if app.conductor_mode {
        deactivate_conductor_mode(app).await;
    } else {
        activate_conductor(app).await;
    }
}

pub async fn activate_conductor(app: &mut App) {
    toggle_conductor_mode(app, true);
}

async fn deactivate_conductor_mode(app: &mut App) {
    toggle_conductor_mode(app, false);
}

fn toggle_conductor_mode(app: &mut App, activate: bool) {
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
        session.turn_count += 1;

        let tool_schemas: Vec<ToolSchema> = app
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

        let system_prompt = base_prompt.map(|base| {
            if ctx.system_prompt_suffix.is_empty() {
                base
            } else {
                format!("{base}\n\n{}", ctx.system_prompt_suffix)
            }
        });

        if !ctx.newly_loaded.is_empty() {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: crate::tui::rendering::helpers::format_context_tree(&ctx.newly_loaded),
                }));
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
                            handle_sidebar_click(app, mouse);
                        }
                        _ => {}
                    }
                }
                _ = tick.tick() => {
                    redraw(app, terminal);
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
                            handle_sidebar_click(app, mouse);
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

                drain_pending_events(app);
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

                drain_pending_events(app);
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

fn drain_pending_events(app: &mut App) {
    use crossterm::event::{self as ct_event, Event as CEvent};
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

fn handle_sidebar_click(app: &mut App, mouse: crossterm::event::MouseEvent) {
    use crate::tui::components::sidebar::SidebarSelection;
    use crossterm::event::MouseEventKind;
    if mouse.kind != MouseEventKind::Down(crossterm::event::MouseButton::Left) {
        return;
    }
    if let Some(sb_area) = app.sidebar_area
        && let Some(sel) = crate::tui::components::sidebar::hit_test(
            sb_area,
            mouse.row,
            mouse.column,
            &app.sidebar_state,
        )
    {
        match &sel {
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
        app.sidebar_state.selected = sel;
    }
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
    app.chat_area_height = chunks[0].height;
    app.frame_tick = app.frame_tick.wrapping_add(1);
    app.recompute_display_lines(sz.width);
    let _ = terminal.draw(|f| app.render(f));
}

pub fn apply_reload(app: &mut App, output: crate::tui::app::ReloadOutput) {
    use crate::tui::picker::PickerItem;

    app.tools.write().retain_builtins();

    // Register all plugin tools via adapter
    if let Some(rt) = &app.plugin_runtime {
        crate::plugin::plugin_tool_adapter::register_plugin_tools(rt, &mut app.tools.write());
    }

    // Re-discover markdown skills and register isTool skills
    let skills = crate::session::skills::discover_layered(
        Some(&app.project),
        &crate::config::paths::user_home(),
        &app.config.skills.dirs,
    );
    crate::tools::skill_tool::register_skill_tools(&skills, &mut app.tools.write());

    // Rebuild command items
    {
        let rt_guard = app.plugin_runtime.as_ref().map(|rt| rt.lock());
        let command_list = crate::commands::dispatcher::list_commands_with_plugins(
            &skills,
            Some(&app.plugin_manager),
            rt_guard.as_deref(),
        );
        app.command_items = command_list
            .iter()
            .map(|cmd| PickerItem {
                id: cmd.name.clone(),
                label: cmd.name.clone(),
                description: cmd.summary.clone(),
                source_tag: crate::tui::app::command_source_tag(&cmd.source),
            })
            .collect();
    }

    // Show result
    if let Some(ref result) = output.plugin_result {
        let mut parts = Vec::new();
        for build in &result.builds {
            if !build.success {
                parts.push(format!("build {} FAILED", build.name));
            }
        }
        if !result.errors.is_empty() {
            parts.push(format!("errors: {}", result.errors.join("; ")));
        }
        let msg = if parts.is_empty() {
            format!(
                "Reload complete. {} tools registered.",
                app.tools.read().count()
            )
        } else {
            format!(
                "Reload complete ({} tools). {}",
                app.tools.read().count(),
                parts.join(" | ")
            )
        };
        app.show_toast(msg);
    } else {
        app.show_toast(format!(
            "Reload complete. {} tools registered.",
            app.tools.read().count()
        ));
    }
}

fn format_bullet(lines: &mut Vec<String>, name: &str, description: &str, tag: &str) {
    if description.is_empty() {
        lines.push(format!("- **{name}**{tag}"));
    } else {
        lines.push(format!("- **{name}**{tag} — {description}"));
    }
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
    redraw(app, terminal);

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
                handle_sidebar_click(app, mouse);
            }
            _ => {}
        }
        redraw(app, terminal);
    };

    let form = app.tool_form.take();
    redraw(app, terminal);

    match result {
        Some(()) => form,
        None => None,
    }
}

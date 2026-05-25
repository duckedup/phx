use std::sync::Arc;

use crate::session::agent_loop::Session;
use crate::session::message::Message;
use crate::store::session_store::SessionId;
use crate::tui::app::App;
use crate::tui::tabs::{AssistantLine, ChatItem, ChatLine};

pub async fn handle_command(app: &mut App, input: &str) {
    let trimmed = input.trim().trim_start_matches('/');
    let cmd_name = trimmed.split_whitespace().next().unwrap_or("");

    if app.remote.is_some() && matches!(cmd_name, "sessions" | "resume") {
        handle_remote_sessions(app).await;
        return;
    }

    let skills = crate::session::skills::discover_layered(
        Some(&app.project),
        &crate::config::paths::user_home(),
        &app.config.skills.dirs,
    );
    let result = {
        // Resolve the command synchronously under the plugin lock, then drop it
        // before any .await to avoid blocking the tokio runtime.
        let sync_result = {
            let rt_guard = app.plugin_runtime.as_ref().map(|rt| rt.lock());
            crate::commands::dispatcher::try_dispatch_sync(
                input,
                &app.config,
                &skills,
                &app.store,
                &app.project,
                Some(&app.plugin_manager),
                rt_guard.as_deref(),
            )
        };
        match sync_result {
            Some(r) => r,
            None => {
                crate::commands::dispatcher::dispatch_async(input, &app.store, &app.project).await
            }
        }
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
            }
            app.show_toast("Session cleared");
        }
        crate::commands::CommandResult::InjectContext {
            name,
            content,
            model_override,
        } => {
            if app.session.is_none() {
                app.session = Some(Session::new(
                    SessionId::new(),
                    crate::config::SessionProfile::default(),
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

                    let model_msg = if let Some(ref model_id) = model_override {
                        apply_skill_model_override(app, model_id)
                    } else {
                        None
                    };

                    if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                        let mut status = format!("Skill loaded: {preview}");
                        if let Some(msg) = model_msg {
                            status.push_str(&format!("\n{msg}"));
                        }
                        tab.chat_lines.push(ChatItem::Line(ChatLine {
                            role: crate::session::message::Role::System,
                            content: status,
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
                app.show_toast(format!("Theme: {}", entry.name));
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
                app.show_toast("No sessions to resume");
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
            } else {
                app.show_toast("No active session to compact");
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
                crate::tui::conversation::deactivate_conductor_mode(app).await;
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
        crate::tui::conversation::deactivate_conductor_mode(app).await;
    } else {
        crate::tui::conversation::activate_conductor(app).await;
    }
}

fn apply_skill_model_override(app: &mut App, model_id: &str) -> Option<String> {
    use crate::providers::{model_info, registry::create_provider};

    let target_kind = match model_info::provider_kind_for_model(model_id) {
        Some(kind) => kind,
        None => {
            tracing::warn!("skill model '{model_id}' not in known models, ignoring");
            return Some(format!("Model '{model_id}' not recognized, using default."));
        }
    };

    let matching_provider = app
        .config
        .providers
        .iter()
        .find(|(_, profile)| profile.kind == target_kind);

    let (provider_name, base_profile) = match matching_provider {
        Some((name, profile)) => (name.clone(), profile.clone()),
        None => {
            tracing::warn!(
                "no configured provider for kind {:?} (model '{model_id}'), falling back",
                target_kind,
            );
            return Some(format!(
                "No provider configured for model '{model_id}', using default."
            ));
        }
    };

    let mut profile = base_profile;
    profile.model = model_id.to_string();

    match create_provider(&provider_name, &profile) {
        Ok(p) => {
            app.provider = Some(Arc::from(p));
            if let Some(session) = &mut app.session {
                session.provider_name.clone_from(&provider_name);
                session.model_name = model_id.to_string();
            }
            Some(format!("Model switched to {model_id} for this skill."))
        }
        Err(e) => {
            tracing::warn!(
                "failed to create provider for skill model '{model_id}': {e}, falling back"
            );
            Some(format!(
                "Failed to switch to model '{model_id}': {e}. Using default."
            ))
        }
    }
}

async fn handle_remote_sessions(app: &mut App) {
    use crate::tui::picker::{PickerItem, PickerMode, PickerState};

    let client = match app.remote.as_ref() {
        Some(c) => Arc::clone(c),
        None => return,
    };

    match client.send("session.list", serde_json::json!({})).await {
        Ok(resp) => {
            let sessions = resp
                .get("result")
                .and_then(|r| r.as_array())
                .cloned()
                .unwrap_or_default();

            if sessions.is_empty() {
                app.show_toast("No remote sessions to resume");
                return;
            }

            let items: Vec<PickerItem> = sessions
                .iter()
                .filter_map(|s| {
                    let id = s.get("id")?.as_str()?;
                    let name = s
                        .get("display_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(untitled)");
                    let provider = s.get("provider").and_then(|v| v.as_str()).unwrap_or("");
                    let model = s.get("model").and_then(|v| v.as_str()).unwrap_or("");
                    let description = if model.is_empty() {
                        provider.to_string()
                    } else {
                        format!("{provider}/{model}")
                    };
                    Some(PickerItem {
                        id: id.to_string(),
                        label: name.to_string(),
                        description,
                        source_tag: None,
                    })
                })
                .collect();

            if items.is_empty() {
                app.show_toast("No remote sessions to resume");
            } else {
                app.picker = Some(PickerState::new(items, PickerMode::Session));
            }
        }
        Err(e) => {
            if let Some(tab) = app.tabs.get_mut(app.active_tab) {
                tab.chat_lines.push(ChatItem::Line(ChatLine {
                    role: crate::session::message::Role::System,
                    content: format!("Failed to list remote sessions: {e}"),
                }));
            }
        }
    }
}

fn format_bullet(lines: &mut Vec<String>, name: &str, description: &str, tag: &str) {
    if description.is_empty() {
        lines.push(format!("- **{name}**{tag}"));
    } else {
        lines.push(format!("- **{name}**{tag} — {description}"));
    }
}

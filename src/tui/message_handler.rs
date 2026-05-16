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

    if app.conductor_mode {
        let lower = text.trim().to_lowercase();
        let is_approval = matches!(
            lower.as_str(),
            "go" | "yes"
                | "approved"
                | "approve"
                | "lgtm"
                | "do it"
                | "ship it"
                | "proceed"
                | "looks good"
                | "go ahead"
                | "start"
                | "run it"
                | "execute"
        );
        if is_approval {
            app.orch_ctx
                .plan_approved
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
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

pub fn handle_command(app: &mut App, input: &str) {
    let skills = crate::session::skills::discover_layered(
        Some(&app.project),
        &crate::config::paths::user_home(),
        &app.config.skills.dirs,
    );
    let rt_guard = app.plugin_runtime.as_ref().map(|rt| rt.lock());
    let result = crate::commands::dispatcher::dispatch_with_plugins(
        input,
        &app.config,
        &skills,
        &app.store,
        &app.project,
        Some(&app.plugin_manager),
        rt_guard.as_deref(),
    );
    drop(rt_guard);

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
            handle_conductor_command(app);
        }
        crate::commands::CommandResult::Solo => {
            if app.conductor_mode {
                deactivate_conductor_mode(app);
            } else {
                app.show_toast("Already in solo mode");
            }
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
                let rt_guard = rt.lock();
                let ui_config = rt_guard.tool_ui(&command).cloned();
                let description = rt_guard
                    .command_tools()
                    .iter()
                    .find(|t| t.command == command)
                    .map(|t| t.description.clone())
                    .unwrap_or_default();
                drop(rt_guard);

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
                    match rt.lock().toggle_tool(&command, &args_json) {
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
                // Ensure project plugin dir is included (install may create it)
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

fn handle_conductor_command(app: &mut App) {
    if app.conductor_mode {
        deactivate_conductor_mode(app);
        return;
    }

    let git_check = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(&app.project)
        .output();
    if !git_check.is_ok_and(|o| o.status.success()) {
        app.show_toast("Conductor requires a git repository");
        return;
    }

    let orch = &app.config.conductor;
    let needs_onboarding = orch.conductor_provider.is_none() || orch.agent_provider.is_none();

    if needs_onboarding {
        let items = build_conductor_picker_items(&app.config);
        if items.is_empty() {
            app.show_toast("No providers configured — use /connect first");
            return;
        }

        app.conductor_setup = Some(crate::tui::conductor_setup::ConductorSetup::new(items));
    } else {
        activate_conductor(app);
        app.show_toast("Conductor mode");
    }
}

pub fn build_conductor_picker_items(
    config: &crate::config::schema::Config,
) -> Vec<(String, String, String)> {
    let catalog = crate::providers::model_info::known_models();
    let mut items = Vec::new();

    for (name, profile) in &config.providers {
        let mut models_for_provider: Vec<&crate::providers::model_info::ModelInfo> = catalog
            .iter()
            .filter(|m| m.provider_kind == profile.kind)
            .collect();
        models_for_provider.sort_by(|a, b| {
            a.input_cost_per_mtok
                .partial_cmp(&b.input_cost_per_mtok)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        if models_for_provider.is_empty() {
            let id = format!("{name}/{}", profile.model);
            items.push((id.clone(), id, format!("{:?}", profile.kind)));
        } else {
            for model in models_for_provider {
                let id = format!("{name}/{}", model.id);
                let cost = if model.input_cost_per_mtok > 0.0 {
                    format!(
                        "${:.0}/{:.0} per Mtok",
                        model.input_cost_per_mtok, model.output_cost_per_mtok
                    )
                } else {
                    "free/local".to_string()
                };
                items.push((id, format!("{name}/{}", model.display_name), cost));
            }
        }
    }

    items
}

const CONDUCTOR_TAG: &str = "[phx:conductor]";

const CONDUCTOR_SYSTEM_PROMPT: &str = "\
[phx:conductor]\n\
You are the CONDUCTOR — an orchestrator that plans work, gets user approval, then delegates to agents.\n\
\n\
## Tools\n\
- spawn_agent: Spawn a child agent in an isolated git worktree.\n\
- wait_agents: Block until ALL running agents are done. Call this ONCE after spawning — it returns when everything finishes.\n\
- collect_agent: Get final output + diff from a completed agent.\n\
- cancel_agent: Cancel a running/queued agent.\n\
- merge_agent: Merge a completed agent's branch back into the parent.\n\
- check_agents: Quick status check (rarely needed — wait_agents is preferred).\n\
\n\
## Workflow — ALWAYS follow this order:\n\
\n\
### 1. Plan\n\
When the user gives you a task, analyze it and create a plan:\n\
- Break the work into independent sub-tasks\n\
- For each sub-task, describe what the agent will do\n\
- Present the plan to the user in a clear numbered list\n\
\n\
### 2. Approve\n\
Ask the user to approve the plan before spawning any agents.\n\
Wait for explicit confirmation (\"go\", \"yes\", \"approved\", etc.).\n\
If the user suggests changes, update the plan and ask again.\n\
NEVER spawn agents without user approval.\n\
\n\
### 3. Execute\n\
Once approved:\n\
- Spawn one agent per sub-task with a detailed, self-contained prompt\n\
- Call wait_agents ONCE — it blocks until all agents finish (do NOT loop or poll)\n\
- Collect results from each agent with collect_agent\n\
- Merge successful worktrees back with merge_agent\n\
- Report results to the user\n\
\n\
## Guidelines\n\
- Be concise — the user can see agent status in the panel\n\
- Give each agent enough context to work independently\n\
- Choose cheaper models for simple tasks\n\
- If an agent fails, tell the user and ask how to proceed\n\
- Use wait_agents instead of check_agents — it blocks until done, no polling needed\n\
- Do NOT call check_agents in a loop — if you need status, call wait_agents once";

pub fn activate_conductor(app: &mut App) {
    toggle_conductor_mode(app, true);
}

fn deactivate_conductor_mode(app: &mut App) {
    toggle_conductor_mode(app, false);
}

fn toggle_conductor_mode(app: &mut App, activate: bool) {
    app.conductor_mode = activate;
    app.orch_ctx
        .plan_approved
        .store(false, std::sync::atomic::Ordering::Relaxed);

    if !activate {
        if let Some(session) = &mut app.session {
            session.messages.retain(|m| {
                !(m.role == crate::session::message::Role::System
                    && m.content.contains(CONDUCTOR_TAG))
            });
            session.add_message(Message::system(
                "Mode switched: You are now in solo mode. \
                 Orchestration tools (spawn_agent, check_agents, etc.) are still available \
                 but should only be used if the user explicitly requests multi-agent work.",
            ));
        }

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
        return;
    }

    if activate {
        let custom_agents = crate::session::agents::discover_agents(
            Some(&app.project),
            &crate::config::paths::user_home(),
        );

        *app.orch_ctx.agents.write() = custom_agents.clone();
        *app.orch_ctx.config.write() = app.config.clone();
        let parent_provider = crate::config::loader::active_provider(&app.config)
            .map(|(name, _)| name.to_string())
            .unwrap_or_default();
        *app.orch_ctx.parent_provider.write() = parent_provider;
        *app.orch_ctx.parent_tools.write() = app.tools.read().clone();

        let tracker_result =
            validate_and_build_tracker_context(&app.config, &app.project, &app.tools.read());

        if let TrackerStatus::Broken(ref msg) = tracker_result
            && let Some(tab) = app.tabs.get_mut(app.active_tab)
        {
            tab.chat_lines.push(ChatItem::Line(ChatLine {
                role: crate::session::message::Role::System,
                content: format!("⚠ Tracker issue: {msg}"),
            }));
        }

        if let Some(session) = &mut app.session {
            session.add_message(Message::system(CONDUCTOR_SYSTEM_PROMPT));

            match tracker_result {
                TrackerStatus::Ready(ctx) | TrackerStatus::SpecMode(ctx) => {
                    session.add_message(Message::system(format!("{CONDUCTOR_TAG}\n{ctx}")));
                }
                TrackerStatus::Broken(_) => {
                    session.add_message(Message::system(format!(
                        "{CONDUCTOR_TAG}\n{}",
                        spec_mode_context()
                    )));
                }
            }

            if let Some(model_ctx) = conductor_model_context(&app.config) {
                session.add_message(Message::system(format!("{CONDUCTOR_TAG}\n{model_ctx}")));
            }
            let agent_catalog = crate::session::agents::build_agent_catalog(&custom_agents);
            if !agent_catalog.is_empty() {
                session.add_message(Message::system(format!("{CONDUCTOR_TAG}\n{agent_catalog}")));
            }
        }
    }
}

enum TrackerStatus {
    Ready(String),
    Broken(String),
    SpecMode(String),
}

fn validate_and_build_tracker_context(
    config: &crate::config::schema::Config,
    project: &std::path::Path,
    tools: &crate::tools::traits::ToolRegistry,
) -> TrackerStatus {
    match config.conductor.tracker.as_deref() {
        Some("beads") => validate_beads_tracker(project),
        Some("linear") => validate_linear_tracker(tools),
        Some("jira") => validate_jira_tracker(),
        Some(other) => TrackerStatus::Ready(format!(
            "Ticketing: This project uses {other} for issue tracking.\n\
             Sub-agents should update issue status appropriately."
        )),
        None => TrackerStatus::SpecMode(spec_mode_context()),
    }
}

fn validate_beads_tracker(project: &std::path::Path) -> TrackerStatus {
    let bd_check = std::process::Command::new("bd")
        .arg("ready")
        .current_dir(project)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output();

    match bd_check {
        Ok(output) if output.status.success() => TrackerStatus::Ready(
            "Ticketing: This project uses beads (bd) for issue tracking.\n\
             - Run `bd ready` to find available issues\n\
             - Run `bd show <id>` to see issue details\n\
             - Run `bd update <id> --claim` before starting work\n\
             - Run `bd close <id>` when work is complete\n\
             Sub-agents should claim their assigned issue before starting and close it when done."
                .to_string(),
        ),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            TrackerStatus::Broken(format!(
                "Beads tracker configured but `bd ready` failed: {stderr}"
            ))
        }
        Err(_) => TrackerStatus::Broken(
            "Beads tracker configured but `bd` command not found. Install beads or remove \
             tracker from conductor config."
                .to_string(),
        ),
    }
}

fn validate_linear_tracker(tools: &crate::tools::traits::ToolRegistry) -> TrackerStatus {
    let has_linear_tools = tools.get("linear_list_issues").is_some()
        || tools.get("mcp__linear__list_issues").is_some()
        || tools
            .list_schemas()
            .iter()
            .any(|s| s.name.contains("linear") || s.name.contains("Linear"));
    let has_api_key = std::env::var("LINEAR_API_KEY").is_ok();

    if has_linear_tools || has_api_key {
        TrackerStatus::Ready(
            "Ticketing: This project uses Linear for issue tracking.\n\
             Use the Linear MCP tools to find, claim, and update issues.\n\
             Sub-agents should update issue status to 'In Progress' when starting \
             and 'Done' when complete."
                .to_string(),
        )
    } else {
        TrackerStatus::Broken(
            "Linear tracker configured but no Linear tools or API key found. \
             Enable the Linear MCP extension or set LINEAR_API_KEY."
                .to_string(),
        )
    }
}

fn validate_jira_tracker() -> TrackerStatus {
    let has_api = std::env::var("JIRA_API_TOKEN").is_ok() || std::env::var("JIRA_TOKEN").is_ok();

    if has_api {
        TrackerStatus::Ready(
            "Ticketing: This project uses Jira for issue tracking.\n\
             Sub-agents should transition issues to 'In Progress' when starting \
             and 'Done' when complete."
                .to_string(),
        )
    } else {
        TrackerStatus::Broken(
            "Jira tracker configured but no JIRA_API_TOKEN or JIRA_TOKEN found.".to_string(),
        )
    }
}

fn spec_mode_context() -> String {
    "Task coordination: No ticket system configured — using spec file mode.\n\
     When decomposing work, the conductor will create a spec file for each sub-task \
     in the project directory (e.g. `.phx/specs/task-name.md`). Each spec file \
     contains the task description, acceptance criteria, and status.\n\
     \n\
     Spec file format:\n\
     ```\n\
     # Task: <title>\n\
     Status: unclaimed | in_progress | done\n\
     Agent: <session_id>\n\
     \n\
     ## Description\n\
     <task details>\n\
     \n\
     ## Acceptance Criteria\n\
     - [ ] criterion 1\n\
     - [ ] criterion 2\n\
     ```\n\
     \n\
     Sub-agents should:\n\
     1. Read their assigned spec file at the path given in the prompt.\n\
     2. Update Status to `in_progress` when starting.\n\
     3. Check off acceptance criteria as they complete them.\n\
     4. Update Status to `done` when finished."
        .to_string()
}

fn conductor_model_context(config: &crate::config::schema::Config) -> Option<String> {
    let orch = &config.conductor;
    let has_conductor = orch.conductor_provider.is_some();
    let has_agent = orch.agent_provider.is_some();
    let has_pool = !orch.pool.is_empty();

    if !has_conductor && !has_agent && !has_pool {
        return None;
    }

    let mut ctx = String::from("Model configuration:\n");

    if let (Some(provider), Some(model)) = (&orch.conductor_provider, &orch.conductor_model) {
        ctx.push_str(&format!(
            "- Conductor (you): provider=\"{provider}\", model=\"{model}\"\n"
        ));
    }

    if let (Some(provider), Some(model)) = (&orch.agent_provider, &orch.agent_model) {
        ctx.push_str(&format!(
            "- Default sub-agent: provider=\"{provider}\", model=\"{model}\"\n\
             - Use this for sub-agents unless the task warrants a different model.\n"
        ));
    }

    if has_pool {
        ctx.push_str("- Available model pool:\n");
        for entry in &orch.pool {
            let use_for = if entry.use_for.is_empty() {
                "general".to_string()
            } else {
                entry.use_for.clone()
            };
            ctx.push_str(&format!(
                "  * {}/{} — {use_for}\n",
                entry.provider, entry.model
            ));
        }
        ctx.push_str("- Choose the appropriate model based on task complexity.\n");
    }

    Some(ctx)
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
                            app.handle_key(key);
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
                            app.handle_key(key);
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

                        let dynamic_ui = app
                            .plugin_runtime
                            .as_ref()
                            .and_then(|rt| rt.lock().request_dynamic_ui(&tc.name, &tc.args_json));

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
                    app.handle_key(key);
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

use crate::commands::{connect, model, route, session_cmd, skill, theme};
use crate::config::{Config, ProviderProfile};
use crate::plugin::PluginManager;
use crate::plugin::plugin_runtime::PluginRuntime;
use crate::session::skills::Skill;
use crate::store::session_store::SessionStore;
use crate::tui::theme::ThemeEntry;
use std::path::Path;

#[derive(Debug)]
pub struct ModelChoice {
    pub provider_name: String,
    pub profile: ProviderProfile,
    pub display: String,
}

#[derive(Debug)]
pub struct SessionChoice {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSource {
    Builtin,
    Skill,
    Plugin,
    NativePlugin,
}

#[derive(Debug, Clone)]
pub struct CommandInfo {
    pub name: String,
    pub summary: String,
    pub is_skill: bool,
    pub source: CommandSource,
}

#[derive(Debug)]
pub enum CommandResult {
    Message(String),
    Error(String),
    ModelPicker(Vec<ModelChoice>),
    SessionPicker(Vec<SessionChoice>),
    ThemePicker(Vec<ThemeEntry>),
    ModelsPage,
    ConnectWizard,
    ContextInfo,
    InjectContext {
        name: String,
        content: String,
        model_override: Option<String>,
    },
    PluginCommand {
        plugin_command: String,
        args: String,
    },
    PluginToolCommand {
        command: String,
        args: String,
    },
    Conductor,
    Solo,
    ReloadPlugins,
    ClearSession,
    CompactSession,
    Cleared,
    Compacted,
    Route(route::RouteResult),
    NotACommand,
}

pub fn is_command(input: &str) -> bool {
    input.starts_with('/')
}

pub fn parse(input: &str) -> (&str, &str) {
    let input = input.trim();
    let without_slash = input.strip_prefix('/').unwrap_or(input);
    match without_slash.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (without_slash, ""),
    }
}

pub async fn dispatch(
    input: &str,
    config: &Config,
    skills: &[Skill],
    store: &SessionStore,
    project: &Path,
) -> CommandResult {
    match try_dispatch_sync(input, config, skills, store, project, None, None) {
        Some(r) => r,
        None => dispatch_async(input, store, project).await,
    }
}

/// Resolve a command synchronously. Returns `None` only for commands that
/// require an `.await` (resume/sessions) — the caller must follow up with
/// [`dispatch_async`] in that case. This split lets callers hold short-lived
/// locks (e.g. plugin runtime) without holding them across an await point.
pub fn try_dispatch_sync(
    input: &str,
    config: &Config,
    skills: &[Skill],
    _store: &SessionStore,
    _project: &Path,
    plugins: Option<&PluginManager>,
    plugin_runtime: Option<&PluginRuntime>,
) -> Option<CommandResult> {
    if !is_command(input) {
        return Some(CommandResult::NotACommand);
    }

    let (name, args) = parse(input);

    let result = match name {
        "model" => model::handle_model(args, config),
        "models" => CommandResult::ModelsPage,
        "skill" => skill::handle_skill(args, skills),
        "theme" => theme::handle_theme(args),
        "connect" => connect::handle_connect(),
        "resume" | "sessions" => return None,
        "clear" => session_cmd::handle_clear(),
        "compact" => session_cmd::handle_compact(),
        "route" => CommandResult::Route(route::handle(args, config)),
        "conductor" => CommandResult::Conductor,
        "solo" => CommandResult::Solo,
        "reload" => CommandResult::ReloadPlugins,
        "context" => CommandResult::ContextInfo,
        "help" => CommandResult::Message(help_text(skills, plugins, plugin_runtime)),
        _ => {
            if let Some(pm) = plugins
                && pm.get_command_handler(name).is_some()
            {
                return Some(CommandResult::PluginCommand {
                    plugin_command: name.to_string(),
                    args: args.to_string(),
                });
            }

            if let Some(rt) = plugin_runtime
                && rt.has_command(name)
            {
                return Some(CommandResult::PluginToolCommand {
                    command: name.to_string(),
                    args: args.to_string(),
                });
            }

            if let Some(s) = skills.iter().find(|s| s.name == name) {
                match crate::session::skills::load_skill_body(s) {
                    Ok(body) => CommandResult::InjectContext {
                        name: s.name.clone(),
                        content: body,
                        model_override: s.model.clone(),
                    },
                    Err(e) => CommandResult::Error(format!("failed to load skill: {e}")),
                }
            } else {
                CommandResult::Error(format!("unknown command: {name}"))
            }
        }
    };
    Some(result)
}

/// Handle the async-only commands (resume/sessions). Called when
/// `try_dispatch_sync` returns `None`.
pub async fn dispatch_async(input: &str, store: &SessionStore, project: &Path) -> CommandResult {
    let (name, _) = parse(input);
    match name {
        "resume" | "sessions" => session_cmd::handle_resume(store, project).await,
        _ => CommandResult::Error(format!("unknown command: {name}")),
    }
}

pub fn list_commands(skills: &[Skill]) -> Vec<CommandInfo> {
    list_commands_with_plugins(skills, None, None)
}

pub fn list_commands_with_plugins(
    skills: &[Skill],
    plugins: Option<&PluginManager>,
    plugin_runtime: Option<&PluginRuntime>,
) -> Vec<CommandInfo> {
    let mut cmds: Vec<CommandInfo> = vec![
        CommandInfo {
            name: "model".into(),
            summary: "Switch model or provider".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "models".into(),
            summary: "Open model management page".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "theme".into(),
            summary: "Switch color theme".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "connect".into(),
            summary: "Connect a new provider".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "resume".into(),
            summary: "Resume a previous session".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "sessions".into(),
            summary: "List and resume sessions".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "clear".into(),
            summary: "Clear current session".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "compact".into(),
            summary: "Compact session context".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "context".into(),
            summary: "List tools and skills".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "route".into(),
            summary: "Route tools to specific providers".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "conductor".into(),
            summary: "Multi-agent orchestration mode (Ctrl+2)".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "solo".into(),
            summary: "Single-agent mode (Ctrl+1)".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "reload".into(),
            summary: "Reload plugins".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
        CommandInfo {
            name: "help".into(),
            summary: "Show available commands".into(),
            is_skill: false,
            source: CommandSource::Builtin,
        },
    ];
    for s in skills {
        let summary = if s.description.is_empty() {
            "Skill".into()
        } else {
            s.description.clone()
        };
        cmds.push(CommandInfo {
            name: s.name.clone(),
            summary,
            is_skill: true,
            source: CommandSource::Skill,
        });
    }
    if let Some(pm) = plugins {
        for (name, summary) in pm.plugin_commands() {
            cmds.push(CommandInfo {
                name: name.to_string(),
                summary: if summary.is_empty() {
                    "Plugin command".into()
                } else {
                    summary.to_string()
                },
                is_skill: false,
                source: CommandSource::Plugin,
            });
        }
    }
    if let Some(rt) = plugin_runtime {
        for (name, description) in rt.commands() {
            cmds.push(CommandInfo {
                name: name.to_string(),
                summary: if description.is_empty() {
                    "Plugin tool".into()
                } else {
                    description.to_string()
                },
                is_skill: true,
                source: CommandSource::NativePlugin,
            });
        }
    }
    cmds.sort_by(|a, b| a.name.cmp(&b.name));
    cmds
}

fn help_text(
    skills: &[Skill],
    plugins: Option<&PluginManager>,
    plugin_runtime: Option<&PluginRuntime>,
) -> String {
    let mut lines = vec!["Available commands:".to_string()];
    for cmd in list_commands_with_plugins(skills, plugins, plugin_runtime) {
        lines.push(format!("  /{:<12} {}", cmd.name, cmd.summary));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_command_detection() {
        assert!(is_command("/model"));
        assert!(is_command("/help"));
        assert!(!is_command("hello"));
        assert!(!is_command(""));
    }

    #[test]
    fn parse_splits_name_and_args() {
        assert_eq!(parse("/model sonnet"), ("model", "sonnet"));
        assert_eq!(parse("/help"), ("help", ""));
        assert_eq!(parse("/skill  my-skill  "), ("skill", "my-skill"));
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn dispatch_unknown_command() {
        let config = Config::default();
        let store = SessionStore::new(std::path::PathBuf::from("/tmp/test"));
        let result = dispatch("/nonexistent", &config, &[], &store, Path::new("/test")).await;
        matches!(result, CommandResult::Error(_));
    }

    #[test]
    fn list_commands_includes_builtins() {
        let cmds = list_commands(&[]);
        assert!(cmds.iter().any(|c| c.name == "model"));
        assert!(cmds.iter().any(|c| c.name == "help"));
    }
}

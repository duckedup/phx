use crate::commands::{connect, model, session_cmd, skill, theme};
use crate::config::schema::{Config, ProviderProfile};
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

#[derive(Debug, Clone)]
pub struct CommandInfo {
    pub name: String,
    pub summary: String,
    pub is_skill: bool,
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
    InjectContext { name: String, content: String },
    ClearSession,
    CompactSession,
    Cleared,
    Compacted,
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

pub fn dispatch(
    input: &str,
    config: &Config,
    skills: &[Skill],
    store: &SessionStore,
    project: &Path,
) -> CommandResult {
    if !is_command(input) {
        return CommandResult::NotACommand;
    }

    let (name, args) = parse(input);

    match name {
        "model" => model::handle_model(args, config),
        "models" => CommandResult::ModelsPage,
        "skill" => skill::handle_skill(args, skills),
        "theme" => theme::handle_theme(args),
        "connect" => connect::handle_connect(),
        "resume" => session_cmd::handle_resume(store, project),
        "sessions" => session_cmd::handle_resume(store, project),
        "clear" => session_cmd::handle_clear(),
        "compact" => session_cmd::handle_compact(),
        "help" => CommandResult::Message(help_text(skills)),
        _ => {
            if let Some(s) = skills.iter().find(|s| s.name == name) {
                match crate::session::skills::load_skill_body(s) {
                    Ok(body) => CommandResult::InjectContext {
                        name: s.name.clone(),
                        content: body,
                    },
                    Err(e) => CommandResult::Error(format!("failed to load skill: {e}")),
                }
            } else {
                CommandResult::Error(format!("unknown command: {name}"))
            }
        }
    }
}

pub fn list_commands(skills: &[Skill]) -> Vec<CommandInfo> {
    let mut cmds: Vec<CommandInfo> = vec![
        CommandInfo {
            name: "model".into(),
            summary: "Switch model or provider".into(),
            is_skill: false,
        },
        CommandInfo {
            name: "models".into(),
            summary: "Open model management page".into(),
            is_skill: false,
        },
        CommandInfo {
            name: "theme".into(),
            summary: "Switch color theme".into(),
            is_skill: false,
        },
        CommandInfo {
            name: "connect".into(),
            summary: "Connect a new provider".into(),
            is_skill: false,
        },
        CommandInfo {
            name: "resume".into(),
            summary: "Resume a previous session".into(),
            is_skill: false,
        },
        CommandInfo {
            name: "sessions".into(),
            summary: "List and resume sessions".into(),
            is_skill: false,
        },
        CommandInfo {
            name: "clear".into(),
            summary: "Clear current session".into(),
            is_skill: false,
        },
        CommandInfo {
            name: "compact".into(),
            summary: "Compact session context".into(),
            is_skill: false,
        },
        CommandInfo {
            name: "help".into(),
            summary: "Show available commands".into(),
            is_skill: false,
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
        });
    }
    cmds.sort_by(|a, b| a.name.cmp(&b.name));
    cmds
}

fn help_text(skills: &[Skill]) -> String {
    let mut lines = vec!["Available commands:".to_string()];
    for cmd in list_commands(skills) {
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

    #[test]
    fn dispatch_unknown_command() {
        let config = Config::default();
        let store = SessionStore::new(std::path::PathBuf::from("/tmp/test"));
        let result = dispatch("/nonexistent", &config, &[], &store, Path::new("/test"));
        matches!(result, CommandResult::Error(_));
    }

    #[test]
    fn list_commands_includes_builtins() {
        let cmds = list_commands(&[]);
        assert!(cmds.iter().any(|c| c.name == "model"));
        assert!(cmds.iter().any(|c| c.name == "help"));
    }
}

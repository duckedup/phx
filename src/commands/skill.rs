use crate::commands::dispatcher::CommandResult;
use crate::session::skills::Skill;

pub fn handle_skill(args: &str, skills: &[Skill]) -> CommandResult {
    if args.is_empty() {
        let names: Vec<String> = skills.iter().map(|s| s.name.clone()).collect();
        if names.is_empty() {
            return CommandResult::Message("No skills available.".into());
        }
        return CommandResult::Message(format!("Available skills: {}", names.join(", ")));
    }

    match skills.iter().find(|s| s.name == args) {
        Some(skill) => match crate::session::skills::load_skill_prompt(skill) {
            Ok(prompt) => CommandResult::InjectContext(prompt),
            Err(e) => CommandResult::Error(format!("failed to load skill: {e}")),
        },
        None => CommandResult::Error(format!("unknown skill: {args}")),
    }
}

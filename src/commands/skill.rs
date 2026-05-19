use crate::commands::dispatcher::CommandResult;
use crate::session::skills::Skill;

pub fn handle_skill(args: &str, skills: &[Skill]) -> CommandResult {
    if args.is_empty() {
        if skills.is_empty() {
            return CommandResult::Message("No skills available.".into());
        }
        let lines: Vec<String> = skills
            .iter()
            .map(|s| {
                if s.description.is_empty() {
                    s.name.clone()
                } else {
                    format!("{}: {}", s.name, s.description)
                }
            })
            .collect();
        return CommandResult::Message(format!("Available skills:\n  {}", lines.join("\n  ")));
    }

    match skills.iter().find(|s| s.name == args) {
        Some(skill) => match crate::session::skills::load_skill_body(skill) {
            Ok(body) => CommandResult::InjectContext {
                name: skill.name.clone(),
                content: body,
                model_override: skill.model.clone(),
            },
            Err(e) => CommandResult::Error(format!("failed to load skill: {e}")),
        },
        None => CommandResult::Error(format!("unknown skill: {args}")),
    }
}

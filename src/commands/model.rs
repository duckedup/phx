use crate::commands::dispatcher::{CommandResult, ModelChoice};
use crate::config::schema::{Config, ProviderProfile};
use crate::config::writer;
use crate::providers::model_info;
use std::path::Path;

pub fn handle_model(args: &str, config: &Config) -> CommandResult {
    let choices = list_model_entries(config);
    if args.is_empty() {
        return CommandResult::ModelPicker(choices);
    }
    let needle = args.to_lowercase();
    let found: Vec<ModelChoice> = choices
        .into_iter()
        .filter(|c| {
            c.display.to_lowercase().contains(&needle)
                || c.profile.model.to_lowercase().contains(&needle)
        })
        .collect();

    if found.is_empty() {
        CommandResult::Error(format!("no model matching '{args}'"))
    } else {
        CommandResult::ModelPicker(found)
    }
}

pub fn list_model_entries(config: &Config) -> Vec<ModelChoice> {
    let known = model_info::known_models();
    let mut choices = Vec::new();

    for (name, profile) in &config.providers {
        let models_for_kind: Vec<&model_info::ModelInfo> = known
            .iter()
            .filter(|m| m.provider_kind == profile.kind)
            .collect();

        if models_for_kind.is_empty() {
            choices.push(ModelChoice {
                provider_name: name.clone(),
                profile: profile.clone(),
                display: format!("{} ({})", profile.model, name),
            });
        } else {
            for mi in models_for_kind {
                let is_current = mi.id == profile.model;
                let marker = if is_current { " ●" } else { "" };
                let mut p = profile.clone();
                p.model = mi.id.to_string();
                choices.push(ModelChoice {
                    provider_name: name.clone(),
                    profile: p,
                    display: format!("{}{marker}", mi.display_name),
                });
            }
        }
    }

    choices
}

pub fn apply_model_choice(choice: &ModelChoice, config_path: &Path) -> anyhow::Result<()> {
    let mut config = crate::config::loader::load(Some(config_path))?;
    for (_name, profile) in config.providers.iter_mut() {
        profile.active = false;
    }
    if let Some(profile) = config.providers.get_mut(&choice.provider_name) {
        profile.active = true;
    }
    writer::save(&config, config_path)?;
    Ok(())
}

pub fn add_provider(
    name: &str,
    profile: ProviderProfile,
    config_path: &Path,
) -> anyhow::Result<()> {
    writer::save_provider(config_path, name, &profile)?;
    Ok(())
}

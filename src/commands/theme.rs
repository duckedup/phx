use crate::commands::dispatcher::CommandResult;
use crate::tui::theme as tui_theme;

pub fn handle_theme(args: &str) -> CommandResult {
    if args.is_empty() {
        let themes = tui_theme::list_all();
        return CommandResult::ThemePicker(themes);
    }

    let all = tui_theme::list_all();
    let needle = args.to_lowercase();
    let matches: Vec<_> = all
        .into_iter()
        .filter(|t| {
            t.id.to_lowercase().contains(&needle) || t.name.to_lowercase().contains(&needle)
        })
        .collect();

    match matches.len() {
        0 => CommandResult::Error(format!("unknown theme: {args}")),
        _ => CommandResult::ThemePicker(matches),
    }
}

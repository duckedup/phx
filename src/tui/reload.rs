use crate::tui::app::App;
use crate::tui::picker::PickerItem;

pub fn apply_reload(app: &mut App, output: crate::tui::app::ReloadOutput) {
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

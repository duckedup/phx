use phoenix_plugin_sdk::skill;

skill! {
    name: "plan",
    command: "plan",
    description: "Enter plan mode — research and formulate a plan without changing files",
    keybind: "shift+tab",
    execute(arguments) {
        Ok(phoenix_plugin_sdk::SkillResult::with_toast(
            format!(
                "You are in PLAN MODE. Do not make any changes to source files. \
                 You are to research and formulate a plan for the requested changes only. \
                 {arguments}"
            ),
            "Plan mode activated.",
        ))
    },
    on_exit() {
        Ok(phoenix_plugin_sdk::SkillResult::with_toast(
            "You are now in AGENT MODE. You are free to make changes to source files \
             and execute the plan.",
            "Agent mode — free to edit files.",
        ))
    }
}

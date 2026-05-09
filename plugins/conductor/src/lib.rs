use phoenix_plugin_sdk::skill;

skill! {
    name: "conductor",
    command: "conductor",
    description: "Toggle conductor mode — orchestrate sub-agents",
    keybind: "",
    execute(_arguments) {
        Ok(phoenix_plugin_sdk::SkillResult::toast_only("Conductor mode activated"))
    },
    on_exit() {
        Ok(phoenix_plugin_sdk::SkillResult::toast_only("Conductor mode deactivated"))
    }
}

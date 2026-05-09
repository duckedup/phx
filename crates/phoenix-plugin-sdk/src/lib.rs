pub mod ui;

pub use phoenix_shared;
pub use phoenix_shared::context_types;
pub use phoenix_shared::hook_types;
pub use phoenix_shared::skill_types;
pub use phoenix_shared::tool_types;
pub use phoenix_shared::ui_types;

use ui::UiNode;

/// Result returned by a plugin's `execute` or `on_exit` function.
///
/// All fields are optional — leave empty/`None` to skip.
///
/// - `context` — Sent to the LLM as a message (triggers a response)
/// - `toast` — Shown briefly in the dynamic island / status area
/// - `widget` — UI node tree rendered inline in the chat area
pub struct SkillResult {
    /// Text sent to the LLM as a user message. Empty = skip.
    pub context: String,
    /// Brief text shown in the dynamic island toast. Empty = skip.
    pub toast: String,
    /// Declarative UI tree rendered in the chat area. `None` = skip.
    pub widget: Option<UiNode>,
}

impl SkillResult {
    /// Create a result that only sends context to the LLM.
    pub fn context(text: impl Into<String>) -> Self {
        Self {
            context: text.into(),
            toast: String::new(),
            widget: None,
        }
    }

    /// Create a result with context and a toast message.
    pub fn with_toast(context: impl Into<String>, toast: impl Into<String>) -> Self {
        Self {
            context: context.into(),
            toast: toast.into(),
            widget: None,
        }
    }

    /// Empty result (no-op).
    pub fn empty() -> Self {
        Self {
            context: String::new(),
            toast: String::new(),
            widget: None,
        }
    }
}

/// Internal: convert SDK SkillResult to WIT strings.
#[doc(hidden)]
pub fn __to_wit_result(r: SkillResult) -> (String, String, String) {
    let widget_json = match r.widget {
        Some(node) => node.to_json(),
        None => String::new(),
    };
    (r.context, r.toast, widget_json)
}

/// Declare a Phoenix skill plugin.
///
/// # Basic form
/// ```rust,ignore
/// use phoenix_plugin_sdk::{skill, SkillResult};
/// use phoenix_plugin_sdk::ui::{BoxNode, TextNode};
///
/// skill! {
///     name: "now",
///     command: "now",
///     description: "Show current time",
///     execute(arguments) {
///         Ok(SkillResult {
///             context: "The time is 12:00.".into(),
///             toast: String::new(),
///             widget: Some(
///                 BoxNode::new("Current Time")
///                     .child(TextNode::new("12:00 UTC").bold().fg("cyan"))
///                     .into_node()
///             ),
///         })
///     }
/// }
/// ```
///
/// # Toggle form (keybind + on_exit)
/// ```rust,ignore
/// skill! {
///     name: "plan",
///     command: "plan",
///     description: "Plan mode",
///     keybind: "shift+tab",
///     execute(arguments) {
///         Ok(SkillResult::with_toast(
///             format!("You are in PLAN MODE. {arguments}"),
///             "Plan mode activated.",
///         ))
///     },
///     on_exit() {
///         Ok(SkillResult::with_toast(
///             "You are now in AGENT MODE.",
///             "Agent mode resumed.",
///         ))
///     }
/// }
/// ```
#[macro_export]
macro_rules! skill {
    // Full form: keybind + on_exit + is_tool
    (
        name: $name:expr,
        command: $command:expr,
        description: $desc:expr,
        keybind: $kb:expr,
        is_tool: $is_tool:expr,
        execute($args:ident) $body:block,
        on_exit() $exit_body:block
    ) => {
        ::wit_bindgen::generate!({
            inline: r#"
                package phoenix:plugin@0.1.0;
                world skill-plugin {
                    record plugin-metadata {
                        name: string,
                        command: string,
                        description: string,
                        keybind: string,
                        is-tool: bool,
                    }
                    record skill-result {
                        context: string,
                        toast: string,
                        widget: string,
                    }
                    record command-output {
                        stdout: string,
                        stderr: string,
                        exit-code: s32,
                    }
                    import run-command: func(program: string, args: list<string>) -> result<command-output, string>;
                    export get-metadata: func() -> plugin-metadata;
                    export execute: func(arguments: string) -> result<skill-result, string>;
                    export on-exit: func() -> result<skill-result, string>;
                }
            "#,
            world: "skill-plugin",
        });

        struct PhoenixSkillPlugin;
        export!(PhoenixSkillPlugin);

        fn __convert(r: $crate::SkillResult) -> SkillResult {
            let (context, toast, widget) = $crate::__to_wit_result(r);
            SkillResult { context, toast, widget }
        }

        impl Guest for PhoenixSkillPlugin {
            fn get_metadata() -> PluginMetadata {
                PluginMetadata {
                    name: $name.into(),
                    command: $command.into(),
                    description: $desc.into(),
                    keybind: $kb.into(),
                    is_tool: $is_tool,
                }
            }

            fn execute($args: String) -> Result<SkillResult, String> {
                fn __inner($args: String) -> Result<$crate::SkillResult, String>
                    $body
                __inner($args).map(__convert)
            }

            fn on_exit() -> Result<SkillResult, String> {
                fn __inner() -> Result<$crate::SkillResult, String>
                    $exit_body
                __inner().map(__convert)
            }
        }
    };
    // Full form: keybind + on_exit (no is_tool, defaults false)
    (
        name: $name:expr,
        command: $command:expr,
        description: $desc:expr,
        keybind: $kb:expr,
        execute($args:ident) $body:block,
        on_exit() $exit_body:block
    ) => {
        $crate::skill! {
            name: $name,
            command: $command,
            description: $desc,
            keybind: $kb,
            is_tool: false,
            execute($args) $body,
            on_exit() $exit_body
        }
    };
    // Basic form: no keybind, no exit
    (
        name: $name:expr,
        command: $command:expr,
        description: $desc:expr,
        execute($args:ident) $body:block
    ) => {
        $crate::skill! {
            name: $name,
            command: $command,
            description: $desc,
            keybind: "",
            is_tool: false,
            execute($args) $body,
            on_exit() {
                Ok($crate::SkillResult::empty())
            }
        }
    };
}

/// Declare a Phoenix tool plugin.
///
/// # Example
/// ```rust,ignore
/// use phoenix_plugin_sdk::tool;
///
/// tool! {
///     tools: [
///         {
///             name: "greet",
///             description: "Greet someone",
///             parameters: r#"{"type":"object","properties":{"name":{"type":"string"}},"required":["name"]}"#,
///             invoke(name, args) {
///                 let who = args.get("name").and_then(|v| v.as_str()).unwrap_or("world");
///                 Ok((format!("Hello, {who}!"), false))
///             }
///         }
///     ]
/// }
/// ```
#[macro_export]
macro_rules! tool {
    (
        tools: [
            $(
                {
                    name: $name:expr,
                    description: $desc:expr,
                    parameters: $params:expr,
                    invoke($tool_name_arg:ident, $args_arg:ident) $body:block
                }
            ),+ $(,)?
        ]
    ) => {
        ::wit_bindgen::generate!({
            inline: r#"
                package phoenix:plugin@0.1.0;
                world tool-plugin {
                    record tool-metadata {
                        name: string,
                        description: string,
                        parameters-json: string,
                    }
                    record tool-result {
                        output: string,
                        is-error: bool,
                    }
                    export get-tool-metadata: func() -> list<tool-metadata>;
                    export invoke-tool: func(name: string, args-json: string) -> result<tool-result, string>;
                }
            "#,
            world: "tool-plugin",
        });

        struct PhoenixToolPlugin;
        export!(PhoenixToolPlugin);

        impl Guest for PhoenixToolPlugin {
            fn get_tool_metadata() -> Vec<ToolMetadata> {
                vec![
                    $(
                        ToolMetadata {
                            name: $name.into(),
                            description: $desc.into(),
                            parameters_json: $params.into(),
                        },
                    )+
                ]
            }

            fn invoke_tool(__tool_name: String, __args_json: String) -> Result<ToolResult, String> {
                let __parsed_args: serde_json::Value = serde_json::from_str(&__args_json)
                    .map_err(|e| format!("invalid JSON args: {e}"))?;
                match __tool_name.as_str() {
                    $(
                        $name => {
                            fn __inner($tool_name_arg: String, $args_arg: serde_json::Value) -> Result<(String, bool), String>
                                $body
                            let (output, is_error) = __inner(__tool_name, __parsed_args)?;
                            Ok(ToolResult { output, is_error })
                        }
                    )+
                    other => Err(format!("unknown tool: {other}")),
                }
            }
        }
    };
}

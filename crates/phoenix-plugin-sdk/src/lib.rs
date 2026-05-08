pub mod ui;

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
pub fn __to_wit_result(
    r: SkillResult,
) -> (String, String, String) {
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
    // Full form: keybind + on_exit
    (
        name: $name:expr,
        command: $command:expr,
        description: $desc:expr,
        keybind: $kb:expr,
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
                    }
                    record skill-result {
                        context: string,
                        toast: string,
                        widget: string,
                    }
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
    // Basic form: no keybind, no exit
    (
        name: $name:expr,
        command: $command:expr,
        description: $desc:expr,
        execute($args:ident) $body:block
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
                    }
                    record skill-result {
                        context: string,
                        toast: string,
                        widget: string,
                    }
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
                    keybind: String::new(),
                }
            }

            fn execute($args: String) -> Result<SkillResult, String> {
                fn __inner($args: String) -> Result<$crate::SkillResult, String>
                    $body
                __inner($args).map(__convert)
            }

            fn on_exit() -> Result<SkillResult, String> {
                Ok(SkillResult {
                    context: String::new(),
                    toast: String::new(),
                    widget: String::new(),
                })
            }
        }
    };
}

pub mod ui;

pub use phoenix_shared;
pub use phoenix_shared::context_types;
pub use phoenix_shared::hook_types;
pub use phoenix_shared::skill_types;
pub use phoenix_shared::tool_types;
pub use phoenix_shared::ui_field_types;
pub use phoenix_shared::ui_types;

pub struct ToolOutput {
    pub output: String,
    pub is_error: bool,
    pub toast: String,
    pub widget: String,
}

impl ToolOutput {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            toast: String::new(),
            widget: String::new(),
        }
    }

    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
            toast: String::new(),
            widget: String::new(),
        }
    }

    pub fn with_toast(output: impl Into<String>, toast: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            toast: toast.into(),
            widget: String::new(),
        }
    }

    pub fn toast_only(toast: impl Into<String>) -> Self {
        Self {
            output: String::new(),
            is_error: false,
            toast: toast.into(),
            widget: String::new(),
        }
    }

    pub fn empty() -> Self {
        Self {
            output: String::new(),
            is_error: false,
            toast: String::new(),
            widget: String::new(),
        }
    }
}

#[doc(hidden)]
pub fn __to_wit_tool_result(r: ToolOutput) -> (String, bool, String, String) {
    (r.output, r.is_error, r.toast, r.widget)
}

#[macro_export]
macro_rules! tool {
    (
        tools: [
            $(
                {
                    name: $name:expr,
                    description: $desc:expr,
                    parameters: $params:expr,
                    command: $cmd:expr,
                    keybind: $kb:expr,
                    ui: $ui:expr,
                    invoke($iname:ident, $iargs:ident) $ibody:block,
                    on_exit() $ebody:block
                }
            ),+ $(,)?
        ]
    ) => {
        ::wit_bindgen::generate!({
            inline: r#"
                package phoenix:plugin@0.2.0;
                world tool-plugin {
                    enum ui-field-kind {
                        text-input,
                        text-area,
                        toggle,
                    }
                    record ui-field {
                        key: string,
                        label: string,
                        field-kind: ui-field-kind,
                        required: bool,
                        placeholder: string,
                    }
                    record tool-metadata {
                        name: string,
                        description: string,
                        parameters-json: string,
                        command: string,
                        keybind: string,
                        ui-fields: list<ui-field>,
                    }
                    record tool-result {
                        output: string,
                        is-error: bool,
                        toast: string,
                        widget: string,
                    }
                    record command-output {
                        stdout: string,
                        stderr: string,
                        exit-code: s32,
                    }
                    import run-command: func(program: string, args: list<string>) -> result<command-output, string>;
                    export get-tool-metadata: func() -> list<tool-metadata>;
                    export invoke-tool: func(name: string, args-json: string) -> result<tool-result, string>;
                    export on-exit-tool: func(name: string) -> result<tool-result, string>;
                }
            "#,
            world: "tool-plugin",
        });

        struct PhoenixToolPlugin;
        export!(PhoenixToolPlugin);

        fn __convert(r: $crate::ToolOutput) -> ToolResult {
            let (output, is_error, toast, widget) = $crate::__to_wit_tool_result(r);
            ToolResult { output, is_error, toast, widget }
        }

        fn __convert_ui(fields: Vec<$crate::ui_field_types::UiField>) -> Vec<UiField> {
            fields.into_iter().map(|f| {
                let field_kind = match f.field {
                    $crate::ui_field_types::UiFieldKind::TextInput => UiFieldKind::TextInput,
                    $crate::ui_field_types::UiFieldKind::TextArea => UiFieldKind::TextArea,
                    $crate::ui_field_types::UiFieldKind::Toggle => UiFieldKind::Toggle,
                };
                UiField {
                    key: f.key,
                    label: f.label,
                    field_kind,
                    required: f.required,
                    placeholder: f.placeholder,
                }
            }).collect()
        }

        impl Guest for PhoenixToolPlugin {
            fn get_tool_metadata() -> Vec<ToolMetadata> {
                vec![
                    $(
                        ToolMetadata {
                            name: $name.into(),
                            description: $desc.into(),
                            parameters_json: $params.into(),
                            command: $cmd.into(),
                            keybind: $kb.into(),
                            ui_fields: __convert_ui($ui),
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
                            fn __inner($iname: String, $iargs: serde_json::Value) -> Result<$crate::ToolOutput, String>
                                $ibody
                            __inner(__tool_name, __parsed_args).map(__convert)
                        }
                    )+
                    other => Err(format!("unknown tool: {other}")),
                }
            }

            fn on_exit_tool(__tool_name: String) -> Result<ToolResult, String> {
                match __tool_name.as_str() {
                    $(
                        $name => {
                            fn __inner() -> Result<$crate::ToolOutput, String>
                                $ebody
                            __inner().map(__convert)
                        }
                    )+
                    _ => Ok(ToolResult {
                        output: String::new(),
                        is_error: false,
                        toast: String::new(),
                        widget: String::new(),
                    }),
                }
            }
        }
    };
}

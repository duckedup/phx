pub mod ui;

pub use phoenix_shared;
pub use phoenix_shared::context_types;
pub use phoenix_shared::hook_types;
pub use phoenix_shared::skill_types;
pub use phoenix_shared::tool_types;
pub use phoenix_shared::ui_field_types;
pub use phoenix_shared::ui_types;

pub use clap;
pub use serde;
pub use serde_json;

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

pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub fn run_command(program: &str, args: &[String]) -> Result<CommandOutput, String> {
    let output = std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("failed to run {program}: {e}"))?;

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

#[doc(hidden)]
pub fn __serialize_tool_output(r: &ToolOutput) -> String {
    serde_json::json!({
        "output": r.output,
        "is_error": r.is_error,
        "toast": r.toast,
        "widget": r.widget,
    })
    .to_string()
}

#[macro_export]
macro_rules! tool {
    (
        name: $plugin_name:expr,
        version: $plugin_version:expr,
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
        fn __build_manifest() -> String {
            let tools: Vec<$crate::serde_json::Value> = vec![
                $(
                    {
                        let ui_fields: Vec<$crate::ui_field_types::UiField> = $ui;
                        let ui_json: Vec<$crate::serde_json::Value> = ui_fields.into_iter().map(|f| {
                            $crate::serde_json::json!({
                                "key": f.key,
                                "label": f.label,
                                "field": match f.field {
                                    $crate::ui_field_types::UiFieldKind::TextInput => "text_input",
                                    $crate::ui_field_types::UiFieldKind::TextArea => "text_area",
                                    $crate::ui_field_types::UiFieldKind::Toggle => "toggle",
                                },
                                "required": f.required,
                                "placeholder": f.placeholder,
                                "default_value": f.default_value,
                            })
                        }).collect();
                        $crate::serde_json::json!({
                            "name": $name,
                            "description": $desc,
                            "parameters": $params,
                            "command": $cmd,
                            "keybind": $kb,
                            "ui_fields": ui_json,
                        })
                    },
                )+
            ];
            $crate::serde_json::to_string_pretty(&$crate::serde_json::json!({
                "name": $plugin_name,
                "version": $plugin_version,
                "tools": tools,
            })).unwrap()
        }

        fn __invoke_tool(tool_name: &str, args_json: &str) -> Result<$crate::ToolOutput, String> {
            let __parsed_args: $crate::serde_json::Value = $crate::serde_json::from_str(args_json)
                .map_err(|e| format!("invalid JSON args: {e}"))?;
            match tool_name {
                $(
                    $name => {
                        fn __inner($iname: String, $iargs: $crate::serde_json::Value) -> Result<$crate::ToolOutput, String>
                            $ibody
                        __inner(tool_name.to_string(), __parsed_args)
                    }
                )+
                other => Err(format!("unknown tool: {other}")),
            }
        }

        fn __exit_tool(tool_name: &str) -> Result<$crate::ToolOutput, String> {
            match tool_name {
                $(
                    $name => {
                        fn __inner() -> Result<$crate::ToolOutput, String>
                            $ebody
                        __inner()
                    }
                )+
                _ => Ok($crate::ToolOutput::empty()),
            }
        }

        fn __install_to(dir: &str) {
            let dest = std::path::Path::new(dir);
            std::fs::create_dir_all(dest).expect("failed to create install directory");

            let binary_name = std::env::current_exe()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .unwrap_or_else(|| format!("{}", $plugin_name));

            let mut manifest: $crate::serde_json::Value =
                $crate::serde_json::from_str(&__build_manifest()).unwrap();
            manifest["bin"] = $crate::serde_json::json!(format!("./{binary_name}"));

            let manifest_path = dest.join("manifest.json");
            let formatted = $crate::serde_json::to_string_pretty(&manifest).unwrap();
            std::fs::write(&manifest_path, formatted)
                .expect("failed to write manifest.json");

            let self_path = std::env::current_exe().expect("failed to get current exe path");
            let binary_dest = dest.join(&binary_name);
            std::fs::copy(&self_path, &binary_dest).expect("failed to copy binary");

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    &binary_dest,
                    std::fs::Permissions::from_mode(0o755),
                );
            }

            eprintln!("Installed to {}", dest.display());
            eprintln!("  manifest.json");
            eprintln!("  {binary_name}");
        }

        fn main() {
            use $crate::clap::{Arg, ArgAction, Command};

            let cli = Command::new($plugin_name)
                .version($plugin_version)
                .arg(
                    Arg::new("manifest")
                        .long("manifest")
                        .action(ArgAction::SetTrue)
                        .help("Print plugin manifest as JSON"),
                )
                .subcommand(
                    Command::new("invoke")
                        .about("Invoke a tool")
                        .arg(Arg::new("tool").required(true))
                        .arg(Arg::new("args_json").required(true)),
                )
                .subcommand(
                    Command::new("exit")
                        .about("Exit/cleanup a tool")
                        .arg(Arg::new("tool").required(true)),
                )
                .subcommand(
                    Command::new("install")
                        .about("Install plugin to a directory (manifest.json + binary)")
                        .arg(Arg::new("dir").required(true)),
                );

            let matches = cli.get_matches();

            if matches.get_flag("manifest") {
                println!("{}", __build_manifest());
                return;
            }

            match matches.subcommand() {
                Some(("invoke", sub)) => {
                    let tool = sub.get_one::<String>("tool").unwrap();
                    let args = sub.get_one::<String>("args_json").unwrap();
                    match __invoke_tool(tool, args) {
                        Ok(result) => {
                            println!("{}", $crate::__serialize_tool_output(&result));
                        }
                        Err(e) => {
                            let err_result = $crate::ToolOutput::error(e);
                            println!("{}", $crate::__serialize_tool_output(&err_result));
                            std::process::exit(1);
                        }
                    }
                }
                Some(("exit", sub)) => {
                    let tool = sub.get_one::<String>("tool").unwrap();
                    match __exit_tool(tool) {
                        Ok(result) => {
                            println!("{}", $crate::__serialize_tool_output(&result));
                        }
                        Err(e) => {
                            let err_result = $crate::ToolOutput::error(e);
                            println!("{}", $crate::__serialize_tool_output(&err_result));
                            std::process::exit(1);
                        }
                    }
                }
                Some(("install", sub)) => {
                    let dir = sub.get_one::<String>("dir").unwrap();
                    __install_to(dir);
                }
                _ => {
                    eprintln!("Usage: {} --manifest | invoke <tool> <args> | exit <tool> | install <dir>", $plugin_name);
                    std::process::exit(1);
                }
            }
        }
    };
}

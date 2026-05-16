pub fn tool_call_summary(name: &str, args_json: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(args_json).unwrap_or_default();
    match name {
        "bash" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let display: String = if cmd.chars().count() > 120 {
                let t: String = cmd.chars().take(117).collect();
                format!("{t}...")
            } else {
                cmd.to_string()
            };
            format!("bash > {display}")
        }
        "read" => {
            let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            format!("read > {path}")
        }
        "edit" => {
            let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            format!("edit > {path}")
        }
        "write" => {
            let path = args.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
            format!("write > {path}")
        }
        _ => name.to_string(),
    }
}

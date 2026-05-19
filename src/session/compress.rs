use crate::session::message::{Message, Role, ToolResult};

fn summarize_tool_result(tool_name: &str, args_json: &str, result: &ToolResult) -> String {
    if result.is_error {
        return result.output.clone();
    }

    let line_count = result.output.lines().count();
    let args: serde_json::Value = serde_json::from_str(args_json).unwrap_or_default();

    match tool_name {
        "read" => {
            let path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("[read {path}: {line_count} lines]")
        }
        "write" => {
            let path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("[write {path}: {line_count} lines]")
        }
        "edit" => {
            let path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("[edit {path}: ok]")
        }
        "bash" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("?");
            let exit_code = result
                .output
                .lines()
                .next()
                .and_then(|l| l.strip_prefix("exit code: "))
                .unwrap_or("?");
            if exit_code != "0" && exit_code != "?" {
                return result.output.clone();
            }
            let cmd_short = truncate_cmd(cmd, 60);
            format!("[bash '{cmd_short}' → exit {exit_code}, {line_count} lines]")
        }
        "spawn_agent" | "check_agents" | "collect_agent" | "cancel_agent" | "merge_agent" => {
            let summary = truncate_lines(&result.output, 5);
            format!("[{tool_name}: {summary}]")
        }
        _ => {
            format!("[{tool_name}: {line_count} lines]")
        }
    }
}

fn truncate_cmd(cmd: &str, max: usize) -> String {
    let first_line = cmd.lines().next().unwrap_or(cmd);
    if first_line.len() <= max {
        first_line.to_string()
    } else {
        format!("{}...", &first_line[..max])
    }
}

fn truncate_lines(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().take(max_lines).collect();
    let total = s.lines().count();
    let mut out = lines.join("; ");
    if total > max_lines {
        out.push_str(&format!(" (+{} more lines)", total - max_lines));
    }
    out
}

/// Find the index of the last User message — everything before it is a prior turn.
fn current_turn_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|m| m.role == Role::User)
        .unwrap_or(0)
}

/// Build a mapping from tool_result id → (tool_name, args_json) by scanning
/// the preceding ToolCall messages.
fn build_call_index(messages: &[Message]) -> std::collections::HashMap<String, (String, String)> {
    let mut index = std::collections::HashMap::new();
    for msg in messages {
        if let Some(tc) = &msg.tool_call {
            index.insert(tc.id.clone(), (tc.name.clone(), tc.args_json.clone()));
        }
    }
    index
}

/// Compress messages for the provider wire format.
///
/// - Current turn (from last User message onward): full tool results
/// - Prior turns: tool results replaced with one-line summaries
/// - Error results are always kept in full
/// - Original messages are not modified (returns new Vec)
pub fn compress_for_provider(messages: &[Message]) -> Vec<Message> {
    let turn_start = current_turn_start(messages);
    let call_index = build_call_index(messages);

    messages
        .iter()
        .enumerate()
        .map(|(i, msg)| {
            if i >= turn_start {
                return msg.clone();
            }

            let Some(tr) = &msg.tool_result else {
                return msg.clone();
            };

            if tr.is_error {
                return msg.clone();
            }

            let (tool_name, args_json) = call_index
                .get(&tr.id)
                .cloned()
                .unwrap_or_else(|| ("unknown".into(), "{}".into()));

            let summary = summarize_tool_result(&tool_name, &args_json, tr);

            Message {
                role: msg.role.clone(),
                content: msg.content.clone(),
                tool_call: msg.tool_call.clone(),
                tool_result: Some(ToolResult {
                    id: tr.id.clone(),
                    output: summary,
                    is_error: false,
                }),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::message::{ToolCall, ToolResult};

    fn read_call(id: &str, path: &str) -> Message {
        Message::tool_call(ToolCall {
            id: id.into(),
            name: "read".into(),
            args_json: format!(r#"{{"file_path":"{path}"}}"#),
        })
    }

    fn read_result(id: &str, content: &str) -> Message {
        Message::tool_result(ToolResult {
            id: id.into(),
            output: content.into(),
            is_error: false,
        })
    }

    fn bash_call(id: &str, cmd: &str) -> Message {
        Message::tool_call(ToolCall {
            id: id.into(),
            name: "bash".into(),
            args_json: format!(r#"{{"command":"{cmd}"}}"#),
        })
    }

    fn bash_result(id: &str, exit_code: i32, stdout: &str) -> Message {
        Message::tool_result(ToolResult {
            id: id.into(),
            output: format!("exit code: {exit_code}\n--- stdout ---\n{stdout}\n"),
            is_error: false,
        })
    }

    fn error_result(id: &str, msg: &str) -> Message {
        Message::tool_result(ToolResult {
            id: id.into(),
            output: msg.into(),
            is_error: true,
        })
    }

    #[test]
    fn current_turn_results_kept_full() {
        let messages = vec![
            Message::user("read the file"),
            read_call("1", "/src/main.rs"),
            read_result("1", "fn main() {\n    println!(\"hello\");\n}\n"),
        ];

        let compressed = compress_for_provider(&messages);
        assert_eq!(compressed.len(), 3);
        let tr = compressed[2].tool_result.as_ref().unwrap();
        assert!(tr.output.contains("fn main()"));
    }

    #[test]
    fn prior_turn_results_summarized() {
        let messages = vec![
            // Turn 1
            Message::user("read the file"),
            read_call("1", "/src/main.rs"),
            read_result("1", "fn main() {\n    println!(\"hello\");\n}\n"),
            Message::assistant("I see the main function."),
            // Turn 2 (current)
            Message::user("now edit it"),
        ];

        let compressed = compress_for_provider(&messages);
        let tr = compressed[2].tool_result.as_ref().unwrap();
        assert_eq!(tr.output, "[read /src/main.rs: 3 lines]");
        assert!(!tr.output.contains("fn main"));
    }

    #[test]
    fn error_results_always_kept() {
        let messages = vec![
            // Turn 1
            Message::user("run the test"),
            bash_call("1", "cargo test"),
            error_result("1", "compilation failed: missing semicolon"),
            Message::assistant("There was an error."),
            // Turn 2 (current)
            Message::user("fix it"),
        ];

        let compressed = compress_for_provider(&messages);
        let tr = compressed[2].tool_result.as_ref().unwrap();
        assert_eq!(tr.output, "compilation failed: missing semicolon");
    }

    #[test]
    fn bash_summary_includes_exit_code() {
        let messages = vec![
            Message::user("check status"),
            bash_call("1", "git status"),
            bash_result("1", 0, "On branch main\nnothing to commit"),
            Message::assistant("Clean."),
            Message::user("next"),
        ];

        let compressed = compress_for_provider(&messages);
        let tr = compressed[2].tool_result.as_ref().unwrap();
        assert!(tr.output.starts_with("[bash 'git status' → exit 0,"));
    }

    #[test]
    fn bash_nonzero_exit_kept_full() {
        let messages = vec![
            Message::user("run tests"),
            bash_call("1", "cargo test"),
            bash_result("1", 1, "test foo ... FAILED\nerror: test failed"),
            Message::assistant("Tests failed."),
            Message::user("fix it"),
        ];

        let compressed = compress_for_provider(&messages);
        let tr = compressed[2].tool_result.as_ref().unwrap();
        assert!(tr.output.contains("FAILED"));
        assert!(tr.output.contains("exit code: 1"));
    }

    #[test]
    fn multiple_prior_turns_all_compressed() {
        let messages = vec![
            // Turn 1
            Message::user("read a"),
            read_call("1", "/a.rs"),
            read_result("1", "aaa\nbbb\nccc\n"),
            Message::assistant("Got a."),
            // Turn 2
            Message::user("read b"),
            read_call("2", "/b.rs"),
            read_result("2", "xxx\nyyy\n"),
            Message::assistant("Got b."),
            // Turn 3 (current)
            Message::user("summarize both"),
        ];

        let compressed = compress_for_provider(&messages);
        let tr1 = compressed[2].tool_result.as_ref().unwrap();
        let tr2 = compressed[6].tool_result.as_ref().unwrap();
        assert_eq!(tr1.output, "[read /a.rs: 3 lines]");
        assert_eq!(tr2.output, "[read /b.rs: 2 lines]");
    }

    #[test]
    fn edit_summary() {
        let messages = vec![
            Message::user("edit the file"),
            Message::tool_call(ToolCall {
                id: "1".into(),
                name: "edit".into(),
                args_json: r#"{"file_path":"/src/lib.rs","old_string":"a","new_string":"b"}"#
                    .into(),
            }),
            Message::tool_result(ToolResult {
                id: "1".into(),
                output: "File edited successfully.".into(),
                is_error: false,
            }),
            Message::assistant("Done."),
            Message::user("next"),
        ];

        let compressed = compress_for_provider(&messages);
        let tr = compressed[2].tool_result.as_ref().unwrap();
        assert_eq!(tr.output, "[edit /src/lib.rs: ok]");
    }

    #[test]
    fn no_user_message_keeps_everything() {
        let messages = vec![
            Message::system("system prompt"),
            read_call("1", "/file.rs"),
            read_result("1", "content\n"),
        ];

        let compressed = compress_for_provider(&messages);
        let tr = compressed[2].tool_result.as_ref().unwrap();
        assert!(tr.output.contains("content"));
    }

    #[test]
    fn write_summary() {
        let messages = vec![
            Message::user("create file"),
            Message::tool_call(ToolCall {
                id: "1".into(),
                name: "write".into(),
                args_json: r#"{"file_path":"/new.rs"}"#.into(),
            }),
            Message::tool_result(ToolResult {
                id: "1".into(),
                output: "File written successfully.\n".into(),
                is_error: false,
            }),
            Message::assistant("Created."),
            Message::user("next"),
        ];

        let compressed = compress_for_provider(&messages);
        let tr = compressed[2].tool_result.as_ref().unwrap();
        assert!(tr.output.starts_with("[write /new.rs:"));
    }
}

use crate::tui::tabs::Tab;

pub fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return text.lines().map(String::from).collect();
    }

    let mut result = Vec::new();

    for line in text.split('\n') {
        if line.is_empty() {
            result.push(String::new());
            continue;
        }

        let words: Vec<&str> = line.split_whitespace().collect();
        if words.is_empty() {
            result.push(String::new());
            continue;
        }

        let mut current = String::new();
        let mut col = 0usize;

        for word in &words {
            let wlen = word.chars().count();
            if col == 0 {
                current.push_str(word);
                col = wlen;
            } else if col + 1 + wlen <= max_width {
                current.push(' ');
                current.push_str(word);
                col += 1 + wlen;
            } else {
                result.push(std::mem::take(&mut current));
                current.push_str(word);
                col = wlen;
            }
        }

        if !current.is_empty() {
            result.push(current);
        }
    }

    if result.is_empty() {
        result.push(String::new());
    }

    result
}

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner_frame(idx: usize) -> &'static str {
    SPINNER_FRAMES[idx % SPINNER_FRAMES.len()]
}

pub fn format_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

pub fn tool_call_summary(name: &str, args_json: &str) -> String {
    let args: serde_json::Value = serde_json::from_str(args_json).unwrap_or_default();
    match name {
        "bash" => {
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let display = if cmd.len() > 120 {
                format!("{}...", &cmd[..117])
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

pub fn drain_stream_buffer(tab: &mut Tab) {
    if tab.stream_buffer.is_empty() {
        return;
    }
    let buf_chars = tab.stream_buffer.chars().count();
    let n = if buf_chars > 200 {
        20
    } else if buf_chars > 50 {
        8
    } else {
        4
    }
    .min(buf_chars);

    let byte_pos = tab
        .stream_buffer
        .char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(tab.stream_buffer.len());
    let chunk = tab.stream_buffer[..byte_pos].to_string();
    tab.stream_buffer = tab.stream_buffer[byte_pos..].to_string();
    tab.streaming_text.push_str(&chunk);
}

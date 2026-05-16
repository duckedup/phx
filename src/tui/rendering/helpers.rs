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

use crate::tui::theme::Theme;
use ratatui::style::Color;

const SPINNER_GLYPH: &str = "◆";
const SPINNER_CYCLE: &[(f64, f64)] = &[
    (0.0, 1.0),
    (0.15, 0.85),
    (0.35, 0.65),
    (0.55, 0.45),
    (0.75, 0.25),
    (1.0, 0.0),
    (0.75, 0.25),
    (0.55, 0.45),
    (0.35, 0.65),
    (0.15, 0.85),
];

pub fn spinner_frame(_idx: usize) -> &'static str {
    SPINNER_GLYPH
}

pub fn spinner_color(idx: usize, theme: &Theme) -> Color {
    let (t_bg, _) = SPINNER_CYCLE[idx % SPINNER_CYCLE.len()];
    Theme::blend(theme.accent, theme.background, t_bg)
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
    crate::shared::tool_display::tool_call_summary(name, args_json)
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

pub fn truncate_output(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        return s.to_string();
    }
    match s.char_indices().nth(max_chars) {
        Some((byte_pos, _)) => format!("{}...", &s[..byte_pos]),
        None => s.to_string(),
    }
}

pub fn format_context_tree(names: &[String]) -> String {
    let mut lines = Vec::with_capacity(names.len() + 1);
    lines.push("Context loaded".to_string());
    for (i, name) in names.iter().enumerate() {
        let connector = if i + 1 < names.len() {
            "├──"
        } else {
            "└──"
        };
        lines.push(format!("  {connector} {name}"));
    }
    lines.join("\n")
}

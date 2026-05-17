use ratatui::prelude::*;

use crate::tui::rendering::display::DisplayLine;
use crate::tui::rendering::measure::{display_width, expand_tabs, truncate_to_width};
use crate::tui::theme::Theme;

pub fn is_diff_content(content: &str) -> bool {
    let first = content.lines().next().unwrap_or("");
    if !(first.contains("edited") && first.contains("replaced")) {
        return false;
    }
    content
        .lines()
        .any(|l| l.starts_with("- ") || l.starts_with("+ "))
}

pub fn render_diff(
    lines: &mut Vec<DisplayLine>,
    content: &str,
    theme: &Theme,
    indent: &str,
    content_width: usize,
) {
    let mut header = String::new();
    let mut old_lines: Vec<&str> = Vec::new();
    let mut new_lines: Vec<&str> = Vec::new();

    for line in content.lines() {
        if line.starts_with("edited ") {
            header = line.to_string();
        } else if let Some(rest) = line.strip_prefix("- ") {
            old_lines.push(rest);
        } else if let Some(rest) = line.strip_prefix("+ ") {
            new_lines.push(rest);
        }
    }

    let border = Style::default().fg(theme.tool_border());
    let dim = Style::default().fg(theme.dim());
    let del = Style::default().fg(theme.diff_delete);
    let add = Style::default().fg(theme.diff_add);

    let short_path = extract_short_path(&header);
    let count_text = extract_count(&header);

    let used = 4 + display_width(&short_path) + 4 + display_width(&count_text) + 1;
    let dash_fill = content_width.saturating_sub(used);

    lines.push(DisplayLine::multi(vec![
        (indent.to_string(), Style::default()),
        ("╭─ ".to_string(), border),
        (
            short_path,
            Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
        ),
        (" ── ".to_string(), border),
        (count_text, dim),
        (format!(" {}", "─".repeat(dash_fill)), border),
    ]));

    let text_max = content_width.saturating_sub(8);

    if old_lines.len() == new_lines.len() {
        for (i, (old, new)) in old_lines.iter().zip(new_lines.iter()).enumerate() {
            let ln = format!("{:>2} ", i + 1);
            let old_expanded = expand_tabs(old);
            let new_expanded = expand_tabs(new);
            lines.push(DisplayLine::multi(vec![
                (indent.to_string(), Style::default()),
                ("│ ".to_string(), border),
                (ln, dim),
                ("− ".to_string(), del),
                (truncate_to_width(&old_expanded, text_max), del),
            ]));
            lines.push(DisplayLine::multi(vec![
                (indent.to_string(), Style::default()),
                ("│ ".to_string(), border),
                ("   ".to_string(), Style::default()),
                ("+ ".to_string(), add),
                (truncate_to_width(&new_expanded, text_max), add),
            ]));
        }
    } else {
        for (i, old) in old_lines.iter().enumerate() {
            let ln = format!("{:>2} ", i + 1);
            let old_expanded = expand_tabs(old);
            lines.push(DisplayLine::multi(vec![
                (indent.to_string(), Style::default()),
                ("│ ".to_string(), border),
                (ln, dim),
                ("− ".to_string(), del),
                (truncate_to_width(&old_expanded, text_max), del),
            ]));
        }
        for (i, new) in new_lines.iter().enumerate() {
            let ln = format!("{:>2} ", i + 1);
            let new_expanded = expand_tabs(new);
            lines.push(DisplayLine::multi(vec![
                (indent.to_string(), Style::default()),
                ("│ ".to_string(), border),
                (ln, dim),
                ("+ ".to_string(), add),
                (truncate_to_width(&new_expanded, text_max), add),
            ]));
        }
    }

    lines.push(DisplayLine::multi(vec![
        (indent.to_string(), Style::default()),
        ("╰─ ".to_string(), border),
        ("✓".to_string(), Style::default().fg(theme.success)),
        (" applied".to_string(), dim),
    ]));
}

fn extract_short_path(header: &str) -> String {
    let path = header
        .strip_prefix("edited ")
        .and_then(|s| s.split(':').next())
        .unwrap_or("file");

    let p = std::path::Path::new(path);
    let filename = p.file_name().and_then(|n| n.to_str()).unwrap_or(path);
    let parent = p
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str());

    match parent {
        Some(dir) => format!("{dir}/{filename}"),
        None => filename.to_string(),
    }
}

fn extract_count(header: &str) -> String {
    header.split(": ").nth(1).unwrap_or("replaced").to_string()
}

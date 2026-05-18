use ratatui::prelude::*;

use crate::tui::rendering::display::DisplayLine;
use crate::tui::rendering::measure::{display_width, expand_tabs, pad_to_width, truncate_to_width};
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

    // indent + "│ " + "NN " + "− " + old + " │ " + "NN " + "+ " + new
    let indent_w = display_width(indent);
    let overhead = indent_w + 2 + 3 + 2 + 3 + 3 + 2;
    let half = content_width.saturating_sub(overhead) / 2;
    let row_count = old_lines.len().max(new_lines.len());

    for i in 0..row_count {
        let old = old_lines.get(i).copied().unwrap_or("");
        let new = new_lines.get(i).copied().unwrap_or("");
        let old_expanded = expand_tabs(old);
        let new_expanded = expand_tabs(new);

        let mut parts = vec![
            (indent.to_string(), Style::default()),
            ("│ ".to_string(), border),
        ];

        if !old.is_empty() {
            parts.push((format!("{:>2} ", i + 1), dim));
            let old_text = truncate_to_width(&old_expanded, half);
            let padded = pad_to_width(&old_text, half);
            parts.push(("− ".to_string(), del));
            parts.push((padded, del));
        } else {
            parts.push(("   ".to_string(), Style::default()));
            parts.push(("  ".to_string(), Style::default()));
            parts.push((" ".repeat(half), Style::default()));
        }

        parts.push((" │ ".to_string(), border));

        if !new.is_empty() {
            parts.push((format!("{:>2} ", i + 1), dim));
            let new_text = truncate_to_width(&new_expanded, half);
            parts.push(("+ ".to_string(), add));
            parts.push((new_text, add));
        }

        lines.push(DisplayLine::multi(parts));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::theme::default_theme;

    fn span_text(lines: &[DisplayLine]) -> Vec<String> {
        lines
            .iter()
            .map(|dl| dl.spans.iter().map(|(t, _)| t.as_str()).collect::<String>())
            .collect()
    }

    #[test]
    fn is_diff_content_detects_edit_output() {
        let content = "edited /tmp/test.rs: replaced 1 occurrence(s)\n- old\n+ new\n";
        assert!(is_diff_content(content));
    }

    #[test]
    fn is_diff_content_rejects_non_diff() {
        assert!(!is_diff_content("just some text"));
        assert!(!is_diff_content("edited but no replaced"));
    }

    #[test]
    fn render_diff_produces_header_and_footer() {
        let theme = default_theme();
        let content = "edited /src/main.rs: replaced 1 occurrence(s)\n- old\n+ new\n";
        let mut lines = Vec::new();
        render_diff(&mut lines, content, &theme, "  ", 80);
        let text = span_text(&lines);
        assert!(text[0].contains("╭─"));
        assert!(text[0].contains("src/main.rs"));
        assert!(text.last().unwrap().contains("✓"));
        assert!(text.last().unwrap().contains("applied"));
    }

    #[test]
    fn render_diff_side_by_side_equal_counts() {
        let theme = default_theme();
        let content = "edited /f.rs: replaced 1 occurrence(s)\n- old1\n- old2\n+ new1\n+ new2\n";
        let mut lines = Vec::new();
        render_diff(&mut lines, content, &theme, "", 80);
        let text = span_text(&lines);
        // Each row has old and new side by side
        assert!(text[1].contains("− ") && text[1].contains("old1"));
        assert!(text[1].contains("+ ") && text[1].contains("new1"));
        assert!(text[2].contains("old2") && text[2].contains("new2"));
    }

    #[test]
    fn render_diff_side_by_side_unequal_counts() {
        let theme = default_theme();
        let content = "edited /f.rs: replaced 1 occurrence(s)\n- old1\n- old2\n+ new1\n";
        let mut lines = Vec::new();
        render_diff(&mut lines, content, &theme, "", 80);
        let text = span_text(&lines);
        // Row 1: old1 + new1
        assert!(text[1].contains("old1") && text[1].contains("new1"));
        // Row 2: old2 + empty right side
        assert!(text[2].contains("old2"));
        assert!(!text[2].contains("+ "));
    }

    #[test]
    fn render_diff_expands_tabs() {
        let theme = default_theme();
        let content = "edited /f.rs: replaced 1 occurrence(s)\n- \told\n+ \tnew\n";
        let mut lines = Vec::new();
        render_diff(&mut lines, content, &theme, "", 80);
        let text = span_text(&lines);
        assert!(text[1].contains("    old"));
        assert!(!text[1].contains('\t'));
    }

    #[test]
    fn extract_short_path_gets_parent_and_file() {
        assert_eq!(
            extract_short_path("edited /a/b/c/main.rs: replaced 1 occurrence(s)"),
            "c/main.rs"
        );
    }

    #[test]
    fn extract_short_path_handles_no_parent() {
        assert_eq!(
            extract_short_path("edited main.rs: replaced 1 occurrence(s)"),
            "main.rs"
        );
    }
}

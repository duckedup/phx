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
    let mut old_lines: Vec<(usize, &str)> = Vec::new();
    let mut new_lines: Vec<(usize, &str)> = Vec::new();

    for line in content.lines() {
        if line.starts_with("edited ") {
            header = line.to_string();
        } else if let Some(rest) = line.strip_prefix("- ") {
            let (ln, text) = parse_numbered_line(rest);
            old_lines.push((ln, text));
        } else if let Some(rest) = line.strip_prefix("+ ") {
            let (ln, text) = parse_numbered_line(rest);
            new_lines.push((ln, text));
        }
    }

    let border = Style::default().fg(theme.tool_border());
    let dim = Style::default().fg(theme.dim());

    let del_bg = Theme::blend(theme.diff_delete, theme.background, 0.80);
    let add_bg = Theme::blend(theme.diff_add, theme.background, 0.80);
    let del_style = Style::default().fg(theme.diff_delete).bg(del_bg);
    let del_text = Style::default().fg(theme.foreground).bg(del_bg);
    let del_ln = Style::default().fg(theme.dim()).bg(del_bg);
    let add_style = Style::default().fg(theme.diff_add).bg(add_bg);
    let add_text = Style::default().fg(theme.foreground).bg(add_bg);
    let add_ln = Style::default().fg(theme.dim()).bg(add_bg);

    let short_path = extract_short_path(&header);
    let count_text = extract_count(&header);

    let indent_w = display_width(indent);
    let used = indent_w + 4 + display_width(&short_path) + 4 + display_width(&count_text) + 1;
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

    let max_ln = old_lines
        .iter()
        .chain(new_lines.iter())
        .map(|(ln, _)| *ln)
        .max()
        .unwrap_or(0);
    let ln_w = if max_ln >= 1000 { 4 } else { 3 };
    let text_w = content_width.saturating_sub(indent_w + ln_w + 3);

    for &(ln, text) in &old_lines {
        let expanded = expand_tabs(text);
        let truncated = truncate_to_width(&expanded, text_w);
        lines.push(DisplayLine::multi(vec![
            (indent.to_string(), Style::default()),
            (format!("{:>width$} ", ln, width = ln_w - 1), del_ln),
            ("− ".to_string(), del_style),
            (truncated, del_text),
        ]));
    }

    for &(ln, text) in &new_lines {
        let expanded = expand_tabs(text);
        let truncated = truncate_to_width(&expanded, text_w);
        lines.push(DisplayLine::multi(vec![
            (indent.to_string(), Style::default()),
            (format!("{:>width$} ", ln, width = ln_w - 1), add_ln),
            ("+ ".to_string(), add_style),
            (truncated, add_text),
        ]));
    }

    lines.push(DisplayLine::multi(vec![
        (indent.to_string(), Style::default()),
        ("╰─ ".to_string(), border),
        ("✓".to_string(), Style::default().fg(theme.success)),
        (" applied".to_string(), dim),
    ]));
}

fn parse_numbered_line(rest: &str) -> (usize, &str) {
    if let Some(colon_pos) = rest.find(':')
        && let Ok(ln) = rest[..colon_pos].parse::<usize>()
    {
        return (ln, &rest[colon_pos + 1..]);
    }
    (0, rest)
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
    fn render_diff_stacked_removed_then_added() {
        let theme = default_theme();
        let content = "edited /f.rs: replaced 1 occurrence(s)\n- old1\n- old2\n+ new1\n+ new2\n";
        let mut lines = Vec::new();
        render_diff(&mut lines, content, &theme, "", 80);
        let text = span_text(&lines);
        // Removed lines first
        assert!(text[1].contains("− ") && text[1].contains("old1"));
        assert!(text[2].contains("− ") && text[2].contains("old2"));
        // Added lines after
        assert!(text[3].contains("+ ") && text[3].contains("new1"));
        assert!(text[4].contains("+ ") && text[4].contains("new2"));
    }

    #[test]
    fn render_diff_uses_background_colors() {
        let theme = default_theme();
        let content = "edited /f.rs: replaced 1 occurrence(s)\n- old\n+ new\n";
        let mut lines = Vec::new();
        render_diff(&mut lines, content, &theme, "", 80);
        // Check that removed line has a background set (not default)
        let del_line = &lines[1];
        let has_bg = del_line.spans.iter().any(|(_, s)| s.bg.is_some());
        assert!(has_bg, "removed line should have background color");
        // Check added line
        let add_line = &lines[2];
        let has_bg = add_line.spans.iter().any(|(_, s)| s.bg.is_some());
        assert!(has_bg, "added line should have background color");
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

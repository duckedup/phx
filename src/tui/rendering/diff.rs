use ratatui::prelude::*;

use crate::tui::rendering::display::DisplayLine;
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

pub fn render_side_by_side_diff(
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

    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        (header, Style::default().fg(theme.dim())),
    ]));

    let gutter = 4;
    let sep_w = 3;
    let usable = content_width.saturating_sub(gutter * 2 + sep_w);
    let half = usable / 2;
    if half < 8 {
        for ol in &old_lines {
            lines.push(DisplayLine::styled(
                &format!("{indent}  - {ol}"),
                Style::default().fg(theme.diff_delete),
            ));
        }
        for nl in &new_lines {
            lines.push(DisplayLine::styled(
                &format!("{indent}  + {nl}"),
                Style::default().fg(theme.diff_add),
            ));
        }
        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  "), Style::default()),
            ("✓ done".to_string(), Style::default().fg(theme.success)),
        ]));
        return;
    }

    let lh = pad_to_width("removed", half);
    let rh = pad_to_width("added", half);
    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        (" ".repeat(gutter), Style::default()),
        (
            lh,
            Style::default()
                .fg(theme.diff_delete)
                .add_modifier(Modifier::BOLD),
        ),
        (" │ ".to_string(), Style::default().fg(theme.dim())),
        (" ".repeat(gutter), Style::default()),
        (
            rh,
            Style::default()
                .fg(theme.diff_add)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    let dash_l = "─".repeat(half + gutter);
    let dash_r = "─".repeat(half + gutter);
    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        (dash_l, Style::default().fg(theme.dim())),
        ("─┼─".to_string(), Style::default().fg(theme.dim())),
        (dash_r, Style::default().fg(theme.dim())),
    ]));

    let max_rows = old_lines.len().max(new_lines.len());
    for i in 0..max_rows {
        let left = old_lines.get(i).copied().unwrap_or("");
        let right = new_lines.get(i).copied().unwrap_or("");

        let ln = if i < old_lines.len() {
            format!("{:>2}│ ", i + 1)
        } else {
            "  │ ".to_string()
        };
        let rn = if i < new_lines.len() {
            format!("{:>2}│ ", i + 1)
        } else {
            "  │ ".to_string()
        };

        let lt = pad_to_width(left, half);
        let rt = pad_to_width(right, half);

        let left_style = if left.is_empty() && i >= old_lines.len() {
            Style::default().fg(theme.dim())
        } else {
            Style::default().fg(theme.diff_delete)
        };
        let right_style = if right.is_empty() && i >= new_lines.len() {
            Style::default().fg(theme.dim())
        } else {
            Style::default().fg(theme.diff_add)
        };

        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  "), Style::default()),
            (ln, Style::default().fg(theme.dim())),
            (lt, left_style),
            (" │ ".to_string(), Style::default().fg(theme.dim())),
            (rn, Style::default().fg(theme.dim())),
            (rt, right_style),
        ]));
    }

    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        ("✓ done".to_string(), Style::default().fg(theme.success)),
    ]));
}

pub fn pad_to_width(s: &str, width: usize) -> String {
    let char_count = s.chars().count();
    if char_count > width {
        let truncated: String = s.chars().take(width.saturating_sub(1)).collect();
        format!("{truncated}…")
    } else {
        let padding = " ".repeat(width - char_count);
        format!("{s}{padding}")
    }
}

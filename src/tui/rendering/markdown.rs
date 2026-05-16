use ratatui::prelude::*;

use crate::tui::rendering::display::DisplayLine;
use crate::tui::rendering::helpers::wrap_text;
use crate::tui::theme::Theme;

pub fn render_markdown(
    text: &str,
    theme: &Theme,
    indent: &str,
    max_width: usize,
) -> Vec<DisplayLine> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let code_bg = Theme::blend(theme.foreground, theme.background, 0.92);

    for line in text.split('\n') {
        let trimmed = line.trim_start();

        if trimmed.starts_with("```") {
            if !in_code_block {
                in_code_block = true;
                let lang = trimmed.trim_start_matches('`').trim();
                let label = if lang.is_empty() {
                    String::new()
                } else {
                    format!(" {lang} ")
                };
                lines.push(DisplayLine::multi(vec![(
                    format!("{indent}┌{label}"),
                    Style::default().fg(theme.dim()),
                )]));
            } else {
                in_code_block = false;
                lines.push(DisplayLine::multi(vec![(
                    format!("{indent}└"),
                    Style::default().fg(theme.dim()),
                )]));
            }
            continue;
        }

        if in_code_block {
            let code_width = max_width.saturating_sub(4);
            let display_line = if line.chars().count() > code_width && code_width > 3 {
                let truncated: String = line.chars().take(code_width - 3).collect();
                format!("{truncated}...")
            } else {
                line.to_string()
            };
            lines.push(DisplayLine::multi(vec![
                (format!("{indent}│ "), Style::default().fg(theme.dim())),
                (
                    display_line,
                    Style::default().fg(theme.code_fg()).bg(code_bg),
                ),
            ]));
            continue;
        }

        if let Some(header) = trimmed.strip_prefix("### ") {
            let header = header.trim();
            lines.push(DisplayLine::styled(
                &format!("{indent}{header}"),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if let Some(header) = trimmed.strip_prefix("## ") {
            let header = header.trim();
            lines.push(DisplayLine::styled(
                &format!("{indent}{header}"),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            continue;
        }
        if let Some(header) = trimmed.strip_prefix("# ") {
            let header = header.trim();
            lines.push(DisplayLine::styled(
                &format!("{indent}{header}"),
                Style::default()
                    .fg(theme.primary)
                    .add_modifier(Modifier::BOLD),
            ));
            continue;
        }

        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let item = &trimmed[2..];
            let bullet_indent = format!("{indent}  • ");
            let cont_indent = format!("{indent}    ");
            let item_width = max_width.saturating_sub(4);
            let wrapped = wrap_text(item, item_width);
            for (i, wl) in wrapped.iter().enumerate() {
                let prefix = if i == 0 {
                    bullet_indent.clone()
                } else {
                    cont_indent.clone()
                };
                let mut parts = vec![(prefix, Style::default().fg(theme.dim()))];
                parts.extend(parse_inline_md(wl, theme));
                lines.push(DisplayLine::multi(parts));
            }
            continue;
        }

        if let Some(rest) = strip_numbered_list(trimmed) {
            let prefix_end = trimmed.len() - rest.len();
            let num_prefix = &trimmed[..prefix_end];
            let cont_indent = format!("{indent}    ");
            let item_width = max_width.saturating_sub(4);
            let wrapped = wrap_text(rest, item_width);
            for (i, wl) in wrapped.iter().enumerate() {
                let mut parts = vec![(format!("{indent}  "), Style::default())];
                if i == 0 {
                    parts.push((num_prefix.to_string(), Style::default().fg(theme.dim())));
                } else {
                    parts.push((cont_indent.clone(), Style::default()));
                }
                parts.extend(parse_inline_md(wl, theme));
                lines.push(DisplayLine::multi(parts));
            }
            continue;
        }

        if trimmed.is_empty() {
            lines.push(DisplayLine::empty());
            continue;
        }

        for wl in wrap_text(line, max_width) {
            let mut parts = vec![(indent.to_string(), Style::default())];
            parts.extend(parse_inline_md(&wl, theme));
            lines.push(DisplayLine::multi(parts));
        }
    }

    if in_code_block {
        lines.push(DisplayLine::multi(vec![(
            format!("{indent}└"),
            Style::default().fg(theme.dim()),
        )]));
    }

    lines
}

fn strip_numbered_list(s: &str) -> Option<&str> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if !first.is_ascii_digit() {
        return None;
    }
    for ch in chars.by_ref() {
        if ch == '.' {
            let rest = chars.as_str();
            if let Some(stripped) = rest.strip_prefix(' ') {
                return Some(stripped.trim_start());
            }
            return None;
        }
        if !ch.is_ascii_digit() {
            return None;
        }
    }
    None
}

pub fn parse_inline_md(text: &str, theme: &Theme) -> Vec<(String, Style)> {
    let normal = Style::default().fg(theme.foreground);
    let bold = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::BOLD);
    let italic = Style::default()
        .fg(theme.foreground)
        .add_modifier(Modifier::ITALIC);
    let code = Style::default().fg(theme.code_fg());

    let mut spans: Vec<(String, Style)> = Vec::new();
    let mut buf = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '`' => {
                if !buf.is_empty() {
                    spans.push((std::mem::take(&mut buf), normal));
                }
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '`' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if closed {
                    spans.push((inner, code));
                } else {
                    buf.push('`');
                    buf.push_str(&inner);
                }
            }
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                if !buf.is_empty() {
                    spans.push((std::mem::take(&mut buf), normal));
                }
                let mut inner = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if closed {
                    spans.push((inner, bold));
                } else {
                    buf.push_str("**");
                    buf.push_str(&inner);
                }
            }
            '*' => {
                if !buf.is_empty() {
                    spans.push((std::mem::take(&mut buf), normal));
                }
                let mut inner = String::new();
                let mut closed = false;
                for c in chars.by_ref() {
                    if c == '*' {
                        closed = true;
                        break;
                    }
                    inner.push(c);
                }
                if closed {
                    spans.push((inner, italic));
                } else {
                    buf.push('*');
                    buf.push_str(&inner);
                }
            }
            _ => buf.push(ch),
        }
    }

    if !buf.is_empty() {
        spans.push((buf, normal));
    }

    spans
}

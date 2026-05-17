use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::session::message::Role;
use crate::session::orchestration::{ChildInfo, ChildStatus};
use crate::tui::layout::CHAT_PADDING;
use crate::tui::rendering::display::{
    DisplayLine, build_compact_tool_card, build_item_display_lines, is_tool_call_item,
};
use crate::tui::rendering::helpers::{spinner_color, spinner_frame};
use crate::tui::rendering::markdown::render_markdown;
use crate::tui::rendering::measure::{display_width, truncate_to_width_raw};
use crate::tui::tabs::{ChatItem, ChatLine, Tab};
use crate::tui::theme::Theme;

fn is_check_agents_call(item: &ChatItem) -> bool {
    matches!(item, ChatItem::Line(ChatLine { role: Role::ToolCall, content }) if content.contains("check_agents"))
}

fn is_collect_agent_call(item: &ChatItem) -> bool {
    matches!(item, ChatItem::Line(ChatLine { role: Role::ToolCall, content }) if content.contains("collect_agent"))
}

fn is_tool_result(item: &ChatItem) -> bool {
    matches!(
        item,
        ChatItem::Line(ChatLine {
            role: Role::ToolResult,
            ..
        })
    )
}

pub fn render_chat(
    frame: &mut Frame,
    area: Rect,
    display_lines: &[DisplayLine],
    effective_scroll: usize,
    theme: &Theme,
) {
    render_chat_with_panel(
        frame,
        area,
        display_lines,
        effective_scroll,
        theme,
        None,
        None,
    );
}

pub fn render_chat_with_panel(
    frame: &mut Frame,
    area: Rect,
    display_lines: &[DisplayLine],
    effective_scroll: usize,
    theme: &Theme,
    panel: Option<Rect>,
    hovered_line: Option<usize>,
) {
    let visible = area.height as usize;
    let full_width = area.width as usize;
    let bg = Style::default().bg(theme.background);

    let row_width_at = |y: u16| -> usize {
        if let Some(p) = panel {
            if y >= p.y && y < p.y + p.height {
                (p.x.saturating_sub(area.x + 1)) as usize
            } else {
                full_width
            }
        } else {
            full_width
        }
    };

    let mut screen_row: usize = 0;
    let mut line_idx = effective_scroll;

    while screen_row < visible && line_idx < display_lines.len() {
        let dl = &display_lines[line_idx];
        let abs_idx = line_idx;
        let is_hovered = hovered_line == Some(abs_idx)
            && (dl.file_path.is_some() || dl.tool_detail_idx.is_some());

        let full_line = if is_hovered {
            Line::from(
                dl.spans
                    .iter()
                    .map(|(t, s)| {
                        let is_content = !t.trim().is_empty() && !t.trim().starts_with('\u{2502}');
                        if is_content {
                            Span::styled(
                                t.clone(),
                                s.fg(theme.accent).add_modifier(Modifier::UNDERLINED),
                            )
                        } else {
                            Span::styled(t.clone(), *s)
                        }
                    })
                    .collect::<Vec<_>>(),
            )
        } else {
            dl.to_line()
        };

        let total_width: usize = full_line
            .spans
            .iter()
            .map(|s| display_width(&s.content))
            .sum();
        let y = area.y + screen_row as u16;
        let rw = row_width_at(y);

        if rw == 0 || total_width == 0 {
            render_row(frame, area.x, y, full_width, Line::from(""), bg);
            screen_row += 1;
            line_idx += 1;
            continue;
        }

        if total_width <= rw {
            let padded = pad_line(full_line, rw, bg);
            render_row(frame, area.x, y, full_width, padded, bg);
            screen_row += 1;
        } else {
            let mut remaining = full_line;
            while screen_row < visible {
                let y = area.y + screen_row as u16;
                let rw = row_width_at(y);
                if rw == 0 {
                    render_row(frame, area.x, y, full_width, Line::from(""), bg);
                    screen_row += 1;
                    break;
                }
                let rem_width: usize = remaining
                    .spans
                    .iter()
                    .map(|s| display_width(&s.content))
                    .sum();
                if rem_width == 0 {
                    break;
                }
                if rem_width <= rw {
                    let padded = pad_line(remaining, rw, bg);
                    render_row(frame, area.x, y, full_width, padded, bg);
                    screen_row += 1;
                    break;
                }
                let chunk = truncate_line(&remaining, rw);
                let chunk_w: usize = chunk.spans.iter().map(|s| display_width(&s.content)).sum();
                let padded = pad_line(chunk, rw, bg);
                render_row(frame, area.x, y, full_width, padded, bg);
                remaining = skip_line_cols(&remaining, chunk_w);
                screen_row += 1;
            }
        }
        line_idx += 1;
    }

    while screen_row < visible {
        let y = area.y + screen_row as u16;
        render_row(frame, area.x, y, full_width, Line::from(""), bg);
        screen_row += 1;
    }
}

fn render_row(frame: &mut Frame, x: u16, y: u16, width: usize, line: Line<'static>, bg: Style) {
    let rect = Rect {
        x,
        y,
        width: width as u16,
        height: 1,
    };
    frame.render_widget(Paragraph::new(line).style(bg), rect);
}

fn pad_line(mut line: Line<'static>, target: usize, bg: Style) -> Line<'static> {
    let actual: usize = line.spans.iter().map(|s| display_width(&s.content)).sum();
    if actual < target {
        line.spans
            .push(Span::styled(" ".repeat(target - actual), bg));
    }
    line
}

fn skip_line_cols(line: &Line<'static>, skip_cols: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut skipped = 0;
    for span in &line.spans {
        let w = display_width(&span.content);
        if skipped >= skip_cols {
            spans.push(span.clone());
        } else if skipped + w > skip_cols {
            let to_skip = skip_cols - skipped;
            let mut col = 0;
            let mut byte_start = 0;
            for (i, c) in span.content.char_indices() {
                if col >= to_skip {
                    byte_start = i;
                    break;
                }
                col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                byte_start = i + c.len_utf8();
            }
            let remainder = &span.content[byte_start..];
            if !remainder.is_empty() {
                spans.push(Span::styled(remainder.to_string(), span.style));
            }
            skipped = skip_cols;
            continue;
        }
        skipped += w;
    }
    Line::from(spans)
}

fn truncate_line(line: &Line<'static>, max_cols: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut remaining = max_cols;
    for span in &line.spans {
        if remaining == 0 {
            break;
        }
        let w = display_width(&span.content);
        if w <= remaining {
            spans.push(span.clone());
            remaining -= w;
        } else {
            let truncated = truncate_to_width_raw(&span.content, remaining);
            remaining = remaining.saturating_sub(display_width(&truncated));
            spans.push(Span::styled(truncated, span.style));
        }
    }
    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Live agent tree — rebuilds from pool state every frame
// ---------------------------------------------------------------------------

fn build_live_agents_tree(
    lines: &mut Vec<DisplayLine>,
    agents: &[ChildInfo],
    theme: &Theme,
    pad: u16,
    frame_tick: u64,
) {
    let indent = " ".repeat(pad as usize);
    let dim = Style::default().fg(theme.dim());

    if agents.is_empty() {
        return;
    }

    let header_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        ("Agents".to_string(), header_style),
    ]));

    for (i, agent) in agents.iter().enumerate() {
        let is_last = i + 1 == agents.len();
        let connector = if is_last { "╰" } else { "├" };
        let is_working =
            agent.status == ChildStatus::Running || agent.status == ChildStatus::Queued;

        let (status_icon, status_style) = if is_working {
            let frame_idx = ((frame_tick / 4) + i as u64) as usize;
            let spin = spinner_frame(frame_idx);
            let color = spinner_color(frame_idx, theme);
            (
                spin.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        } else if agent.status == ChildStatus::Done {
            ("✓".to_string(), Style::default().fg(theme.success))
        } else {
            ("✗".to_string(), Style::default().fg(theme.error))
        };

        let status_text = match &agent.status {
            ChildStatus::Running => "working",
            ChildStatus::Queued => "queued",
            ChildStatus::Done => "done",
            ChildStatus::Error(_) => "error",
            ChildStatus::Cancelled => "cancelled",
        };

        let status_label = if is_working {
            let dots_idx = ((frame_tick / 8) + i as u64) as usize % 4;
            let dots = ["   ", ".  ", ".. ", "..."][dots_idx];
            format!("{status_text}{dots}")
        } else {
            status_text.to_string()
        };

        let elapsed = format!("{:.0}s", agent.elapsed_s);

        let mut parts = vec![
            (format!("{indent}  "), Style::default()),
            (format!("{connector}── "), dim),
            (format!("{status_icon} "), status_style),
            (agent.task.clone(), Style::default().fg(theme.foreground)),
            (
                format!(" {status_label}"),
                if is_working {
                    Style::default().fg(theme.accent)
                } else {
                    dim
                },
            ),
            (format!(" {elapsed}"), dim),
        ];

        if let Some(tool) = &agent.active_tool {
            parts.push((format!(" ({tool})"), dim));
        }

        lines.push(DisplayLine::multi(parts));
    }
    lines.push(DisplayLine::empty());
}

fn build_collect_agent_display(
    lines: &mut Vec<DisplayLine>,
    result_content: &str,
    theme: &Theme,
    pad: u16,
) {
    let indent = " ".repeat(pad as usize);
    let dim = Style::default().fg(theme.dim());
    let accent = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);
    let border_style = Style::default().fg(theme.tool_border());

    let mut task = "";
    let mut status = "";
    let mut model = "";
    let mut elapsed = "";
    let mut changes = "";
    let mut branch = "";
    let mut output_lines: Vec<&str> = Vec::new();
    let mut in_output = false;

    for line in result_content.lines() {
        if in_output {
            output_lines.push(line);
        } else if let Some(v) = line.strip_prefix("Task:") {
            task = v.trim();
        } else if let Some(v) = line.strip_prefix("Status:") {
            status = v.trim();
        } else if let Some(v) = line.strip_prefix("Model:") {
            model = v.trim();
        } else if let Some(v) = line.strip_prefix("Elapsed:") {
            elapsed = v.trim();
        } else if let Some(v) = line.strip_prefix("Changes:") {
            changes = v.trim();
        } else if let Some(v) = line.strip_prefix("Branch:") {
            branch = v.trim();
        } else if line.starts_with("Output:") {
            in_output = true;
        }
    }

    let (icon, icon_style) = match status {
        "done" => ("✓", Style::default().fg(theme.success)),
        s if s.starts_with("cancel") => ("✗", dim),
        _ => ("✗", Style::default().fg(theme.error)),
    };

    lines.push(DisplayLine::multi(vec![
        (format!("{indent}  "), Style::default()),
        (format!("{icon} "), icon_style),
        (task.to_string(), accent),
        (format!("  {status} · {elapsed}"), dim),
    ]));

    if !model.is_empty() {
        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  │ "), border_style),
            (format!("model {model}"), dim),
        ]));
    }

    if !changes.is_empty() {
        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  │ "), border_style),
            (changes.to_string(), Style::default().fg(theme.foreground)),
        ]));
    }

    if !branch.is_empty() {
        lines.push(DisplayLine::multi(vec![
            (format!("{indent}  │ "), border_style),
            (format!("branch {branch}"), dim),
        ]));
    }

    if !output_lines.is_empty() {
        for ol in &output_lines {
            lines.push(DisplayLine::multi(vec![
                (format!("{indent}  │ "), border_style),
                (ol.to_string(), Style::default().fg(theme.foreground)),
            ]));
        }
    }

    lines.push(DisplayLine::empty());
}

// ---------------------------------------------------------------------------
// Main display line computation
// ---------------------------------------------------------------------------

pub fn compute_display_lines(
    tab: Option<&Tab>,
    theme: &Theme,
    is_running: bool,
    frame_tick: u64,
    widths: (u16, u16),
    _turn_count: u32,
    live_agents: &[ChildInfo],
) -> Vec<DisplayLine> {
    let (area_width, full_area_width) = widths;
    let pad = CHAT_PADDING;
    let content_width = area_width as usize;
    let full_content_width = full_area_width as usize;
    if content_width == 0 {
        return Vec::new();
    }

    let tab = match tab {
        Some(t) => t,
        None => {
            return vec![
                DisplayLine::empty(),
                DisplayLine::styled(
                    "  Press Enter to start a new session.",
                    Style::default().fg(theme.dim()),
                ),
            ];
        }
    };

    let mut lines = Vec::new();

    let items = &tab.chat_lines;

    // Find the LAST check_agents call — only that one renders the live tree
    let last_check_idx = items.iter().rposition(is_check_agents_call);

    let mut i = 0;
    while i < items.len() {
        if is_check_agents_call(&items[i]) {
            // Skip the call and its result
            let mut j = i;
            while j < items.len() {
                if is_check_agents_call(&items[j]) {
                    if j + 1 < items.len() && is_tool_result(&items[j + 1]) {
                        j += 2;
                    } else {
                        j += 1;
                    }
                } else {
                    break;
                }
            }
            // Only render the live tree at the LAST check_agents position
            if Some(i) == last_check_idx || (last_check_idx.is_some_and(|li| li > i && li < j)) {
                build_live_agents_tree(&mut lines, live_agents, theme, pad, frame_tick);
            }
            i = j;
        } else if is_collect_agent_call(&items[i]) {
            // Render collect_agent call header, then style its result
            build_item_display_lines(
                &mut lines,
                &items[i],
                theme,
                content_width,
                full_content_width,
                pad,
            );
            i += 1;
            if i < items.len() && is_tool_result(&items[i]) {
                if let ChatItem::Line(cl) = &items[i] {
                    build_collect_agent_display(&mut lines, &cl.content, theme, pad);
                }
                i += 1;
            }
        } else if is_tool_call_item(&items[i]) {
            let mut call_end = i;
            while call_end < items.len() && is_tool_call_item(&items[call_end]) {
                call_end += 1;
            }
            let num_calls = call_end - i;

            let mut result_end = call_end;
            while result_end < items.len() && is_tool_result(&items[result_end]) {
                result_end += 1;
            }
            let num_results = result_end - call_end;

            if num_results == num_calls && num_calls > 0 {
                if num_calls > 1 {
                    lines.push(DisplayLine::empty());
                }
                for k in 0..num_calls {
                    let call_idx = i + k;
                    let result_idx = call_end + k;
                    if let (ChatItem::Line(call), ChatItem::Line(result)) =
                        (&items[call_idx], &items[result_idx])
                    {
                        build_compact_tool_card(&mut lines, call, result, theme, pad, result_idx);
                    }
                }
                lines.push(DisplayLine::empty());
                i = result_end;
            } else {
                build_item_display_lines(
                    &mut lines,
                    &items[i],
                    theme,
                    content_width,
                    full_content_width,
                    pad,
                );
                i += 1;
            }
        } else {
            build_item_display_lines(
                &mut lines,
                &items[i],
                theme,
                content_width,
                full_content_width,
                pad,
            );
            i += 1;
        }
    }

    if !tab.streaming_text.is_empty() {
        let body_indent = format!("{}  ", " ".repeat(pad as usize));
        let md_lines = render_markdown(
            &tab.streaming_text,
            theme,
            &body_indent,
            content_width.saturating_sub(2),
        );
        lines.extend(md_lines);
    }

    let is_thinking = is_running && tab.streaming_text.is_empty() && tab.stream_buffer.is_empty();
    if is_thinking {
        let frame_idx = (frame_tick / 4) as usize;
        let spin = spinner_frame(frame_idx);
        let color = spinner_color(frame_idx, theme);

        let thinking_msgs = ["thinking", "thinking.", "thinking..", "thinking..."];
        let msg = thinking_msgs[(frame_tick / 12) as usize % thinking_msgs.len()];
        lines.push(DisplayLine::multi(vec![
            (format!("{}  ", " ".repeat(pad as usize)), Style::default()),
            (
                format!("{spin} "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            (msg.to_string(), Style::default().fg(theme.dim())),
        ]));
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn make_display_line(text: &str) -> DisplayLine {
        DisplayLine::styled(text, Style::default())
    }

    #[test]
    fn render_chat_fills_empty_rows_with_background() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::default_theme();
        let lines: Vec<DisplayLine> = vec![make_display_line("hello")];

        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 40, 10);
                render_chat(f, area, &lines, 0, &theme);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        assert_eq!(buf[(0, 0)].symbol(), "h");
        assert_eq!(buf[(1, 0)].symbol(), "e");
        // Row 1 should be blank (only 1 display line)
        assert_eq!(buf[(0, 1)].symbol(), " ");
    }

    #[test]
    fn render_chat_with_panel_truncates_on_panel_rows() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::default_theme();

        let long_text = "a".repeat(40);
        let lines: Vec<DisplayLine> = (0..10).map(|_| make_display_line(&long_text)).collect();

        let panel = Some(Rect::new(30, 5, 10, 5));

        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 40, 10);
                render_chat_with_panel(f, area, &lines, 0, &theme, panel, None);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        // Row 0 (above panel): text should extend to column 39
        assert_eq!(buf[(39, 0)].symbol(), "a");
        // Row 7 (on panel): text should NOT extend to column 30+
        // The panel starts at x=30, so row_width = 30 - 0 - 1 = 29
        // Column 29 should be blank (padding), not "a"
        assert_eq!(buf[(29, 7)].symbol(), " ");
    }

    #[test]
    fn render_chat_wraps_long_line_on_panel_rows() {
        let backend = TestBackend::new(40, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = crate::tui::theme::default_theme();

        // One long line that needs wrapping when panel narrows the row
        let long_text = "abcdefghij".repeat(4); // 40 chars
        let lines: Vec<DisplayLine> = vec![make_display_line(&long_text)];

        // Panel covers rows 0-9 at x=20, so row_width = 19
        let panel = Some(Rect::new(20, 0, 20, 10));

        terminal
            .draw(|f| {
                let area = Rect::new(0, 0, 40, 10);
                render_chat_with_panel(f, area, &lines, 0, &theme, panel, None);
            })
            .unwrap();

        let buf = terminal.backend().buffer();
        // Row 0: first 19 chars of the line (row_width = 20 - 0 - 1 = 19)
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(18, 0)].symbol(), "i");
        // Row 1: continuation — starts with char 20 of the 40-char string
        // "abcdefghij" repeats, so char 19 is "j" (0-indexed)
        assert_eq!(buf[(0, 1)].symbol(), "j");
    }

    #[test]
    fn truncate_line_respects_display_width() {
        let line = Line::from(vec![Span::raw("hello "), Span::raw("world")]);
        let result = truncate_line(&line, 8);
        let width: usize = result.spans.iter().map(|s| display_width(&s.content)).sum();
        assert!(width <= 8);
    }

    #[test]
    fn skip_line_cols_returns_remainder() {
        let line = Line::from(vec![Span::raw("hello"), Span::raw(" world")]);
        let remainder = skip_line_cols(&line, 5);
        let text: String = remainder
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(text, " world");
    }

    #[test]
    fn skip_line_cols_handles_mid_span_split() {
        let line = Line::from(vec![Span::raw("abcdef")]);
        let remainder = skip_line_cols(&line, 3);
        let text: String = remainder
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(text, "def");
    }

    #[test]
    fn skip_line_cols_empty_remainder() {
        let line = Line::from(vec![Span::raw("abc")]);
        let remainder = skip_line_cols(&line, 3);
        assert!(remainder.spans.is_empty() || remainder.spans.iter().all(|s| s.content.is_empty()));
    }

    #[test]
    fn pad_line_fills_to_target() {
        let line = Line::from(vec![Span::raw("hi")]);
        let bg = Style::default();
        let padded = pad_line(line, 10, bg);
        let width: usize = padded.spans.iter().map(|s| display_width(&s.content)).sum();
        assert_eq!(width, 10);
    }

    fn make_tab_with_items(items: Vec<ChatItem>) -> Tab {
        let (_tx, rx) = tokio::sync::broadcast::channel(1);
        let history = std::env::temp_dir().join("phx-test-display");
        let mut tab = Tab::new("test".into(), rx, history);
        tab.chat_lines = items;
        tab
    }

    #[test]
    fn compute_pairs_sequential_tool_call_and_result() {
        let theme = crate::tui::theme::default_theme();
        let tab = make_tab_with_items(vec![
            ChatItem::Line(ChatLine {
                role: Role::ToolCall,
                content: "bash > ls".into(),
            }),
            ChatItem::Line(ChatLine {
                role: Role::ToolResult,
                content: "file1\nfile2\nfile3".into(),
            }),
        ]);
        let lines = compute_display_lines(Some(&tab), &theme, false, 0, (80, 80), 0, &[]);
        let has_tool_detail = lines.iter().any(|dl| dl.tool_detail_idx.is_some());
        assert!(has_tool_detail, "compact card should have tool_detail_idx");
        let detail_line = lines
            .iter()
            .find(|dl| dl.tool_detail_idx.is_some())
            .unwrap();
        assert_eq!(detail_line.tool_detail_idx, Some(1));
        let text: String = detail_line.spans.iter().map(|(t, _)| t.as_str()).collect();
        assert!(text.contains("bash"), "should show tool name");
        assert!(text.contains("3 lines"), "should show line count summary");
    }

    #[test]
    fn compute_pairs_parallel_tool_calls() {
        let theme = crate::tui::theme::default_theme();
        let tab = make_tab_with_items(vec![
            ChatItem::Line(ChatLine {
                role: Role::ToolCall,
                content: "bash > ls".into(),
            }),
            ChatItem::Line(ChatLine {
                role: Role::ToolCall,
                content: "read > src/main.rs".into(),
            }),
            ChatItem::Line(ChatLine {
                role: Role::ToolResult,
                content: "file1".into(),
            }),
            ChatItem::Line(ChatLine {
                role: Role::ToolResult,
                content: "fn main() {}".into(),
            }),
        ]);
        let lines = compute_display_lines(Some(&tab), &theme, false, 0, (80, 80), 0, &[]);
        let detail_lines: Vec<_> = lines
            .iter()
            .filter(|dl| dl.tool_detail_idx.is_some())
            .collect();
        assert_eq!(detail_lines.len(), 2, "should have 2 compact cards");
        assert_eq!(detail_lines[0].tool_detail_idx, Some(2));
        assert_eq!(detail_lines[1].tool_detail_idx, Some(3));
    }

    #[test]
    fn compute_unmatched_tool_call_renders_normally() {
        let theme = crate::tui::theme::default_theme();
        let tab = make_tab_with_items(vec![ChatItem::Line(ChatLine {
            role: Role::ToolCall,
            content: "bash > running".into(),
        })]);
        let lines = compute_display_lines(Some(&tab), &theme, false, 0, (80, 80), 0, &[]);
        let has_tool_detail = lines.iter().any(|dl| dl.tool_detail_idx.is_some());
        assert!(
            !has_tool_detail,
            "unmatched call should not produce compact card"
        );
    }

    #[test]
    fn compute_partial_parallel_first_call_renders_normally() {
        let theme = crate::tui::theme::default_theme();
        let tab = make_tab_with_items(vec![
            ChatItem::Line(ChatLine {
                role: Role::ToolCall,
                content: "bash > ls".into(),
            }),
            ChatItem::Line(ChatLine {
                role: Role::ToolCall,
                content: "read > file".into(),
            }),
            ChatItem::Line(ChatLine {
                role: Role::ToolResult,
                content: "output".into(),
            }),
        ]);
        let lines = compute_display_lines(Some(&tab), &theme, false, 0, (80, 80), 0, &[]);
        let detail_lines: Vec<_> = lines
            .iter()
            .filter(|dl| dl.tool_detail_idx.is_some())
            .collect();
        assert_eq!(
            detail_lines.len(),
            1,
            "only the second call should pair with the result"
        );
    }

    #[test]
    fn compute_no_results_renders_all_normally() {
        let theme = crate::tui::theme::default_theme();
        let tab = make_tab_with_items(vec![
            ChatItem::Line(ChatLine {
                role: Role::ToolCall,
                content: "bash > ls".into(),
            }),
            ChatItem::Line(ChatLine {
                role: Role::ToolCall,
                content: "read > file".into(),
            }),
        ]);
        let lines = compute_display_lines(Some(&tab), &theme, false, 0, (80, 80), 0, &[]);
        let has_tool_detail = lines.iter().any(|dl| dl.tool_detail_idx.is_some());
        assert!(!has_tool_detail, "no results means no compact cards");
    }
}

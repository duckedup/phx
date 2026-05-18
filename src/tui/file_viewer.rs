use std::path::{Path, PathBuf};

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

use crate::tui::rendering::measure::display_width;
use crate::tui::theme::Theme;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

pub struct FileTab {
    pub path: PathBuf,
    pub display_name: String,
    pub lines: Vec<Vec<(String, Style)>>,
    pub scroll_offset: usize,
    pub total_lines: usize,
    pub language: String,
    pub is_virtual: bool,
}

impl FileTab {
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    pub fn scroll_down(&mut self, n: usize, visible: usize) {
        let max = self.total_lines.saturating_sub(visible);
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }
}

pub struct FileViewerState {
    pub tabs: Vec<FileTab>,
    pub active_idx: Option<usize>,
    pub hovered_close: Option<usize>,
    syntax_set: SyntaxSet,
}

impl FileViewerState {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active_idx: None,
            hovered_close: None,
            syntax_set: SyntaxSet::load_defaults_newlines(),
        }
    }

    pub fn open_file(&mut self, path: &Path, tui_theme: &Theme) -> Result<(), String> {
        if let Some(idx) = self.tabs.iter().position(|t| t.path == path) {
            self.active_idx = Some(idx);
            return Ok(());
        }

        let content =
            std::fs::read_to_string(path).map_err(|e| format!("Cannot read file: {e}"))?;

        let (highlighted, language) = self.highlight_content(&content, path, tui_theme);

        let total_lines = highlighted.len();
        let display_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        self.tabs.push(FileTab {
            path: path.to_path_buf(),
            display_name,
            lines: highlighted,
            scroll_offset: 0,
            total_lines,
            language,
            is_virtual: false,
        });
        self.active_idx = Some(self.tabs.len() - 1);
        Ok(())
    }

    pub fn open_content(&mut self, name: &str, content: &str, tui_theme: &Theme) {
        if let Some(idx) = self
            .tabs
            .iter()
            .position(|t| t.is_virtual && t.display_name == name)
        {
            self.active_idx = Some(idx);
            return;
        }

        let lines = highlight_tool_output(content, tui_theme);
        let total_lines = lines.len();

        self.tabs.push(FileTab {
            path: PathBuf::new(),
            display_name: name.to_string(),
            lines,
            scroll_offset: 0,
            total_lines,
            language: "Tool Result".to_string(),
            is_virtual: true,
        });
        self.active_idx = Some(self.tabs.len() - 1);
    }

    fn highlight_content(
        &self,
        content: &str,
        path: &Path,
        tui_theme: &Theme,
    ) -> (Vec<Vec<(String, Style)>>, String) {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let syntax = self
            .syntax_set
            .find_syntax_by_extension(ext)
            .unwrap_or_else(|| self.syntax_set.find_syntax_plain_text());

        let mut parse_state = ParseState::new(syntax);
        let mut scope_stack = ScopeStack::new();

        let mut result = Vec::new();
        for line in content.lines() {
            let expanded = line.replace('\t', "    ");
            let line_nl = format!("{expanded}\n");
            let ops = parse_state
                .parse_line(&line_nl, &self.syntax_set)
                .unwrap_or_default();

            let mut parts: Vec<(String, Style)> = Vec::new();
            let mut prev = 0usize;

            for (byte_pos, op) in &ops {
                let pos = *byte_pos;
                if pos > prev {
                    let text = &line_nl[prev..pos];
                    let cleaned = text.trim_end_matches('\n');
                    if !cleaned.is_empty() {
                        let style = scope_to_style(&scope_stack, tui_theme);
                        parts.push((cleaned.to_string(), style));
                    }
                }
                scope_stack.apply(op).ok();
                prev = pos;
            }
            if prev < line_nl.len() {
                let text = &line_nl[prev..];
                let cleaned = text.trim_end_matches('\n');
                if !cleaned.is_empty() {
                    let style = scope_to_style(&scope_stack, tui_theme);
                    parts.push((cleaned.to_string(), style));
                }
            }

            result.push(parts);
        }

        if result.is_empty() {
            result.push(Vec::new());
        }

        (result, syntax.name.clone())
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        self.tabs.remove(idx);
        if self.tabs.is_empty() {
            self.active_idx = None;
        } else if let Some(active) = self.active_idx {
            if active == idx {
                self.active_idx = if idx > 0 { Some(idx - 1) } else { Some(0) };
            } else if active > idx {
                self.active_idx = Some(active - 1);
            }
        }
    }

    pub fn active_tab(&self) -> Option<&FileTab> {
        self.active_idx.and_then(|i| self.tabs.get(i))
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut FileTab> {
        self.active_idx.and_then(|i| self.tabs.get_mut(i))
    }

    pub fn has_tabs(&self) -> bool {
        !self.tabs.is_empty()
    }

    pub fn is_viewing_file(&self) -> bool {
        self.active_idx.is_some()
    }

    pub fn switch_to_chat(&mut self) {
        self.active_idx = None;
    }

    pub fn switch_to_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            self.active_idx = Some(idx);
        }
    }
}

// ---------------------------------------------------------------------------
// Tool output highlighting (standalone, reusable)
// ---------------------------------------------------------------------------

pub fn highlight_tool_output(content: &str, theme: &Theme) -> Vec<Vec<(String, Style)>> {
    if crate::tui::rendering::diff::is_diff_content(content) {
        return highlight_diff_output(content, theme);
    }

    let fg = Style::default().fg(theme.foreground);
    let error_style = Style::default().fg(theme.error);
    let add_style = Style::default().fg(theme.diff_add);
    let del_style = Style::default().fg(theme.diff_delete);
    let info_style = Style::default()
        .fg(theme.info)
        .add_modifier(Modifier::ITALIC);

    let cmd_style = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD);

    let mut result = Vec::new();
    for line in content.lines() {
        let expanded = line.replace('\t', "    ");
        let trimmed = expanded.trim_start();
        let style = if trimmed.starts_with("$ ") {
            cmd_style
        } else if trimmed.starts_with("error") || trimmed.starts_with("Error") {
            error_style
        } else if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
            add_style
        } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
            del_style
        } else if trimmed.starts_with("@@") {
            info_style
        } else {
            fg
        };
        result.push(vec![(expanded, style)]);
    }
    if result.is_empty() {
        result.push(vec![(
            "(empty)".to_string(),
            Style::default().fg(theme.dim()),
        )]);
    }
    result
}

fn highlight_diff_output(content: &str, theme: &Theme) -> Vec<Vec<(String, Style)>> {
    let border = Style::default().fg(theme.tool_border());
    let dim = Style::default().fg(theme.dim());
    let info_bold = Style::default().fg(theme.info).add_modifier(Modifier::BOLD);

    let del_bg = Theme::blend(theme.diff_delete, theme.background, 0.80);
    let add_bg = Theme::blend(theme.diff_add, theme.background, 0.80);
    let del_style = Style::default().fg(theme.diff_delete).bg(del_bg);
    let del_text = Style::default().fg(theme.foreground).bg(del_bg);
    let del_ln = Style::default().fg(theme.dim()).bg(del_bg);
    let add_style = Style::default().fg(theme.diff_add).bg(add_bg);
    let add_text = Style::default().fg(theme.foreground).bg(add_bg);
    let add_ln = Style::default().fg(theme.dim()).bg(add_bg);

    let mut header = String::new();
    let mut old_lines: Vec<(usize, String)> = Vec::new();
    let mut new_lines: Vec<(usize, String)> = Vec::new();

    for line in content.lines() {
        if line.starts_with("edited ") {
            header = line.to_string();
        } else if let Some(rest) = line.strip_prefix("- ") {
            let (ln, text) = parse_diff_line_number(rest);
            old_lines.push((ln, text.replace('\t', "    ")));
        } else if let Some(rest) = line.strip_prefix("+ ") {
            let (ln, text) = parse_diff_line_number(rest);
            new_lines.push((ln, text.replace('\t', "    ")));
        }
    }

    let short_path = header
        .strip_prefix("edited ")
        .and_then(|s| s.split(':').next())
        .unwrap_or("file");
    let count_text = header.split(": ").nth(1).unwrap_or("replaced");

    let mut result = Vec::new();

    result.push(vec![
        ("╭─ ".to_string(), border),
        (short_path.to_string(), info_bold),
        (" ── ".to_string(), border),
        (count_text.to_string(), dim),
    ]);

    for (ln, text) in &old_lines {
        result.push(vec![
            ("│ ".to_string(), border),
            (format!("{:>3} ", ln), del_ln),
            ("− ".to_string(), del_style),
            (text.clone(), del_text),
        ]);
    }

    for (ln, text) in &new_lines {
        result.push(vec![
            ("│ ".to_string(), border),
            (format!("{:>3} ", ln), add_ln),
            ("+ ".to_string(), add_style),
            (text.clone(), add_text),
        ]);
    }

    result.push(vec![
        ("╰─ ".to_string(), border),
        ("✓".to_string(), Style::default().fg(theme.success)),
        (" applied".to_string(), dim),
    ]);

    result
}

fn parse_diff_line_number(rest: &str) -> (usize, &str) {
    if let Some(colon_pos) = rest.find(':')
        && let Ok(ln) = rest[..colon_pos].parse::<usize>()
    {
        return (ln, &rest[colon_pos + 1..]);
    }
    (0, rest)
}

// ---------------------------------------------------------------------------
// Tab bar rendering
// ---------------------------------------------------------------------------

pub const TAB_BAR_HEIGHT: u16 = 3;

pub fn render_tab_bar(frame: &mut Frame, area: Rect, state: &FileViewerState, theme: &Theme) {
    let bg = Style::default().bg(theme.background);
    let dim = theme.dim();
    let dim_border = Theme::blend(theme.foreground, theme.background, 0.8);
    let w = area.width as usize;

    let mut top_spans: Vec<Span> = Vec::new();
    let mut mid_spans: Vec<Span> = Vec::new();
    let mut bot_spans: Vec<Span> = Vec::new();

    top_spans.push(Span::styled("  ", bg));
    mid_spans.push(Span::styled("  ", bg));
    bot_spans.push(Span::styled("  ", bg));

    // Chat tab
    let chat_active = !state.is_viewing_file();
    let chat_border = if chat_active {
        theme.accent
    } else {
        dim_border
    };
    let bs = Style::default().fg(chat_border);
    let chat_label = " Chat ";
    let chat_w = chat_label.len();
    top_spans.push(Span::styled(
        format!("\u{256d}{}\u{256e}", "\u{2500}".repeat(chat_w)),
        bs,
    ));
    mid_spans.push(Span::styled("\u{2502}", bs));
    if chat_active {
        mid_spans.push(Span::styled(
            chat_label,
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        mid_spans.push(Span::styled(chat_label, Style::default().fg(dim)));
    }
    mid_spans.push(Span::styled("\u{2502}", bs));
    bot_spans.push(Span::styled(
        format!("\u{2570}{}\u{256f}", "\u{2500}".repeat(chat_w)),
        bs,
    ));

    // File tabs
    for (i, tab) in state.tabs.iter().enumerate() {
        let is_active = state.active_idx == Some(i);
        let tab_border = if is_active { theme.accent } else { dim_border };
        let tbs = Style::default().fg(tab_border);

        top_spans.push(Span::styled(" ", bg));
        mid_spans.push(Span::styled(" ", bg));
        bot_spans.push(Span::styled(" ", bg));

        let close_hovered = state.hovered_close == Some(i);
        let close_style = if close_hovered {
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default().fg(theme.dim())
        } else {
            Style::default().fg(Theme::blend(dim_border, theme.background, 0.5))
        };

        let name = &tab.display_name;
        let inner_w = 1 + display_width(name) + 2 + 1; // " name  ×"
        top_spans.push(Span::styled(
            format!("\u{256d}{}\u{256e}", "\u{2500}".repeat(inner_w)),
            tbs,
        ));

        mid_spans.push(Span::styled("\u{2502}", tbs));
        mid_spans.push(Span::styled(" ", bg));
        if is_active {
            mid_spans.push(Span::styled(
                name.clone(),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            mid_spans.push(Span::styled(name.clone(), Style::default().fg(dim)));
        }
        mid_spans.push(Span::styled(" ", bg));
        mid_spans.push(Span::styled("\u{00d7}", close_style));
        mid_spans.push(Span::styled(" ", bg));
        mid_spans.push(Span::styled("\u{2502}", tbs));

        bot_spans.push(Span::styled(
            format!("\u{2570}{}\u{256f}", "\u{2500}".repeat(inner_w)),
            tbs,
        ));
    }

    // Fill remaining width
    for spans in [&mut top_spans, &mut mid_spans, &mut bot_spans] {
        let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        let remaining = w.saturating_sub(used);
        if remaining > 0 {
            spans.push(Span::styled(" ".repeat(remaining), bg));
        }
    }

    let lines = vec![
        Line::from(top_spans),
        Line::from(mid_spans),
        Line::from(bot_spans),
    ];
    frame.render_widget(Paragraph::new(lines).style(bg), area);
}

// ---------------------------------------------------------------------------
// Tab bar hit testing
// ---------------------------------------------------------------------------

pub enum TabBarHit {
    Chat,
    FileTab(usize),
    CloseTab(usize),
}

pub fn tab_bar_hit_test(
    area: Rect,
    row: u16,
    col: u16,
    state: &FileViewerState,
) -> Option<TabBarHit> {
    let tab_row = area.y + 1;
    if row != tab_row || col < area.x || col >= area.x + area.width {
        return None;
    }

    let rel = (col - area.x) as usize;

    // "  " (2) + "│" (1) + " Chat " (6) + "│" (1) = 10
    let chat_start = 2;
    let chat_end = chat_start + 8; // includes borders
    if rel >= chat_start && rel < chat_end {
        return Some(TabBarHit::Chat);
    }

    // " " (1) gap between tabs
    let mut pos = chat_end;
    for (i, tab) in state.tabs.iter().enumerate() {
        pos += 1; // " " gap
        // "│" (1) + " " (1) + name + " " (1) + "×" (1) + " " (1) + "│" (1)
        let name_len = display_width(&tab.display_name);
        let tab_start = pos; // "│"
        let close_col = pos + 1 + 1 + name_len + 1; // after "│ name "
        let tab_end = close_col + 1 + 1 + 1; // "× " + "│"

        if rel == close_col {
            return Some(TabBarHit::CloseTab(i));
        }
        if rel >= tab_start && rel < tab_end {
            return Some(TabBarHit::FileTab(i));
        }

        pos = tab_end;
    }

    None
}

// ---------------------------------------------------------------------------
// File content rendering
// ---------------------------------------------------------------------------

pub fn render_file_content(frame: &mut Frame, area: Rect, tab: &FileTab, theme: &Theme) {
    let visible = area.height as usize;
    let total_width = area.width as usize;
    let gutter_digits = if tab.total_lines == 0 {
        1
    } else {
        format!("{}", tab.total_lines).len()
    };
    let gutter_width = gutter_digits + 1; // e.g. "123 "
    let sep_width = 2; // "│ "
    let content_max = total_width.saturating_sub(gutter_width + sep_width);

    let gutter_style = Style::default().fg(theme.dim()).bg(theme.background);
    let sep_style = Style::default()
        .fg(theme.tool_border())
        .bg(theme.background);
    let bg = Style::default().bg(theme.background);

    for i in 0..visible {
        let line_idx = tab.scroll_offset + i;
        let y = area.y + i as u16;
        let mut spans: Vec<Span<'static>> = Vec::new();

        if line_idx < tab.lines.len() {
            let num = format!("{:>width$} ", line_idx + 1, width = gutter_digits);
            spans.push(Span::styled(num, gutter_style));
            spans.push(Span::styled("\u{2502} ", sep_style));

            let mut col = 0usize;
            for (text, style) in &tab.lines[line_idx] {
                let chars: usize = text.chars().count();
                if col + chars <= content_max {
                    spans.push(Span::styled(text.clone(), style.bg(theme.background)));
                    col += chars;
                } else {
                    let take = content_max.saturating_sub(col);
                    if take > 0 {
                        let truncated: String = text.chars().take(take).collect();
                        spans.push(Span::styled(truncated, style.bg(theme.background)));
                    }
                    break;
                }
            }
        } else {
            spans.push(Span::styled(" ".repeat(gutter_width), gutter_style));
            spans.push(Span::styled("\u{2502} ", sep_style));
        }

        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let remaining = total_width.saturating_sub(used);
        if remaining > 0 {
            spans.push(Span::styled(" ".repeat(remaining), bg));
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// File status bar (replaces input area when viewing)
// ---------------------------------------------------------------------------

pub fn render_file_status(frame: &mut Frame, area: Rect, tab: &FileTab, theme: &Theme) {
    let bg = theme.status_bar_bg();
    let fg = theme.status_bar_fg();
    let dim = theme.status_bar_dim();

    let path_display = if tab.is_virtual {
        std::borrow::Cow::Borrowed("(tool output)")
    } else {
        tab.path.to_string_lossy()
    };
    let scroll_pct = if tab.total_lines == 0 {
        100
    } else {
        ((tab.scroll_offset as f64 / tab.total_lines.max(1) as f64) * 100.0) as usize
    };

    let right = format!(" {}%  Ln {} ", scroll_pct, tab.scroll_offset + 1);
    let left = format!(
        "  {}  {} lines  {}",
        tab.language, tab.total_lines, path_display
    );

    let total_width = area.width as usize;
    let right_len = right.chars().count();
    let left_max = total_width.saturating_sub(right_len);
    let left_display: String = left.chars().take(left_max).collect();
    let left_len = left_display.chars().count();
    let gap = total_width.saturating_sub(left_len + right_len);

    let line = Line::from(vec![
        Span::styled(left_display, Style::default().fg(fg).bg(bg)),
        Span::styled(" ".repeat(gap), Style::default().bg(bg)),
        Span::styled(right, Style::default().fg(dim).bg(bg)),
    ]);

    frame.render_widget(Paragraph::new(line), area);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn scope_to_style(stack: &ScopeStack, theme: &Theme) -> Style {
    let scopes = stack.as_slice();
    for scope in scopes.iter().rev() {
        let s = scope.build_string();
        if s.starts_with("comment") {
            return Style::default()
                .fg(theme.dim())
                .add_modifier(Modifier::ITALIC);
        }
        if s.starts_with("string") {
            return Style::default().fg(theme.success);
        }
        if s.starts_with("constant.numeric") {
            return Style::default().fg(theme.warning);
        }
        if s.starts_with("constant.language") {
            return Style::default().fg(theme.warning);
        }
        if s.starts_with("constant") {
            return Style::default().fg(theme.warning);
        }
        if s.starts_with("keyword") || s.starts_with("storage") {
            return Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD);
        }
        if s.starts_with("entity.name.function") || s.starts_with("entity.name.method") {
            return Style::default().fg(theme.info);
        }
        if s.starts_with("entity.name.type")
            || s.starts_with("entity.name.class")
            || s.starts_with("entity.name.struct")
        {
            return Style::default().fg(theme.primary);
        }
        if s.starts_with("entity.name") {
            return Style::default().fg(theme.info);
        }
        if s.starts_with("support.function") || s.starts_with("support.method") {
            return Style::default().fg(theme.info);
        }
        if s.starts_with("support.type") || s.starts_with("support.class") {
            return Style::default().fg(theme.primary);
        }
        if s.starts_with("variable.parameter") {
            return Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::ITALIC);
        }
        if s.starts_with("variable") {
            return Style::default().fg(theme.foreground);
        }
        if s.starts_with("punctuation") {
            return Style::default().fg(theme.foreground);
        }
    }
    Style::default().fg(theme.foreground)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> Theme {
        crate::tui::theme::default_theme()
    }

    #[test]
    fn open_content_creates_virtual_tab() {
        let theme = test_theme();
        let mut fv = FileViewerState::new();
        fv.open_content("⚙ bash", "hello\nworld", &theme);

        assert_eq!(fv.tabs.len(), 1);
        assert_eq!(fv.active_idx, Some(0));
        let tab = &fv.tabs[0];
        assert!(tab.is_virtual);
        assert_eq!(tab.display_name, "⚙ bash");
        assert_eq!(tab.total_lines, 2);
        assert_eq!(tab.language, "Tool Result");
        assert!(tab.path.as_os_str().is_empty());
    }

    #[test]
    fn open_content_deduplicates_same_name() {
        let theme = test_theme();
        let mut fv = FileViewerState::new();
        fv.open_content("⚙ bash", "hello", &theme);
        fv.open_content("⚙ bash", "different content", &theme);

        assert_eq!(fv.tabs.len(), 1, "should reuse existing virtual tab");
        assert_eq!(fv.active_idx, Some(0));
    }

    #[test]
    fn open_content_different_names_creates_separate_tabs() {
        let theme = test_theme();
        let mut fv = FileViewerState::new();
        fv.open_content("⚙ bash", "hello", &theme);
        fv.open_content("⚙ read", "world", &theme);

        assert_eq!(fv.tabs.len(), 2);
        assert_eq!(fv.active_idx, Some(1));
    }

    #[test]
    fn virtual_and_file_tabs_coexist() {
        let theme = test_theme();
        let mut fv = FileViewerState::new();
        fv.open_content("⚙ bash", "output", &theme);

        let tmp = std::env::temp_dir().join("phx-test-fv-coexist.txt");
        std::fs::write(&tmp, "file content").unwrap();
        let _ = fv.open_file(&tmp, &theme);

        assert_eq!(fv.tabs.len(), 2);
        assert!(fv.tabs[0].is_virtual);
        assert!(!fv.tabs[1].is_virtual);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn close_virtual_tab() {
        let theme = test_theme();
        let mut fv = FileViewerState::new();
        fv.open_content("⚙ bash", "output", &theme);
        assert_eq!(fv.tabs.len(), 1);

        fv.close_tab(0);
        assert!(fv.tabs.is_empty());
        assert!(fv.active_idx.is_none());
    }

    #[test]
    fn highlight_plain_colors_diff_lines() {
        let theme = test_theme();
        let content = "+added\n-removed\n context\n@@hunk@@";
        let lines = highlight_tool_output(content, &theme);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0][0].1, Style::default().fg(theme.diff_add));
        assert_eq!(lines[1][0].1, Style::default().fg(theme.diff_delete));
        assert_eq!(lines[2][0].1, Style::default().fg(theme.foreground));
        assert_eq!(
            lines[3][0].1,
            Style::default()
                .fg(theme.info)
                .add_modifier(Modifier::ITALIC)
        );
    }

    #[test]
    fn highlight_plain_empty_content() {
        let theme = test_theme();
        let lines = highlight_tool_output("", &theme);
        assert_eq!(lines.len(), 1);
        assert!(lines[0][0].0.contains("empty"));
    }

    #[test]
    fn diff_content_renders_with_borders() {
        let theme = test_theme();
        let content = "edited /src/main.rs: replaced 1 occurrence(s)\n- old_line\n+ new_line\n";
        let lines = highlight_tool_output(content, &theme);

        let all_text: Vec<String> = lines
            .iter()
            .map(|parts| parts.iter().map(|(t, _)| t.as_str()).collect::<String>())
            .collect();

        assert!(all_text[0].contains("╭─"), "should have header border");
        assert!(all_text[0].contains("main.rs"), "should show filename");
        let has_minus = all_text
            .iter()
            .any(|l| l.contains("− ") && l.contains("old_line"));
        let has_plus = all_text
            .iter()
            .any(|l| l.contains("+ ") && l.contains("new_line"));
        assert!(has_minus, "should render old line");
        assert!(has_plus, "should render new line");
        let last = all_text.last().unwrap();
        assert!(last.contains("✓") && last.contains("applied"));
    }

    #[test]
    fn tab_hit_test_with_multibyte_name() {
        let theme = test_theme();
        let mut fv = FileViewerState::new();
        fv.open_content("⚙ bash", "output", &theme);

        let area = Rect::new(0, 0, 80, 3);
        let tab_row = area.y + 1;

        // Chat tab occupies cols 2..10
        assert!(matches!(
            tab_bar_hit_test(area, tab_row, 5, &fv),
            Some(TabBarHit::Chat)
        ));

        // Tool tab starts after chat (10) + gap (1) = col 11
        // inner_w = 1 + display_width("⚙ bash") + 2 + 1
        let name_dw = display_width("⚙ bash");
        let inner_w = 1 + name_dw + 2 + 1;
        // Tab occupies cols 11 .. 11 + 1 + inner_w + 1
        let tab_end = 11 + 1 + inner_w + 1;

        // Clicking inside the tab region should hit the tab
        let mid = 11 + 2;
        assert!(
            matches!(
                tab_bar_hit_test(area, tab_row, mid as u16, &fv),
                Some(TabBarHit::FileTab(0))
            ),
            "click inside tab region should hit the tab"
        );

        // Close button is at: 11 (border) + 1 (space) + name_dw (name) + 1 (space) = close col
        let close_col = 11 + 1 + 1 + name_dw + 1;
        assert!(
            matches!(
                tab_bar_hit_test(area, tab_row, close_col as u16, &fv),
                Some(TabBarHit::CloseTab(0))
            ),
            "click on close button should hit CloseTab"
        );

        // Past the tab end should be empty
        assert!(
            tab_bar_hit_test(area, tab_row, (tab_end + 2) as u16, &fv).is_none(),
            "click past tab should be empty"
        );
    }
}

use std::path::{Path, PathBuf};

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

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
        });
        self.active_idx = Some(self.tabs.len() - 1);
        Ok(())
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
        let inner_w = 1 + name.len() + 2 + 1; // " name  ×"
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
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
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
        let name_len = tab.display_name.chars().count();
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

    let path_display = tab.path.to_string_lossy();
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

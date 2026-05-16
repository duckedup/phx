use std::path::{Path, PathBuf};

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style as SyntectStyle, ThemeSet};
use syntect::parsing::SyntaxSet;

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
    syntax_set: SyntaxSet,
    theme_set: ThemeSet,
}

impl FileViewerState {
    pub fn new() -> Self {
        Self {
            tabs: Vec::new(),
            active_idx: None,
            syntax_set: SyntaxSet::load_defaults_newlines(),
            theme_set: ThemeSet::load_defaults(),
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

        let theme_name = pick_syntect_theme(tui_theme, &self.theme_set);
        let syntect_theme = &self.theme_set.themes[theme_name];
        let mut h = HighlightLines::new(syntax, syntect_theme);

        let mut result = Vec::new();
        for line in content.lines() {
            let expanded = line.replace('\t', "    ");
            let line_nl = format!("{expanded}\n");
            let ranges = h
                .highlight_line(&line_nl, &self.syntax_set)
                .unwrap_or_default();
            let parts: Vec<(String, Style)> = ranges
                .iter()
                .map(|(s, text)| {
                    let cleaned = text.trim_end_matches('\n');
                    (cleaned.to_string(), syntect_to_ratatui(s))
                })
                .filter(|(text, _)| !text.is_empty())
                .collect();
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
    let border_fg = Theme::blend(theme.foreground, theme.background, 0.85);
    let bg = Style::default().bg(theme.background);
    let dim = theme.dim();
    let w = area.width as usize;

    // Top border
    let top = Line::from(Span::styled(
        format!(
            "  \u{256d}{}\u{256e}",
            "\u{2500}".repeat(w.saturating_sub(4))
        ),
        Style::default().fg(border_fg),
    ));

    // Tab content
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled("  \u{2502} ", Style::default().fg(border_fg)));

    let chat_active = !state.is_viewing_file();
    if chat_active {
        spans.push(Span::styled(
            "\u{25c6} ",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            "Chat",
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ));
    } else {
        spans.push(Span::styled("\u{25c7} ", Style::default().fg(dim)));
        spans.push(Span::styled("Chat", Style::default().fg(dim)));
    }

    for (i, tab) in state.tabs.iter().enumerate() {
        let is_active = state.active_idx == Some(i);
        spans.push(Span::styled("    ", bg));

        if is_active {
            spans.push(Span::styled(
                "\u{25c6} ",
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                tab.display_name.clone(),
                Style::default()
                    .fg(theme.foreground)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled("  \u{00d7}", Style::default().fg(theme.error)));
        } else {
            spans.push(Span::styled("\u{25c7} ", Style::default().fg(dim)));
            spans.push(Span::styled(
                tab.display_name.clone(),
                Style::default().fg(dim),
            ));
            spans.push(Span::styled(
                "  \u{00d7}",
                Style::default().fg(Theme::blend(dim, theme.background, 0.5)),
            ));
        }
    }

    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let remaining = w.saturating_sub(used + 2);
    spans.push(Span::styled(
        format!("{} \u{2502}", " ".repeat(remaining)),
        Style::default().fg(border_fg),
    ));
    let mid = Line::from(spans);

    // Bottom border
    let bot = Line::from(Span::styled(
        format!(
            "  \u{2570}{}\u{256f}",
            "\u{2500}".repeat(w.saturating_sub(4))
        ),
        Style::default().fg(border_fg),
    ));

    let lines = vec![top, mid, bot];
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

    // "  │ " (4) + "◆ " (2) + "Chat" (4) = 10
    let chat_start = 4;
    let chat_end = chat_start + 6; // "◆ Chat"
    if rel >= chat_start && rel < chat_end {
        return Some(TabBarHit::Chat);
    }

    // "   " gap (3) between tabs
    let mut pos = chat_end;
    for (i, tab) in state.tabs.iter().enumerate() {
        pos += 3; // "   " gap
        let icon_len = 2; // "◆ "
        let name_len = tab.display_name.chars().count();
        let close_len = 2; // " ×"

        let tab_start = pos;
        let close_start = pos + icon_len + name_len;
        let tab_end = close_start + close_len;

        if rel >= close_start && rel < tab_end {
            return Some(TabBarHit::CloseTab(i));
        }
        if rel >= tab_start && rel < close_start {
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

fn syntect_to_ratatui(style: &SyntectStyle) -> Style {
    let fg = Color::Rgb(style.foreground.r, style.foreground.g, style.foreground.b);
    let mut s = Style::default().fg(fg);
    if style.font_style.contains(FontStyle::BOLD) {
        s = s.add_modifier(Modifier::BOLD);
    }
    if style.font_style.contains(FontStyle::ITALIC) {
        s = s.add_modifier(Modifier::ITALIC);
    }
    if style.font_style.contains(FontStyle::UNDERLINE) {
        s = s.add_modifier(Modifier::UNDERLINED);
    }
    s
}

fn pick_syntect_theme<'a>(theme: &Theme, theme_set: &'a ThemeSet) -> &'a str {
    let is_dark = match theme.background {
        Color::Rgb(r, g, b) => (0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64) < 128.0,
        _ => true,
    };
    if is_dark {
        if theme_set.themes.contains_key("base16-eighties.dark") {
            "base16-eighties.dark"
        } else {
            theme_set
                .themes
                .keys()
                .next()
                .map(|s| s.as_str())
                .unwrap_or("base16-eighties.dark")
        }
    } else if theme_set.themes.contains_key("InspiredGitHub") {
        "InspiredGitHub"
    } else {
        theme_set
            .themes
            .keys()
            .next()
            .map(|s| s.as_str())
            .unwrap_or("InspiredGitHub")
    }
}

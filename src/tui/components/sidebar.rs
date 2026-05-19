use std::collections::HashSet;

use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::session::orchestration::{ChildInfo, ChildStatus};
use crate::tui::theme::Theme;

pub const SIDEBAR_WIDTH: u16 = 28;

#[derive(Clone, Debug, PartialEq)]
pub enum SidebarSelection {
    Conductor,
    Agent(String),
}

pub struct SidebarState {
    pub agents: Vec<ChildInfo>,
    pub selected: SidebarSelection,
    pub scroll: usize,
    pub dismissed: HashSet<String>,
}

impl SidebarState {
    pub fn new() -> Self {
        Self {
            agents: Vec::new(),
            selected: SidebarSelection::Conductor,
            scroll: 0,
            dismissed: HashSet::new(),
        }
    }

    pub fn update(&mut self, mut agents: Vec<ChildInfo>) {
        agents.sort_by(|a, b| {
            a.elapsed_s
                .partial_cmp(&b.elapsed_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        agents.retain(|a| !self.dismissed.contains(&a.session_id));
        self.agents = agents;
    }

    pub fn dismiss(&mut self, session_id: &str) {
        self.dismissed.insert(session_id.to_string());
        self.agents.retain(|a| a.session_id != session_id);
    }

    pub fn visible_agents(&self) -> &[ChildInfo] {
        &self.agents
    }

    /// Adjust scroll so the selected item stays visible within `visible_height` lines.
    pub fn ensure_selected_visible(&mut self, visible_height: usize) {
        let line = match &self.selected {
            SidebarSelection::Conductor => 0,
            SidebarSelection::Agent(id) => {
                if let Some(pos) = self.agents.iter().position(|a| &a.session_id == id) {
                    2 + pos * 2
                } else {
                    0
                }
            }
        };
        let line_end = line + 1;
        if line < self.scroll {
            self.scroll = line;
        } else if line_end >= self.scroll + visible_height {
            self.scroll = line_end.saturating_sub(visible_height) + 1;
        }
    }
}

fn status_icon(status: &ChildStatus, theme: &Theme) -> (&'static str, Color) {
    match status {
        ChildStatus::Running => ("◆", theme.accent),
        ChildStatus::Done => ("◆", theme.success),
        ChildStatus::Queued => ("◇", theme.warning),
        ChildStatus::Error(_) => ("◆", theme.error),
        ChildStatus::Cancelled => ("◇", theme.dim()),
    }
}

fn status_label(status: &ChildStatus) -> &str {
    match status {
        ChildStatus::Running => "running",
        ChildStatus::Done => "done",
        ChildStatus::Queued => "queued",
        ChildStatus::Error(_) => "error",
        ChildStatus::Cancelled => "cancelled",
    }
}

fn agent_display_name(info: &ChildInfo) -> &str {
    &info.task
}

pub fn render_sidebar(frame: &mut Frame, area: Rect, state: &SidebarState, theme: &Theme) {
    let separator_fg = Theme::blend(theme.foreground, theme.background, 0.85);

    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(separator_fg))
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_height = inner.height as usize;
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        " CONDUCTOR",
        Style::default()
            .fg(theme.dim())
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    let conductor_selected = state.selected == SidebarSelection::Conductor;
    let sel_bg = Theme::blend(theme.accent, theme.background, 0.85);
    let bg = if conductor_selected {
        sel_bg
    } else {
        theme.background
    };

    lines.push(Line::from(vec![
        Span::styled(
            if conductor_selected { " ▸ " } else { "   " },
            Style::default().fg(theme.accent).bg(bg),
        ),
        Span::styled(
            "◆ ",
            Style::default()
                .fg(theme.accent)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Conductor",
            Style::default()
                .fg(if conductor_selected {
                    theme.accent
                } else {
                    theme.foreground
                })
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    if !state.agents.is_empty() {
        lines.push(Line::from(""));
    }

    for agent in &state.agents {
        let is_selected =
            matches!(&state.selected, SidebarSelection::Agent(id) if id == &agent.session_id);
        let (icon, icon_color) = status_icon(&agent.status, theme);
        let name = agent_display_name(agent);
        let max_name = (inner.width as usize).saturating_sub(5);
        let display_name: String = if name.chars().count() > max_name {
            name.chars()
                .take(max_name.saturating_sub(1))
                .chain(std::iter::once('…'))
                .collect()
        } else {
            name.to_string()
        };

        let bg = if is_selected {
            sel_bg
        } else {
            theme.background
        };

        lines.push(Line::from(vec![
            Span::styled(
                if is_selected { " ▸ " } else { "   " },
                Style::default().fg(theme.accent).bg(bg),
            ),
            Span::styled(format!("{icon} "), Style::default().fg(icon_color).bg(bg)),
            Span::styled(
                display_name,
                Style::default()
                    .fg(if is_selected {
                        theme.accent
                    } else {
                        theme.foreground
                    })
                    .bg(bg),
            ),
        ]));

        let mut detail_parts = vec![status_label(&agent.status).to_string()];
        if let Some(tool) = &agent.active_tool {
            let tool_short: String = if tool.chars().count() > 12 {
                tool.chars().take(12).collect()
            } else {
                tool.clone()
            };
            detail_parts.push(tool_short);
        }

        lines.push(Line::from(Span::styled(
            format!("     {}", detail_parts.join(" · ")),
            Style::default().fg(theme.dim()).bg(bg),
        )));
    }

    let scroll = state.scroll.min(lines.len().saturating_sub(visible_height));
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(visible_height)
        .collect();

    let paragraph = Paragraph::new(visible_lines).style(Style::default().bg(theme.background));
    frame.render_widget(paragraph, inner);
}

#[derive(Debug, Clone)]
pub enum HitResult {
    Select(SidebarSelection),
    Dismiss(String),
}

/// Returns a `HitResult` if the click landed on a panel item or dismiss button.
pub fn hit_test(area: Rect, row: u16, col: u16, state: &SidebarState) -> Option<HitResult> {
    use ratatui::widgets::{Block, BorderType, Borders, Padding};

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);

    if col < inner.x
        || col >= inner.x + inner.width
        || row < inner.y
        || row >= inner.y + inner.height
    {
        return None;
    }

    let rel_row = (row - inner.y) as usize;
    let rel_col = (col - inner.x) as usize;
    let scrolled = rel_row + state.scroll;

    if scrolled <= 1 {
        return Some(HitResult::Select(SidebarSelection::Conductor));
    }

    if state.agents.is_empty() {
        return None;
    }

    let agent_start = 2;
    for (i, agent) in state.agents.iter().enumerate() {
        let line = agent_start + i * 2;
        if scrolled >= line && scrolled < line + 2 {
            if is_agent_finished(&agent.status)
                && scrolled == line
                && rel_col + 3 >= inner.width as usize
            {
                return Some(HitResult::Dismiss(agent.session_id.clone()));
            }
            return Some(HitResult::Select(SidebarSelection::Agent(
                agent.session_id.clone(),
            )));
        }
    }

    None
}

fn is_agent_finished(status: &ChildStatus) -> bool {
    matches!(
        status,
        ChildStatus::Done | ChildStatus::Error(_) | ChildStatus::Cancelled
    )
}

pub fn collapsed_tab_rect(chat: Rect, agent_count: usize) -> Rect {
    let label_len = if agent_count > 0 {
        let digits = if agent_count >= 10 { 2 } else { 1 };
        // "◀ N agents (^B)"
        1 + 1 + digits + 1 + 6 + 1 + 4
    } else {
        // "◀ conductor (^B)"
        1 + 1 + 9 + 1 + 4
    };
    let w = (label_len as u16 + 4).min(chat.width.saturating_sub(4)); // +4 for border + padding
    let h: u16 = 3;
    if w < 8 || chat.height < h + 1 {
        return Rect::default();
    }
    Rect {
        x: chat.x + chat.width - w,
        y: chat.y + chat.height - h,
        width: w,
        height: h,
    }
}

pub fn render_collapsed_tab(frame: &mut Frame, chat: Rect, agent_count: usize, theme: &Theme) {
    use ratatui::widgets::{Clear, Padding};

    let area = collapsed_tab_rect(chat, agent_count);
    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(Clear, area);

    let border_fg = Theme::blend(theme.accent, theme.background, 0.5);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_fg))
        .style(Style::default().bg(theme.background))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let label = if agent_count > 0 {
        format!("{agent_count} agents")
    } else {
        "conductor".to_string()
    };

    let line = Line::from(vec![
        Span::styled("◀ ", Style::default().fg(theme.accent)),
        Span::styled(
            label,
            Style::default()
                .fg(theme.foreground)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (^B)", Style::default().fg(theme.dim())),
    ]);

    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.background)),
        inner,
    );
}

pub fn collapsed_tab_hit_test(chat: Rect, agent_count: usize, row: u16, col: u16) -> bool {
    let area = collapsed_tab_rect(chat, agent_count);
    col >= area.x && col < area.x + area.width && row >= area.y && row < area.y + area.height
}

pub fn render_agent_panel(
    frame: &mut Frame,
    area: Rect,
    state: &SidebarState,
    theme: &Theme,
    focused: bool,
    active_session_id: Option<&str>,
) {
    use ratatui::widgets::{Clear, Padding};

    let sel_bg = Theme::blend(theme.accent, theme.background, 0.85);
    let separator_fg = if focused {
        theme.accent
    } else {
        Theme::blend(theme.accent, theme.background, 0.5)
    };

    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(separator_fg))
        .style(Style::default().bg(theme.background))
        .padding(Padding::horizontal(1));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    let conductor_sel = state.selected == SidebarSelection::Conductor;
    let conductor_active = active_session_id.is_none();
    let cond_highlight = (conductor_sel && focused) || conductor_active;
    let cond_bg = if cond_highlight {
        sel_bg
    } else {
        theme.background
    };
    let cond_fg = if cond_highlight {
        theme.accent
    } else {
        theme.foreground
    };
    lines.push(Line::from(vec![
        Span::styled(
            if conductor_sel && focused {
                "▸ "
            } else {
                "  "
            },
            Style::default().fg(theme.accent).bg(cond_bg),
        ),
        Span::styled(
            "◆ Conductor",
            Style::default()
                .fg(cond_fg)
                .bg(cond_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    lines.push(Line::from(""));

    for agent in &state.agents {
        let is_selected =
            matches!(&state.selected, SidebarSelection::Agent(id) if id == &agent.session_id);
        let is_active = active_session_id == Some(agent.session_id.as_str());
        let (icon, icon_color) = status_icon(&agent.status, theme);
        let name = agent_display_name(agent);
        let finished = is_agent_finished(&agent.status);
        let prefix_width = 4; // "▸ " (2) + "◆ " (2)
        let suffix_width = if finished { 2 } else { 0 }; // "✕ "
        let max_name = (inner.width as usize).saturating_sub(prefix_width + suffix_width);
        let display_name: String = if name.chars().count() > max_name {
            name.chars()
                .take(max_name.saturating_sub(1))
                .chain(std::iter::once('…'))
                .collect()
        } else {
            name.to_string()
        };

        let highlight = is_selected && focused;
        let bg = if highlight {
            sel_bg
        } else if is_active {
            Theme::blend(theme.accent, theme.background, 0.92)
        } else {
            theme.background
        };
        let fg = if highlight || is_active {
            theme.accent
        } else {
            theme.foreground
        };

        let mut spans = vec![
            Span::styled(
                if highlight { "▸ " } else { "  " },
                Style::default().fg(theme.accent).bg(bg),
            ),
            Span::styled(format!("{icon} "), Style::default().fg(icon_color).bg(bg)),
            Span::styled(display_name.clone(), Style::default().fg(fg).bg(bg)),
        ];

        if finished {
            let name_width = 2 + 2 + display_name.chars().count();
            let pad = (inner.width as usize).saturating_sub(name_width + 2);
            spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
            spans.push(Span::styled("✕ ", Style::default().fg(theme.dim()).bg(bg)));
        }

        lines.push(Line::from(spans));

        let mut detail = status_label(&agent.status).to_string();
        if let Some(tool) = &agent.active_tool {
            let max_tool = (inner.width as usize).saturating_sub(detail.len() + 8);
            let short: String = if tool.chars().count() > max_tool {
                let t: String = tool.chars().take(max_tool.saturating_sub(1)).collect();
                format!("{t}\u{2026}")
            } else {
                tool.clone()
            };
            detail.push_str(&format!(" \u{00b7} {short}"));
        }
        lines.push(Line::from(Span::styled(
            format!("    {detail}"),
            Style::default().fg(theme.dim()).bg(bg),
        )));
    }

    let visible = inner.height as usize;
    let scroll = state.scroll.min(lines.len().saturating_sub(visible));
    let visible_lines: Vec<Line> = lines.into_iter().skip(scroll).take(visible).collect();

    frame.render_widget(
        Paragraph::new(visible_lines).style(Style::default().bg(theme.background)),
        inner,
    );
}

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
}

impl SidebarState {
    pub fn new() -> Self {
        Self {
            agents: Vec::new(),
            selected: SidebarSelection::Conductor,
            scroll: 0,
        }
    }

    pub fn update(&mut self, mut agents: Vec<ChildInfo>) {
        agents.sort_by(|a, b| {
            a.elapsed_s
                .partial_cmp(&b.elapsed_s)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        self.agents = agents;
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
        let max_name = (inner.width as usize).saturating_sub(8);
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

/// Returns `Some(SidebarSelection)` if the click landed on a panel item.
pub fn hit_test(area: Rect, row: u16, col: u16, state: &SidebarState) -> Option<SidebarSelection> {
    if col < area.x || col >= area.x + area.width || row < area.y || row >= area.y + area.height {
        return None;
    }

    // inner_y: 0-based line inside the border (top border = 1 row)
    let inner_y = (row - area.y).saturating_sub(1) as usize;

    // Line 0: ◆ Conductor
    if inner_y == 0 {
        return Some(SidebarSelection::Conductor);
    }

    // Line 1: "waiting…" (no agents) or blank
    // Line 2: blank separator (only when agents exist)
    let agent_start = if state.agents.is_empty() {
        return None;
    } else {
        2
    };

    for (i, agent) in state.agents.iter().enumerate() {
        let line = agent_start + i * 2;
        if inner_y >= line && inner_y < line + 2 {
            return Some(SidebarSelection::Agent(agent.session_id.clone()));
        }
    }

    None
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

    if state.agents.is_empty() {
        lines.push(Line::from(Span::styled(
            "  waiting…",
            Style::default().fg(theme.dim()),
        )));
    }

    lines.push(Line::from(""));

    for agent in &state.agents {
        let is_selected =
            matches!(&state.selected, SidebarSelection::Agent(id) if id == &agent.session_id);
        let is_active = active_session_id == Some(agent.session_id.as_str());
        let (icon, icon_color) = status_icon(&agent.status, theme);
        let name = agent_display_name(agent);
        let max_name = (inner.width as usize).saturating_sub(8);
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

        lines.push(Line::from(vec![
            Span::styled(
                if highlight { "▸ " } else { "  " },
                Style::default().fg(theme.accent).bg(bg),
            ),
            Span::styled(format!("{icon} "), Style::default().fg(icon_color).bg(bg)),
            Span::styled(display_name, Style::default().fg(fg).bg(bg)),
        ]));

        let mut detail = status_label(&agent.status).to_string();
        if let Some(tool) = &agent.active_tool {
            let short: String = if tool.chars().count() > 14 {
                tool.chars().take(14).collect()
            } else {
                tool.clone()
            };
            detail.push_str(&format!(" · {short}"));
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

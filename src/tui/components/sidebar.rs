use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::session::orchestration::{ChildInfo, ChildStatus};
use crate::tui::theme::Theme;

pub const SIDEBAR_WIDTH: u16 = 26;

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

fn status_icon(status: &ChildStatus) -> (&str, Color) {
    match status {
        ChildStatus::Running => ("●", Color::Green),
        ChildStatus::Done => ("✓", Color::Cyan),
        ChildStatus::Queued => ("○", Color::Yellow),
        ChildStatus::Error(_) => ("✗", Color::Red),
        ChildStatus::Cancelled => ("◌", Color::DarkGray),
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

fn agent_display_name(info: &ChildInfo) -> String {
    let model_short = info.model.rsplit('/').next().unwrap_or(&info.model);
    let model_short: String = if model_short.chars().count() > 14 {
        model_short.chars().take(14).collect()
    } else {
        model_short.to_string()
    };
    format!("{}/{}", info.provider, model_short)
}

pub fn render_sidebar(frame: &mut Frame, area: Rect, state: &SidebarState, theme: &Theme) {
    let block = Block::default()
        .title(" CONDUCTOR ")
        .title_style(
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(theme.dim()))
        .style(Style::default().bg(theme.background));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible_height = inner.height as usize;
    let mut lines: Vec<Line> = Vec::new();

    // -- Conductor entry (always first) --
    let conductor_selected = state.selected == SidebarSelection::Conductor;
    let bg = if conductor_selected {
        theme.user_bubble_bg()
    } else {
        theme.background
    };

    lines.push(Line::from(vec![
        Span::styled(
            if conductor_selected { " > " } else { "   " },
            Style::default().fg(theme.accent).bg(bg),
        ),
        Span::styled("● ", Style::default().fg(Color::Magenta).bg(bg)),
        Span::styled(
            "Conductor",
            Style::default()
                .fg(theme.foreground)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]));

    if !state.agents.is_empty() {
        lines.push(Line::from(""));
    }

    // -- Child agents --
    for (i, agent) in state.agents.iter().enumerate() {
        let is_selected =
            matches!(&state.selected, SidebarSelection::Agent(id) if id == &agent.session_id);
        let (icon, icon_color) = status_icon(&agent.status);
        let name = agent_display_name(agent);

        let bg = if is_selected {
            theme.user_bubble_bg()
        } else {
            theme.background
        };

        let prefix = if is_selected { " > " } else { "   " };
        let tree = if i + 1 < state.agents.len() {
            "├─"
        } else {
            "└─"
        };

        lines.push(Line::from(vec![
            Span::styled(prefix, Style::default().fg(theme.accent).bg(bg)),
            Span::styled(format!("{tree} "), Style::default().fg(theme.dim()).bg(bg)),
            Span::styled(format!("{icon} "), Style::default().fg(icon_color).bg(bg)),
            Span::styled(name, Style::default().fg(theme.foreground).bg(bg)),
        ]));

        let status_text = format!("        {}", status_label(&agent.status));
        lines.push(Line::from(vec![Span::styled(
            status_text,
            Style::default().fg(theme.dim()).bg(bg),
        )]));

        if let Some(tool) = &agent.active_tool {
            let tool_short: String = if tool.chars().count() > 16 {
                tool.chars().take(16).collect()
            } else {
                tool.clone()
            };
            lines.push(Line::from(vec![Span::styled(
                format!("        ⚡ {tool_short}"),
                Style::default().fg(theme.warning).bg(bg),
            )]));
        }
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

/// Returns `Some(SidebarSelection)` if the click landed on a sidebar item.
pub fn hit_test(area: Rect, row: u16, col: u16, state: &SidebarState) -> Option<SidebarSelection> {
    if col < area.x || col >= area.x + area.width || row < area.y || row >= area.y + area.height {
        return None;
    }

    let inner_y = row - area.y - 1; // -1 for top border
    let mut current_line = 0u16;

    // Conductor entry = 1 line
    if inner_y == current_line {
        return Some(SidebarSelection::Conductor);
    }
    current_line += 1;

    // Blank separator between conductor and agents
    if !state.agents.is_empty() {
        current_line += 1;
    }

    for agent in &state.agents {
        let lines_for_agent = if agent.active_tool.is_some() { 3 } else { 2 };

        if inner_y >= current_line && inner_y < current_line + lines_for_agent {
            return Some(SidebarSelection::Agent(agent.session_id.clone()));
        }
        current_line += lines_for_agent;
    }

    None
}

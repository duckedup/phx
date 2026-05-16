use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::components::dialog as d;
use crate::tui::theme::Theme;

const STEPS: &[&str] = &["Conductor", "Agents", "Tracker"];

// ── Data ────────────────────────────────────────────────────────

struct ModelEntry {
    id: String,
    provider: String,
    model_name: String,
    cost: String,
}

struct TrackerEntry {
    id: &'static str,
    label: &'static str,
    desc: &'static str,
}

const TRACKERS: &[TrackerEntry] = &[
    TrackerEntry {
        id: "beads",
        label: "Beads",
        desc: "Git-native issue tracking (bd CLI)",
    },
    TrackerEntry {
        id: "linear",
        label: "Linear",
        desc: "Linear project management (linear-mg CLI)",
    },
    TrackerEntry {
        id: "none",
        label: "None",
        desc: "No tracker — tasks given directly",
    },
];

/// A row in the flat display list — either a group header or a selectable model.
enum Row {
    Header(String),
    Model(usize),
}

// ── Step ────────────────────────────────────────────────────────

enum Step {
    ConductorModel,
    AgentModel,
    Tracker,
}

impl Step {
    fn index(&self) -> usize {
        match self {
            Step::ConductorModel => 0,
            Step::AgentModel => 1,
            Step::Tracker => 2,
        }
    }
}

// ── Public API ──────────────────────────────────────────────────

pub enum SetupAction {
    None,
    Cancelled,
    Complete {
        conductor_provider: String,
        conductor_model: String,
        agent_provider: String,
        agent_model: String,
        tracker: Option<String>,
    },
}

pub struct ConductorSetup {
    step: Step,
    models: Vec<ModelEntry>,
    rows: Vec<Row>,
    selected: usize,
    conductor_provider: String,
    conductor_model: String,
    agent_provider: String,
    agent_model: String,
}

impl ConductorSetup {
    pub fn new(model_items: Vec<(String, String, String)>) -> Self {
        let models: Vec<ModelEntry> = model_items
            .into_iter()
            .map(|(id, label, desc)| {
                let (provider, model_name) = label
                    .split_once('/')
                    .map(|(p, m)| (p.to_string(), m.to_string()))
                    .unwrap_or_else(|| (label.clone(), label.clone()));
                ModelEntry {
                    id,
                    provider,
                    model_name,
                    cost: desc,
                }
            })
            .collect();

        let rows = build_grouped_rows(&models);

        Self {
            step: Step::ConductorModel,
            models,
            rows,
            selected: 0,
            conductor_provider: String::new(),
            conductor_model: String::new(),
            agent_provider: String::new(),
            agent_model: String::new(),
        }
    }

    fn reset_selection(&mut self) {
        self.selected = 0;
    }

    fn selected_model_idx(&self) -> Option<usize> {
        let mut model_pos = 0;
        for row in &self.rows {
            match row {
                Row::Header(_) => {}
                Row::Model(idx) => {
                    if model_pos == self.selected {
                        return Some(*idx);
                    }
                    model_pos += 1;
                }
            }
        }
        None
    }

    fn selectable_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| matches!(r, Row::Model(_)))
            .count()
    }

    // ── Key handlers ────────────────────────────────────────────

    pub fn handle_key(&mut self, key: KeyEvent) -> SetupAction {
        match self.step {
            Step::ConductorModel | Step::AgentModel => self.handle_model_key(key),
            Step::Tracker => self.handle_tracker_key(key),
        }
    }

    fn handle_model_key(&mut self, key: KeyEvent) -> SetupAction {
        let count = self.selectable_count();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if self.selected > 0 => self.selected -= 1,
            KeyCode::Down | KeyCode::Char('j') if self.selected + 1 < count => self.selected += 1,
            KeyCode::Enter => {
                if let Some(idx) = self.selected_model_idx() {
                    let entry = &self.models[idx];
                    if let Some((provider, model)) = entry.id.split_once('/') {
                        match self.step {
                            Step::ConductorModel => {
                                self.conductor_provider = provider.to_string();
                                self.conductor_model = model.to_string();
                                self.step = Step::AgentModel;
                                self.reset_selection();
                            }
                            Step::AgentModel => {
                                self.agent_provider = provider.to_string();
                                self.agent_model = model.to_string();
                                self.step = Step::Tracker;
                                self.selected = 0;
                            }
                            _ => {}
                        }
                    }
                }
            }
            KeyCode::Esc => {
                return match self.step {
                    Step::ConductorModel => SetupAction::Cancelled,
                    Step::AgentModel => {
                        self.step = Step::ConductorModel;
                        self.reset_selection();
                        SetupAction::None
                    }
                    _ => SetupAction::None,
                };
            }
            _ => {}
        }
        SetupAction::None
    }

    fn handle_tracker_key(&mut self, key: KeyEvent) -> SetupAction {
        match key.code {
            KeyCode::Up if self.selected > 0 => self.selected -= 1,
            KeyCode::Down if self.selected + 1 < TRACKERS.len() => self.selected += 1,
            KeyCode::Enter => {
                let tracker = &TRACKERS[self.selected];
                let tracker_val = if tracker.id == "none" {
                    None
                } else {
                    Some(tracker.id.to_string())
                };
                return SetupAction::Complete {
                    conductor_provider: self.conductor_provider.clone(),
                    conductor_model: self.conductor_model.clone(),
                    agent_provider: self.agent_provider.clone(),
                    agent_model: self.agent_model.clone(),
                    tracker: tracker_val,
                };
            }
            KeyCode::Esc => {
                self.step = Step::AgentModel;
                self.reset_selection();
            }
            _ => {}
        }
        SetupAction::None
    }

    // ── Rendering ───────────────────────────────────────────────

    pub fn render(&self, frame: &mut Frame, theme: &Theme) {
        d::render_backdrop(frame, theme);

        let popup = d::centered(frame.area(), 72, 28);
        frame.render_widget(Clear, popup);

        let block = d::dialog_block(theme);
        let content = block.inner(popup);
        frame.render_widget(block, popup);

        let rows = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(content);

        self.render_header(frame, rows[0], theme);
        match self.step {
            Step::ConductorModel => self.render_model_list(
                frame,
                rows[1],
                theme,
                "Lead model for planning & delegation",
            ),
            Step::AgentModel => {
                self.render_model_list(frame, rows[1], theme, "Model each sub-agent will use")
            }
            Step::Tracker => self.render_tracker_list(frame, rows[1], theme),
        }
        self.render_footer(frame, rows[2], theme);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let steps = d::step_indicator(STEPS, self.step.index(), theme);
        frame.render_widget(
            Paragraph::new(steps).alignment(ratatui::layout::Alignment::Center),
            area,
        );
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let text = match self.step {
            Step::ConductorModel | Step::AgentModel => "↑↓ navigate  ⏎ select  esc back",
            Step::Tracker => "↑↓ navigate  ⏎ select  esc back",
        };
        frame.render_widget(Paragraph::new(d::footer_hints(text, theme)), area);
    }

    fn render_model_list(&self, frame: &mut Frame, area: Rect, theme: &Theme, subtitle: &str) {
        let heading_text = match self.step {
            Step::ConductorModel => "Conductor model",
            _ => "Sub-agent model",
        };

        let header_h: usize = 3;
        let max_visible = (area.height as usize).saturating_sub(header_h);

        let mut display: Vec<(&Row, Option<usize>)> = Vec::new();
        let mut model_idx = 0;
        for row in &self.rows {
            match row {
                Row::Header(_) => display.push((row, None)),
                Row::Model(_) => {
                    display.push((row, Some(model_idx)));
                    model_idx += 1;
                }
            }
        }

        let selected_display_idx = display
            .iter()
            .position(|(_, mi)| *mi == Some(self.selected))
            .unwrap_or(0);
        let scroll = if selected_display_idx >= max_visible {
            selected_display_idx + 1 - max_visible
        } else {
            0
        };

        let sel_bg = Theme::blend(theme.accent, theme.background, 0.85);
        let mut lines: Vec<Line> = vec![
            Line::from(""),
            d::heading(heading_text, theme),
            d::hint(subtitle, theme),
        ];

        let end = display.len().min(scroll + max_visible);
        for (row, mi) in &display[scroll..end] {
            match row {
                Row::Header(name) => {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        name.to_uppercase(),
                        Style::default().fg(theme.dim()),
                    )));
                }
                Row::Model(idx) => {
                    let entry = &self.models[*idx];
                    let sel = *mi == Some(self.selected);
                    let bg = if sel { sel_bg } else { theme.background };
                    let fg = if sel { theme.accent } else { theme.foreground };
                    let style = Style::default().fg(fg).bg(bg);
                    let bold_style = style.add_modifier(Modifier::BOLD);

                    let fill = " "
                        .repeat((area.width as usize).saturating_sub(4 + entry.model_name.len()));
                    lines.push(Line::from(vec![
                        Span::styled(if sel { " ▸ " } else { "   " }, bold_style),
                        Span::styled(entry.model_name.as_str(), bold_style),
                        Span::styled(fill, Style::default().bg(bg)),
                    ]));
                }
            }
        }

        if end < display.len() {
            let remaining = display[end..]
                .iter()
                .filter(|(r, _)| matches!(r, Row::Model(_)))
                .count();
            if remaining > 0 {
                lines.push(d::hint_owned(format!("   ↓ {} more", remaining), theme));
            }
        }

        frame.render_widget(Paragraph::new(lines), area);
    }

    fn render_tracker_list(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let sel_bg = Theme::blend(theme.accent, theme.background, 0.85);
        let mut lines: Vec<Line> = vec![
            Line::from(""),
            d::heading("Issue tracker", theme),
            d::hint("How should the conductor track work?", theme),
            Line::from(""),
        ];

        for (i, tracker) in TRACKERS.iter().enumerate() {
            let sel = i == self.selected;
            let bg = if sel { sel_bg } else { theme.background };
            let fg = if sel { theme.accent } else { theme.foreground };
            let desc_fg = if sel { theme.accent } else { theme.dim() };

            let fill = " ".repeat((area.width as usize).saturating_sub(4 + tracker.label.len()));
            lines.push(Line::from(vec![
                Span::styled(
                    if sel { " ▸ " } else { "   " },
                    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    tracker.label,
                    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
                ),
                Span::styled(fill, Style::default().bg(bg)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("   ", Style::default().bg(bg)),
                Span::styled(tracker.desc, Style::default().fg(desc_fg).bg(bg)),
            ]));
            if i + 1 < TRACKERS.len() {
                lines.push(Line::from(""));
            }
        }

        frame.render_widget(Paragraph::new(lines), area);
    }
}

// ── Helpers ─────────────────────────────────────────────────────

fn build_grouped_rows(models: &[ModelEntry]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut last_provider = String::new();

    for (i, entry) in models.iter().enumerate() {
        if entry.provider != last_provider {
            rows.push(Row::Header(entry.provider.clone()));
            last_provider.clone_from(&entry.provider);
        }
        rows.push(Row::Model(i));
    }
    rows
}

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::config::schema::{Config, ProviderKind};
use crate::tui::theme::Theme;

pub struct ProviderEntry {
    pub name: String,
    pub kind: ProviderKind,
    pub model: String,
    pub active: bool,
    pub has_credential: bool,
}

pub struct ModelsPageState {
    pub entries: Vec<ProviderEntry>,
    pub cursor: usize,
    pub confirm_delete: Option<usize>,
}

pub enum ModelsPageAction {
    None,
    Close,
    AddProvider,
    DeleteProvider { name: String },
    SwitchTo { name: String, model: String },
    EditApiKey { name: String, kind: ProviderKind },
}

impl ModelsPageState {
    pub fn new(config: &Config) -> Self {
        let entries = build_entries(config);
        Self {
            entries,
            cursor: 0,
            confirm_delete: None,
        }
    }

    pub fn refresh(&mut self, config: &Config) {
        let old_cursor = self.cursor;
        self.entries = build_entries(config);
        self.cursor = old_cursor.min(self.entries.len().saturating_sub(1));
        self.confirm_delete = None;
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> ModelsPageAction {
        if let Some(delete_idx) = self.confirm_delete {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm_delete = None;
                    if let Some(entry) = self.entries.get(delete_idx) {
                        return ModelsPageAction::DeleteProvider {
                            name: entry.name.clone(),
                        };
                    }
                }
                _ => {
                    self.confirm_delete = None;
                }
            }
            return ModelsPageAction::None;
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ModelsPageAction::Close,
            KeyCode::Up | KeyCode::Char('k') => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                ModelsPageAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.cursor + 1 < self.entries.len() {
                    self.cursor += 1;
                }
                ModelsPageAction::None
            }
            KeyCode::Char('a') => ModelsPageAction::AddProvider,
            KeyCode::Char('d') | KeyCode::Delete => {
                if !self.entries.is_empty() {
                    self.confirm_delete = Some(self.cursor);
                }
                ModelsPageAction::None
            }
            KeyCode::Char('e') => {
                if let Some(entry) = self.entries.get(self.cursor) {
                    ModelsPageAction::EditApiKey {
                        name: entry.name.clone(),
                        kind: entry.kind,
                    }
                } else {
                    ModelsPageAction::None
                }
            }
            KeyCode::Enter => {
                if let Some(entry) = self.entries.get(self.cursor) {
                    ModelsPageAction::SwitchTo {
                        name: entry.name.clone(),
                        model: entry.model.clone(),
                    }
                } else {
                    ModelsPageAction::None
                }
            }
            _ => ModelsPageAction::None,
        }
    }
}

fn build_entries(config: &Config) -> Vec<ProviderEntry> {
    config
        .providers
        .iter()
        .map(|(name, profile)| ProviderEntry {
            name: name.clone(),
            kind: profile.kind,
            model: profile.model.clone(),
            active: profile.active,
            has_credential: profile.kind.is_local() || profile.resolve_credential().is_some(),
        })
        .collect()
}

fn kind_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Claude => "Anthropic",
        ProviderKind::OpenAI => "OpenAI",
        ProviderKind::Gemini => "Gemini",
        ProviderKind::Vertex => "Vertex",
        ProviderKind::Ollama => "Ollama",
        ProviderKind::LlamaCpp => "LlamaCpp",
        ProviderKind::Nvidia => "Nvidia",
    }
}

pub fn render_models_page(frame: &mut Frame, state: &ModelsPageState, theme: &Theme) {
    let area = frame.area();

    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        area,
    );

    let popup_width = 70u16.min(area.width.saturating_sub(4));
    let popup_height = 24u16.min(area.height.saturating_sub(2));
    let popup_area = Rect {
        x: (area.width.saturating_sub(popup_width)) / 2,
        y: (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .border_type(BorderType::Rounded)
        .style(Style::default().bg(theme.background))
        .title(" Providers ");

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 4 {
        return;
    }

    let help_height = 3u16;
    let list_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: inner.height.saturating_sub(help_height),
    };
    let help_area = Rect {
        x: inner.x,
        y: inner.y + list_area.height,
        width: inner.width,
        height: help_height,
    };

    if state.entries.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No providers configured.",
                Style::default().fg(theme.dim()),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Press 'a' to add a provider.",
                Style::default().fg(theme.foreground),
            )),
        ];
        frame.render_widget(Paragraph::new(lines), list_area);
    } else {
        let header_line = Line::from(vec![
            Span::styled(
                format!("  {:<3} {:<14} {:<10} {:<28} ", "", "Name", "Type", "Model"),
                Style::default()
                    .fg(theme.dim())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Key", Style::default().fg(theme.dim())),
        ]);

        let header_area = Rect {
            height: 1,
            ..list_area
        };
        let items_area = Rect {
            y: list_area.y + 1,
            height: list_area.height.saturating_sub(1),
            ..list_area
        };

        frame.render_widget(Paragraph::new(header_line), header_area);

        let visible = items_area.height as usize;
        let scroll = state.cursor.saturating_sub(visible.saturating_sub(1));

        let items: Vec<ListItem> = state
            .entries
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible)
            .map(|(i, entry)| {
                let is_selected = i == state.cursor;
                let active_marker = if entry.active { "●" } else { " " };
                let key_status = if entry.kind.is_local() {
                    "local"
                } else if entry.has_credential {
                    "  ✓"
                } else {
                    "  ✗"
                };

                let key_style = if is_selected {
                    Style::default().fg(theme.background).bg(theme.accent)
                } else if !entry.has_credential && !entry.kind.is_local() {
                    Style::default().fg(theme.warning)
                } else {
                    Style::default().fg(theme.success)
                };

                let base_style = if is_selected {
                    Style::default().fg(theme.background).bg(theme.accent)
                } else {
                    Style::default().fg(theme.foreground)
                };

                let active_style = if is_selected {
                    base_style
                } else if entry.active {
                    Style::default().fg(theme.success)
                } else {
                    Style::default().fg(theme.dim())
                };

                ListItem::new(Line::from(vec![
                    Span::styled(format!("  {active_marker:<3}"), active_style),
                    Span::styled(format!("{:<14}", entry.name), base_style),
                    Span::styled(format!("{:<10}", kind_label(entry.kind)), base_style),
                    Span::styled(format!("{:<28}", entry.model), base_style),
                    Span::styled(key_status.to_string(), key_style),
                ]))
            })
            .collect();

        let list = List::new(items);
        frame.render_widget(list, items_area);
    }

    if let Some(idx) = state.confirm_delete {
        let name = state
            .entries
            .get(idx)
            .map(|e| e.name.as_str())
            .unwrap_or("?");
        let confirm = Line::from(vec![
            Span::styled("  Delete '", Style::default().fg(theme.warning)),
            Span::styled(name, Style::default().fg(theme.error)),
            Span::styled(
                "'? Press y to confirm, any other key to cancel",
                Style::default().fg(theme.warning),
            ),
        ]);
        frame.render_widget(Paragraph::new(confirm), help_area);
    } else {
        let help_lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::styled("  ↑↓", Style::default().fg(theme.accent)),
                Span::styled(" navigate  ", Style::default().fg(theme.dim())),
                Span::styled("Enter", Style::default().fg(theme.accent)),
                Span::styled(" switch  ", Style::default().fg(theme.dim())),
                Span::styled("a", Style::default().fg(theme.accent)),
                Span::styled(" add  ", Style::default().fg(theme.dim())),
                Span::styled("e", Style::default().fg(theme.accent)),
                Span::styled(" edit key  ", Style::default().fg(theme.dim())),
                Span::styled("d", Style::default().fg(theme.accent)),
                Span::styled(" delete  ", Style::default().fg(theme.dim())),
                Span::styled("Esc", Style::default().fg(theme.accent)),
                Span::styled(" close", Style::default().fg(theme.dim())),
            ]),
        ];
        frame.render_widget(Paragraph::new(help_lines), help_area);
    }
}

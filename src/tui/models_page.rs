use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::commands::model::list_model_entries;
use crate::config::schema::Config;

pub fn render_models_page(config: &Config, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Models ")
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let entries = list_model_entries(config);
    let rows: Vec<Row> = entries
        .iter()
        .map(|e| {
            let active = if e.profile.active { "*" } else { " " };
            Row::new(vec![
                Cell::from(active),
                Cell::from(e.provider_name.as_str()),
                Cell::from(e.profile.model.as_str()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(20),
            Constraint::Min(30),
        ],
    )
    .header(
        Row::new(vec!["", "Provider", "Model"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    );

    frame.render_widget(table, inner);
}

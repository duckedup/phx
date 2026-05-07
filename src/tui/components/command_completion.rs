use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::picker::{PickerItem, PickerMode, PickerState};
use crate::tui::theme::Theme;

pub enum CompletionAction {
    None,
    Dismiss,
    Complete(String),
    Accept(String),
}

pub fn handle_key(picker: &mut PickerState, key: KeyEvent) -> CompletionAction {
    match key.code {
        KeyCode::Up => {
            picker.move_up();
            CompletionAction::None
        }
        KeyCode::Down => {
            picker.move_down();
            CompletionAction::None
        }
        KeyCode::Tab => {
            let cmd = picker.selected().map(|s| format!("/{}", s.id));
            match cmd {
                Some(c) => CompletionAction::Complete(c),
                None => CompletionAction::Dismiss,
            }
        }
        KeyCode::Esc => CompletionAction::Dismiss,
        KeyCode::Enter
            if !key.modifiers.contains(KeyModifiers::SHIFT)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            let cmd = picker.selected().map(|s| format!("/{}", s.id));
            match cmd {
                Some(c) => CompletionAction::Accept(c),
                None => CompletionAction::Dismiss,
            }
        }
        _ => CompletionAction::None,
    }
}

pub fn update_completion(buffer: &str, command_items: &[PickerItem]) -> Option<PickerState> {
    if !buffer.starts_with('/') || buffer.contains(' ') || buffer.contains('\n') {
        return None;
    }

    let filter = &buffer[1..];
    let mut picker = PickerState::new(command_items.to_vec(), PickerMode::CommandComplete);
    picker.set_filter(filter);

    if picker.visible_count() == 0 {
        return None;
    }

    Some(picker)
}

pub fn render_command_completion(
    frame: &mut Frame,
    input_area: Rect,
    picker: &PickerState,
    theme: &Theme,
) {
    let max_visible = 8usize;
    let count = picker.visible_count().min(max_visible);
    if count == 0 {
        return;
    }

    let popup_height = count as u16 + 2;
    let popup_width = input_area.width.min(50);
    let popup_area = Rect {
        x: input_area.x,
        y: input_area.y.saturating_sub(popup_height),
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = picker
        .filtered
        .iter()
        .take(max_visible)
        .enumerate()
        .map(|(i, &idx)| {
            let item = &picker.items[idx];
            let style = if i == picker.cursor {
                Style::default().fg(theme.background).bg(theme.accent)
            } else {
                Style::default().fg(theme.foreground)
            };
            ListItem::new(format!(" /{:<14} {}", item.label, item.description)).style(style)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.primary))
        .border_type(BorderType::Rounded)
        .style(Style::default().bg(theme.background));

    let list = List::new(items).block(block);
    frame.render_widget(list, popup_area);
}

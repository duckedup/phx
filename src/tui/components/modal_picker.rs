use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::tui::picker::{PickerItem, PickerMode, PickerState};
use crate::tui::theme::Theme;

pub enum PickerAction {
    None,
    Dismiss,
    Select(PickerItem),
    PreviewTheme,
}

pub fn handle_key(picker: &mut PickerState, key: KeyEvent) -> PickerAction {
    let mode = picker.mode.clone();

    match key.code {
        KeyCode::Up => {
            picker.move_up();
            if mode == PickerMode::Theme {
                PickerAction::PreviewTheme
            } else {
                PickerAction::None
            }
        }
        KeyCode::Down => {
            picker.move_down();
            if mode == PickerMode::Theme {
                PickerAction::PreviewTheme
            } else {
                PickerAction::None
            }
        }
        KeyCode::Enter => {
            if let Some(selected) = picker.selected().cloned() {
                PickerAction::Select(selected)
            } else {
                PickerAction::Dismiss
            }
        }
        KeyCode::Esc => PickerAction::Dismiss,
        KeyCode::Char(c) => {
            let mut filter = picker.filter.clone();
            filter.push(c);
            picker.set_filter(&filter);
            if mode == PickerMode::Theme {
                PickerAction::PreviewTheme
            } else {
                PickerAction::None
            }
        }
        KeyCode::Backspace => {
            let mut filter = picker.filter.clone();
            filter.pop();
            picker.set_filter(&filter);
            if mode == PickerMode::Theme {
                PickerAction::PreviewTheme
            } else {
                PickerAction::None
            }
        }
        _ => PickerAction::None,
    }
}

pub fn popup_rect(term_area: Rect) -> Rect {
    let popup_width = 50u16.min(term_area.width.saturating_sub(4));
    let popup_height = 20u16.min(term_area.height.saturating_sub(4));
    Rect {
        x: (term_area.width.saturating_sub(popup_width)) / 2,
        y: (term_area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    }
}

pub fn handle_click(picker: &PickerState, term_area: Rect, row: u16, col: u16) -> PickerAction {
    let popup = popup_rect(term_area);
    if row < popup.y
        || row >= popup.y + popup.height
        || col < popup.x
        || col >= popup.x + popup.width
    {
        return PickerAction::Dismiss;
    }
    let inner_y = popup.y + 1;
    let inner_height = popup.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return PickerAction::None;
    }
    let scroll = picker.cursor.saturating_sub(inner_height.saturating_sub(1));
    if row < inner_y || row >= inner_y + inner_height as u16 {
        return PickerAction::None;
    }
    let clicked_idx = scroll + (row - inner_y) as usize;
    if clicked_idx < picker.filtered.len() {
        let item = picker.items[picker.filtered[clicked_idx]].clone();
        PickerAction::Select(item)
    } else {
        PickerAction::None
    }
}

pub fn handle_hover(picker: &mut PickerState, term_area: Rect, row: u16, col: u16) -> PickerAction {
    let popup = popup_rect(term_area);
    if row < popup.y
        || row >= popup.y + popup.height
        || col < popup.x
        || col >= popup.x + popup.width
    {
        return PickerAction::None;
    }
    let inner_y = popup.y + 1;
    let inner_height = popup.height.saturating_sub(2) as usize;
    if inner_height == 0 {
        return PickerAction::None;
    }
    let scroll = picker.cursor.saturating_sub(inner_height.saturating_sub(1));
    if row < inner_y || row >= inner_y + inner_height as u16 {
        return PickerAction::None;
    }
    let hovered_idx = scroll + (row - inner_y) as usize;
    if hovered_idx < picker.filtered.len() && hovered_idx != picker.cursor {
        picker.cursor = hovered_idx;
        if picker.mode == PickerMode::Theme {
            PickerAction::PreviewTheme
        } else {
            PickerAction::None
        }
    } else {
        PickerAction::None
    }
}

pub fn render_modal_picker(frame: &mut Frame, picker: &PickerState, theme: &Theme) {
    let area = frame.area();
    let popup_area = popup_rect(area);

    frame.render_widget(Clear, popup_area);

    let title = match picker.mode {
        PickerMode::Theme => " Theme ",
        PickerMode::Model => " Model ",
        PickerMode::Session => " Session ",
        PickerMode::CommandComplete => "",
    };

    let inner_height = popup_area.height.saturating_sub(2) as usize;
    let scroll = picker.cursor.saturating_sub(inner_height.saturating_sub(1));

    let items: Vec<ListItem> = picker
        .filtered
        .iter()
        .skip(scroll)
        .take(inner_height)
        .enumerate()
        .map(|(i, &idx)| {
            let item = &picker.items[idx];
            let display_idx = scroll + i;
            let style = if display_idx == picker.cursor {
                Style::default().fg(theme.background).bg(theme.accent)
            } else {
                Style::default().fg(theme.foreground)
            };
            let text = if item.description.is_empty() {
                format!("  {}", item.label)
            } else {
                format!("  {:<20} {}", item.label, item.description)
            };
            ListItem::new(text).style(style)
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .border_type(BorderType::Rounded)
        .style(Style::default().bg(theme.background))
        .title(title);

    let list = List::new(items).block(block);
    frame.render_widget(list, popup_area);
}

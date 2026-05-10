use std::rc::Rc;

use ratatui::prelude::*;

use super::components::sidebar::SIDEBAR_WIDTH;

pub const MAX_INPUT_HEIGHT: u16 = 10;
pub const STATUS_HEIGHT: u16 = 1;
pub const CHAT_PADDING: u16 = 2;

pub const MAX_FORM_HEIGHT: u16 = 30;

pub fn main_layout(area: Rect, input_lines: u16) -> Rc<[Rect]> {
    let input_height = (input_lines + 1).clamp(3, MAX_INPUT_HEIGHT);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(input_height),
            Constraint::Length(STATUS_HEIGHT),
        ])
        .split(area)
}

pub fn main_layout_with_form(area: Rect, form_height: u16) -> Rc<[Rect]> {
    let max_form = (area.height / 2).clamp(MAX_INPUT_HEIGHT, MAX_FORM_HEIGHT);
    let height = form_height.clamp(6, max_form);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(height),
            Constraint::Length(STATUS_HEIGHT),
        ])
        .split(area)
}

pub fn split_sidebar(area: Rect) -> (Rect, Rect) {
    if area.width > SIDEBAR_WIDTH + 40 {
        let horiz = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(40)])
            .split(area);
        (horiz[0], horiz[1])
    } else {
        (Rect::default(), area)
    }
}

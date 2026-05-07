use std::rc::Rc;

use ratatui::prelude::*;

pub const MAX_INPUT_HEIGHT: u16 = 10;
pub const STATUS_HEIGHT: u16 = 1;
pub const CHAT_PADDING: u16 = 2;

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

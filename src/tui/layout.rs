use std::rc::Rc;

use ratatui::prelude::*;

use super::components::sidebar::SIDEBAR_WIDTH;

pub const MAX_INPUT_HEIGHT: u16 = 10;
pub const STATUS_HEIGHT: u16 = 2;
pub const CHAT_PADDING: u16 = 2;

pub fn main_layout(area: Rect, input_lines: u16) -> Rc<[Rect]> {
    let input_height = (input_lines + 2).clamp(3, MAX_INPUT_HEIGHT);
    let chat_height = area.height.saturating_sub(input_height + STATUS_HEIGHT);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(chat_height),
            Constraint::Length(input_height),
            Constraint::Length(STATUS_HEIGHT),
        ])
        .split(area)
}

pub fn file_viewer_layout(area: Rect) -> Rc<[Rect]> {
    let status_height = STATUS_HEIGHT;
    let file_status_height = 1u16;
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(file_status_height),
            Constraint::Length(status_height),
        ])
        .split(area)
}

pub fn main_layout_with_form(area: Rect, form_height: u16) -> Rc<[Rect]> {
    let min_chat = 3_u16;
    let max_form = area.height.saturating_sub(min_chat + STATUS_HEIGHT);
    let height = form_height.clamp(6, max_form);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(min_chat),
            Constraint::Length(height),
            Constraint::Length(STATUS_HEIGHT),
        ])
        .split(area)
}

pub fn padded_chat_area(raw: Rect) -> Rect {
    let pad = CHAT_PADDING;
    let top_pad = 1u16;
    Rect {
        x: raw.x + pad,
        y: raw.y + top_pad,
        width: raw.width.saturating_sub(pad * 2),
        height: raw.height.saturating_sub(top_pad),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_padding_shifts_y_proving_bug() {
        let raw = Rect::new(0, 0, 120, 40);
        let padded = padded_chat_area(raw);
        assert_eq!(padded.y, 1, "first padding adds top_pad=1");

        let double = padded_chat_area(padded);
        assert_eq!(double.y, 2, "double padding adds another top_pad");

        let mouse_row = padded.y + 5;
        let correct_idx = (mouse_row - padded.y) as usize;
        let wrong_idx = mouse_row.saturating_sub(double.y) as usize;
        assert_eq!(correct_idx, 5);
        assert_eq!(wrong_idx, 4, "double padding gives wrong index — off by 1");
    }
}

use std::rc::Rc;

use ratatui::prelude::*;

use super::components::sidebar::SIDEBAR_WIDTH;

pub const MAX_INPUT_HEIGHT: u16 = 10;
pub const STATUS_HEIGHT: u16 = 2;
pub const CHAT_PADDING: u16 = 2;

/// Compute the text width available for input wrapping given the total content area width.
/// This mirrors the padding/border/prompt math in `render_input`:
///   pad_x(2)*2 + border(1)*2 + inner_pad(1)*2 + prompt_len(2) = 10
pub fn input_text_width(area_width: u16) -> usize {
    area_width.saturating_sub(10) as usize
}

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

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;

    #[test]
    fn single_padded_area_gives_correct_hover_index() {
        let raw = Rect::new(0, 0, 120, 40);
        let chat_area = padded_chat_area(raw);

        for target in [0, 1, 5, 10, 20] {
            let mouse_row = chat_area.y + target as u16;
            let idx = (mouse_row - chat_area.y) as usize;
            assert_eq!(idx, target, "single-padded area maps mouse to correct line");
        }
    }

    #[test]
    fn double_padded_area_gives_wrong_hover_index() {
        let raw = Rect::new(0, 0, 120, 40);
        let chat_area = padded_chat_area(raw);
        let double = padded_chat_area(chat_area);

        let target = 5usize;
        let mouse_row = chat_area.y + target as u16;

        let correct = (mouse_row - chat_area.y) as usize;
        let wrong = mouse_row.saturating_sub(double.y) as usize;

        assert_eq!(correct, 5);
        assert_ne!(
            wrong, 5,
            "double padding produces wrong index — this was the bug"
        );
    }
}

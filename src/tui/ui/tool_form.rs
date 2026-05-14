use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use phoenix_shared::ui_field_types::{ToolUiConfig, UiFieldKind};

use crate::tui::theme::Theme;

use super::form_field::{TextAreaField, TextField};

type SelectionRange = ((usize, usize), (usize, usize));

pub struct SelectOption {
    pub value: String,
    pub label: String,
    pub description: String,
}

pub struct SelectState {
    pub options: Vec<SelectOption>,
    pub selected: Option<usize>,
    pub hover: usize,
    pub allow_other: bool,
    pub other_active: bool,
    pub other_text: TextField,
}

pub enum FormFieldValue {
    Text(TextField),
    TextArea(Box<TextAreaField>),
    Toggle(bool),
    Select(SelectState),
    SelectPaged(SelectState),
}

pub struct FormField {
    pub key: String,
    pub label: String,
    pub required: bool,
    pub value: FormFieldValue,
}

impl FormField {
    pub fn is_empty(&self) -> bool {
        match &self.value {
            FormFieldValue::Text(t) => t.is_empty(),
            FormFieldValue::TextArea(t) => t.is_empty(),
            FormFieldValue::Toggle(_) => false,
            FormFieldValue::Select(s) | FormFieldValue::SelectPaged(s) => {
                if s.other_active {
                    s.other_text.is_empty()
                } else {
                    s.selected.is_none()
                }
            }
        }
    }

    pub fn json_value(&self) -> serde_json::Value {
        match &self.value {
            FormFieldValue::Text(t) => serde_json::Value::String(t.value.clone()),
            FormFieldValue::TextArea(t) => serde_json::Value::String(t.value()),
            FormFieldValue::Toggle(b) => serde_json::Value::Bool(*b),
            FormFieldValue::Select(s) | FormFieldValue::SelectPaged(s) => {
                if s.other_active {
                    serde_json::Value::String(s.other_text.value.clone())
                } else if let Some(idx) = s.selected {
                    serde_json::Value::String(s.options[idx].value.clone())
                } else {
                    serde_json::Value::Null
                }
            }
        }
    }

    pub fn display_value(&self) -> String {
        match &self.value {
            FormFieldValue::Text(t) => t.value.clone(),
            FormFieldValue::TextArea(t) => t.value(),
            FormFieldValue::Toggle(b) => if *b { "yes" } else { "no" }.to_string(),
            FormFieldValue::Select(s) | FormFieldValue::SelectPaged(s) => {
                if s.other_active {
                    format!("(other) {}", s.other_text.value)
                } else if let Some(idx) = s.selected {
                    s.options[idx].label.clone()
                } else {
                    String::new()
                }
            }
        }
    }
}

pub struct ToolFormState {
    pub tool_name: String,
    pub description: String,
    pub fields: Vec<FormField>,
    pub focused_index: usize,
    pub submit_focused: bool,
}

pub enum FormAction {
    None,
    Submit(serde_json::Value),
    Cancel,
}

impl ToolFormState {
    pub fn from_ui(tool_name: String, description: String, config: &ToolUiConfig) -> Self {
        let fields = config
            .fields
            .iter()
            .map(|f| {
                let value = match f.field {
                    UiFieldKind::TextInput => {
                        let tf = if f.default_value.is_empty() {
                            TextField::new(&f.placeholder)
                        } else {
                            TextField::new(&f.placeholder).with_value(&f.default_value)
                        };
                        FormFieldValue::Text(tf)
                    }
                    UiFieldKind::TextArea => {
                        FormFieldValue::TextArea(Box::new(TextAreaField::new(&f.placeholder)))
                    }
                    UiFieldKind::Toggle => FormFieldValue::Toggle(false),
                    UiFieldKind::Select | UiFieldKind::SelectPaged => {
                        let options = f
                            .options
                            .iter()
                            .map(|o| SelectOption {
                                value: o.value.clone(),
                                label: o.label.clone(),
                                description: o.description.clone(),
                            })
                            .collect();
                        let state = SelectState {
                            options,
                            selected: None,
                            hover: 0,
                            allow_other: f.allow_other,
                            other_active: false,
                            other_text: TextField::new("Type your answer..."),
                        };
                        if f.field == UiFieldKind::SelectPaged {
                            FormFieldValue::SelectPaged(state)
                        } else {
                            FormFieldValue::Select(state)
                        }
                    }
                };
                FormField {
                    key: f.key.clone(),
                    label: f.label.clone(),
                    required: f.required,
                    value,
                }
            })
            .collect();

        Self {
            tool_name,
            description,
            fields,
            focused_index: 0,
            submit_focused: false,
        }
    }

    fn can_submit(&self) -> bool {
        self.fields.iter().all(|f| !f.required || !f.is_empty())
    }

    pub fn collect_json(&self) -> serde_json::Value {
        self.collect_values()
    }

    fn collect_values(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        for field in &self.fields {
            if !field.is_empty() {
                map.insert(field.key.clone(), field.json_value());
            }
        }
        serde_json::Value::Object(map)
    }

    pub fn focused_textarea_mut(&mut self) -> Option<&mut TextAreaField> {
        if self.submit_focused {
            return None;
        }
        match &mut self.fields[self.focused_index].value {
            FormFieldValue::TextArea(ta) => Some(ta),
            _ => None,
        }
    }
}

pub fn handle_key(state: &mut ToolFormState, key: KeyEvent) -> FormAction {
    let is_dismiss = key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL));

    if is_dismiss {
        return FormAction::Cancel;
    }

    if state.submit_focused {
        match key.code {
            KeyCode::Enter if state.can_submit() => {
                return FormAction::Submit(state.collect_values());
            }
            KeyCode::Up => {
                state.submit_focused = false;
                if !state.fields.is_empty() {
                    state.focused_index = state.fields.len() - 1;
                }
            }
            KeyCode::BackTab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                state.submit_focused = false;
                if !state.fields.is_empty() {
                    state.focused_index = state.fields.len() - 1;
                }
            }
            _ => {}
        }
        return FormAction::None;
    }

    if state.fields.is_empty() {
        return FormAction::None;
    }

    let consumes_arrows = matches!(
        state.fields[state.focused_index].value,
        FormFieldValue::TextArea(_) | FormFieldValue::Select(_) | FormFieldValue::SelectPaged(_)
    );

    match key.code {
        KeyCode::Tab => {
            if state.focused_index + 1 < state.fields.len() {
                state.focused_index += 1;
            } else {
                state.submit_focused = true;
            }
            return FormAction::None;
        }
        KeyCode::BackTab if key.modifiers.contains(KeyModifiers::SHIFT) => {
            if state.focused_index > 0 {
                state.focused_index -= 1;
            }
            return FormAction::None;
        }
        KeyCode::Down if !consumes_arrows => {
            if state.focused_index + 1 < state.fields.len() {
                state.focused_index += 1;
            } else {
                state.submit_focused = true;
            }
            return FormAction::None;
        }
        KeyCode::Up if !consumes_arrows => {
            if state.focused_index > 0 {
                state.focused_index -= 1;
            }
            return FormAction::None;
        }
        KeyCode::Enter if !consumes_arrows => {
            if state.focused_index + 1 < state.fields.len() {
                state.focused_index += 1;
            } else {
                state.submit_focused = true;
            }
            return FormAction::None;
        }
        _ => {}
    }

    let field = &mut state.fields[state.focused_index];
    match &mut field.value {
        FormFieldValue::Text(t) => {
            t.handle_key(key);
        }
        FormFieldValue::TextArea(t) => {
            let handled = t.handle_key(key);
            if !handled {
                match key.code {
                    KeyCode::Down if state.focused_index + 1 < state.fields.len() => {
                        state.focused_index += 1;
                    }
                    KeyCode::Down => {
                        state.submit_focused = true;
                    }
                    KeyCode::Up if state.focused_index > 0 => {
                        state.focused_index -= 1;
                    }
                    _ => {}
                }
            }
        }
        FormFieldValue::Toggle(b) => {
            if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') {
                *b = !*b;
            }
        }
        FormFieldValue::Select(s) => match key.code {
            KeyCode::Up => {
                if s.hover > 0 {
                    s.hover -= 1;
                } else if state.focused_index > 0 {
                    state.focused_index -= 1;
                }
            }
            KeyCode::Down => {
                if s.hover + 1 < s.options.len() {
                    s.hover += 1;
                } else if state.focused_index + 1 < state.fields.len() {
                    state.focused_index += 1;
                } else {
                    state.submit_focused = true;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                s.selected = Some(s.hover);
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let idx = (c as usize) - ('1' as usize);
                if idx < s.options.len() {
                    s.selected = Some(idx);
                    s.hover = idx;
                }
            }
            _ => {}
        },
        FormFieldValue::SelectPaged(s) => {
            if s.other_active {
                match key.code {
                    KeyCode::Up => {
                        s.other_active = false;
                        s.hover = s.options.len().saturating_sub(1);
                    }
                    KeyCode::Enter => {
                        if !s.other_text.is_empty() {
                            if state.focused_index + 1 < state.fields.len() {
                                state.focused_index += 1;
                            } else {
                                state.submit_focused = true;
                            }
                        }
                    }
                    _ => {
                        s.other_text.handle_key(key);
                    }
                }
            } else {
                match key.code {
                    KeyCode::Up if s.hover > 0 => {
                        s.hover -= 1;
                    }
                    KeyCode::Down if s.hover + 1 < s.options.len() => {
                        s.hover += 1;
                    }
                    KeyCode::Down if s.allow_other && s.hover + 1 == s.options.len() => {
                        s.other_active = true;
                        s.selected = None;
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        s.selected = Some(s.hover);
                        s.other_active = false;
                        s.other_text.value.clear();
                        s.other_text.cursor = 0;
                        if state.focused_index + 1 < state.fields.len() {
                            state.focused_index += 1;
                        } else {
                            state.submit_focused = true;
                        }
                    }
                    KeyCode::Char(c) if c.is_ascii_digit() => {
                        let idx = (c as usize) - ('1' as usize);
                        if idx < s.options.len() {
                            s.selected = Some(idx);
                            s.hover = idx;
                            s.other_active = false;
                            s.other_text.value.clear();
                            s.other_text.cursor = 0;
                            if state.focused_index + 1 < state.fields.len() {
                                state.focused_index += 1;
                            } else {
                                state.submit_focused = true;
                            }
                        }
                    }
                    KeyCode::Left if state.focused_index > 0 => {
                        state.focused_index -= 1;
                    }
                    KeyCode::Right => {
                        if state.focused_index + 1 < state.fields.len() {
                            state.focused_index += 1;
                        } else {
                            state.submit_focused = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    FormAction::None
}

pub fn handle_paste(state: &mut ToolFormState, text: &str) {
    if state.submit_focused || state.fields.is_empty() {
        return;
    }
    if let FormFieldValue::TextArea(ta) = &mut state.fields[state.focused_index].value {
        ta.insert_str(text);
    } else if let FormFieldValue::Text(tf) = &mut state.fields[state.focused_index].value {
        tf.value.insert_str(tf.cursor, text);
        tf.cursor += text.len();
    }
}

pub fn handle_copy(state: &ToolFormState) -> Option<String> {
    if state.submit_focused || state.fields.is_empty() {
        return None;
    }
    if let FormFieldValue::TextArea(ta) = &state.fields[state.focused_index].value
        && ta.textarea.is_selecting()
    {
        let text = ta.selected_text();
        if !text.is_empty() {
            return Some(text);
        }
    }
    None
}

pub fn cancel_selection(state: &mut ToolFormState) {
    if state.submit_focused || state.fields.is_empty() {
        return;
    }
    if let FormFieldValue::TextArea(ta) = &mut state.fields[state.focused_index].value {
        ta.textarea.cancel_selection();
    }
}

pub fn cut_selection(state: &mut ToolFormState) -> Option<String> {
    if state.submit_focused || state.fields.is_empty() {
        return None;
    }
    if let FormFieldValue::TextArea(ta) = &mut state.fields[state.focused_index].value
        && ta.textarea.is_selecting()
    {
        let text = ta.selected_text();
        if !text.is_empty() {
            ta.textarea.cut();
            return Some(text);
        }
    }
    None
}

const INDENT: &str = "  ";

pub fn render_tool_form(frame: &mut Frame, area: Rect, state: &ToolFormState, theme: &Theme) {
    let bg = theme.background;
    let border_fg = theme.separator();

    let sep = "\u{2500}".repeat(area.width as usize);
    let top = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(sep).style(Style::default().fg(border_fg).bg(bg)),
        top,
    );

    let mut y = area.y + 1;
    let content_width = area.width;

    let name_line = Line::from(vec![
        Span::styled(INDENT, Style::default().bg(bg)),
        Span::styled(
            &state.tool_name,
            Style::default()
                .fg(theme.primary)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    if y < area.bottom() {
        frame.render_widget(
            Paragraph::new(name_line).style(Style::default().bg(bg)),
            Rect {
                x: area.x,
                y,
                width: content_width,
                height: 1,
            },
        );
        y += 1;
    }

    if !state.description.is_empty() && y < area.bottom() {
        let desc_line = Line::from(vec![
            Span::styled(INDENT, Style::default().bg(bg)),
            Span::styled(&state.description, Style::default().fg(theme.dim())),
        ]);
        frame.render_widget(
            Paragraph::new(desc_line).style(Style::default().bg(bg)),
            Rect {
                x: area.x,
                y,
                width: content_width,
                height: 1,
            },
        );
        y += 1;
    }

    if y < area.bottom() {
        y += 1;
    }

    let field_x = area.x + INDENT.len() as u16;
    let field_width = content_width.saturating_sub(INDENT.len() as u16 + 1);

    // Count paged selects for stepper rendering
    let paged_fields: Vec<usize> = state
        .fields
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.value, FormFieldValue::SelectPaged(_)))
        .map(|(i, _)| i)
        .collect();
    let has_paged = !paged_fields.is_empty();

    // Render stepper bar for paged selects
    if has_paged && y < area.bottom() {
        let mut spans = vec![Span::styled(INDENT, Style::default())];
        for (page_num, &field_idx) in paged_fields.iter().enumerate() {
            let f = &state.fields[field_idx];
            let is_active = !state.submit_focused && state.focused_index == field_idx;
            let is_answered = !f.is_empty();
            let marker = if is_answered {
                "\u{25cf}"
            } else if is_active {
                "\u{25c9}"
            } else {
                "\u{25cb}"
            };
            let color = if is_active {
                theme.accent
            } else if is_answered {
                Color::Green
            } else {
                theme.dim()
            };
            spans.push(Span::styled(
                format!("{marker} {} ", page_num + 1),
                Style::default().fg(color),
            ));
        }
        let answered_count = paged_fields
            .iter()
            .filter(|&&i| !state.fields[i].is_empty())
            .count();
        spans.push(Span::styled(
            format!("  ({answered_count}/{})", paged_fields.len()),
            Style::default().fg(theme.dim()),
        ));
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)),
            Rect {
                x: area.x,
                y,
                width: content_width,
                height: 1,
            },
        );
        y += 2;
    }

    for (i, field) in state.fields.iter().enumerate() {
        if y >= area.bottom() {
            break;
        }

        let focused = !state.submit_focused && state.focused_index == i;

        // SelectPaged: only render the focused one with full options
        if let FormFieldValue::SelectPaged(_) = &field.value
            && !focused
        {
            continue;
        }

        let label_prefix = if field.required { "* " } else { "" };
        let label_style = if focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.foreground)
        };

        let label_line = Line::from(vec![
            Span::styled(INDENT, Style::default()),
            Span::styled(label_prefix, Style::default().fg(theme.accent)),
            Span::styled(&field.label, label_style),
        ]);
        frame.render_widget(
            Paragraph::new(label_line).style(Style::default().bg(bg)),
            Rect {
                x: area.x,
                y,
                width: content_width,
                height: 1,
            },
        );
        y += 1;

        if y >= area.bottom() {
            break;
        }

        match &field.value {
            FormFieldValue::Text(tf) => {
                let fa = Rect {
                    x: field_x,
                    y,
                    width: field_width,
                    height: 1,
                };
                tf.render(frame, fa, focused, theme);
                y += 1;
            }
            FormFieldValue::TextArea(ta) => {
                let remaining = area.bottom().saturating_sub(y);
                let height = (ta.line_count() as u16).clamp(1, 8).min(remaining);
                let fa = Rect {
                    x: field_x,
                    y,
                    width: field_width,
                    height,
                };
                render_textarea_field(frame, fa, ta, focused, theme);
                y += fa.height;
            }
            FormFieldValue::Toggle(val) => {
                let indicator = if *val { "[x]" } else { "[ ]" };
                let toggle_style = if focused {
                    Style::default().fg(theme.accent)
                } else {
                    Style::default().fg(theme.foreground)
                };
                let line = Line::from(Span::styled(indicator, toggle_style));
                frame.render_widget(
                    Paragraph::new(line).style(Style::default().bg(bg)),
                    Rect {
                        x: field_x,
                        y,
                        width: field_width,
                        height: 1,
                    },
                );
                y += 1;
            }
            FormFieldValue::Select(s) | FormFieldValue::SelectPaged(s) => {
                for (j, opt) in s.options.iter().enumerate() {
                    if y >= area.bottom() {
                        break;
                    }
                    let is_selected = !s.other_active && s.selected == Some(j);
                    let is_hover = focused && !s.other_active && s.hover == j;
                    let radio = if is_selected { "(\u{25cf})" } else { "( )" };
                    let radio_color = if is_selected {
                        Color::Green
                    } else {
                        theme.dim()
                    };
                    let text_fg = if is_hover {
                        theme.accent
                    } else if is_selected {
                        Color::Green
                    } else {
                        theme.foreground
                    };
                    let pointer = if is_hover { "> " } else { "  " };
                    let desc = if opt.description.is_empty() {
                        String::new()
                    } else {
                        format!(" \u{2014} {}", opt.description)
                    };
                    let line = Line::from(vec![
                        Span::styled(pointer, Style::default().fg(theme.accent)),
                        Span::styled(format!("{radio} "), Style::default().fg(radio_color)),
                        Span::styled(&opt.label, Style::default().fg(text_fg)),
                        Span::styled(desc, Style::default().fg(theme.dim())),
                    ]);
                    frame.render_widget(
                        Paragraph::new(line).style(Style::default().bg(bg)),
                        Rect {
                            x: field_x,
                            y,
                            width: field_width,
                            height: 1,
                        },
                    );
                    y += 1;
                }
                if s.allow_other && y < area.bottom() {
                    let is_active = s.other_active;
                    let label_fg = if is_active { theme.accent } else { theme.dim() };
                    let prefix = if is_active { "> " } else { "  " };
                    let label_line = Line::from(vec![
                        Span::styled(prefix, Style::default().fg(theme.accent)),
                        Span::styled("or type your own:", Style::default().fg(label_fg)),
                    ]);
                    frame.render_widget(
                        Paragraph::new(label_line).style(Style::default().bg(bg)),
                        Rect {
                            x: field_x,
                            y,
                            width: field_width,
                            height: 1,
                        },
                    );
                    y += 1;
                    if y < area.bottom() {
                        let input_x = field_x + 4;
                        let input_w = field_width.saturating_sub(5);
                        let fa = Rect {
                            x: input_x,
                            y,
                            width: input_w,
                            height: 1,
                        };
                        s.other_text.render(frame, fa, is_active, theme);
                        y += 1;
                    }
                }
            }
        }

        y += 1;
    }

    if y < area.bottom() {
        let (icon, label_style) = if state.submit_focused && state.can_submit() {
            (
                Span::styled("\u{25cf} ", Style::default().fg(Color::Green)),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
        } else if state.can_submit() {
            (
                Span::styled("\u{25cb} ", Style::default().fg(theme.foreground)),
                Style::default().fg(theme.foreground),
            )
        } else {
            (
                Span::styled("\u{25cb} ", Style::default().fg(theme.dim())),
                Style::default().fg(theme.dim()),
            )
        };
        let submit_line = Line::from(vec![
            Span::styled(INDENT, Style::default()),
            icon,
            Span::styled("Submit", label_style),
        ]);
        frame.render_widget(
            Paragraph::new(submit_line).style(Style::default().bg(bg)),
            Rect {
                x: area.x,
                y,
                width: content_width,
                height: 1,
            },
        );
    }
}

fn render_textarea_field(
    frame: &mut Frame,
    area: Rect,
    ta: &TextAreaField,
    focused: bool,
    theme: &Theme,
) {
    let bg = theme.background;
    let fg = if focused {
        theme.foreground
    } else {
        theme.dim()
    };

    if ta.is_empty() && !focused {
        let line = Line::from(Span::styled(
            &ta.placeholder,
            Style::default().fg(theme.dim()),
        ));
        frame.render_widget(Paragraph::new(line).style(Style::default().bg(bg)), area);
        return;
    }

    let lines = ta.textarea.lines();
    let cursor = ta.textarea.cursor();
    let selection = ta.textarea.selection_range();
    let visible_rows = area.height as usize;
    let scroll = if cursor.0 >= visible_rows {
        cursor.0 - visible_rows + 1
    } else {
        0
    };

    let text_style = Style::default().fg(fg);
    let select_style = Style::default().fg(theme.background).bg(theme.foreground);

    for (i, line) in lines.iter().skip(scroll).take(visible_rows).enumerate() {
        let row_y = area.y + i as u16;
        let line_idx = scroll + i;

        let spans = if let Some(sel) = &selection {
            build_selected_spans(line, line_idx, sel, text_style, select_style)
        } else {
            vec![Span::styled(line.as_str(), text_style)]
        };

        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)),
            Rect {
                x: area.x,
                y: row_y,
                width: area.width,
                height: 1,
            },
        );
    }

    if focused {
        let cursor_screen_row = cursor.0.saturating_sub(scroll);
        if cursor_screen_row < visible_rows {
            let cx = area.x + cursor.1 as u16;
            let cy = area.y + cursor_screen_row as u16;
            if cx < area.right() {
                frame.set_cursor_position((cx, cy));
            }
        }
    }
}

fn build_selected_spans<'a>(
    text: &str,
    line_idx: usize,
    sel: &SelectionRange,
    normal: Style,
    highlight: Style,
) -> Vec<Span<'a>> {
    let &((sr, sc), (er, ec)) = sel;
    if line_idx < sr || line_idx > er {
        return vec![Span::styled(text.to_string(), normal)];
    }

    let (sel_start, sel_end) = if line_idx == sr && line_idx == er {
        (sc, ec)
    } else if line_idx == sr {
        (sc, text.chars().count())
    } else if line_idx == er {
        (0, ec)
    } else {
        (0, text.chars().count())
    };

    let chars: Vec<char> = text.chars().collect();
    let mut spans = Vec::new();

    if sel_start > 0 {
        let before: String = chars[..sel_start.min(chars.len())].iter().collect();
        spans.push(Span::styled(before, normal));
    }
    let selected: String = chars[sel_start.min(chars.len())..sel_end.min(chars.len())]
        .iter()
        .collect();
    if !selected.is_empty() {
        spans.push(Span::styled(selected, highlight));
    }
    if sel_end < chars.len() {
        let after: String = chars[sel_end..].iter().collect();
        spans.push(Span::styled(after, normal));
    }

    if spans.is_empty() {
        spans.push(Span::styled(text.to_string(), normal));
    }

    spans
}

pub fn form_height(state: &ToolFormState) -> u16 {
    let mut h: u16 = 2; // separator + name
    if !state.description.is_empty() {
        h += 1;
    }
    h += 1; // spacing after header

    let has_paged = state
        .fields
        .iter()
        .any(|f| matches!(f.value, FormFieldValue::SelectPaged(_)));
    if has_paged {
        h += 2; // stepper bar + spacing
    }

    for (i, field) in state.fields.iter().enumerate() {
        let focused = !state.submit_focused && state.focused_index == i;
        match &field.value {
            FormFieldValue::SelectPaged(s) => {
                if focused {
                    h += 1; // label
                    h += s.options.len() as u16;
                    if s.allow_other {
                        h += 2; // "or type your own:" + text input
                    }
                    h += 1; // spacing
                }
            }
            FormFieldValue::Select(s) => {
                h += 1; // label
                h += s.options.len() as u16;
                if s.allow_other {
                    h += 2;
                }
                h += 1; // spacing
            }
            FormFieldValue::TextArea(ta) => {
                h += 1; // label
                h += (ta.line_count() as u16).clamp(1, 8);
                h += 1; // spacing
            }
            _ => {
                h += 1; // label
                h += 1; // field
                h += 1; // spacing
            }
        }
    }
    h += 1; // submit button
    h
}

pub fn format_answers(state: &ToolFormState) -> String {
    let mut out = String::new();
    for field in &state.fields {
        out.push_str(&format!("{}: {}\n", field.label, field.display_value()));
    }
    out
}

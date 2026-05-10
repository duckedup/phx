use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use phoenix_shared::ui_field_types::{ToolUiConfig, UiFieldKind};

use crate::tui::theme::Theme;

use super::form_field::{TextAreaField, TextField};

type SelectionRange = ((usize, usize), (usize, usize));

pub enum FormFieldValue {
    Text(TextField),
    TextArea(Box<TextAreaField>),
    Toggle(bool),
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
        }
    }

    pub fn json_value(&self) -> serde_json::Value {
        match &self.value {
            FormFieldValue::Text(t) => serde_json::Value::String(t.value.clone()),
            FormFieldValue::TextArea(t) => serde_json::Value::String(t.value()),
            FormFieldValue::Toggle(b) => serde_json::Value::Bool(*b),
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

    let is_textarea = matches!(
        state.fields[state.focused_index].value,
        FormFieldValue::TextArea(_)
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
        KeyCode::Down if !is_textarea => {
            if state.focused_index + 1 < state.fields.len() {
                state.focused_index += 1;
            } else {
                state.submit_focused = true;
            }
            return FormAction::None;
        }
        KeyCode::Up if !is_textarea => {
            if state.focused_index > 0 {
                state.focused_index -= 1;
            }
            return FormAction::None;
        }
        KeyCode::Enter if !is_textarea => {
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

    for (i, field) in state.fields.iter().enumerate() {
        if y >= area.bottom() {
            break;
        }

        let focused = !state.submit_focused && state.focused_index == i;
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
    let mut h: u16 = 4; // separator + name + description + spacing
    for field in &state.fields {
        h += 1; // label
        match &field.value {
            FormFieldValue::TextArea(ta) => {
                h += (ta.line_count() as u16).clamp(1, 8);
            }
            _ => {
                h += 1; // single-line field
            }
        }
        h += 1; // spacing between fields
    }
    h += 2; // spacing + submit button
    h
}

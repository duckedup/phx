use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerMode {
    Model,
    Session,
    Theme,
    CommandComplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
    pub description: String,
    #[serde(default)]
    pub source_tag: Option<String>,
}

pub struct PickerState {
    pub items: Vec<PickerItem>,
    pub filtered: Vec<usize>,
    pub cursor: usize,
    pub filter: String,
    pub mode: PickerMode,
}

impl PickerState {
    pub fn new(items: Vec<PickerItem>, mode: PickerMode) -> Self {
        let filtered: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            filtered,
            cursor: 0,
            filter: String::new(),
            mode,
        }
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_string();
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| fuzzy_match(&item.label, filter))
            .map(|(i, _)| i)
            .collect();
        if self.cursor >= self.filtered.len() {
            self.cursor = self.filtered.len().saturating_sub(1);
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor + 1 < self.filtered.len() {
            self.cursor += 1;
        }
    }

    pub fn selected(&self) -> Option<&PickerItem> {
        self.filtered
            .get(self.cursor)
            .and_then(|&idx| self.items.get(idx))
    }

    pub fn visible_count(&self) -> usize {
        self.filtered.len()
    }
}

fn fuzzy_match(text: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return true;
    }
    let text_lower = text.to_lowercase();
    let pattern_lower = pattern.to_lowercase();

    if text_lower.starts_with(&pattern_lower) {
        return true;
    }
    if text_lower.contains(&pattern_lower) {
        return true;
    }
    subsequence_match(&text_lower, &pattern_lower)
}

fn subsequence_match(text: &str, pattern: &str) -> bool {
    let mut pattern_chars = pattern.chars();
    let mut current = pattern_chars.next();
    for c in text.chars() {
        if let Some(pc) = current {
            if c == pc {
                current = pattern_chars.next();
            }
        } else {
            return true;
        }
    }
    current.is_none()
}

pub fn fuzzy_score(text: &str, pattern: &str) -> u32 {
    if pattern.is_empty() {
        return 0;
    }
    let text_lower = text.to_lowercase();
    let pattern_lower = pattern.to_lowercase();

    if text_lower.starts_with(&pattern_lower) {
        return 3;
    }
    if text_lower.contains(&pattern_lower) {
        return 2;
    }
    if subsequence_match(&text_lower, &pattern_lower) {
        return 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items() -> Vec<PickerItem> {
        vec![
            PickerItem {
                id: "a".into(),
                label: "model".into(),
                description: "".into(),
                source_tag: None,
            },
            PickerItem {
                id: "b".into(),
                label: "theme".into(),
                description: "".into(),
                source_tag: None,
            },
            PickerItem {
                id: "c".into(),
                label: "resume".into(),
                description: "".into(),
                source_tag: None,
            },
        ]
    }

    #[test]
    fn picker_initial_state() {
        let picker = PickerState::new(items(), PickerMode::CommandComplete);
        assert_eq!(picker.visible_count(), 3);
        assert_eq!(picker.cursor, 0);
    }

    #[test]
    fn picker_filter() {
        let mut picker = PickerState::new(items(), PickerMode::CommandComplete);
        picker.set_filter("mo");
        assert_eq!(picker.visible_count(), 1);
        assert_eq!(picker.selected().unwrap().label, "model");
    }

    #[test]
    fn picker_movement_bounds() {
        let mut picker = PickerState::new(items(), PickerMode::CommandComplete);
        picker.move_up();
        assert_eq!(picker.cursor, 0);
        picker.move_down();
        picker.move_down();
        picker.move_down();
        assert_eq!(picker.cursor, 2);
    }

    #[test]
    fn fuzzy_prefix_beats_substring() {
        assert!(fuzzy_score("model", "mo") > fuzzy_score("remote", "mo"));
    }

    #[test]
    fn fuzzy_substring_beats_subsequence() {
        assert!(fuzzy_score("theme", "hem") > fuzzy_score("the_match", "hem"));
    }

    #[test]
    fn fuzzy_subsequence_matches() {
        assert!(fuzzy_match("model", "mdl"));
    }
}

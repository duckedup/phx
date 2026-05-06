use ratatui::style::Color;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub primary: Color,
    pub accent: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub diff_add: Color,
    pub diff_delete: Color,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeEntry {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
struct ThemeJson {
    id: String,
    name: String,
    background: String,
    foreground: String,
    primary: String,
    accent: String,
    success: String,
    warning: String,
    error: String,
    info: String,
    diff_add: String,
    diff_delete: String,
}

impl Theme {
    pub fn dim(&self) -> Color {
        Self::blend(self.foreground, self.background, 0.5)
    }

    pub fn code_fg(&self) -> Color {
        self.accent
    }

    pub fn user_bubble_bg(&self) -> Color {
        Self::darken(self.primary, 0.7)
    }

    pub fn selection_bg(&self) -> Color {
        Self::blend(self.accent, self.background, 0.35)
    }

    pub fn selection_fg(&self) -> Color {
        self.foreground
    }

    pub fn separator(&self) -> Color {
        Self::blend(self.foreground, self.background, 0.75)
    }

    pub fn header_bg(&self) -> Color {
        Self::blend(self.foreground, self.background, 0.88)
    }

    pub fn input_bg(&self) -> Color {
        Self::blend(self.primary, self.background, 0.90)
    }

    pub fn tool_border(&self) -> Color {
        Self::blend(self.info, self.background, 0.55)
    }

    pub fn blend(a: Color, b: Color, t: f64) -> Color {
        let (ar, ag, ab) = color_rgb(a);
        let (br, bg, bb) = color_rgb(b);
        Color::Rgb(lerp(ar, br, t), lerp(ag, bg, t), lerp(ab, bb, t))
    }

    pub fn darken(c: Color, factor: f64) -> Color {
        let (r, g, b) = color_rgb(c);
        Color::Rgb(
            (r as f64 * factor) as u8,
            (g as f64 * factor) as u8,
            (b as f64 * factor) as u8,
        )
    }
}

fn lerp(a: u8, b: u8, t: f64) -> u8 {
    (a as f64 + (b as f64 - a as f64) * t) as u8
}

fn color_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (128, 128, 128),
    }
}

fn hex_to_rgb(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

fn theme_from_json(j: &ThemeJson) -> Option<Theme> {
    Some(Theme {
        background: hex_to_rgb(&j.background)?,
        foreground: hex_to_rgb(&j.foreground)?,
        primary: hex_to_rgb(&j.primary)?,
        accent: hex_to_rgb(&j.accent)?,
        success: hex_to_rgb(&j.success)?,
        warning: hex_to_rgb(&j.warning)?,
        error: hex_to_rgb(&j.error)?,
        info: hex_to_rgb(&j.info)?,
        diff_add: hex_to_rgb(&j.diff_add)?,
        diff_delete: hex_to_rgb(&j.diff_delete)?,
    })
}

const BUILTIN_THEMES_JSON: &str = include_str!("themes.json");

fn load_builtin_themes() -> Vec<(ThemeEntry, Theme)> {
    let themes: Vec<ThemeJson> = serde_json::from_str(BUILTIN_THEMES_JSON).unwrap_or_default();
    themes
        .iter()
        .filter_map(|j| {
            let entry = ThemeEntry {
                id: j.id.clone(),
                name: j.name.clone(),
            };
            let theme = theme_from_json(j)?;
            Some((entry, theme))
        })
        .collect()
}

pub fn list_all() -> Vec<ThemeEntry> {
    let mut entries: Vec<ThemeEntry> = load_builtin_themes().into_iter().map(|(e, _)| e).collect();
    entries.sort_by_key(|a| a.name.to_lowercase());
    entries
}

pub fn get_by_name(name: &str) -> Option<Theme> {
    load_builtin_themes()
        .into_iter()
        .find(|(e, _)| e.id == name || e.name.eq_ignore_ascii_case(name))
        .map(|(_, t)| t)
}

pub fn default_theme() -> Theme {
    get_by_name("dracula").unwrap_or(Theme {
        background: Color::Rgb(40, 42, 54),
        foreground: Color::Rgb(248, 248, 242),
        primary: Color::Rgb(139, 233, 253),
        accent: Color::Rgb(189, 147, 249),
        success: Color::Rgb(80, 250, 123),
        warning: Color::Rgb(241, 250, 140),
        error: Color::Rgb(255, 85, 85),
        info: Color::Rgb(139, 233, 253),
        diff_add: Color::Rgb(80, 250, 123),
        diff_delete: Color::Rgb(255, 85, 85),
    })
}

pub fn load_from_file(path: &std::path::Path) -> Option<Theme> {
    let content = std::fs::read_to_string(path).ok()?;
    let j: ThemeJson = serde_json::from_str(&content).ok()?;
    theme_from_json(&j)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_to_rgb_valid() {
        assert_eq!(hex_to_rgb("#ff0000"), Some(Color::Rgb(255, 0, 0)));
        assert_eq!(hex_to_rgb("00ff00"), Some(Color::Rgb(0, 255, 0)));
    }

    #[test]
    fn hex_to_rgb_invalid() {
        assert_eq!(hex_to_rgb("xyz"), None);
        assert_eq!(hex_to_rgb("#gggggg"), None);
    }

    #[test]
    fn blend_midpoint() {
        let result = Theme::blend(Color::Rgb(0, 0, 0), Color::Rgb(100, 100, 100), 0.5);
        assert_eq!(result, Color::Rgb(50, 50, 50));
    }

    #[test]
    fn darken_factor() {
        let result = Theme::darken(Color::Rgb(200, 100, 50), 0.5);
        assert_eq!(result, Color::Rgb(100, 50, 25));
    }

    #[test]
    fn default_theme_exists() {
        let t = default_theme();
        assert!(matches!(t.background, Color::Rgb(_, _, _)));
    }

    #[test]
    fn builtin_themes_load() {
        let themes = list_all();
        assert!(!themes.is_empty());
    }

    #[test]
    fn get_by_name_found() {
        let themes = list_all();
        if let Some(first) = themes.first() {
            assert!(get_by_name(&first.id).is_some());
        }
    }
}

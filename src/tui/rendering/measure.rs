use unicode_width::UnicodeWidthChar;

pub const TAB_WIDTH: usize = 4;

pub fn display_width(text: &str) -> usize {
    text.chars()
        .map(|c| {
            if c == '\t' {
                TAB_WIDTH
            } else {
                UnicodeWidthChar::width(c).unwrap_or(0)
            }
        })
        .sum()
}

pub fn expand_tabs(text: &str) -> String {
    if !text.contains('\t') {
        return text.to_string();
    }
    let spaces = " ".repeat(TAB_WIDTH);
    text.replace('\t', &spaces)
}

pub fn truncate_to_width(text: &str, max_cols: usize) -> String {
    if max_cols == 0 {
        return String::new();
    }
    let mut width = 0;
    let mut out = String::new();
    for c in text.chars() {
        let cw = char_width(c);
        if width + cw > max_cols {
            while width + 1 > max_cols {
                if let Some(last) = out.pop() {
                    width = width.saturating_sub(char_width(last));
                } else {
                    break;
                }
            }
            out.push('…');
            return out;
        }
        if c == '\t' {
            out.push_str(&" ".repeat(TAB_WIDTH));
        } else {
            out.push(c);
        }
        width += cw;
    }
    out
}

pub fn truncate_to_width_raw(text: &str, max_cols: usize) -> String {
    let mut width = 0;
    let mut out = String::new();
    for c in text.chars() {
        let cw = char_width(c);
        if width + cw > max_cols {
            break;
        }
        if c == '\t' {
            out.push_str(&" ".repeat(TAB_WIDTH));
        } else {
            out.push(c);
        }
        width += cw;
    }
    out
}

pub fn pad_to_width(text: &str, target_cols: usize) -> String {
    let w = display_width(text);
    if w >= target_cols {
        text.to_string()
    } else {
        let mut out = String::with_capacity(text.len() + (target_cols - w));
        out.push_str(text);
        for _ in 0..(target_cols - w) {
            out.push(' ');
        }
        out
    }
}

fn char_width(c: char) -> usize {
    if c == '\t' {
        TAB_WIDTH
    } else {
        UnicodeWidthChar::width(c).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_width() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn tab_width() {
        assert_eq!(display_width("\t"), TAB_WIDTH);
        assert_eq!(display_width("a\tb"), 1 + TAB_WIDTH + 1);
    }

    #[test]
    fn wide_chars() {
        assert_eq!(display_width("中"), 2);
        assert_eq!(display_width("中文"), 4);
    }

    #[test]
    fn expand_tabs_basic() {
        assert_eq!(expand_tabs("a\tb"), "a    b");
        assert_eq!(expand_tabs("\t\t"), "        ");
        assert_eq!(expand_tabs("no tabs"), "no tabs");
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate_to_width("hi", 10), "hi");
    }

    #[test]
    fn truncate_long_gets_ellipsis() {
        let result = truncate_to_width("hello world", 5);
        assert!(display_width(&result) <= 5);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_zero() {
        assert_eq!(truncate_to_width("anything", 0), "");
    }

    #[test]
    fn truncate_tab_fits() {
        let r = truncate_to_width("a\tb", 4);
        assert!(display_width(&r) <= 4);
    }

    #[test]
    fn truncate_raw_no_ellipsis() {
        let r = truncate_to_width_raw("hello world", 5);
        assert!(display_width(&r) <= 5);
        assert!(!r.ends_with('…'));
    }

    #[test]
    fn pad_short() {
        assert_eq!(pad_to_width("hi", 5), "hi   ");
    }

    #[test]
    fn pad_long_unchanged() {
        assert_eq!(pad_to_width("hello world", 5), "hello world");
    }

    #[test]
    fn pad_wide_chars() {
        let r = pad_to_width("中", 5);
        assert_eq!(display_width(&r), 5);
    }
}

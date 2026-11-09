//! Small text/layout helpers shared by the pager, dashboard and plain mode.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Each char of `text` with the display width it contributes: the width of
/// its whole grapheme cluster on the cluster's first char, 0 on the others.
pub fn drain_pending_input() {
    while crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
        if crossterm::event::read().is_err() {
            break;
        }
    }
}

pub fn char_widths(text: &str) -> impl Iterator<Item = (char, usize)> + '_ {
    text.graphemes(true).flat_map(|grapheme| {
        let width = UnicodeWidthStr::width(grapheme);
        grapheme
            .chars()
            .enumerate()
            .map(move |(i, ch)| (ch, if i == 0 { width } else { 0 }))
    })
}

pub fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let target = max_width - 1;
    let mut out = String::new();
    let mut width = 0usize;
    for (ch, cw) in char_widths(text) {
        if width + cw > target {
            break;
        }
        out.push(ch);
        width += cw;
    }
    out.push('…');
    out
}

pub fn centered_text(text: &str, width: usize) -> String {
    let text_w = UnicodeWidthStr::width(text);
    if text_w >= width {
        truncate_with_ellipsis(text, width)
    } else {
        format!("{}{}", " ".repeat((width - text_w) / 2), text)
    }
}

/// Centered popup rect, kept one cell inside `area` when possible.
pub fn centered_rect_size(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(3);
    let height = height.min(area.height.saturating_sub(2)).max(3);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// SGR escape that switches to `style` (empty when the style is plain).
pub fn ansi_style_prefix(style: Style) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(fg) = style.fg
        && let Some(code) = ansi_fg_code(fg)
    {
        parts.push(code);
    }
    if let Some(bg) = style.bg
        && let Some(code) = ansi_bg_code(bg)
    {
        parts.push(code);
    }
    let m = style.add_modifier;
    if m.contains(Modifier::BOLD) {
        parts.push("1".to_string());
    }
    if m.contains(Modifier::DIM) {
        parts.push("2".to_string());
    }
    if m.contains(Modifier::ITALIC) {
        parts.push("3".to_string());
    }
    if m.contains(Modifier::UNDERLINED) {
        parts.push("4".to_string());
    }
    if m.contains(Modifier::SLOW_BLINK) {
        parts.push("5".to_string());
    }
    if m.contains(Modifier::RAPID_BLINK) {
        parts.push("6".to_string());
    }
    if m.contains(Modifier::REVERSED) {
        parts.push("7".to_string());
    }
    if m.contains(Modifier::HIDDEN) {
        parts.push("8".to_string());
    }
    if m.contains(Modifier::CROSSED_OUT) {
        parts.push("9".to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", parts.join(";"))
    }
}

pub fn ansi_fg_code(c: Color) -> Option<String> {
    match c {
        Color::Reset => Some("39".to_string()),
        Color::Black => Some("30".to_string()),
        Color::Red => Some("31".to_string()),
        Color::Green => Some("32".to_string()),
        Color::Yellow => Some("33".to_string()),
        Color::Blue => Some("34".to_string()),
        Color::Magenta => Some("35".to_string()),
        Color::Cyan => Some("36".to_string()),
        Color::Gray => Some("37".to_string()),
        Color::DarkGray => Some("90".to_string()),
        Color::LightRed => Some("91".to_string()),
        Color::LightGreen => Some("92".to_string()),
        Color::LightYellow => Some("93".to_string()),
        Color::LightBlue => Some("94".to_string()),
        Color::LightMagenta => Some("95".to_string()),
        Color::LightCyan => Some("96".to_string()),
        Color::White => Some("97".to_string()),
        Color::Rgb(r, g, b) => Some(format!("38;2;{r};{g};{b}")),
        Color::Indexed(v) => Some(format!("38;5;{v}")),
    }
}

pub fn ansi_bg_code(c: Color) -> Option<String> {
    match c {
        Color::Reset => Some("49".to_string()),
        Color::Black => Some("40".to_string()),
        Color::Red => Some("41".to_string()),
        Color::Green => Some("42".to_string()),
        Color::Yellow => Some("43".to_string()),
        Color::Blue => Some("44".to_string()),
        Color::Magenta => Some("45".to_string()),
        Color::Cyan => Some("46".to_string()),
        Color::Gray => Some("47".to_string()),
        Color::DarkGray => Some("100".to_string()),
        Color::LightRed => Some("101".to_string()),
        Color::LightGreen => Some("102".to_string()),
        Color::LightYellow => Some("103".to_string()),
        Color::LightBlue => Some("104".to_string()),
        Color::LightMagenta => Some("105".to_string()),
        Color::LightCyan => Some("106".to_string()),
        Color::White => Some("107".to_string()),
        Color::Rgb(r, g, b) => Some(format!("48;2;{r};{g};{b}")),
        Color::Indexed(v) => Some(format!("48;5;{v}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_with_ellipsis_respects_width() {
        let out = truncate_with_ellipsis("abcdef", 4);
        assert_eq!(UnicodeWidthStr::width(out.as_str()), 4);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn centered_text_adds_left_padding() {
        let out = centered_text("A", 5);
        assert!(out.starts_with("  "));
    }

    #[test]
    fn char_widths_match_terminal_width_for_emoji_sequences() {
        for text in [
            "⚠️ (needs TPM)",
            "➡️ Requires",
            "1️⃣ Disk",
            "❌",
            "plain",
            "日本",
        ] {
            let summed: usize = char_widths(text).map(|(_, w)| w).sum();
            assert_eq!(summed, UnicodeWidthStr::width(text), "{text:?}");
        }
        assert_eq!(
            char_widths("⚠️").map(|(_, w)| w).collect::<Vec<_>>(),
            vec![2, 0]
        );
    }

    #[test]
    fn ansi_style_prefix_encodes_color_and_weight() {
        let style = Style::default()
            .fg(Color::Rgb(1, 2, 3))
            .add_modifier(Modifier::BOLD);
        assert_eq!(ansi_style_prefix(style), "\x1b[38;2;1;2;3;1m");
        assert_eq!(ansi_style_prefix(Style::default()), "");
    }
}

use ratatui::style::{Color, Modifier, Style};

use crate::config::ColorsFile;

#[derive(Clone, Debug)]
pub struct AppTheme {
    pub syntax_theme: String,
    pub normal: Style,
    pub heading_h1: Style,
    pub heading_h2: Style,
    pub heading_h3: Style,
    pub heading: Style,
    pub quote: Style,
    pub list_marker: Style,
    pub inline_code: Style,
    pub code: Style,
    pub link: Style,
    pub status: Style,
    pub status_error: Style,
    pub popup_title: Style,
    pub popup_key: Style,
    pub popup_hint: Style,
    pub search_hit: Style,
    pub search_current: Style,
    pub cursor_line: Style,
    pub line_number: Style,
    pub code_palette: CodePalette,
}

#[derive(Clone, Copy, Debug)]
pub struct CodePalette {
    pub black: Color,
    pub grey: Color,
    pub white: Color,
    pub purple: Color,
    pub pink: Color,
    pub blue: Color,
    pub cyan: Color,
    pub green: Color,
    pub red: Color,
    pub yellow: Color,
    pub orange: Color,
}

impl Default for CodePalette {
    fn default() -> Self {
        Self {
            black: Color::Rgb(42, 47, 56),
            grey: Color::Rgb(136, 150, 167),
            white: Color::Rgb(212, 228, 255),
            purple: Color::Rgb(27, 187, 166),
            pink: Color::Rgb(151, 138, 255),
            blue: Color::Rgb(246, 190, 250),
            cyan: Color::Rgb(116, 168, 251),
            green: Color::Rgb(151, 138, 255),
            red: Color::Rgb(234, 121, 242),
            yellow: Color::Rgb(242, 247, 255),
            orange: Color::Rgb(163, 198, 255),
        }
    }
}

impl Default for AppTheme {
    fn default() -> Self {
        Self::soapy()
    }
}

impl AppTheme {
    pub fn soapy() -> Self {
        Self {
            syntax_theme: "base16-eighties.dark".to_string(),
            normal: Style::default().fg(Color::Rgb(212, 228, 255)),
            heading_h1: Style::default()
                .fg(Color::Rgb(151, 138, 255))
                .add_modifier(Modifier::BOLD),
            heading_h2: Style::default()
                .fg(Color::Rgb(151, 138, 255))
                .add_modifier(Modifier::BOLD),
            heading_h3: Style::default()
                .fg(Color::Rgb(246, 190, 250))
                .add_modifier(Modifier::BOLD),
            heading: Style::default()
                .fg(Color::Rgb(151, 138, 255))
                .add_modifier(Modifier::BOLD),
            quote: Style::default()
                .fg(Color::Rgb(136, 150, 167))
                .add_modifier(Modifier::ITALIC),
            list_marker: Style::default()
                .fg(Color::Rgb(246, 190, 250))
                .add_modifier(Modifier::BOLD),
            inline_code: Style::default()
                .fg(Color::Rgb(27, 187, 166))
                .bg(Color::Reset),
            code: Style::default().bg(Color::Reset),
            link: Style::default()
                .fg(Color::Rgb(116, 168, 251))
                .add_modifier(Modifier::UNDERLINED),
            status: Style::default()
                .fg(Color::Rgb(212, 228, 255))
                .bg(Color::Rgb(38, 42, 40)),
            status_error: Style::default()
                .fg(Color::Rgb(242, 247, 255))
                .bg(Color::Rgb(234, 121, 242)),
            popup_title: Style::default()
                .fg(Color::Rgb(151, 138, 255))
                .add_modifier(Modifier::BOLD),
            popup_key: Style::default()
                .fg(Color::Rgb(27, 187, 166))
                .add_modifier(Modifier::BOLD),
            popup_hint: Style::default().fg(Color::Rgb(116, 168, 251)),
            search_hit: Style::default()
                .bg(Color::Rgb(151, 138, 255))
                .fg(Color::Rgb(42, 47, 56)),
            search_current: Style::default()
                .bg(Color::Rgb(27, 187, 166))
                .fg(Color::Rgb(42, 47, 56))
                .add_modifier(Modifier::BOLD),
            cursor_line: Style::default().bg(Color::Rgb(99, 112, 138)),
            line_number: Style::default().fg(Color::Rgb(136, 150, 167)),
            code_palette: CodePalette::default(),
        }
    }

    pub fn apply_overrides(&mut self, c: &ColorsFile) {
        if let Some(color) = c.normal_fg.as_deref().and_then(parse_color) {
            self.normal = self.normal.fg(color);
        }
        if let Some(color) = c.heading_fg.as_deref().and_then(parse_color) {
            self.heading = self.heading.fg(color);
        }
        if let Some(color) = c.quote_fg.as_deref().and_then(parse_color) {
            self.quote = self.quote.fg(color);
        }
        if let Some(color) = c.list_marker_fg.as_deref().and_then(parse_color) {
            self.list_marker = self.list_marker.fg(color);
        }
        if let Some(color) = c.inline_code_fg.as_deref().and_then(parse_color) {
            self.inline_code = self.inline_code.fg(color);
        }
        if let Some(color) = c.link_fg.as_deref().and_then(parse_color) {
            self.link = self.link.fg(color);
        }
        if let Some(color) = c.status_fg.as_deref().and_then(parse_color) {
            self.status = self.status.fg(color);
        }
        if let Some(color) = c.status_bg.as_deref().and_then(parse_color) {
            self.status = self.status.bg(color);
        }
        if let Some(color) = c.search_hit_fg.as_deref().and_then(parse_color) {
            self.search_hit = self.search_hit.fg(color);
        }
        if let Some(color) = c.search_hit_bg.as_deref().and_then(parse_color) {
            self.search_hit = self.search_hit.bg(color);
        }
        if let Some(color) = c.search_current_fg.as_deref().and_then(parse_color) {
            self.search_current = self.search_current.fg(color);
        }
        if let Some(color) = c.search_current_bg.as_deref().and_then(parse_color) {
            self.search_current = self.search_current.bg(color);
        }
        if let Some(color) = c.cursor_line_bg.as_deref().and_then(parse_color) {
            self.cursor_line = self.cursor_line.bg(color);
        }
        if let Some(color) = c.line_number_fg.as_deref().and_then(parse_color) {
            self.line_number = self.line_number.fg(color);
        }
        if let Some(color) = c.code_black.as_deref().and_then(parse_color) {
            self.code_palette.black = color;
        }
        if let Some(color) = c.code_grey.as_deref().and_then(parse_color) {
            self.code_palette.grey = color;
        }
        if let Some(color) = c.code_white.as_deref().and_then(parse_color) {
            self.code_palette.white = color;
        }
        if let Some(color) = c.code_purple.as_deref().and_then(parse_color) {
            self.code_palette.purple = color;
        }
        if let Some(color) = c.code_pink.as_deref().and_then(parse_color) {
            self.code_palette.pink = color;
        }
        if let Some(color) = c.code_blue.as_deref().and_then(parse_color) {
            self.code_palette.blue = color;
        }
        if let Some(color) = c.code_cyan.as_deref().and_then(parse_color) {
            self.code_palette.cyan = color;
        }
        if let Some(color) = c.code_green.as_deref().and_then(parse_color) {
            self.code_palette.green = color;
        }
        if let Some(color) = c.code_red.as_deref().and_then(parse_color) {
            self.code_palette.red = color;
        }
        if let Some(color) = c.code_yellow.as_deref().and_then(parse_color) {
            self.code_palette.yellow = color;
        }
        if let Some(color) = c.code_orange.as_deref().and_then(parse_color) {
            self.code_palette.orange = color;
        }
        if colors_has_any(c) {
            derive_theme_from_palette(self);
        }
        self.syntax_theme = syntect_theme_for_theme(self);
    }
}

fn colors_has_any(c: &ColorsFile) -> bool {
    [
        &c.normal_fg,
        &c.heading_fg,
        &c.quote_fg,
        &c.list_marker_fg,
        &c.inline_code_fg,
        &c.link_fg,
        &c.status_fg,
        &c.status_bg,
        &c.search_hit_fg,
        &c.search_hit_bg,
        &c.search_current_fg,
        &c.search_current_bg,
        &c.cursor_line_bg,
        &c.line_number_fg,
        &c.code_black,
        &c.code_grey,
        &c.code_white,
        &c.code_purple,
        &c.code_pink,
        &c.code_blue,
        &c.code_cyan,
        &c.code_green,
        &c.code_red,
        &c.code_yellow,
        &c.code_orange,
    ]
    .iter()
    .any(|v| v.is_some())
}

fn derive_theme_from_palette(t: &mut AppTheme) {
    let normal_fg = t.normal.fg.unwrap_or(Color::White);
    let heading_fg = t.heading.fg.unwrap_or(t.code_palette.purple);
    let quote_fg = t.quote.fg.unwrap_or(t.code_palette.grey);
    let list_fg = t.list_marker.fg.unwrap_or(t.code_palette.blue);
    let inline_fg = t.inline_code.fg.unwrap_or(t.code_palette.cyan);
    let link_fg = t.link.fg.unwrap_or(t.code_palette.cyan);
    let status_fg = t.status.fg.unwrap_or(normal_fg);
    let status_bg = t.status.bg.unwrap_or(t.code_palette.black);
    let line_fg = t.line_number.fg.unwrap_or(quote_fg);
    let cursor_bg = t.cursor_line.bg.unwrap_or(t.code_palette.black);

    t.normal = Style::default().fg(normal_fg);
    t.heading = Style::default().fg(heading_fg).add_modifier(Modifier::BOLD);
    t.heading_h1 = Style::default().fg(heading_fg).add_modifier(Modifier::BOLD);
    t.heading_h2 = Style::default()
        .fg(t.code_palette.pink)
        .add_modifier(Modifier::BOLD);
    t.heading_h3 = Style::default()
        .fg(t.code_palette.blue)
        .add_modifier(Modifier::BOLD);
    t.quote = Style::default().fg(quote_fg).add_modifier(Modifier::ITALIC);
    t.list_marker = Style::default().fg(list_fg).add_modifier(Modifier::BOLD);
    t.inline_code = Style::default().fg(inline_fg).bg(Color::Reset);
    t.code = Style::default().bg(Color::Reset);
    t.link = Style::default()
        .fg(link_fg)
        .add_modifier(Modifier::UNDERLINED);
    t.status = Style::default().fg(status_fg).bg(status_bg);
    t.status_error = Style::default()
        .fg(t.code_palette.yellow)
        .bg(t.code_palette.red);
    t.popup_title = Style::default().fg(heading_fg).add_modifier(Modifier::BOLD);
    t.popup_key = Style::default().fg(inline_fg).add_modifier(Modifier::BOLD);
    t.popup_hint = Style::default().fg(link_fg);

    let search_hit_fg = t.search_hit.fg.unwrap_or(t.code_palette.black);
    let search_hit_bg = t.search_hit.bg.unwrap_or(t.code_palette.blue);
    t.search_hit = Style::default().fg(search_hit_fg).bg(search_hit_bg);

    let search_current_fg = t.search_current.fg.unwrap_or(t.code_palette.black);
    let search_current_bg = t.search_current.bg.unwrap_or(t.code_palette.cyan);
    t.search_current = Style::default()
        .fg(search_current_fg)
        .bg(search_current_bg)
        .add_modifier(Modifier::BOLD);

    t.cursor_line = Style::default().bg(cursor_bg);
    t.line_number = Style::default().fg(line_fg);
}

fn syntect_theme_for_theme(t: &AppTheme) -> String {
    let bg = t.code.bg.or(t.status.bg).unwrap_or(Color::Black);
    let lightness = color_luma(bg);
    if lightness < 120 {
        if is_dark_syntect_theme_name(&t.syntax_theme) {
            t.syntax_theme.clone()
        } else {
            "base16-eighties.dark".to_string()
        }
    } else {
        "InspiredGitHub".to_string()
    }
}

fn is_dark_syntect_theme_name(name: &str) -> bool {
    !name.eq_ignore_ascii_case("InspiredGitHub")
}

fn color_luma(c: Color) -> u16 {
    match c {
        Color::Rgb(r, g, b) => ((r as u16 * 3) + (g as u16 * 6) + (b as u16)) / 10,
        Color::Black => 0,
        Color::DarkGray => 64,
        Color::Gray => 190,
        Color::White => 255,
        Color::Red => 76,
        Color::Green => 150,
        Color::Blue => 29,
        Color::Yellow => 226,
        Color::Magenta => 105,
        Color::Cyan => 178,
        Color::LightRed => 140,
        Color::LightGreen => 200,
        Color::LightBlue => 120,
        Color::LightYellow => 240,
        Color::LightMagenta => 170,
        Color::LightCyan => 210,
        _ => 128,
    }
}

fn parse_color(s: &str) -> Option<Color> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
    }
    match t.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        "lightred" => Some(Color::LightRed),
        "lightgreen" => Some(Color::LightGreen),
        "lightyellow" => Some(Color::LightYellow),
        "lightblue" => Some(Color::LightBlue),
        "lightmagenta" => Some(Color::LightMagenta),
        "lightcyan" => Some(Color::LightCyan),
        _ => None,
    }
}

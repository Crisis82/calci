use ratatui::style::{Color, Modifier, Style};

use crate::config::ColorsFile;

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum ThemeName {
    Oxocarbon,
    DarkHorizon,
    Dark,
    Light,
    Dracula,
    SolarizedDark,
}

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
            black: Color::Rgb(27, 27, 27),
            grey: Color::Rgb(107, 107, 107),
            white: Color::Rgb(242, 244, 248),
            purple: Color::Rgb(190, 149, 255),
            pink: Color::Rgb(199, 209, 255),
            blue: Color::Rgb(120, 169, 255),
            cyan: Color::Rgb(130, 207, 255),
            green: Color::Rgb(41, 211, 152),
            red: Color::Rgb(238, 83, 150),
            yellow: Color::Rgb(255, 255, 255),
            orange: Color::Rgb(182, 194, 255),
        }
    }
}

impl AppTheme {
    pub fn from_name(name: ThemeName) -> Self {
        match name {
            ThemeName::Oxocarbon => Self {
                syntax_theme: "base16-eighties.dark".to_string(),
                normal: Style::default().fg(Color::Rgb(242, 244, 248)),
                heading_h1: Style::default()
                    .fg(Color::Rgb(190, 149, 255))
                    .add_modifier(Modifier::BOLD),
                heading_h2: Style::default()
                    .fg(Color::Rgb(178, 148, 255))
                    .add_modifier(Modifier::BOLD),
                heading_h3: Style::default()
                    .fg(Color::Rgb(130, 207, 255))
                    .add_modifier(Modifier::BOLD),
                heading: Style::default()
                    .fg(Color::Rgb(178, 148, 255))
                    .add_modifier(Modifier::BOLD),
                quote: Style::default()
                    .fg(Color::Rgb(107, 107, 107))
                    .add_modifier(Modifier::ITALIC),
                list_marker: Style::default()
                    .fg(Color::Rgb(120, 169, 255))
                    .add_modifier(Modifier::BOLD),
                inline_code: Style::default()
                    .fg(Color::Rgb(61, 219, 217))
                    .bg(Color::Reset),
                code: Style::default().bg(Color::Reset),
                link: Style::default()
                    .fg(Color::Rgb(130, 207, 255))
                    .add_modifier(Modifier::UNDERLINED),
                status: Style::default()
                    .fg(Color::Rgb(242, 244, 248))
                    .bg(Color::Rgb(36, 36, 36)),
                status_error: Style::default()
                    .fg(Color::Rgb(255, 255, 255))
                    .bg(Color::Rgb(238, 83, 150)),
                popup_title: Style::default()
                    .fg(Color::Rgb(190, 149, 255))
                    .add_modifier(Modifier::BOLD),
                popup_key: Style::default()
                    .fg(Color::Rgb(61, 219, 217))
                    .add_modifier(Modifier::BOLD),
                popup_hint: Style::default().fg(Color::Rgb(130, 207, 255)),
                search_hit: Style::default()
                    .bg(Color::Rgb(182, 194, 255))
                    .fg(Color::Black),
                search_current: Style::default()
                    .bg(Color::Rgb(61, 219, 217))
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                cursor_line: Style::default().bg(Color::Rgb(46, 46, 46)),
                line_number: Style::default().fg(Color::Rgb(107, 107, 107)),
                code_palette: CodePalette::default(),
            },
            ThemeName::DarkHorizon => Self {
                syntax_theme: "base16-eighties.dark".to_string(),
                normal: Style::default().fg(Color::Rgb(221, 221, 221)),
                heading_h1: Style::default()
                    .fg(Color::Rgb(240, 117, 181))
                    .add_modifier(Modifier::BOLD),
                heading_h2: Style::default()
                    .fg(Color::Rgb(184, 119, 219))
                    .add_modifier(Modifier::BOLD),
                heading_h3: Style::default()
                    .fg(Color::Rgb(63, 196, 222))
                    .add_modifier(Modifier::BOLD),
                heading: Style::default()
                    .fg(Color::Rgb(184, 119, 219))
                    .add_modifier(Modifier::BOLD),
                quote: Style::default()
                    .fg(Color::Rgb(82, 82, 82))
                    .add_modifier(Modifier::ITALIC),
                list_marker: Style::default()
                    .fg(Color::Rgb(63, 196, 222))
                    .add_modifier(Modifier::BOLD),
                inline_code: Style::default()
                    .fg(Color::Rgb(250, 183, 149))
                    .bg(Color::Reset),
                code: Style::default().bg(Color::Reset),
                link: Style::default()
                    .fg(Color::Rgb(89, 225, 227))
                    .add_modifier(Modifier::UNDERLINED),
                status: Style::default()
                    .fg(Color::Rgb(221, 221, 221))
                    .bg(Color::Rgb(39, 38, 38)),
                status_error: Style::default()
                    .fg(Color::Rgb(255, 255, 255))
                    .bg(Color::Rgb(233, 86, 120)),
                popup_title: Style::default()
                    .fg(Color::Rgb(240, 117, 181))
                    .add_modifier(Modifier::BOLD),
                popup_key: Style::default()
                    .fg(Color::Rgb(63, 196, 222))
                    .add_modifier(Modifier::BOLD),
                popup_hint: Style::default().fg(Color::Rgb(89, 225, 227)),
                search_hit: Style::default()
                    .bg(Color::Rgb(250, 183, 149))
                    .fg(Color::Black),
                search_current: Style::default()
                    .bg(Color::Rgb(63, 196, 222))
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                cursor_line: Style::default().bg(Color::Rgb(43, 43, 43)),
                line_number: Style::default().fg(Color::Rgb(82, 82, 82)),
                code_palette: CodePalette::default(),
            },
            ThemeName::Light => Self {
                syntax_theme: "InspiredGitHub".to_string(),
                normal: Style::default().fg(Color::Black),
                heading_h1: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                heading_h2: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                heading_h3: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                heading: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                quote: Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
                list_marker: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                inline_code: Style::default().fg(Color::Red).bg(Color::Reset),
                code: Style::default().bg(Color::Reset),
                link: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::UNDERLINED),
                status: Style::default()
                    .fg(Color::Black)
                    .bg(Color::Rgb(220, 220, 220)),
                status_error: Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(170, 30, 30)),
                popup_title: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                popup_key: Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
                popup_hint: Style::default().fg(Color::Blue),
                search_hit: Style::default().bg(Color::Yellow).fg(Color::Black),
                search_current: Style::default()
                    .bg(Color::Rgb(255, 186, 0))
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                cursor_line: Style::default().bg(Color::Rgb(230, 240, 255)),
                line_number: Style::default().fg(Color::DarkGray),
                code_palette: CodePalette::default(),
            },
            ThemeName::Dracula => Self {
                syntax_theme: "base16-eighties.dark".to_string(),
                normal: Style::default().fg(Color::Rgb(248, 248, 242)),
                heading_h1: Style::default()
                    .fg(Color::Rgb(255, 121, 198))
                    .add_modifier(Modifier::BOLD),
                heading_h2: Style::default()
                    .fg(Color::Rgb(189, 147, 249))
                    .add_modifier(Modifier::BOLD),
                heading_h3: Style::default()
                    .fg(Color::Rgb(139, 233, 253))
                    .add_modifier(Modifier::BOLD),
                heading: Style::default()
                    .fg(Color::Rgb(189, 147, 249))
                    .add_modifier(Modifier::BOLD),
                quote: Style::default()
                    .fg(Color::Rgb(98, 114, 164))
                    .add_modifier(Modifier::ITALIC),
                list_marker: Style::default().fg(Color::Rgb(80, 250, 123)),
                inline_code: Style::default()
                    .fg(Color::Rgb(255, 184, 108))
                    .bg(Color::Reset),
                code: Style::default().bg(Color::Reset),
                link: Style::default()
                    .fg(Color::Rgb(139, 233, 253))
                    .add_modifier(Modifier::UNDERLINED),
                status: Style::default()
                    .fg(Color::Rgb(248, 248, 242))
                    .bg(Color::Rgb(68, 71, 90)),
                status_error: Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(255, 85, 85)),
                popup_title: Style::default()
                    .fg(Color::Rgb(255, 121, 198))
                    .add_modifier(Modifier::BOLD),
                popup_key: Style::default()
                    .fg(Color::Rgb(139, 233, 253))
                    .add_modifier(Modifier::BOLD),
                popup_hint: Style::default().fg(Color::Rgb(80, 250, 123)),
                search_hit: Style::default()
                    .bg(Color::Rgb(80, 250, 123))
                    .fg(Color::Black),
                search_current: Style::default()
                    .bg(Color::Rgb(255, 184, 108))
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                cursor_line: Style::default().bg(Color::Rgb(56, 58, 75)),
                line_number: Style::default().fg(Color::Rgb(120, 120, 130)),
                code_palette: CodePalette::default(),
            },
            ThemeName::SolarizedDark => Self {
                syntax_theme: "Solarized (dark)".to_string(),
                normal: Style::default().fg(Color::Rgb(131, 148, 150)),
                heading_h1: Style::default()
                    .fg(Color::Rgb(211, 54, 130))
                    .add_modifier(Modifier::BOLD),
                heading_h2: Style::default()
                    .fg(Color::Rgb(38, 139, 210))
                    .add_modifier(Modifier::BOLD),
                heading_h3: Style::default()
                    .fg(Color::Rgb(42, 161, 152))
                    .add_modifier(Modifier::BOLD),
                heading: Style::default()
                    .fg(Color::Rgb(38, 139, 210))
                    .add_modifier(Modifier::BOLD),
                quote: Style::default()
                    .fg(Color::Rgb(88, 110, 117))
                    .add_modifier(Modifier::ITALIC),
                list_marker: Style::default().fg(Color::Rgb(42, 161, 152)),
                inline_code: Style::default()
                    .fg(Color::Rgb(220, 50, 47))
                    .bg(Color::Reset),
                code: Style::default().bg(Color::Reset),
                link: Style::default()
                    .fg(Color::Rgb(181, 137, 0))
                    .add_modifier(Modifier::UNDERLINED),
                status: Style::default()
                    .fg(Color::Rgb(131, 148, 150))
                    .bg(Color::Rgb(0, 43, 54)),
                status_error: Style::default()
                    .fg(Color::Rgb(253, 246, 227))
                    .bg(Color::Rgb(220, 50, 47)),
                popup_title: Style::default()
                    .fg(Color::Rgb(38, 139, 210))
                    .add_modifier(Modifier::BOLD),
                popup_key: Style::default()
                    .fg(Color::Rgb(181, 137, 0))
                    .add_modifier(Modifier::BOLD),
                popup_hint: Style::default().fg(Color::Rgb(42, 161, 152)),
                search_hit: Style::default()
                    .bg(Color::Rgb(181, 137, 0))
                    .fg(Color::Black),
                search_current: Style::default()
                    .bg(Color::Rgb(38, 139, 210))
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                cursor_line: Style::default().bg(Color::Rgb(12, 62, 74)),
                line_number: Style::default().fg(Color::Rgb(88, 110, 117)),
                code_palette: CodePalette::default(),
            },
            ThemeName::Dark => Self {
                syntax_theme: "Solarized (dark)".to_string(),
                normal: Style::default().fg(Color::Gray),
                heading_h1: Style::default()
                    .fg(Color::LightMagenta)
                    .add_modifier(Modifier::BOLD),
                heading_h2: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                heading_h3: Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
                heading: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                quote: Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
                list_marker: Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
                inline_code: Style::default().fg(Color::Yellow).bg(Color::Reset),
                code: Style::default().bg(Color::Reset),
                link: Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::UNDERLINED),
                status: Style::default().fg(Color::White).bg(Color::Rgb(40, 40, 40)),
                status_error: Style::default()
                    .fg(Color::White)
                    .bg(Color::Rgb(150, 40, 40)),
                popup_title: Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
                popup_key: Style::default()
                    .fg(Color::LightBlue)
                    .add_modifier(Modifier::BOLD),
                popup_hint: Style::default().fg(Color::Yellow),
                search_hit: Style::default().bg(Color::Yellow).fg(Color::Black),
                search_current: Style::default()
                    .bg(Color::LightBlue)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
                cursor_line: Style::default().bg(Color::Rgb(45, 45, 45)),
                line_number: Style::default().fg(Color::DarkGray),
                code_palette: CodePalette::default(),
            },
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

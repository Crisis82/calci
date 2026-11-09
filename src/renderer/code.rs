//! Code blocks: syntax highlighting theme, block padding and `{: title=...}` titles.

use super::*;

pub(super) const BLOCK_TITLE_MARKER: &str = "__calci_block_title__:";
pub(super) const BLOCK_PAD_X: usize = 2;
pub(super) const BLOCK_PAD_TOP: usize = 1;
pub(super) const BLOCK_PAD_BOTTOM: usize = 1;
pub(super) const BLOCK_TITLE_GAP: usize = 1;

/// Syntax definitions are expensive to load (tens of ms); load them once per
/// process instead of on every render/resize/reload.
pub(super) fn syntax_set() -> &'static SyntaxSet {
    static SYNTAX_SET: std::sync::OnceLock<SyntaxSet> = std::sync::OnceLock::new();
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

pub(super) fn exact_semantic_theme(app_theme: &AppTheme) -> Theme {
    let p = &app_theme.code_palette;
    let scopes = vec![
        theme_item("comment", p.grey, true, false),
        theme_item("punctuation.definition.comment", p.grey, true, false),
        // Keep punctuation and operators uncolored: default code text, non-bold.
        theme_item(
            "keyword.operator, punctuation.accessor, punctuation.separator.key-value, punctuation.definition.variable, meta.delimiter, meta.brace",
            p.white,
            false,
            false,
        ),
        // Rust-targeted refinements
        theme_item(
            "source.rust support.macro, source.rust entity.name.macro, source.rust meta.macro",
            p.green,
            false,
            false,
        ),
        theme_item(
            "source.rust meta.attribute, source.rust entity.other.attribute-name",
            p.orange,
            false,
            false,
        ),
        theme_item("source.rust entity.name.namespace", p.pink, false, true),
        theme_item(
            "source.rust support.function.builtin, source.rust support.function.std, source.rust support.function.prelude",
            p.cyan,
            false,
            false,
        ),
        // TypeScript / TSX-targeted refinements
        theme_item(
            "source.ts support.function.builtin, source.tsx support.function.builtin, source.ts variable.language.this, source.tsx variable.language.this",
            p.cyan,
            false,
            false,
        ),
        theme_item(
            "source.ts entity.name.namespace, source.tsx entity.name.namespace",
            p.pink,
            false,
            true,
        ),
        theme_item(
            "source.ts support.type, source.tsx support.type, source.ts support.class, source.tsx support.class",
            p.pink,
            false,
            true,
        ),
        // Python-targeted refinements
        theme_item(
            "source.python support.function.builtin, source.python support.type",
            p.cyan,
            false,
            false,
        ),
        theme_item(
            "source.python entity.name.namespace, source.python meta.import",
            p.pink,
            false,
            true,
        ),
        theme_item(
            "source.python variable.parameter.function.language.special.self, source.python variable.language.self",
            p.cyan,
            false,
            false,
        ),
        // Shared semantic groups
        theme_item(
            "entity.name.macro, support.macro, meta.macro",
            p.green,
            false,
            false,
        ),
        theme_item(
            "meta.attribute, entity.other.attribute-name",
            p.orange,
            false,
            false,
        ),
        theme_item("entity.name.namespace, support.module", p.pink, false, true),
        theme_item(
            "support.function.builtin, support.class.builtin, support.type.builtin, support.constant, support.variable, variable.language, variable.language.this, variable.language.self, variable.language.super",
            p.cyan,
            false,
            false,
        ),
        theme_item(
            "constant.numeric, constant.other.number",
            p.orange,
            false,
            false,
        ),
        theme_item(
            "constant.language, constant.character.escape, constant.other.symbol",
            p.red,
            false,
            false,
        ),
        theme_item("constant", p.red, false, false),
        theme_item(
            "keyword.control, keyword.declaration, storage.modifier",
            p.purple,
            false,
            true,
        ),
        theme_item("keyword.other, storage, storage.type", p.pink, false, true),
        theme_item(
            "entity.name.type, support.type, support.class",
            p.pink,
            false,
            true,
        ),
        theme_item(
            "entity.name.function, support.function, variable.function",
            p.blue,
            false,
            false,
        ),
        theme_item("string, string.*", p.cyan, false, false),
        theme_item("variable.parameter", p.white, false, false),
        theme_item("variable, variable.other", p.white, false, false),
        theme_item(
            "entity.name.tag, support.constant, support.variable",
            p.green,
            false,
            false,
        ),
        theme_item(
            "invalid, invalid.illegal, invalid.deprecated",
            p.red,
            false,
            true,
        ),
    ];

    let settings = ThemeSettings {
        foreground: Some(to_syn_color(p.white)),
        background: app_theme.code.bg.map(to_syn_color),
        caret: app_theme.cursor_line.bg.map(to_syn_color),
        ..ThemeSettings::default()
    };

    Theme {
        name: Some("calci-exact-semantic".to_string()),
        author: Some("calci".to_string()),
        settings,
        scopes,
    }
}

pub(super) fn theme_item(
    scope_selector: &str,
    color: Color,
    italic: bool,
    bold: bool,
) -> ThemeItem {
    let mut font_style = SynFontStyle::empty();
    if italic {
        font_style |= SynFontStyle::ITALIC;
    }
    if bold {
        font_style |= SynFontStyle::BOLD;
    }
    ThemeItem {
        scope: ScopeSelectors::from_str(scope_selector).expect("valid scope selector"),
        style: SynStyleModifier {
            foreground: Some(to_syn_color(color)),
            background: None,
            font_style: if italic || bold {
                Some(font_style)
            } else {
                None
            },
        },
    }
}

pub(super) fn to_syn_color(c: Color) -> SynColor {
    let (r, g, b) = color_to_rgb(c);
    SynColor { r, g, b, a: 0xFF }
}

pub(super) fn highlight_code_block(
    code: &str,
    lang: &str,
    syntax_set: &SyntaxSet,
    theme: &Theme,
    fallback_style: Style,
) -> Vec<Line<'static>> {
    let syntax = syntax_set
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text());
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for raw_line in LinesWithEndings::from(code) {
        let highlighted = highlighter.highlight_line(raw_line, syntax_set);
        match highlighted {
            Ok(regions) => {
                let spans = regions
                    .into_iter()
                    .filter_map(|(style, txt)| {
                        let txt = txt.trim_end_matches(&['\r', '\n'][..]);
                        if txt.is_empty() {
                            return None;
                        }
                        let token_fg = ratatui::style::Color::Rgb(
                            style.foreground.r,
                            style.foreground.g,
                            style.foreground.b,
                        );
                        let mut st = fallback_style.fg(token_fg);
                        if style
                            .font_style
                            .contains(syntect::highlighting::FontStyle::BOLD)
                        {
                            st = st.add_modifier(Modifier::BOLD);
                        }
                        if style
                            .font_style
                            .contains(syntect::highlighting::FontStyle::ITALIC)
                        {
                            st = st.add_modifier(Modifier::ITALIC);
                        }
                        Some(Span::styled(txt.to_string(), st))
                    })
                    .collect::<Vec<_>>();
                if spans.is_empty() {
                    out.push(Line::from(""));
                } else {
                    out.push(Line::from(spans));
                }
            }
            Err(_) => out.push(Line::from(vec![Span::styled(
                raw_line.trim_end_matches(&['\r', '\n'][..]).to_string(),
                fallback_style,
            )])),
        }
    }
    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

pub(super) fn plain_code_lines(code: &str, style: Style) -> Vec<Line<'static>> {
    let mut out = code
        .lines()
        .map(|line| Line::from(vec![Span::styled(line.to_string(), style)]))
        .collect::<Vec<_>>();
    if out.is_empty() {
        out.push(Line::from(vec![Span::styled(String::new(), style)]));
    }
    out
}

pub(super) struct BlockRenderLine {
    pub(super) line: Line<'static>,
    pub(super) is_code_content: bool,
}

pub(super) fn pad_block_lines(
    mut lines: Vec<Line<'static>>,
    title: Option<&str>,
    pad_style: Style,
    title_style: Style,
) -> Vec<BlockRenderLine> {
    if let Some(title) = title
        && !title.trim().is_empty()
    {
        lines.insert(
            0,
            Line::from(vec![Span::styled(title.to_string(), title_style)]),
        );
    }
    let mut lines = if lines.is_empty() {
        vec![Line::from("")]
    } else {
        lines
    };
    let max_width = lines.iter().map(line_display_width).max().unwrap_or(0);
    let left_pad = " ".repeat(BLOCK_PAD_X);
    let block_width = (max_width + (BLOCK_PAD_X * 2)).max(1);
    let vertical_pad = " ".repeat(block_width);
    let mut out =
        Vec::with_capacity(lines.len() + BLOCK_PAD_TOP + BLOCK_PAD_BOTTOM + BLOCK_TITLE_GAP);
    for _ in 0..BLOCK_PAD_TOP {
        out.push(BlockRenderLine {
            line: Line::from(vec![Span::styled(vertical_pad.clone(), pad_style)]),
            is_code_content: false,
        });
    }
    for (idx, line) in lines.iter_mut().enumerate() {
        let current = line_display_width(line);
        let right_fill = " ".repeat(max_width.saturating_sub(current) + BLOCK_PAD_X);
        line.spans
            .insert(0, Span::styled(left_pad.clone(), pad_style));
        line.spans.push(Span::styled(right_fill, pad_style));
        let is_title_line = title.is_some() && idx == 0;
        out.push(BlockRenderLine {
            line: line.clone(),
            is_code_content: !is_title_line,
        });
        if is_title_line {
            for _ in 0..BLOCK_TITLE_GAP {
                out.push(BlockRenderLine {
                    line: Line::from(vec![Span::styled(vertical_pad.clone(), pad_style)]),
                    is_code_content: false,
                });
            }
        }
    }
    for _ in 0..BLOCK_PAD_BOTTOM {
        out.push(BlockRenderLine {
            line: Line::from(vec![Span::styled(vertical_pad.clone(), pad_style)]),
            is_code_content: false,
        });
    }
    out
}

pub(super) fn line_display_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

pub(super) fn color_to_rgb(color: ratatui::style::Color) -> (u8, u8, u8) {
    use ratatui::style::Color;
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::DarkGray => (80, 80, 80),
        Color::Gray => (150, 150, 150),
        Color::White => (255, 255, 255),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::Indexed(_) | Color::Reset => (150, 150, 150),
    }
}

pub(super) fn preprocess_block_title_attributes(markdown: &str) -> String {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0usize;
    while i < lines.len() {
        if let Some((fence_char, fence_count)) = parse_fence_start(lines[i]) {
            let mut block = vec![lines[i].to_string()];
            i += 1;
            while i < lines.len() {
                let line = lines[i];
                block.push(line.to_string());
                if is_fence_end(line, fence_char, fence_count) {
                    if i + 1 < lines.len()
                        && let Some(title) = parse_jekyll_block_title(lines[i + 1])
                    {
                        block.insert(1, format!("{BLOCK_TITLE_MARKER}{title}"));
                        i += 1;
                    }
                    break;
                }
                i += 1;
            }
            out.extend(block);
            i += 1;
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    let mut rendered = out.join("\n");
    if markdown.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
}

pub(super) fn parse_fence_start(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|c| *c == ch).count();
    if count >= 3 { Some((ch, count)) } else { None }
}

pub(super) fn is_fence_end(line: &str, fence_char: char, fence_count: usize) -> bool {
    let trimmed = line.trim_start();
    let run = trimmed.chars().take_while(|c| *c == fence_char).count();
    if run < fence_count {
        return false;
    }
    trimmed[run..].trim().is_empty()
}

pub(super) fn parse_jekyll_block_title(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !(trimmed.starts_with("{:") && trimmed.ends_with('}')) {
        return None;
    }
    let inner = trimmed[2..trimmed.len().saturating_sub(1)].trim();
    let title_pos = inner.find("title")?;
    let mut rem = inner[title_pos + "title".len()..].trim_start();
    rem = rem.strip_prefix('=')?.trim_start();
    if let Some(rest) = rem.strip_prefix('"') {
        let end = rest.find('"')?;
        return Some(rest[..end].to_string());
    }
    if let Some(rest) = rem.strip_prefix('\'') {
        let end = rest.find('\'')?;
        return Some(rest[..end].to_string());
    }
    None
}

pub(super) fn extract_block_title_marker(line: &str) -> Option<&str> {
    line.strip_prefix(BLOCK_TITLE_MARKER)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub(super) fn split_block_title_marker(code: &str) -> (Option<String>, String) {
    let mut lines = code.lines();
    let Some(first) = lines.next() else {
        return (None, String::new());
    };
    if let Some(title) = extract_block_title_marker(first.trim()) {
        let mut rest = lines.collect::<Vec<_>>().join("\n");
        if code.ends_with('\n') && !rest.is_empty() {
            rest.push('\n');
        }
        return (Some(title.to_string()), rest);
    }
    (None, code.to_string())
}

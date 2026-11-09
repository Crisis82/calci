//! Inline/display math: kitty placeholder spans (RaTeX images) with the
//! LaTeX source as fallback, and `$...$` preprocessing.

use super::*;

pub fn preprocess_math(markdown: &str) -> String {
    normalize_loose_inline_math(markdown)
}

/// Math (and its LaTeX-source fallback) uses the normal text colour, like
/// printed mathematics; the math font already sets it apart.
pub(super) fn math_span_style(theme: &AppTheme, base: Style) -> Style {
    match theme.normal.fg {
        Some(fg) => base.fg(fg),
        None => base,
    }
}

pub(super) fn normalize_loose_inline_math(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            out.push(chars[i]);
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        if chars[i] == '$' {
            if i + 1 < chars.len() && chars[i + 1] == '$' {
                out.push('$');
                out.push('$');
                i += 2;
                continue;
            }
            let mut j = i + 1;
            let mut found = None;
            while j < chars.len() {
                if chars[j] == '\\' && j + 1 < chars.len() {
                    j += 2;
                    continue;
                }
                if chars[j] == '\n' || chars[j] == '\r' {
                    break;
                }
                if chars[j] == '$' {
                    if j + 1 < chars.len() && chars[j + 1] == '$' {
                        j += 2;
                        continue;
                    }
                    found = Some(j);
                    break;
                }
                j += 1;
            }
            if let Some(end) = found {
                let raw: String = chars[i + 1..end].iter().collect();
                let trimmed = raw.trim();
                if !trimmed.is_empty() && raw != trimmed && looks_like_math(trimmed) {
                    out.push('$');
                    out.push_str(trimmed);
                    out.push('$');
                } else {
                    out.push('$');
                    out.push_str(&raw);
                    out.push('$');
                }
                i = end + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

pub(super) fn looks_like_math(s: &str) -> bool {
    s.contains('\\')
        || s.contains('^')
        || s.contains('_')
        || s.contains('{')
        || s.contains('}')
        || s.contains('=')
        || s.contains('+')
        || s.contains('-')
        || s.contains('*')
        || s.contains('/')
        || s.contains('<')
        || s.contains('>')
}

pub(super) fn trim_common_leading_spaces(lines: &[String]) -> Vec<String> {
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.chars().take_while(|c| *c == ' ').count())
        .min()
        .unwrap_or(0);
    if min_indent == 0 {
        return lines.to_vec();
    }
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                l.chars().skip(min_indent).collect::<String>()
            }
        })
        .collect()
}

/// Colour the math images are drawn in: the normal text colour (light grey
/// when the theme leaves it to the terminal).
fn math_rgb(theme: &AppTheme) -> (u8, u8, u8) {
    match theme.normal.fg {
        Some(Color::Rgb(r, g, b)) => (r, g, b),
        _ => (0xd4, 0xd4, 0xd4),
    }
}

/// Inline math: a one-row image as placeholder cells, or (no kitty graphics,
/// or invalid LaTeX) the source between `$`.
pub(super) fn inline_math_span(settings: &RenderSettings, latex: &str) -> Span<'static> {
    let latex = latex.trim();
    if let Some(store) = &settings.math_images
        && let Some((id, cols)) =
            store
                .borrow_mut()
                .inline(latex, math_rgb(&settings.theme), settings.width as usize)
    {
        return Span::styled(
            placeholder::row_text(0, cols),
            Style::default().fg(placeholder::id_color(id)),
        );
    }
    Span::styled(
        format!("${latex}$"),
        math_span_style(&settings.theme, settings.theme.normal),
    )
}

/// Display math: image rows as placeholder cells, or the LaTeX source lines
/// in `fallback_style`.
pub(super) fn display_math_lines(
    settings: &RenderSettings,
    latex: &str,
    fallback_style: Style,
) -> Vec<Line<'static>> {
    let max_cols = (settings.width as usize)
        .saturating_sub(BLOCK_PAD_X * 2)
        .max(1);
    if let Some(store) = &settings.math_images
        && let Some((id, cols, rows)) =
            store
                .borrow_mut()
                .display(latex.trim(), math_rgb(&settings.theme), max_cols)
    {
        let style = Style::default().fg(placeholder::id_color(id));
        return (0..rows)
            .map(|row| Line::from(Span::styled(placeholder::row_text(row, cols), style)))
            .collect();
    }
    let source: Vec<String> = latex
        .trim_matches('\n')
        .lines()
        .map(|line| expand_tabs(line.trim_end()))
        .collect();
    trim_common_leading_spaces(&source)
        .into_iter()
        .map(|line| Line::from(Span::styled(line, fallback_style)))
        .collect()
}

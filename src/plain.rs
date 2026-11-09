//! Plain (non-pager) output: rendered lines as ANSI-styled text.

use crate::util::ansi_style_prefix;
use crate::util::char_widths;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Append document text, dropping control characters (ESC, BEL, ...) so a
/// markdown file cannot inject terminal escape sequences in plain mode
/// (e.g. retitling the terminal or writing the clipboard via OSC 52).
/// Matches what ratatui does for the pager. Tabs are kept.
fn push_without_control_chars(out: &mut String, text: &str) {
    out.extend(text.chars().filter(|c| *c == '\t' || !c.is_control()));
}

fn line_to_ansi(line: &Line<'_>, default_style: Style) -> String {
    let mut out = String::new();
    for sp in &line.spans {
        let style = default_style.patch(sp.style);
        let prefix = ansi_style_prefix(style);
        if prefix.is_empty() {
            push_without_control_chars(&mut out, sp.content.as_ref());
        } else {
            out.push_str(prefix.as_str());
            push_without_control_chars(&mut out, sp.content.as_ref());
            out.push_str("\x1b[0m");
        }
    }
    out
}

pub(crate) fn line_to_ansi_with_links(
    render_line: &crate::renderer::RenderLine,
    default_style: Style,
    link_style: Style,
) -> String {
    // Table rows are laid out to exact column widths; an appended " (url)"
    // would break the borders, so table links keep only their text.
    if render_line.link_ranges.is_empty() || render_line.kind == crate::renderer::LineKind::Table {
        return line_to_ansi(&render_line.line, default_style);
    }

    let mut inserts: Vec<(usize, String)> = render_line
        .link_ranges
        .iter()
        .filter(|lr| lr.kind == crate::renderer::RangeKind::Url)
        .filter_map(|lr| {
            let shown = text_for_display_range(&render_line.line, lr.start, lr.end);
            if should_append_link_suffix(shown.as_str(), lr.url.as_str()) {
                Some((lr.end, format!(" ({})", lr.url)))
            } else {
                None
            }
        })
        .collect();
    inserts.sort_by_key(|(col, _)| *col);

    let mut out_spans: Vec<Span<'static>> = Vec::new();
    let mut insert_idx = 0usize;
    let mut col = 0usize;

    for sp in &render_line.line.spans {
        let style = sp.style;
        let mut buf = String::new();
        for (ch, ch_width) in char_widths(&sp.content) {
            while insert_idx < inserts.len() && inserts[insert_idx].0 == col {
                if !buf.is_empty() {
                    out_spans.push(Span::styled(std::mem::take(&mut buf), style));
                }
                out_spans.push(Span::styled(inserts[insert_idx].1.clone(), link_style));
                insert_idx += 1;
            }
            buf.push(ch);
            col += ch_width;
        }
        if !buf.is_empty() {
            out_spans.push(Span::styled(buf, style));
        }
    }

    while insert_idx < inserts.len() {
        out_spans.push(Span::styled(inserts[insert_idx].1.clone(), link_style));
        insert_idx += 1;
    }

    line_to_ansi(&Line::from(out_spans), default_style)
}

fn text_for_display_range(line: &Line<'_>, start: usize, end: usize) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    let mut in_range = false;
    for sp in &line.spans {
        for (ch, w) in char_widths(&sp.content) {
            // Zero-width chars belong to the grapheme before them.
            if w > 0 {
                in_range = col < end && col + w > start;
            }
            if in_range {
                out.push(ch);
            }
            col += w;
        }
    }
    out
}

fn should_append_link_suffix(display_text: &str, url: &str) -> bool {
    let d = display_text.trim();
    let u = url.trim();
    if d.eq_ignore_ascii_case(u) {
        return false;
    }
    if d.len() >= 2 && d.starts_with('<') && d.ends_with('>') {
        let inner = d[1..d.len() - 1].trim();
        if inner.eq_ignore_ascii_case(u) {
            return false;
        }
    }
    true
}

/// Math images for plain output on a kitty-graphics terminal: `None` when
/// stdout is not one (math is then printed as LaTeX source).
pub(crate) fn math_images_for_stdout(
    graphics: bool,
    font: crate::config::MathFont,
) -> Option<std::rc::Rc<std::cell::RefCell<crate::tex::MathImages>>> {
    use std::io::IsTerminal;
    if !graphics || !std::io::stdout().is_terminal() {
        return None;
    }
    crate::tex::MathImages::for_terminal(font)
        .map(|store| std::rc::Rc::new(std::cell::RefCell::new(store)))
}

/// Transmit every math image of the document, before its placeholder text
/// is printed. Plain output stays in scrollback, so images are not freed.
pub(crate) fn transmit_math_images(
    out: &mut impl std::io::Write,
    store: &mut crate::tex::MathImages,
) -> std::io::Result<()> {
    for math in store.iter_mut() {
        if !math.uploaded {
            let image = &math.image;
            crate::placeholder::transmit_virtual(out, math.id, &image.png, image.cols, image.rows)?;
            math.uploaded = true;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Modifier;

    #[test]
    fn plain_math_is_transmitted_then_printed_as_placeholders() {
        let store = std::rc::Rc::new(std::cell::RefCell::new(crate::tex::MathImages::new(
            (10, 20),
            crate::config::MathFont::Fira,
        )));
        let settings = crate::renderer::RenderSettings {
            math_images: Some(store.clone()),
            ..Default::default()
        };
        let doc = crate::renderer::render_markdown("a $x^2$ b\n\n$$\\frac{a}{b}$$\n", &settings)
            .expect("render");
        let mut out = Vec::new();
        transmit_math_images(&mut out, &mut store.borrow_mut()).unwrap();
        let transmitted = String::from_utf8(out).unwrap();
        assert_eq!(transmitted.matches("\x1b_Ga=T,U=1,").count(), 2);

        let printed: String = doc
            .lines
            .iter()
            .map(|l| line_to_ansi_with_links(l, Style::default(), Style::default()))
            .collect();
        assert!(printed.contains(crate::placeholder::PLACEHOLDER));
        assert!(printed.contains("\x1b[38;2;"));

        let mut again = Vec::new();
        transmit_math_images(&mut again, &mut store.borrow_mut()).unwrap();
        assert!(again.is_empty(), "each image is sent once");
    }

    #[test]
    fn ansi_render_strips_control_characters_from_content() {
        let line = Line::from(vec![Span::raw("a\x1b]0;PWNED\x07b\tc")]);
        let out = line_to_ansi(&line, Style::default());
        assert!(!out.contains('\x07'));
        assert!(!out.contains("\x1b]"));
        assert!(out.contains("a]0;PWNEDb\tc"));
    }

    #[test]
    fn ansi_render_includes_bold_escape() {
        let line = Line::from(vec![Span::styled(
            "x",
            Style::default().add_modifier(Modifier::BOLD),
        )]);
        let out = line_to_ansi(&line, Style::default());
        assert!(out.contains("\x1b[1m"));
        assert!(out.ends_with("\x1b[0m"));
    }

    #[test]
    fn autolink_suffix_is_not_duplicated() {
        assert!(!should_append_link_suffix(
            "https://example.com",
            "https://example.com"
        ));
        assert!(!should_append_link_suffix(
            "<https://example.com>",
            "https://example.com"
        ));
        assert!(should_append_link_suffix("Example", "https://example.com"));
    }

    /// Writes what `--plain` sends to a kitty terminal with 10x20 px cells,
    /// for an offline emulator: `CALCI_PLAIN_OUT=<file> CALCI_PLAIN_IN=<md>
    /// cargo test --release plain_dump -- --ignored`
    #[test]
    #[ignore]
    fn plain_dump() {
        let out_path = std::env::var("CALCI_PLAIN_OUT").expect("CALCI_PLAIN_OUT");
        let markdown =
            std::fs::read_to_string(std::env::var("CALCI_PLAIN_IN").expect("CALCI_PLAIN_IN"))
                .unwrap();
        // CALCI_FONT: a `math_font` config value; CALCI_CELL: "WxH" pixels.
        let font: crate::config::MathFont = std::env::var("CALCI_FONT")
            .map(|f| {
                toml::from_str::<std::collections::HashMap<String, crate::config::MathFont>>(
                    &format!("f = \"{f}\""),
                )
                .unwrap()["f"]
            })
            .unwrap_or_default();
        let cell = std::env::var("CALCI_CELL")
            .ok()
            .and_then(|c| {
                c.split_once('x')
                    .map(|(w, h)| (w.parse().unwrap(), h.parse().unwrap()))
            })
            .unwrap_or((10, 20));
        let store = std::rc::Rc::new(std::cell::RefCell::new(crate::tex::MathImages::new(
            cell, font,
        )));
        let settings = crate::renderer::RenderSettings {
            width: 70,
            math_images: Some(store.clone()),
            ..Default::default()
        };
        let pre = crate::renderer::preprocess_math(&markdown);
        let doc = crate::renderer::render_markdown(&pre, &settings).expect("render");
        let mut out = Vec::new();
        transmit_math_images(&mut out, &mut store.borrow_mut()).unwrap();
        for l in &doc.lines {
            out.extend(line_to_ansi_with_links(l, Style::default(), Style::default()).into_bytes());
            out.push(b'\n');
        }
        std::fs::write(out_path, out).unwrap();
    }
}

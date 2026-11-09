use anyhow::Context;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;
use std::str::FromStr;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSettings;
use syntect::highlighting::{
    Color as SynColor, FontStyle as SynFontStyle, ScopeSelectors,
    StyleModifier as SynStyleModifier, Theme, ThemeItem,
};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use unicode_width::UnicodeWidthStr;

mod code;
mod front_matter;
mod math;
mod table;

use code::*;
pub use front_matter::FrontMatter;
use front_matter::*;
pub use math::preprocess_math;
use math::*;
use table::*;

use crate::config::TableRowSeparator;
use crate::placeholder;
use crate::tex::MathImages;
use crate::theme::AppTheme;
use std::cell::RefCell;
use std::rc::Rc;

const LIST_PAD_X: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Normal,
    Heading,
    Code,
    Ascii,
    Math,
    Image,
    Table,
    Quote,
}

#[derive(Clone, Debug)]
pub struct RenderLine {
    pub line: Line<'static>,
    pub kind: LineKind,
    pub link_ranges: Vec<LinkRange>,
    pub code_block_index: Option<usize>,
    pub image_index: Option<usize>,
    pub heading_level: Option<u8>,
    /// Display columns of the list marker prefix (indent + bullet/number +
    /// checkbox); wrapped continuation rows are indented by this much.
    pub hang_indent: usize,
    /// Set on the first line of a task list item (`- [ ]` / `- [x]`).
    pub task: Option<TaskMarker>,
}

/// A rendered task-list checkbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskMarker {
    /// Position of this task among all task items of the document, in source
    /// order; used to find the marker again in the source file.
    pub ordinal: usize,
    pub checked: bool,
    /// Display columns of the checkbox glyph within the line.
    pub start: usize,
    pub end: usize,
}

/// Metadata for the line currently being assembled.
#[derive(Default)]
struct PendingLineMeta {
    hang: usize,
    task: Option<TaskMarker>,
}

const HEADING_H3_GLYPH: &str = "▸ ";
const LIST_BULLETS: [&str; 3] = ["•", "–", "·"];
const TASK_UNCHECKED: &str = "\u{f0131}"; // nf-md-checkbox_blank_outline
const TASK_CHECKED: &str = "\u{f0135}"; // nf-md-checkbox_marked

#[derive(Clone, Debug)]
pub struct LinkRange {
    pub start: usize,
    pub end: usize,
    /// URL to open, or the text to copy for [`RangeKind::Copy`].
    pub url: String,
    pub kind: RangeKind,
}

/// What clicking a [`LinkRange`] does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RangeKind {
    /// Open `url` (links, remote images).
    #[default]
    Url,
    /// Copy `url` (the text) to the clipboard (inline code).
    Copy,
}

/// Text as it will be drawn: tabs become spaces and other control characters
/// are dropped. ratatui drops control characters while drawing, so keeping
/// them would make the layout's widths disagree with the screen.
fn displayable(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(
        text.chars()
            .filter_map(|c| match c {
                '\t' => Some(' '),
                c if c.is_control() => None,
                c => Some(c),
            })
            .collect(),
    )
}

/// Expand tabs to the next multiple of `CODE_TAB_WIDTH` columns (per line)
/// and drop other control characters, for drawing code blocks.
fn expand_tabs(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    for (i, line) in code.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut col = 0usize;
        for (ch, width) in crate::util::char_widths(line) {
            match ch {
                '\t' => {
                    let spaces = CODE_TAB_WIDTH - col % CODE_TAB_WIDTH;
                    out.extend(std::iter::repeat_n(' ', spaces));
                    col += spaces;
                }
                c if c.is_control() => {}
                c => {
                    out.push(c);
                    col += width;
                }
            }
        }
    }
    out
}

const CODE_TAB_WIDTH: usize = 4;

#[derive(Clone, Debug, Default)]
pub struct RenderDoc {
    pub front_matter: Option<FrontMatter>,
    pub lines: Vec<RenderLine>,
    pub code_blocks: Vec<String>,
    pub images: Vec<RenderImage>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderImage {
    pub source: String,
    pub alt: String,
    pub title: Option<String>,
    pub width_percent: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct RenderSettings {
    /// Columns available to content (tables are fitted into this width).
    pub width: u16,
    pub theme: AppTheme,
    pub table_row_separator: TableRowSeparator,
    /// Without a front-matter title, use a leading `# H1` as the document
    /// title (removed from the body). The pager shows it in the sticky bar.
    pub promote_h1_title: bool,
    /// Draw math as kitty images (Unicode placeholders) from this store;
    /// without one, math shows as LaTeX source.
    pub math_images: Option<Rc<RefCell<MathImages>>>,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            width: 100,
            theme: AppTheme::default(),
            table_row_separator: TableRowSeparator::Never,
            promote_h1_title: false,
            math_images: None,
        }
    }
}

/// Quote text uses the theme's `quote` color (`[pager] quote` in color.toml).
fn quote_text_style(theme: &AppTheme, active_mods: Modifier) -> Style {
    theme
        .normal
        .patch(theme.quote)
        .add_modifier(Modifier::ITALIC)
        .add_modifier(active_mods)
}

#[derive(Clone, Copy, Debug)]
struct SmartQuoteState {
    next_double_open: bool,
    next_single_open: bool,
    prev_char: Option<char>,
}

impl Default for SmartQuoteState {
    fn default() -> Self {
        Self {
            next_double_open: true,
            next_single_open: true,
            prev_char: None,
        }
    }
}

fn smart_quotes_text(text: &str, state: &mut SmartQuoteState) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());

    for (idx, ch) in chars.iter().copied().enumerate() {
        let next = chars.get(idx + 1).copied();
        let mapped = match ch {
            '"' => {
                let emit_open = choose_open_quote(state.prev_char, next, state.next_double_open);
                state.next_double_open = !emit_open;
                if emit_open { '“' } else { '”' }
            }
            '\'' => {
                if is_apostrophe_like(state.prev_char, next) {
                    '’'
                } else {
                    let emit_open =
                        choose_open_quote(state.prev_char, next, state.next_single_open);
                    state.next_single_open = !emit_open;
                    if emit_open { '‘' } else { '’' }
                }
            }
            other => other,
        };
        out.push(mapped);
        state.prev_char = Some(mapped);
    }

    out
}

fn choose_open_quote(prev: Option<char>, next: Option<char>, fallback_open: bool) -> bool {
    let open_hint = match prev {
        None => true,
        Some(c) => is_open_quote_context(c),
    };
    let close_hint = match next {
        None => true,
        Some(c) => is_close_quote_context(c),
    };

    if open_hint && !close_hint {
        true
    } else if close_hint && !open_hint {
        false
    } else {
        fallback_open
    }
}

fn is_open_quote_context(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | '[' | '{' | '<' | '/' | '-' | '—' | '–')
}

fn is_close_quote_context(c: char) -> bool {
    c.is_whitespace() || matches!(c, ')' | ']' | '}' | '>' | '.' | ',' | ';' | ':' | '!' | '?')
}

fn is_apostrophe_like(prev: Option<char>, next: Option<char>) -> bool {
    matches!((prev, next), (Some(p), Some(n)) if p.is_alphanumeric() && n.is_alphanumeric())
        || matches!(
            (prev, next),
            (None | Some(' ' | '\t' | '\n' | '\r' | '(' | '[' | '{'), Some(n)) if n.is_ascii_digit()
        )
}

fn parser_options() -> Options {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_MATH);
    options
}

/// Flip the `ordinal`-th task marker (`[ ]` <-> `[x]`) in the raw markdown
/// source and return the updated text.
///
/// The rendered view comes from preprocessed markdown, so the marker is
/// located again by parsing the original source. To avoid editing the wrong
/// line, this refuses unless the source has exactly `expected_total` task
/// markers and the target's state matches `expected_checked`.
pub fn toggle_task_marker(
    source: &str,
    ordinal: usize,
    expected_checked: bool,
    expected_total: usize,
) -> anyhow::Result<String> {
    let (_, body) = extract_front_matter(source);
    let offset = source.len() - body.len();
    let markers: Vec<(std::ops::Range<usize>, bool)> = Parser::new_ext(&body, parser_options())
        .into_offset_iter()
        .filter_map(|(event, range)| match event {
            Event::TaskListMarker(checked) => Some((range, checked)),
            _ => None,
        })
        .collect();
    let mismatch =
        || anyhow::anyhow!("task list in the file doesn't match the view; press r to reload");
    if markers.len() != expected_total {
        return Err(mismatch());
    }
    let (range, checked) = markers.get(ordinal).cloned().ok_or_else(mismatch)?;
    if checked != expected_checked {
        return Err(mismatch());
    }
    let start = offset + range.start;
    let marker = source.get(start..offset + range.end).ok_or_else(mismatch)?;
    let open = marker.find('[').ok_or_else(mismatch)?;
    let inner_start = start + open + 1;
    let inner = source[inner_start..].chars().next().ok_or_else(mismatch)?;
    if !matches!(inner, ' ' | 'x' | 'X') {
        return Err(mismatch());
    }
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..inner_start]);
    out.push(if expected_checked { ' ' } else { 'x' });
    out.push_str(&source[inner_start + inner.len_utf8()..]);
    Ok(out)
}

pub fn render_markdown(
    markdown: &str,
    settings: &RenderSettings,
) -> Result<RenderDoc, anyhow::Error> {
    let (front_matter, markdown_body) = extract_front_matter(markdown);
    let markdown = preprocess_block_title_attributes(&markdown_body);
    let mut doc = RenderDoc {
        front_matter,
        ..RenderDoc::default()
    };
    let mut parser = Parser::new_ext(&markdown, parser_options())
        .into_offset_iter()
        .peekable();
    let syntax_set = syntax_set();
    let syntax_theme = exact_semantic_theme(&settings.theme);

    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut heading_level: Option<HeadingLevel> = None;
    let mut quote_depth = 0usize;
    let mut pending_link: Option<String> = None;
    let mut pending_image: Option<PendingImage> = None;
    let mut list_stack: Vec<ListState> = Vec::new();
    let mut table: Option<TableState> = None;
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut current_line_links: Vec<LinkRange> = Vec::new();
    let mut active_mods = Modifier::empty();
    let mut smart_quote_state = SmartQuoteState::default();
    let mut pending_paragraph_end: Option<usize> = None;
    let mut pending_meta = PendingLineMeta::default();
    let mut task_count = 0usize;
    // Text indent of each open list item; continuation lines start there.
    let mut item_indents: Vec<usize> = Vec::new();
    let mut item_marker_span: Option<usize> = None;

    while let Some((event, event_range)) = parser.next() {
        if let Some(paragraph_end) = pending_paragraph_end.take() {
            let blank_rows = if matches!(event, Event::Start(Tag::List(_))) && list_stack.is_empty()
            {
                blank_rows_between_offsets(&markdown, paragraph_end, event_range.start)
            } else {
                1
            };
            // A paragraph break inside a blockquote keeps the quote bar; the
            // break after the quote's last paragraph does not.
            let blank_kind =
                if quote_depth > 0 && !matches!(event, Event::End(TagEnd::BlockQuote(_))) {
                    LineKind::Quote
                } else {
                    LineKind::Normal
                };
            for _ in 0..blank_rows {
                doc.lines.push(RenderLine {
                    line: Line::from(""),
                    kind: blank_kind,
                    link_ranges: vec![],
                    code_block_index: None,
                    image_index: None,
                    heading_level: None,
                    hang_indent: 0,
                    task: None,
                });
            }
        }
        if let Some(tbl) = table.as_mut() {
            match &event {
                Event::Start(Tag::TableHead) => continue,
                Event::Start(Tag::TableRow) => {
                    tbl.current_row.clear();
                    continue;
                }
                Event::End(TagEnd::TableHead) | Event::End(TagEnd::TableRow) => {
                    if !tbl.current_cell.is_empty() {
                        let cell = std::mem::take(&mut tbl.current_cell);
                        tbl.current_row.push(cell);
                    }
                    if !tbl.current_row.is_empty() {
                        let row = std::mem::take(&mut tbl.current_row);
                        tbl.rows.push(row);
                    }
                    if matches!(event, Event::End(TagEnd::TableHead)) {
                        tbl.header_rows = tbl.rows.len();
                    }
                    continue;
                }
                Event::Start(Tag::TableCell) => {
                    tbl.current_cell = TableCell::default();
                    continue;
                }
                Event::End(TagEnd::TableCell) => {
                    let cell = std::mem::take(&mut tbl.current_cell);
                    tbl.current_row.push(cell);
                    continue;
                }
                Event::Text(text) => {
                    let display = smart_quotes_text(&displayable(text), &mut smart_quote_state);
                    if let Some(url) = pending_link.as_deref() {
                        let style = settings.theme.link.add_modifier(active_mods);
                        tbl.current_cell
                            .push_range(display, style, url, RangeKind::Url);
                    } else {
                        let style = settings.theme.normal.add_modifier(active_mods);
                        tbl.current_cell.push(display, style);
                    }
                    continue;
                }
                Event::Code(code) => {
                    let shown = displayable(code).into_owned();
                    if let Some(url) = pending_link.as_deref() {
                        let style = settings.theme.link.add_modifier(active_mods);
                        tbl.current_cell
                            .push_range(shown, style, url, RangeKind::Url);
                    } else {
                        let style = settings.theme.inline_code.add_modifier(active_mods);
                        tbl.current_cell
                            .push_range(shown, style, code, RangeKind::Copy);
                    }
                    continue;
                }
                Event::InlineMath(m) => {
                    let span = inline_math_span(settings, m);
                    tbl.current_cell.push(span.content.into_owned(), span.style);
                    continue;
                }
                Event::SoftBreak | Event::HardBreak => {
                    tbl.current_cell
                        .push(" ".to_string(), settings.theme.normal);
                    continue;
                }
                Event::End(TagEnd::Table) => {
                    doc.lines.extend(render_table(tbl, settings));
                    doc.lines.push(RenderLine {
                        line: Line::from(""),
                        kind: LineKind::Normal,
                        link_ranges: vec![],
                        code_block_index: None,
                        image_index: None,
                        heading_level: None,
                        hang_indent: 0,
                        task: None,
                    });
                    table = None;
                    continue;
                }
                // Inline formatting and links inside cells: fall through so the
                // shared handlers below update `active_mods` / `pending_link`.
                Event::Start(
                    Tag::Strong | Tag::Emphasis | Tag::Strikethrough | Tag::Link { .. },
                )
                | Event::End(
                    TagEnd::Strong | TagEnd::Emphasis | TagEnd::Strikethrough | TagEnd::Link,
                ) => {}
                _ => continue,
            }
        }

        match event {
            Event::Start(tag) => match tag {
                Tag::Table(aligns) => {
                    push_current_line(
                        &mut doc,
                        &mut current_line,
                        &mut current_line_links,
                        &mut pending_meta,
                        current_line_kind(quote_depth),
                    );
                    table = Some(TableState::new(aligns.to_vec()));
                }
                Tag::CodeBlock(kind) => {
                    in_code = true;
                    code_buf.clear();
                    code_lang.clear();
                    match kind {
                        CodeBlockKind::Fenced(lang) => code_lang = lang.to_string(),
                        CodeBlockKind::Indented => {}
                    }
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
                            &mut pending_meta,
                            current_line_kind(quote_depth),
                        );
                    }
                }
                Tag::Heading { level, .. } => {
                    heading_level = Some(level);
                }
                Tag::BlockQuote(_) => {
                    quote_depth += 1;
                }
                Tag::List(start) => {
                    let next = start.unwrap_or(1);
                    let ordered = start.is_some();
                    list_stack.push(ListState { ordered, next });
                }
                Tag::Item => {
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
                            &mut pending_meta,
                            current_line_kind(quote_depth),
                        );
                    }
                    let indent = format!(
                        "{}{}",
                        " ".repeat(LIST_PAD_X),
                        "  ".repeat(list_stack.len().saturating_sub(1))
                    );
                    let depth = list_stack.len().saturating_sub(1);
                    let marker = match list_stack.last_mut() {
                        Some(last) if last.ordered => {
                            let m = format!("{}. ", last.next);
                            last.next += 1;
                            m
                        }
                        _ => format!("{} ", LIST_BULLETS[depth % LIST_BULLETS.len()]),
                    };
                    pending_meta.hang = UnicodeWidthStr::width(indent.as_str())
                        + UnicodeWidthStr::width(marker.as_str());
                    item_indents.push(pending_meta.hang);
                    current_line.push(Span::raw(indent));
                    item_marker_span = Some(current_line.len());
                    current_line.push(Span::styled(marker, settings.theme.list_marker));
                }
                Tag::Link { dest_url, .. } => {
                    pending_link = Some(dest_url.to_string());
                }
                Tag::Image {
                    dest_url, title, ..
                } => {
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
                            &mut pending_meta,
                            current_line_kind(quote_depth),
                        );
                    }
                    pending_image = Some(PendingImage {
                        source: dest_url.to_string(),
                        title: if title.trim().is_empty() {
                            None
                        } else {
                            Some(title.to_string())
                        },
                        alt: String::new(),
                    });
                }
                Tag::Strong => {
                    active_mods |= Modifier::BOLD;
                }
                Tag::Emphasis => {
                    active_mods |= Modifier::ITALIC;
                }
                Tag::Strikethrough => {
                    active_mods |= Modifier::CROSSED_OUT;
                }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::CodeBlock => {
                    in_code = false;
                    let (code_title, code_payload) = split_block_title_marker(&code_buf);
                    let block_index = doc.code_blocks.len();
                    doc.code_blocks.push(code_payload.clone());
                    let block_style = settings
                        .theme
                        .normal
                        .patch(settings.theme.code)
                        .bg(Color::Reset);
                    let math_block_style = math_span_style(&settings.theme, block_style);
                    let is_math_block = code_lang == "math" || code_lang == "latex";
                    let lang_trimmed = code_lang.trim();
                    let is_ascii_block = matches!(lang_trimmed, "text" | "txt" | "plain");
                    let rendered_code_payload = expand_tabs(&code_payload);
                    let code_lines = if is_math_block {
                        display_math_lines(settings, &code_payload, math_block_style)
                    } else if code_lang.trim().is_empty() {
                        plain_code_lines(&rendered_code_payload, block_style)
                    } else {
                        highlight_code_block(
                            &rendered_code_payload,
                            &code_lang,
                            syntax_set,
                            &syntax_theme,
                            block_style,
                        )
                    };
                    let title_style = block_style
                        .patch(settings.theme.line_number)
                        .add_modifier(Modifier::ITALIC);
                    let padded_code_lines = pad_block_lines(
                        code_lines,
                        code_title.as_deref(),
                        block_style,
                        title_style,
                    );
                    for rendered in padded_code_lines {
                        doc.lines.push(RenderLine {
                            line: rendered.line,
                            kind: if is_math_block {
                                LineKind::Math
                            } else if is_ascii_block {
                                LineKind::Ascii
                            } else {
                                LineKind::Code
                            },
                            link_ranges: vec![],
                            code_block_index: if rendered.is_code_content {
                                Some(block_index)
                            } else {
                                None
                            },
                            image_index: None,
                            heading_level: None,
                            hang_indent: 0,
                            task: None,
                        });
                    }
                    code_buf.clear();
                    code_lang.clear();
                    doc.lines.push(RenderLine {
                        line: Line::from(""),
                        kind: LineKind::Normal,
                        link_ranges: vec![],
                        code_block_index: None,
                        image_index: None,
                        heading_level: None,
                        hang_indent: 0,
                        task: None,
                    });
                }
                TagEnd::Heading(_) => {
                    let level = heading_to_u8(heading_level);
                    let promote = settings.promote_h1_title
                        && level == Some(1)
                        && !doc.front_matter.as_ref().is_some_and(|fm| {
                            fm.title.as_deref().is_some_and(|t| !t.trim().is_empty())
                        })
                        && doc.lines.iter().all(is_blank_render_line);
                    if promote {
                        let title = current_line
                            .drain(..)
                            .map(|s| s.content.into_owned())
                            .collect::<String>();
                        current_line_links.clear();
                        doc.front_matter
                            .get_or_insert_with(FrontMatter::default)
                            .title = Some(title.trim().to_string());
                        doc.lines.clear();
                        heading_level = None;
                        continue;
                    }
                    if !current_line.is_empty() {
                        let mut line = Line::from(std::mem::take(&mut current_line));
                        if level == Some(3) {
                            line.spans.insert(0, Span::raw(HEADING_H3_GLYPH));
                        }
                        line.spans
                            .iter_mut()
                            .for_each(|s| s.style = settings.theme.heading);
                        doc.lines.push(RenderLine {
                            line,
                            kind: LineKind::Heading,
                            link_ranges: std::mem::take(&mut current_line_links),
                            code_block_index: None,
                            image_index: None,
                            heading_level: level,
                            hang_indent: 0,
                            task: None,
                        });
                        if matches!(level, Some(1 | 2)) {
                            let rule_style = if level == Some(1) {
                                settings.theme.heading_h1
                            } else {
                                settings.theme.heading_h2
                            }
                            .remove_modifier(Modifier::BOLD);
                            doc.lines.push(RenderLine {
                                line: Line::from(Span::styled(
                                    "─".repeat(settings.width as usize),
                                    rule_style,
                                )),
                                kind: LineKind::Normal,
                                link_ranges: vec![],
                                code_block_index: None,
                                image_index: None,
                                heading_level: None,
                                hang_indent: 0,
                                task: None,
                            });
                        }
                    }
                    heading_level = None;
                    doc.lines.push(RenderLine {
                        line: Line::from(""),
                        kind: LineKind::Normal,
                        link_ranges: vec![],
                        code_block_index: None,
                        image_index: None,
                        heading_level: None,
                        hang_indent: 0,
                        task: None,
                    });
                }
                TagEnd::BlockQuote(_) => {
                    quote_depth = quote_depth.saturating_sub(1);
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
                            &mut pending_meta,
                            current_line_kind(quote_depth),
                        );
                    }
                }
                TagEnd::Paragraph => {
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
                            &mut pending_meta,
                            current_line_kind(quote_depth),
                        );
                    }
                    if list_stack.is_empty() {
                        pending_paragraph_end = Some(event_range.end);
                    }
                }
                TagEnd::Link => {
                    pending_link = None;
                }
                TagEnd::Image => {
                    if let Some(image) = pending_image.take() {
                        let width_percent = consume_trailing_image_width_attr(&mut parser);
                        let image_index = doc.images.len();
                        let display_alt = image.alt.trim().to_string();
                        doc.images.push(RenderImage {
                            source: image.source.clone(),
                            alt: display_alt.clone(),
                            title: image.title.clone(),
                            width_percent,
                        });
                        let label = if display_alt.is_empty() {
                            format!("[image] {}", image.source)
                        } else {
                            format!("[image] {display_alt}")
                        };
                        let style = settings.theme.line_number.add_modifier(Modifier::ITALIC);
                        let is_remote = image.source.starts_with("http://")
                            || image.source.starts_with("https://");
                        let link_ranges = if is_remote {
                            vec![LinkRange {
                                start: 0,
                                end: UnicodeWidthStr::width(label.as_str()),
                                url: image.source,
                                kind: RangeKind::Url,
                            }]
                        } else {
                            vec![]
                        };
                        doc.lines.push(RenderLine {
                            line: Line::from(vec![Span::styled(label, style)]),
                            kind: LineKind::Image,
                            link_ranges,
                            code_block_index: None,
                            image_index: Some(image_index),
                            heading_level: None,
                            hang_indent: 0,
                            task: None,
                        });
                    }
                }
                TagEnd::List(_) => {
                    let _ = list_stack.pop();
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
                            &mut pending_meta,
                            current_line_kind(quote_depth),
                        );
                    }
                    if list_stack.is_empty() && !doc.lines.last().is_some_and(is_blank_render_line)
                    {
                        doc.lines.push(RenderLine {
                            line: Line::from(""),
                            kind: LineKind::Normal,
                            link_ranges: vec![],
                            code_block_index: None,
                            image_index: None,
                            heading_level: None,
                            hang_indent: 0,
                            task: None,
                        });
                    }
                }
                TagEnd::Item => {
                    item_indents.pop();
                    item_marker_span = None;
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
                            &mut pending_meta,
                            current_line_kind(quote_depth),
                        );
                    }
                }
                TagEnd::Strong => {
                    active_mods.remove(Modifier::BOLD);
                }
                TagEnd::Emphasis => {
                    active_mods.remove(Modifier::ITALIC);
                }
                TagEnd::Strikethrough => {
                    active_mods.remove(Modifier::CROSSED_OUT);
                }
                _ => {}
            },
            Event::Code(code) => {
                if let Some(image) = pending_image.as_mut() {
                    image.alt.push_str(&code);
                    continue;
                }
                indent_list_continuation(&mut current_line, &mut pending_meta, &item_indents);
                let shown = displayable(&code).into_owned();
                let start = line_width(&current_line);
                let width = UnicodeWidthStr::width(shown.as_str());
                // Inline code copies its text when clicked; inside a link it
                // is part of the link instead.
                let (url, kind) = match pending_link.clone() {
                    Some(url) => (url, RangeKind::Url),
                    None => (code.to_string(), RangeKind::Copy),
                };
                if width > 0 {
                    current_line_links.push(LinkRange {
                        start,
                        end: start + width,
                        url,
                        kind,
                    });
                }
                let style = if kind == RangeKind::Url {
                    settings.theme.link.add_modifier(active_mods)
                } else {
                    settings.theme.inline_code.add_modifier(active_mods)
                };
                current_line.push(Span::styled(shown, style));
            }
            Event::TaskListMarker(checked) => {
                let glyph = if checked {
                    TASK_CHECKED
                } else {
                    TASK_UNCHECKED
                };
                // The checkbox replaces the item's bullet (ordered items keep
                // their number and get the box after it).
                let replace_bullet = item_marker_span
                    .filter(|&i| i < current_line.len())
                    .filter(|_| !list_stack.last().is_some_and(|l| l.ordered));
                let start = match replace_bullet {
                    Some(i) => {
                        let start = line_width(&current_line[..i]);
                        current_line[i] =
                            Span::styled(format!("{glyph} "), settings.theme.list_marker);
                        start
                    }
                    None => {
                        let start = line_width(&current_line);
                        current_line.push(Span::styled(
                            format!("{glyph} "),
                            settings.theme.list_marker,
                        ));
                        start
                    }
                };
                pending_meta.hang = line_width(&current_line);
                if let Some(indent) = item_indents.last_mut() {
                    *indent = pending_meta.hang;
                }
                pending_meta.task = Some(TaskMarker {
                    ordinal: task_count,
                    checked,
                    start,
                    end: start + UnicodeWidthStr::width(glyph),
                });
                task_count += 1;
            }
            Event::Text(text) => {
                if let Some(image) = pending_image.as_mut() {
                    image.alt.push_str(&text);
                    continue;
                }
                if in_code {
                    code_buf.push_str(&text);
                    continue;
                }
                indent_list_continuation(&mut current_line, &mut pending_meta, &item_indents);
                let display_text = smart_quotes_text(&displayable(&text), &mut smart_quote_state);
                let style = if quote_depth > 0 {
                    quote_text_style(&settings.theme, active_mods)
                } else {
                    settings.theme.normal.add_modifier(active_mods)
                };
                if let Some(url) = pending_link.clone() {
                    let link_text = display_text;
                    let start = current_line
                        .iter()
                        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                        .sum::<usize>();
                    let link_width = UnicodeWidthStr::width(link_text.as_str());
                    let mut link_style = settings.theme.link.add_modifier(active_mods);
                    if quote_depth > 0 {
                        link_style = link_style.add_modifier(Modifier::ITALIC);
                    }
                    current_line.push(Span::styled(link_text.clone(), link_style));
                    if link_width > 0 {
                        current_line_links.push(LinkRange {
                            start,
                            end: start + link_width,
                            url,
                            kind: RangeKind::Url,
                        });
                    }
                } else {
                    current_line.push(Span::styled(display_text, style));
                }
            }
            Event::SoftBreak => {
                if let Some(image) = pending_image.as_mut() {
                    image.alt.push(' ');
                    continue;
                }
                if current_line.is_empty()
                    && matches!(
                        doc.lines.last().map(|l| l.kind),
                        Some(LineKind::Math | LineKind::Code | LineKind::Ascii)
                    )
                {
                    continue;
                }
                push_current_line(
                    &mut doc,
                    &mut current_line,
                    &mut current_line_links,
                    &mut pending_meta,
                    current_line_kind(quote_depth),
                );
            }
            Event::HardBreak => {
                if let Some(image) = pending_image.as_mut() {
                    image.alt.push(' ');
                    continue;
                }
                if current_line.is_empty()
                    && matches!(
                        doc.lines.last().map(|l| l.kind),
                        Some(LineKind::Math | LineKind::Code | LineKind::Ascii)
                    )
                {
                    continue;
                }
                push_current_line(
                    &mut doc,
                    &mut current_line,
                    &mut current_line_links,
                    &mut pending_meta,
                    current_line_kind(quote_depth),
                );
            }
            Event::Rule => {
                if !current_line.is_empty() {
                    push_current_line(
                        &mut doc,
                        &mut current_line,
                        &mut current_line_links,
                        &mut pending_meta,
                        current_line_kind(quote_depth),
                    );
                }
                doc.lines.push(RenderLine {
                    line: Line::from("─".repeat(settings.width as usize)),
                    kind: LineKind::Normal,
                    link_ranges: vec![],
                    code_block_index: None,
                    image_index: None,
                    heading_level: None,
                    hang_indent: 0,
                    task: None,
                });
            }
            Event::InlineMath(text) => {
                if let Some(image) = pending_image.as_mut() {
                    image.alt.push_str(&format!("${}$", text.trim()));
                    continue;
                }
                indent_list_continuation(&mut current_line, &mut pending_meta, &item_indents);
                current_line.push(inline_math_span(settings, &text));
            }
            Event::DisplayMath(text) => {
                if !current_line.is_empty() {
                    push_current_line(
                        &mut doc,
                        &mut current_line,
                        &mut current_line_links,
                        &mut pending_meta,
                        current_line_kind(quote_depth),
                    );
                }
                let block_style = settings
                    .theme
                    .normal
                    .patch(settings.theme.code)
                    .bg(Color::Reset);
                let math_style = math_span_style(&settings.theme, block_style);
                let padded_math_lines = pad_block_lines(
                    display_math_lines(settings, &text, math_style),
                    None,
                    block_style,
                    block_style,
                );
                for rendered in padded_math_lines {
                    doc.lines.push(RenderLine {
                        line: rendered.line,
                        kind: LineKind::Math,
                        link_ranges: vec![],
                        code_block_index: None,
                        image_index: None,
                        heading_level: None,
                        hang_indent: 0,
                        task: None,
                    });
                }
            }
            _ => {}
        }
    }

    if !current_line.is_empty() {
        push_current_line(
            &mut doc,
            &mut current_line,
            &mut current_line_links,
            &mut pending_meta,
            current_line_kind(quote_depth),
        );
    }
    trim_trailing_empty_lines(&mut doc.lines);

    Ok(doc)
}

fn push_current_line(
    doc: &mut RenderDoc,
    spans: &mut Vec<Span<'static>>,
    line_links: &mut Vec<LinkRange>,
    meta: &mut PendingLineMeta,
    kind: LineKind,
) {
    let PendingLineMeta {
        hang: hang_indent,
        task,
    } = std::mem::take(meta);
    if spans.is_empty() {
        line_links.clear();
        doc.lines.push(RenderLine {
            line: Line::from(""),
            kind: LineKind::Normal,
            link_ranges: vec![],
            code_block_index: None,
            image_index: None,
            heading_level: None,
            hang_indent: 0,
            task: None,
        });
    } else {
        doc.lines.push(RenderLine {
            line: Line::from(std::mem::take(spans)),
            kind,
            link_ranges: std::mem::take(line_links),
            code_block_index: None,
            image_index: None,
            heading_level: None,
            hang_indent,
            task,
        });
    }
}

fn line_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// Inside a list item, a line that starts after the item's first line (soft
/// break, second paragraph) is indented to the item's text column.
fn indent_list_continuation(
    current_line: &mut Vec<Span<'static>>,
    meta: &mut PendingLineMeta,
    item_indents: &[usize],
) {
    if !current_line.is_empty() {
        return;
    }
    if let Some(&indent) = item_indents.last() {
        current_line.push(Span::raw(" ".repeat(indent)));
        meta.hang = indent;
    }
}

fn current_line_kind(quote_depth: usize) -> LineKind {
    if quote_depth > 0 {
        LineKind::Quote
    } else {
        LineKind::Normal
    }
}

fn trim_trailing_empty_lines(lines: &mut Vec<RenderLine>) {
    while let Some(last) = lines.last() {
        let text = last
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        if text.trim().is_empty()
            && !matches!(last.kind, LineKind::Code | LineKind::Ascii | LineKind::Math)
        {
            lines.pop();
        } else {
            break;
        }
    }
}

fn is_blank_render_line(line: &RenderLine) -> bool {
    line.line
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<String>()
        .trim()
        .is_empty()
}

#[derive(Clone, Copy, Debug)]
struct ListState {
    ordered: bool,
    next: u64,
}

#[derive(Clone, Debug)]
struct PendingImage {
    source: String,
    title: Option<String>,
    alt: String,
}

fn parse_image_width_percent_attr(text: &str) -> Option<u8> {
    let trimmed = text.trim();
    if !(trimmed.starts_with('{') && trimmed.ends_with('}')) {
        return None;
    }
    let inner = trimmed[1..trimmed.len().saturating_sub(1)].trim();
    for token in inner.split_whitespace() {
        let Some(value) = token.strip_prefix("width=") else {
            continue;
        };
        let Some(percent) = value.strip_suffix('%') else {
            continue;
        };
        let parsed = percent.parse::<u8>().ok()?;
        if (1..=100).contains(&parsed) {
            return Some(parsed);
        }
    }
    None
}

fn consume_trailing_image_width_attr<'a, I>(parser: &mut std::iter::Peekable<I>) -> Option<u8>
where
    I: Iterator<Item = (Event<'a>, std::ops::Range<usize>)>,
{
    let width = match parser.peek() {
        Some((Event::Text(text), _)) => parse_image_width_percent_attr(text),
        _ => None,
    };
    if width.is_some() {
        let _ = parser.next();
    }
    width
}

fn blank_rows_between_offsets(source: &str, start: usize, end: usize) -> usize {
    if end <= start || start >= source.len() {
        return 0;
    }
    let end = end.min(source.len());
    source[start..end].chars().filter(|ch| *ch == '\n').count()
}

fn heading_to_u8(level: Option<HeadingLevel>) -> Option<u8> {
    match level {
        Some(HeadingLevel::H1) => Some(1),
        Some(HeadingLevel::H2) => Some(2),
        Some(HeadingLevel::H3) => Some(3),
        Some(HeadingLevel::H4) => Some(4),
        Some(HeadingLevel::H5) => Some(5),
        Some(HeadingLevel::H6) => Some(6),
        None => None,
    }
}

pub fn open_in_editor(path: &std::path::Path) -> Result<(), anyhow::Error> {
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(path)
        .status()
        .with_context(|| format!("failed to run editor '{}'", editor))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!("editor exited with status: {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::code::*;
    use super::front_matter::*;
    use super::math::*;
    use super::table::*;
    use super::*;
    use crate::theme::AppTheme;
    use ratatui::style::Modifier;

    fn settings() -> RenderSettings {
        RenderSettings {
            width: 80,
            theme: AppTheme::default(),
            table_row_separator: TableRowSeparator::Never,
            promote_h1_title: false,
            math_images: None,
        }
    }

    fn table_lines(md: &str, width: u16, sep: TableRowSeparator) -> Vec<RenderLine> {
        let settings = RenderSettings {
            width,
            table_row_separator: sep,
            ..settings()
        };
        render_markdown(md, &settings)
            .expect("render")
            .lines
            .into_iter()
            .filter(|l| l.kind == LineKind::Table)
            .collect()
    }

    fn text_of(line: &RenderLine) -> String {
        line.line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    const WIDE_TABLE: &str = "| Name | Notes |\n|---|---|\n| a | short |\n| b | a very long cell that keeps going on and on well past any sane terminal width for sure |\n";

    #[test]
    fn task_items_render_checkbox_glyphs_with_markers() {
        let md = "- [ ] open task\n- [x] done task\n- plain item\n  - nested\n    - deeper\n";
        let doc = render_markdown(md, &settings()).expect("render");
        let texts: Vec<String> = doc.lines.iter().map(text_of).collect();
        assert!(
            texts.contains(&format!("  {TASK_UNCHECKED} open task")),
            "{texts:?}"
        );
        assert!(
            texts.contains(&format!("  {TASK_CHECKED} done task")),
            "{texts:?}"
        );
        assert!(texts.contains(&"  • plain item".to_string()), "{texts:?}");
        assert!(texts.contains(&"    – nested".to_string()), "{texts:?}");
        assert!(texts.contains(&"      · deeper".to_string()), "{texts:?}");
        let tasks: Vec<TaskMarker> = doc.lines.iter().filter_map(|l| l.task).collect();
        assert_eq!(tasks.len(), 2);
        assert_eq!((tasks[0].ordinal, tasks[0].checked), (0, false));
        assert_eq!((tasks[1].ordinal, tasks[1].checked), (1, true));
        assert_eq!((tasks[0].start, tasks[0].end), (2, 3));
    }

    #[test]
    fn list_item_continuation_lines_are_indented() {
        let doc = render_markdown("- first line\n  second line\n", &settings()).expect("render");
        let second = doc
            .lines
            .iter()
            .find(|l| text_of(l).contains("second"))
            .expect("second line");
        assert_eq!(text_of(second), "    second line");
        assert_eq!(second.hang_indent, 4);
    }

    #[test]
    fn toggle_task_marker_flips_only_the_target() {
        let src = "---\ntitle: t\n---\n- [ ] a\n- [x] b\n\n```\n- [ ] not a task\n```\n";
        let out = toggle_task_marker(src, 0, false, 2).expect("toggle");
        assert_eq!(out, src.replacen("- [ ] a", "- [x] a", 1));
        let out = toggle_task_marker(src, 1, true, 2).expect("toggle");
        assert_eq!(out, src.replacen("- [x] b", "- [ ] b", 1));
    }

    #[test]
    fn toggle_task_marker_refuses_on_mismatch() {
        let src = "- [ ] a\n- [x] b\n";
        assert!(toggle_task_marker(src, 0, true, 2).is_err());
        assert!(toggle_task_marker(src, 0, false, 3).is_err());
        assert!(toggle_task_marker(src, 5, false, 2).is_err());
    }

    fn promoting() -> RenderSettings {
        RenderSettings {
            promote_h1_title: true,
            ..settings()
        }
    }

    #[test]
    fn leading_h1_becomes_title_without_front_matter() {
        let doc = render_markdown("# My Notes\n\nbody text\n", &promoting()).expect("render");
        assert_eq!(
            doc.front_matter.and_then(|fm| fm.title).as_deref(),
            Some("My Notes")
        );
        assert_eq!(text_of(&doc.lines[0]), "body text");
    }

    #[test]
    fn h1_stays_in_body_when_front_matter_has_title_or_not_leading() {
        let doc =
            render_markdown("---\ntitle: Site\n---\n# Chapter\n", &promoting()).expect("render");
        assert_eq!(
            doc.front_matter.and_then(|fm| fm.title).as_deref(),
            Some("Site")
        );
        assert!(doc.lines.iter().any(|l| text_of(l) == "Chapter"));

        let doc = render_markdown("intro\n\n# Later\n", &promoting()).expect("render");
        assert!(doc.front_matter.and_then(|fm| fm.title).is_none());
        assert!(doc.lines.iter().any(|l| text_of(l) == "Later"));

        // Plain mode (no promotion) keeps the H1.
        let doc = render_markdown("# My Notes\n", &settings()).expect("render");
        assert_eq!(text_of(&doc.lines[0]), "My Notes");
    }

    #[test]
    fn headings_drop_hash_markers_and_get_glyph_or_rule() {
        let doc =
            render_markdown("## Two\n\n### Three\n\n#### Four\n", &settings()).expect("render");
        let texts: Vec<String> = doc.lines.iter().map(text_of).collect();
        assert!(!texts.iter().any(|t| t.contains('#')), "{texts:?}");
        let two = texts.iter().position(|t| t == "Two").expect("h2");
        assert_eq!(texts[two + 1], "─".repeat(80));
        assert!(texts.contains(&format!("{HEADING_H3_GLYPH}Three")));
        assert!(texts.contains(&"Four".to_string()));
    }

    #[test]
    fn literal_greater_than_paragraph_is_not_a_quote() {
        let doc = render_markdown("\\> not a quote\n", &settings()).expect("render");
        let line = doc
            .lines
            .iter()
            .find(|l| text_of(l).contains("not a quote"))
            .expect("line");
        assert_eq!(text_of(line), "> not a quote");
        assert_eq!(line.kind, LineKind::Normal);
    }

    fn words_of(text: &str) -> CellWords {
        let mut cell = TableCell::default();
        cell.push(text.to_string(), Style::default());
        CellWords::of(&cell)
    }

    fn grid(rows: &[&[&str]]) -> Vec<Vec<CellWords>> {
        rows.iter()
            .map(|r| r.iter().map(|t| words_of(t)).collect())
            .collect()
    }

    #[test]
    fn predicted_cell_lines_match_actual_wrapping() {
        let samples = [
            "when true images referenced with http and https urls are fetched",
            "a supercalifragilisticexpialidocious word",
            "x",
            "",
        ];
        for text in samples {
            let spans = vec![Span::raw(text.to_string())];
            for width in 1..30 {
                let actual = crate::layout::wrap_styled(&spans, &[], width).len();
                assert_eq!(words_of(text).lines_at(width), actual, "{text:?} @ {width}");
            }
        }
    }

    #[test]
    fn wide_column_shrinks_and_short_column_keeps_whole_words() {
        let rows = grid(&[
            &["Setting name", "Description"],
            &[
                "remote_images_enabled",
                "when true images referenced with http and https urls are fetched in the background with a timeout and a size cap",
            ],
            &["line_numbers", "show line numbers in the gutter"],
        ]);
        let widths = fit_column_widths(&rows, 60);
        assert_eq!(widths[0], 21, "short column must keep its natural width");
        assert_eq!(widths.iter().sum::<usize>() + 7, 60);
    }

    #[test]
    fn width_is_taken_where_it_adds_the_fewest_lines() {
        // Column 1 is widest but only one cell is long; column 0 has many
        // medium cells. Wrapping the single long cell is cheaper.
        let rows = grid(&[
            &["alpha beta gamma", "x"],
            &["delta epsilon zeta", "y"],
            &[
                "eta theta iota",
                "one two three four five six seven eight nine ten",
            ],
            &["kappa lambda mu", "z"],
        ]);
        let widths = fit_column_widths(&rows, 50);
        assert_eq!(widths[0], 18, "medium column stays intact");
        let height: usize = rows
            .iter()
            .map(|r| {
                r.iter()
                    .zip(&widths)
                    .map(|(c, w)| c.lines_at(*w))
                    .max()
                    .unwrap()
            })
            .sum();
        assert_eq!(height, 5, "only the long cell wraps, onto two lines");
    }

    #[test]
    fn words_are_only_broken_when_nothing_else_fits() {
        let rows = grid(&[&["abcdefghij", "klmnopqrst"]]);
        // 10 + 10 + 7 = 27 fits whole words
        assert_eq!(fit_column_widths(&rows, 27), vec![10, 10]);
        // narrower: squeezed, every word breaks, still fits
        let widths = fit_column_widths(&rows, 17);
        assert_eq!(widths.iter().sum::<usize>() + 7, 17);
    }

    #[test]
    fn emoji_presentation_sequences_keep_table_borders_aligned() {
        let md = "| Attack Scenario | Result |\n|---|---|\n| Sleep-state theft | ❌ |\n| Evil-maid attack | ⚠️ (needs TPM/Secure Boot) |\n";
        for width in [80, 30] {
            let lines = table_lines(md, width, TableRowSeparator::Never);
            let border = UnicodeWidthStr::width(text_of(&lines[0]).as_str());
            for line in &lines {
                assert_eq!(
                    UnicodeWidthStr::width(text_of(line).as_str()),
                    border,
                    "{:?} @ {width}",
                    text_of(line)
                );
            }
        }
    }

    #[test]
    fn narrow_table_keeps_natural_widths() {
        let lines = table_lines(
            "| a | b |\n|---|---|\n| 1 | 2 |\n",
            80,
            TableRowSeparator::Never,
        );
        let texts: Vec<_> = lines.iter().map(text_of).collect();
        assert_eq!(texts[0], "┌───┬───┐");
        assert_eq!(texts[1], "│ a │ b │");
        assert_eq!(texts.len(), 5);
    }

    #[test]
    fn wide_table_fits_width_and_wraps_cells() {
        let lines = table_lines(WIDE_TABLE, 40, TableRowSeparator::Never);
        for line in &lines {
            assert!(
                UnicodeWidthStr::width(text_of(line).as_str()) <= 40,
                "{:?}",
                text_of(line)
            );
        }
        // header + sep + 'a' row + several 'b' rows + borders
        assert!(lines.len() > 6);
        // all rows have the same width, so borders line up
        let w = UnicodeWidthStr::width(text_of(&lines[0]).as_str());
        assert!(
            lines
                .iter()
                .all(|l| UnicodeWidthStr::width(text_of(l).as_str()) == w)
        );
        // no separator between body rows by default
        assert_eq!(
            lines.iter().filter(|l| text_of(l).starts_with('├')).count(),
            1
        );
    }

    #[test]
    fn row_separator_modes() {
        let count = |md: &str, sep| {
            table_lines(md, 40, sep)
                .iter()
                .filter(|l| text_of(l).starts_with('├'))
                .count()
        };
        let small = "| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n";
        assert_eq!(count(WIDE_TABLE, TableRowSeparator::Wrapped), 2);
        assert_eq!(count(small, TableRowSeparator::Wrapped), 1);
        assert_eq!(count(small, TableRowSeparator::Always), 2);
    }

    #[test]
    fn squeezed_table_hard_breaks_and_still_fits() {
        let md = "| aaaaaaaaaa | bbbbbbbbbb | cccccccccc | dddddddddd |\n|---|---|---|---|\n| 1 | 2 | 3 | 4 |\n";
        let lines = table_lines(md, 24, TableRowSeparator::Never);
        assert!(
            lines
                .iter()
                .all(|l| UnicodeWidthStr::width(text_of(l).as_str()) <= 24)
        );
    }

    #[test]
    fn table_cells_keep_styles_and_clickable_links() {
        let md = "| x | y |\n|---|---|\n| **bold** `code` | [site](https://s.test) |\n";
        let lines = table_lines(md, 80, TableRowSeparator::Never);
        let row = lines
            .iter()
            .find(|l| text_of(l).contains("bold"))
            .expect("row");
        assert!(
            row.line
                .spans
                .iter()
                .any(|s| s.content == "bold" && s.style.add_modifier.contains(Modifier::BOLD))
        );
        let text = text_of(row);
        let col_of = |needle: &str| UnicodeWidthStr::width(&text[..text.find(needle).unwrap()]);
        let link = row
            .link_ranges
            .iter()
            .find(|r| r.kind == RangeKind::Url)
            .expect("link");
        assert_eq!(link.url, "https://s.test");
        assert_eq!((link.start, link.end), (col_of("site"), col_of("site") + 4));
        let copy = row
            .link_ranges
            .iter()
            .find(|r| r.kind == RangeKind::Copy)
            .expect("inline code copy range");
        assert_eq!(copy.url, "code");
        assert_eq!((copy.start, copy.end), (col_of("code"), col_of("code") + 4));
    }

    #[test]
    fn inline_code_copies_its_raw_text_and_code_in_links_stays_a_link() {
        let doc = render_markdown(
            "run `dd if=/dev/urandom` or [`docs`](https://d.test)\n",
            &settings(),
        )
        .expect("render");
        let ranges = &doc.lines[0].link_ranges;
        assert_eq!(ranges.len(), 2);
        assert_eq!(
            (ranges[0].kind, ranges[0].url.as_str()),
            (RangeKind::Copy, "dd if=/dev/urandom")
        );
        assert_eq!(
            (ranges[1].kind, ranges[1].url.as_str()),
            (RangeKind::Url, "https://d.test")
        );
    }

    #[test]
    fn tabs_in_code_blocks_are_expanded_and_no_extra_blank_line() {
        let doc = render_markdown("```\n\tindented\na\tb\n```\n", &settings()).expect("render");
        let code: Vec<String> = doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Code)
            .map(text_of)
            .collect();
        let content: Vec<&str> = code
            .iter()
            .map(|l| l.trim_end())
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(content, vec!["      indented", "  a   b"]);
        // padding top + 2 lines + padding bottom
        assert_eq!(code.len(), 4, "{code:?}");
        assert!(!code.iter().any(|l| l.contains('\t')));
        // the copied text keeps the original tabs
        assert_eq!(doc.code_blocks[0], "\tindented\na\tb\n");
    }

    #[test]
    fn control_characters_never_reach_rendered_text() {
        let doc = render_markdown("a\tb\u{7}c `x\u{1b}y`\n", &settings()).expect("render");
        let text = text_of(&doc.lines[0]);
        assert_eq!(text, "a bc xy");
    }

    #[test]
    fn preprocess_math_works() {
        let md = "x: $ x^2 $";
        let out = preprocess_math(md);
        assert!(out.contains("$x^2$"));
        let doc = render_markdown(&out, &settings()).expect("render");
        let joined = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(joined.contains("$x^2$"), "parsed as math: {joined}");
    }

    #[test]
    fn smart_quotes_are_used_in_rendered_text() {
        let doc = render_markdown("\"text\" and 'quoted' and don't", &settings()).expect("render");
        let joined = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(joined.contains("“text”"));
        assert!(joined.contains("‘quoted’"));
        assert!(joined.contains("don’t"));
    }

    #[test]
    fn smart_quotes_pair_across_inline_spans() {
        let doc = render_markdown("\"*text*\"", &settings()).expect("render");
        let joined = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(joined.contains("“text”"));
    }

    #[test]
    fn markdown_image_emits_image_line_and_metadata() {
        let doc = render_markdown(
            "![diagram alt](./img/diag.png \"Diagram Title\")",
            &settings(),
        )
        .expect("render");
        assert_eq!(doc.images.len(), 1);
        assert_eq!(doc.images[0].source, "./img/diag.png");
        assert_eq!(doc.images[0].alt, "diagram alt");
        assert_eq!(doc.images[0].title.as_deref(), Some("Diagram Title"));
        assert_eq!(doc.images[0].width_percent, None);

        let image_line = doc
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Image)
            .expect("image line");
        let line_text = image_line
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(line_text.contains("[image] diagram alt"));
        assert_eq!(image_line.image_index, Some(0));
    }

    #[test]
    fn markdown_url_image_exposes_openable_link() {
        let url = "https://example.com/plot.png";
        let doc = render_markdown(&format!("![plot]({url})"), &settings()).expect("render");
        let image_line = doc
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Image)
            .expect("image line");
        assert_eq!(image_line.link_ranges[0].url, url);
    }

    #[test]
    fn markdown_image_width_percent_attribute_is_parsed() {
        let doc =
            render_markdown("![diagram](./img/diag.png){width=65%}", &settings()).expect("render");
        assert_eq!(doc.images.len(), 1);
        assert_eq!(doc.images[0].width_percent, Some(65));
        let joined = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(!joined.contains("{width=65%}"));
    }

    #[test]
    fn markdown_invalid_image_width_percent_attribute_is_ignored() {
        let doc =
            render_markdown("![diagram](./img/diag.png){width=140%}", &settings()).expect("render");
        assert_eq!(doc.images.len(), 1);
        assert_eq!(doc.images[0].width_percent, None);
    }

    #[test]
    fn code_fence_quotes_stay_literal() {
        let doc = render_markdown("```text\n\"text\" 'word'\n```", &settings()).expect("render");
        let joined = doc
            .lines
            .iter()
            .filter(|l| matches!(l.kind, LineKind::Code | LineKind::Ascii))
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(joined.contains("\"text\" 'word'"));
        assert!(!joined.contains("“text” ‘word’"));
    }

    #[test]
    fn escaped_math_markers_stay_literal_inside_fenced_text_code() {
        let md = "```text\nvalue = \\$x^2\\$\n```";
        let doc = render_markdown(md, &settings()).expect("render");
        let joined = doc
            .lines
            .iter()
            .filter(|l| matches!(l.kind, LineKind::Code | LineKind::Ascii))
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(joined.contains(r"value = \$x^2\$"));
    }

    #[test]
    fn unescaped_math_markers_stay_literal_inside_fenced_text_code() {
        let md = "```text\nvalue = $x^2$\n```";
        let doc = render_markdown(md, &settings()).expect("render");
        let joined = doc
            .lines
            .iter()
            .filter(|l| matches!(l.kind, LineKind::Code | LineKind::Ascii))
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(joined.contains("value = $x^2$"));
        assert!(!joined.contains("value = x²"));
    }

    #[test]
    fn render_code_block_highlight_non_empty() {
        let md = "```rust\nfn main() {}\n```";
        let doc = render_markdown(md, &settings()).expect("render");
        assert!(doc.lines.iter().any(|l| l.kind == LineKind::Code));
    }

    #[test]
    fn python_comment_does_not_bleed_to_next_line() {
        let md = "```python\na = 1 # comment\nb = 2\n```";
        let doc = render_markdown(md, &settings()).expect("render");
        let code_rows: Vec<&RenderLine> = doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Code)
            .collect();
        let first = code_rows
            .iter()
            .find(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .contains("a = 1")
            })
            .expect("first python line");
        let second = code_rows
            .iter()
            .find(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .contains("b = 2")
            })
            .expect("second python line");

        let comment_fg = first
            .line
            .spans
            .iter()
            .find(|s| s.content.contains("comment"))
            .and_then(|s| s.style.fg);
        let b_fg = second
            .line
            .spans
            .iter()
            .find(|s| s.content.contains('b'))
            .and_then(|s| s.style.fg);
        assert_eq!(comment_fg, Some(settings().theme.code_palette.grey));
        assert_ne!(b_fg, Some(settings().theme.code_palette.grey));
    }

    #[test]
    fn comment_delimiters_are_comment_colored() {
        let md = "```python\n# top comment\nx = 1  # inline\n```\n```rust\n// rust comment\nlet a = 1;\n```\n```bash\n# shell comment\necho ok\n```";
        let doc = render_markdown(md, &settings()).expect("render");
        let grey = Some(settings().theme.code_palette.grey);

        let lines: Vec<String> = doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Code)
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(lines.iter().any(|l| l.contains("# top comment")));
        assert!(lines.iter().any(|l| l.contains("// rust comment")));
        assert!(lines.iter().any(|l| l.contains("# shell comment")));

        let mut saw_hash_or_slashes = false;
        for l in doc.lines.iter().filter(|l| l.kind == LineKind::Code) {
            for s in &l.line.spans {
                if s.content.contains('#') || s.content.contains("//") {
                    saw_hash_or_slashes = true;
                    assert_eq!(s.style.fg, grey);
                }
            }
        }
        assert!(saw_hash_or_slashes);
    }

    #[test]
    fn exact_semantic_theme_uses_code_palette_colors() {
        let mut theme = AppTheme::default();
        theme.code_palette.cyan = ratatui::style::Color::Rgb(1, 2, 3);
        let syn = super::exact_semantic_theme(&theme);
        assert!(syn.scopes.iter().any(|item| {
            item.style.foreground == Some(super::to_syn_color(theme.code_palette.cyan))
        }));
    }

    #[test]
    fn link_ranges_track_multiple_links_on_same_line() {
        let doc = render_markdown("[a](https://a.test) and [b](https://b.test)", &settings())
            .expect("render");
        let line = doc
            .lines
            .iter()
            .find(|l| !l.link_ranges.is_empty())
            .expect("line with links");
        assert_eq!(line.link_ranges.len(), 2);
        assert_eq!(line.link_ranges[0].url, "https://a.test");
        assert_eq!(line.link_ranges[1].url, "https://b.test");
        assert!(line.link_ranges[0].end <= line.link_ranges[1].start);
    }

    #[test]
    fn markdown_links_are_preserved() {
        let doc = render_markdown("[x](https://example.com)", &settings()).expect("render");
        let link = doc
            .lines
            .iter()
            .flat_map(|l| &l.link_ranges)
            .next()
            .expect("link");
        assert_eq!(link.url, "https://example.com");
    }

    #[test]
    fn markdown_link_does_not_add_raw_url_line() {
        let doc = render_markdown("[x](https://example.com)", &settings()).expect("render");
        let joined = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(!joined.contains("[1] https://example.com"));
    }

    #[test]
    fn math_code_block_without_graphics_shows_latex_source() {
        let md = "```math\n\\frac{a}{b}\n```";
        let doc = render_markdown(md, &settings()).expect("render");
        let math_span = doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Math)
            .flat_map(|l| l.line.spans.iter())
            .find(|s| s.content.as_ref().contains(r"\frac{a}{b}"))
            .expect("math span");
        assert_eq!(math_span.style.fg, settings().theme.normal.fg);
    }

    fn graphics_settings() -> RenderSettings {
        RenderSettings {
            math_images: Some(Rc::new(RefCell::new(MathImages::new(
                (10, 20),
                crate::config::MathFont::Fira,
            )))),
            ..settings()
        }
    }

    fn placeholder_spans(doc: &RenderDoc) -> Vec<(LineKind, Span<'static>)> {
        doc.lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(move |s| (l.kind, s.clone())))
            .filter(|(_, s)| s.content.starts_with(placeholder::PLACEHOLDER))
            .collect()
    }

    #[test]
    fn display_math_with_graphics_is_placeholder_rows() {
        for md in ["$$\\frac{a}{b}$$", "```math\n\\frac{a}{b}\n```"] {
            let doc = render_markdown(md, &graphics_settings()).expect("render");
            let spans = placeholder_spans(&doc);
            assert!(spans.len() >= 2, "a fraction spans several rows");
            assert!(spans.iter().all(|(kind, _)| *kind == LineKind::Math));
            let fg = spans[0].1.style.fg;
            assert!(spans.iter().all(|(_, s)| s.style.fg == fg), "one image");
        }
    }

    #[test]
    fn inline_math_with_graphics_is_one_placeholder_run() {
        let doc =
            render_markdown("a $x^2$ b $\\frac{1}{2}$ c", &graphics_settings()).expect("render");
        let spans = placeholder_spans(&doc);
        assert_eq!(spans.len(), 2);
        assert_ne!(spans[0].1.style.fg, spans[1].1.style.fg);
        let text: String = doc.lines[0]
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert!(text.starts_with("a ") && text.ends_with(" c"));
    }

    #[test]
    fn inline_math_in_tables_is_measured_in_cells() {
        let md = "| a | b |\n|---|---|\n| $x^2$ | y |\n";
        let doc = render_markdown(md, &graphics_settings()).expect("render");
        assert_eq!(placeholder_spans(&doc).len(), 1);
    }

    #[test]
    fn invalid_math_with_graphics_falls_back_to_source() {
        let doc = render_markdown("a $\\frac{1}$ b\n\n$$\\frac{a}$$", &graphics_settings())
            .expect("render");
        assert!(placeholder_spans(&doc).is_empty());
        let text: String = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(text.contains("$\\frac{1}$"));
        assert!(text.contains("\\frac{a}"));
    }

    #[test]
    fn style_markers_apply() {
        let doc = render_markdown("**b** *i* ~~s~~", &settings()).expect("render");
        let text = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<String>();
        assert!(text.contains('b'));
        assert!(text.contains('i'));
        assert!(text.contains('s'));
    }

    #[test]
    fn quote_text_uses_dimmed_color_and_italic() {
        let doc = render_markdown("> quoted text", &settings()).expect("render");
        let quote_line = doc
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Quote)
            .expect("quote line");
        let text_span = quote_line
            .line
            .spans
            .iter()
            .find(|s| s.content.contains("quoted"))
            .expect("quote span");
        assert_eq!(text_span.style.fg, settings().theme.quote.fg);
        assert!(text_span.style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn display_math_source_is_left_trimmed_consistently() {
        let md = "$$\n    a = b\n      m^{e^{d}} = m\n$$";
        let doc = render_markdown(md, &settings()).expect("render");
        let math_lines: Vec<String> = doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Math)
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .filter(|l| !l.trim().is_empty())
            .collect();
        assert_eq!(math_lines.len(), 2);
        assert!(math_lines[0].trim_end().ends_with("a = b"));
        let indent = |l: &str| l.chars().take_while(|c| *c == ' ').count();
        assert_eq!(indent(&math_lines[1]), indent(&math_lines[0]) + 2);
    }

    #[test]
    fn fenced_code_block_is_padded() {
        let md = "```rust\nfn main() {}\n```";
        let doc = render_markdown(md, &settings()).expect("render");
        let code_lines: Vec<String> = doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Code)
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(code_lines.len() >= 2);
        let leading_blank = code_lines
            .iter()
            .take_while(|l| l.trim().is_empty())
            .count();
        assert_eq!(leading_blank, BLOCK_PAD_TOP);
        let content = code_lines
            .iter()
            .find(|l| l.contains("fn main()"))
            .expect("content line");
        assert!(content.starts_with(&" ".repeat(BLOCK_PAD_X)));
        assert!(content.ends_with(&" ".repeat(BLOCK_PAD_X)));
    }

    #[test]
    fn jekyll_block_title_is_rendered_inside_code_block() {
        let md = "```rust\nfn main() {}\n```\n{: title=\"main.rs\"}";
        let doc = render_markdown(md, &settings()).expect("render");
        let code_lines: Vec<String> = doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Code)
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(code_lines.iter().any(|l| l.contains("main.rs")));
        let title_idx = doc
            .lines
            .iter()
            .position(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .contains("main.rs")
            })
            .expect("title line");
        assert_eq!(doc.lines[title_idx].code_block_index, None);
        let gap_idx = title_idx + 1;
        let gap_text = doc.lines[gap_idx]
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(gap_text.trim().is_empty());
        let content_idx = title_idx + 2;
        let content_text = doc.lines[content_idx]
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(content_text.contains("fn main() {}"));
        assert!(doc.lines[content_idx].code_block_index.is_some());
        let title_text = doc.lines[title_idx]
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(title_text.starts_with(' '));
        assert!(
            !doc.lines
                .iter()
                .any(|l| l.line.spans.iter().any(|s| s.content.contains("{: title=")))
        );
    }

    #[test]
    fn markdown_is_rewritten_with_hidden_block_title_marker() {
        let md = "```bash\necho hi\n```\n{: title=\"script.sh\"}\n";
        let out = preprocess_block_title_attributes(md);
        assert!(out.contains(BLOCK_TITLE_MARKER));
        assert!(!out.contains("{: title=\"script.sh\"}"));
    }

    #[test]
    fn tables_render_with_grid_separators() {
        let md = "| A | B |\n| - | - |\n| 1 | 2 |";
        let doc = render_markdown(md, &settings()).expect("render");
        let joined = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains('│'));
        assert!(joined.contains('├'));
    }

    #[test]
    fn table_rows_have_consistent_width() {
        use unicode_width::UnicodeWidthStr;
        let md = "| A | B |\n| - | - |\n| short | very very long |";
        let doc = render_markdown(md, &settings()).expect("render");
        let rows: Vec<String> = doc
            .lines
            .iter()
            .filter_map(|l| {
                let s = l
                    .line
                    .spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>();
                if s.contains('│') || s.contains('┌') || s.contains('└') || s.contains('├')
                {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        let widths: Vec<usize> = rows
            .iter()
            .map(|r| UnicodeWidthStr::width(r.as_str()))
            .collect();
        assert!(widths.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn table_header_titles_are_visible() {
        let md = "| H1 | H2 |\n| - | - |\n| one | 1 |";
        let doc = render_markdown(md, &settings()).expect("render");
        let lines: Vec<String> = doc
            .lines
            .iter()
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        let header_idx = lines
            .iter()
            .position(|line| line.contains("H1") && line.contains("H2"))
            .expect("header row");
        let sep_idx = lines
            .iter()
            .position(|line| line.contains('├'))
            .expect("header separator");
        let body_idx = lines
            .iter()
            .position(|line| line.contains("one") && line.contains("1"))
            .expect("body row");
        assert!(header_idx < sep_idx);
        assert!(sep_idx < body_idx);
    }

    #[test]
    fn table_body_rows_remain_visible() {
        let md = "| H1 | H2 |\n| - | - |\n| a | b |\n| c | d |";
        let doc = render_markdown(md, &settings()).expect("render");
        let lines: Vec<String> = doc
            .lines
            .iter()
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>()
            })
            .collect();

        let header_idx = lines
            .iter()
            .position(|line| line.contains("H1") && line.contains("H2"))
            .expect("header row");
        let sep_idx = lines
            .iter()
            .position(|line| line.contains('├'))
            .expect("header separator");
        let body_idx = lines
            .iter()
            .position(|line| line.contains("a") && line.contains("b"))
            .expect("first body row");
        assert!(
            lines
                .iter()
                .any(|line| line.contains("c") && line.contains("d")),
            "second body row"
        );
        assert!(header_idx < sep_idx);
        assert!(sep_idx < body_idx);
    }

    #[test]
    fn list_items_render_on_separate_lines() {
        let doc = render_markdown("- a\n- b", &settings()).expect("render");
        let bullet_lines = doc
            .lines
            .iter()
            .filter_map(|l| {
                let text = l
                    .line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>();
                if text.trim_start().starts_with("• ") {
                    Some(text)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(bullet_lines.len(), 2);
        assert!(bullet_lines.iter().all(|line| line.starts_with("  • ")));
    }

    #[test]
    fn list_has_single_separator_after_block() {
        let doc = render_markdown("before\n\n- a\n- b\n\nafter", &settings()).expect("render");
        let lines = doc
            .lines
            .iter()
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let bullet_positions = lines
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                if s.trim_start().starts_with("• ") {
                    Some(i)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(bullet_positions.len(), 2);
        let first = bullet_positions[0];
        let last = bullet_positions[1];
        assert!(first > 0);
        assert!(last + 1 < lines.len());
        assert!(lines[first - 1].trim().is_empty());
        assert!(lines[last + 1].trim().is_empty());
        assert!(
            !lines[first..=last]
                .iter()
                .any(|l| l.trim().is_empty() && !l.trim_start().starts_with("• "))
        );
    }

    #[test]
    fn list_immediately_after_paragraph_has_no_gap() {
        let doc = render_markdown("before\n- a", &settings()).expect("render");
        let lines = doc
            .lines
            .iter()
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let bullet_idx = lines
            .iter()
            .position(|s| s.trim_start().starts_with("• "))
            .expect("list item");
        assert!(bullet_idx > 0);
        assert_eq!(lines[bullet_idx - 1].trim(), "before");
    }

    #[test]
    fn list_gap_matches_multiple_blank_lines() {
        let doc = render_markdown("before\n\n\n- a", &settings()).expect("render");
        let lines = doc
            .lines
            .iter()
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let bullet_idx = lines
            .iter()
            .position(|s| s.trim_start().starts_with("• "))
            .expect("list item");
        assert!(bullet_idx >= 3);
        assert_eq!(lines[bullet_idx - 1].trim(), "");
        assert_eq!(lines[bullet_idx - 2].trim(), "");
        assert_eq!(lines[bullet_idx - 3].trim(), "before");
    }

    #[test]
    fn list_preserves_gap_after_previous_paragraph() {
        let doc = render_markdown("before\n\n- a", &settings()).expect("render");
        let lines = doc
            .lines
            .iter()
            .map(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>();
        let bullet_idx = lines
            .iter()
            .position(|s| s.trim_start().starts_with("• "))
            .expect("list item");
        assert!(bullet_idx > 0);
        assert_eq!(lines[bullet_idx - 1].trim(), "");
    }

    #[test]
    fn display_math_lines_use_math_kind() {
        let doc = render_markdown("$$\na=b\n$$", &settings()).expect("render");
        assert!(doc.lines.iter().any(|l| l.kind == LineKind::Math));
        assert!(!doc.lines.iter().any(|l| l.kind == LineKind::Code));
    }

    #[test]
    fn display_math_does_not_insert_extra_normal_blank_after_block() {
        let doc = render_markdown("before\n$$\nx\n$$\nafter", &settings()).expect("render");
        let last_math_idx = doc
            .lines
            .iter()
            .rposition(|l| l.kind == LineKind::Math)
            .expect("math lines");
        let after_idx = doc
            .lines
            .iter()
            .position(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .contains("after")
            })
            .expect("after line");
        let extra_normal_blanks = doc.lines[last_math_idx + 1..after_idx]
            .iter()
            .filter(|l| {
                l.kind == LineKind::Normal
                    && l.line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .trim()
                        .is_empty()
            })
            .count();
        assert_eq!(extra_normal_blanks, 0);
    }

    #[test]
    fn inline_math_without_graphics_shows_latex_source_in_text_colour() {
        let doc = render_markdown("a $x$ b", &settings()).expect("render");
        let math_span = doc
            .lines
            .iter()
            .flat_map(|l| l.line.spans.iter())
            .find(|s| s.content.as_ref() == "$x$")
            .expect("inline math span");
        assert_eq!(math_span.style.fg, settings().theme.normal.fg);
    }

    #[test]
    fn text_fence_lines_use_ascii_kind() {
        let doc = render_markdown("```text\nA -> B\n```", &settings()).expect("render");
        assert!(doc.lines.iter().any(|l| l.kind == LineKind::Ascii));
    }

    #[test]
    fn front_matter_is_extracted() {
        let md = "---\ntitle: My title\ndescription: Desc\n---\n# Body";
        let (fm, body) = extract_front_matter(md);
        let fm = fm.expect("fm");
        assert_eq!(fm.title.as_deref(), Some("My title"));
        assert!(body.contains("# Body"));
    }
}

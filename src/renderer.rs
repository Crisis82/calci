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

use crate::theme::AppTheme;

const BLOCK_TITLE_MARKER: &str = "__calci_block_title__:";
const BLOCK_PAD_X: usize = 2;
const BLOCK_PAD_TOP: usize = 1;
const BLOCK_PAD_BOTTOM: usize = 1;
const BLOCK_TITLE_GAP: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Normal,
    Heading,
    Code,
    Table,
    Quote,
}

#[derive(Clone, Debug)]
pub struct RenderLine {
    pub line: Line<'static>,
    pub kind: LineKind,
    pub link_url: Option<String>,
    pub link_ranges: Vec<LinkRange>,
    pub code_block_index: Option<usize>,
    pub heading_level: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct LinkRange {
    pub start: usize,
    pub end: usize,
    pub url: String,
}

#[derive(Clone, Debug, Default)]
pub struct RenderDoc {
    pub front_matter: Option<FrontMatter>,
    pub lines: Vec<RenderLine>,
    pub code_blocks: Vec<String>,
    pub links: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RenderSettings {
    pub width: u16,
    pub theme: AppTheme,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            width: 100,
            theme: AppTheme::from_name(crate::theme::ThemeName::Oxocarbon),
        }
    }
}

pub fn preprocess_math(markdown: &str) -> String {
    normalize_loose_inline_math(markdown)
}

pub fn render_markdown(
    markdown: &str,
    settings: &RenderSettings,
) -> Result<RenderDoc, anyhow::Error> {
    let (front_matter, markdown_body) = extract_front_matter(markdown);
    let markdown = preprocess_block_title_attributes(&markdown_body);
    let mut doc = RenderDoc::default();
    doc.front_matter = front_matter.clone();
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_MATH);

    let parser = Parser::new_ext(&markdown, options);
    let syntax_set = SyntaxSet::load_defaults_newlines();
    let syntax_theme = exact_semantic_theme(&settings.theme);

    let mut in_code = false;
    let mut code_lang = String::new();
    let mut code_buf = String::new();
    let mut heading_level: Option<HeadingLevel> = None;
    let mut quote_depth = 0usize;
    let mut pending_link: Option<String> = None;
    let mut list_stack: Vec<ListState> = Vec::new();
    let mut table: Option<TableState> = None;
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut current_line_links: Vec<LinkRange> = Vec::new();
    let mut active_mods = Modifier::empty();

    for event in parser {
        if let Some(tbl) = table.as_mut() {
            match event {
                Event::Start(Tag::TableHead) => {
                    continue;
                }
                Event::End(TagEnd::TableHead) => {
                    tbl.header_rows = tbl.rows.len();
                    continue;
                }
                Event::Start(Tag::TableRow) => {
                    tbl.current_row.clear();
                    continue;
                }
                Event::End(TagEnd::TableRow) => {
                    if !tbl.current_cell.is_empty() {
                        tbl.current_row.push(tbl.current_cell.clone());
                        tbl.current_cell.clear();
                    }
                    if !tbl.current_row.is_empty() {
                        tbl.rows.push(tbl.current_row.clone());
                        tbl.current_row.clear();
                    }
                    continue;
                }
                Event::Start(Tag::TableCell) => {
                    tbl.current_cell.clear();
                    continue;
                }
                Event::End(TagEnd::TableCell) => {
                    tbl.current_row.push(tbl.current_cell.clone());
                    tbl.current_cell.clear();
                    continue;
                }
                Event::Text(text) => {
                    tbl.current_cell.push_str(&text);
                    continue;
                }
                Event::Code(code) => {
                    tbl.current_cell.push_str(&code);
                    continue;
                }
                Event::InlineMath(m) => {
                    tbl.current_cell
                        .push_str(&calcifer::math::render_inline(&m));
                    continue;
                }
                Event::SoftBreak | Event::HardBreak => {
                    tbl.current_cell.push(' ');
                    continue;
                }
                Event::End(TagEnd::Table) => {
                    for row_line in
                        render_table_lines(tbl, settings.theme.normal, settings.theme.list_marker)
                    {
                        doc.lines.push(RenderLine {
                            line: row_line,
                            kind: LineKind::Table,
                            link_url: None,
                            link_ranges: vec![],
                            code_block_index: None,
                            heading_level: None,
                        });
                    }
                    doc.lines.push(RenderLine {
                        line: Line::from(""),
                        kind: LineKind::Normal,
                        link_url: None,
                        link_ranges: vec![],
                        code_block_index: None,
                        heading_level: None,
                    });
                    table = None;
                    continue;
                }
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
                            current_line_kind(quote_depth),
                        );
                    }
                    let indent = "  ".repeat(list_stack.len().saturating_sub(1));
                    let marker = if let Some(last) = list_stack.last_mut() {
                        if last.ordered {
                            let m = format!("{}. ", last.next);
                            last.next += 1;
                            m
                        } else {
                            "• ".to_string()
                        }
                    } else {
                        "• ".to_string()
                    };
                    current_line.push(Span::styled(marker, settings.theme.list_marker));
                    if !indent.is_empty() {
                        current_line.insert(0, Span::raw(indent));
                    }
                }
                Tag::Link { dest_url, .. } => {
                    pending_link = Some(dest_url.to_string());
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
                    let code_lines = if code_lang == "math" || code_lang == "latex" {
                        calcifer::math::render_block(&code_payload)
                            .into_iter()
                            .map(|l| Line::from(vec![Span::styled(l, block_style)]))
                            .collect::<Vec<_>>()
                    } else if code_lang.trim().is_empty() {
                        plain_code_lines(&code_payload, block_style)
                    } else {
                        highlight_code_block(
                            &code_payload,
                            &code_lang,
                            &syntax_set,
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
                            kind: LineKind::Code,
                            link_url: None,
                            link_ranges: vec![],
                            code_block_index: if rendered.is_code_content {
                                Some(block_index)
                            } else {
                                None
                            },
                            heading_level: None,
                        });
                    }
                    code_buf.clear();
                    code_lang.clear();
                    doc.lines.push(RenderLine {
                        line: Line::from(""),
                        kind: LineKind::Normal,
                        link_url: None,
                        link_ranges: vec![],
                        code_block_index: None,
                        heading_level: None,
                    });
                }
                TagEnd::Heading(_) => {
                    if !current_line.is_empty() {
                        let mut line = Line::from(std::mem::take(&mut current_line));
                        line.spans
                            .insert(0, Span::raw(heading_prefix(heading_level)));
                        line.spans
                            .iter_mut()
                            .for_each(|s| s.style = settings.theme.heading);
                        doc.lines.push(RenderLine {
                            line,
                            kind: LineKind::Heading,
                            link_url: current_line_links.first().map(|l| l.url.clone()),
                            link_ranges: std::mem::take(&mut current_line_links),
                            code_block_index: None,
                            heading_level: heading_to_u8(heading_level),
                        });
                    }
                    heading_level = None;
                    doc.lines.push(RenderLine {
                        line: Line::from(""),
                        kind: LineKind::Normal,
                        link_url: None,
                        link_ranges: vec![],
                        code_block_index: None,
                        heading_level: None,
                    });
                }
                TagEnd::BlockQuote(_) => {
                    quote_depth = quote_depth.saturating_sub(1);
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
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
                            current_line_kind(quote_depth),
                        );
                    }
                    doc.lines.push(RenderLine {
                        line: Line::from(""),
                        kind: LineKind::Normal,
                        link_url: None,
                        link_ranges: vec![],
                        code_block_index: None,
                        heading_level: None,
                    });
                }
                TagEnd::Link => {
                    pending_link = None;
                }
                TagEnd::List(_) => {
                    let _ = list_stack.pop();
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
                            current_line_kind(quote_depth),
                        );
                    }
                }
                TagEnd::Item => {
                    if !current_line.is_empty() {
                        push_current_line(
                            &mut doc,
                            &mut current_line,
                            &mut current_line_links,
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
                current_line.push(Span::styled(
                    code.to_string(),
                    settings.theme.inline_code.add_modifier(active_mods),
                ));
            }
            Event::TaskListMarker(checked) => {
                let mark = if checked { "[x] " } else { "[ ] " };
                current_line.push(Span::styled(mark.to_string(), settings.theme.list_marker));
            }
            Event::Text(text) => {
                if in_code {
                    code_buf.push_str(&text);
                    continue;
                }
                let mut style = if quote_depth > 0 {
                    settings.theme.quote
                } else {
                    settings.theme.normal
                };
                style = style.add_modifier(active_mods);
                if let Some(url) = pending_link.clone() {
                    let link_text = text.to_string();
                    let start = current_line
                        .iter()
                        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
                        .sum::<usize>();
                    let link_width = UnicodeWidthStr::width(link_text.as_str());
                    current_line.push(Span::styled(
                        link_text.clone(),
                        settings.theme.link.add_modifier(active_mods),
                    ));
                    if !doc.links.iter().any(|l| l == &url) {
                        doc.links.push(url.clone());
                    }
                    if link_width > 0 {
                        current_line_links.push(LinkRange {
                            start,
                            end: start + link_width,
                            url,
                        });
                    }
                } else {
                    current_line.push(Span::styled(text.to_string(), style));
                }
            }
            Event::SoftBreak => {
                push_current_line(
                    &mut doc,
                    &mut current_line,
                    &mut current_line_links,
                    current_line_kind(quote_depth),
                );
            }
            Event::HardBreak => {
                push_current_line(
                    &mut doc,
                    &mut current_line,
                    &mut current_line_links,
                    current_line_kind(quote_depth),
                );
            }
            Event::Rule => {
                if !current_line.is_empty() {
                    push_current_line(
                        &mut doc,
                        &mut current_line,
                        &mut current_line_links,
                        current_line_kind(quote_depth),
                    );
                }
                doc.lines.push(RenderLine {
                    line: Line::from("─".repeat(settings.width as usize)),
                    kind: LineKind::Normal,
                    link_url: None,
                    link_ranges: vec![],
                    code_block_index: None,
                    heading_level: None,
                });
            }
            Event::InlineMath(text) => {
                current_line.push(Span::styled(
                    calcifer::math::render_inline(&text),
                    settings.theme.normal,
                ));
            }
            Event::DisplayMath(text) => {
                if !current_line.is_empty() {
                    push_current_line(
                        &mut doc,
                        &mut current_line,
                        &mut current_line_links,
                        current_line_kind(quote_depth),
                    );
                }
                let math_lines = calcifer::math::render_block(&text);
                let trimmed_math_lines = trim_common_leading_spaces(&math_lines);
                let block_style = settings
                    .theme
                    .normal
                    .patch(settings.theme.code)
                    .bg(Color::Reset);
                let math_style = settings.theme.normal.patch(block_style);
                let padded_math_lines = pad_block_lines(
                    trimmed_math_lines
                        .into_iter()
                        .map(|l| Line::from(vec![Span::styled(l, math_style)]))
                        .collect::<Vec<_>>(),
                    None,
                    block_style,
                    block_style,
                );
                for rendered in padded_math_lines {
                    doc.lines.push(RenderLine {
                        line: rendered.line,
                        kind: LineKind::Code,
                        link_url: None,
                        link_ranges: vec![],
                        code_block_index: None,
                        heading_level: None,
                    });
                }
                doc.lines.push(RenderLine {
                    line: Line::from(""),
                    kind: LineKind::Normal,
                    link_url: None,
                    link_ranges: vec![],
                    code_block_index: None,
                    heading_level: None,
                });
            }
            _ => {}
        }
    }

    if !current_line.is_empty() {
        push_current_line(
            &mut doc,
            &mut current_line,
            &mut current_line_links,
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
    kind: LineKind,
) {
    if spans.is_empty() {
        line_links.clear();
        doc.lines.push(RenderLine {
            line: Line::from(""),
            kind: LineKind::Normal,
            link_url: None,
            link_ranges: vec![],
            code_block_index: None,
            heading_level: None,
        });
    } else {
        let text_joined = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
        let final_kind = if matches!(kind, LineKind::Quote) {
            LineKind::Quote
        } else if text_joined.trim_start().starts_with('>') {
            LineKind::Quote
        } else {
            LineKind::Normal
        };
        doc.lines.push(RenderLine {
            line: Line::from(std::mem::take(spans)),
            kind: final_kind,
            link_url: line_links.first().map(|l| l.url.clone()),
            link_ranges: std::mem::take(line_links),
            code_block_index: None,
            heading_level: None,
        });
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
        if text.trim().is_empty() && last.kind != LineKind::Code {
            lines.pop();
        } else {
            break;
        }
    }
}

#[derive(Default)]
struct TableState {
    alignments: Vec<pulldown_cmark::Alignment>,
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_cell: String,
    header_rows: usize,
}

impl TableState {
    fn new(alignments: Vec<pulldown_cmark::Alignment>) -> Self {
        Self {
            alignments,
            ..Default::default()
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ListState {
    ordered: bool,
    next: u64,
}

fn render_table_lines(table: &TableState, normal: Style, marker: Style) -> Vec<Line<'static>> {
    use unicode_width::UnicodeWidthStr;
    if table.rows.is_empty() {
        return vec![];
    }
    let cols = table.rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return vec![];
    }
    let mut widths = vec![0usize; cols];
    for row in &table.rows {
        for (c, cell) in row.iter().enumerate() {
            widths[c] = widths[c].max(UnicodeWidthStr::width(cell.as_str()));
        }
    }
    for w in &mut widths {
        *w = (*w).max(1);
    }

    let top = border_line('┌', '┬', '┐', &widths);
    let mid = border_line('├', '┼', '┤', &widths);
    let bot = border_line('└', '┴', '┘', &widths);

    let header_rows = if table.header_rows == 0 && !table.rows.is_empty() {
        1
    } else {
        table.header_rows.min(table.rows.len())
    };

    let mut out = vec![Line::from(vec![Span::styled(top, marker)])];
    for (idx, row) in table.rows.iter().enumerate() {
        let mut spans = vec![Span::styled("│ ".to_string(), marker)];
        for c in 0..cols {
            let cell = row.get(c).cloned().unwrap_or_default();
            let aligned = align_cell(
                &cell,
                widths[c],
                table
                    .alignments
                    .get(c)
                    .copied()
                    .unwrap_or(pulldown_cmark::Alignment::Left),
            );
            spans.push(Span::styled(aligned, normal));
            if c + 1 == cols {
                spans.push(Span::styled(" │".to_string(), marker));
            } else {
                spans.push(Span::styled(" │ ".to_string(), marker));
            }
        }
        out.push(Line::from(spans));
        if header_rows > 0 && idx + 1 == header_rows {
            out.push(Line::from(vec![Span::styled(mid.clone(), marker)]));
        }
    }
    out.push(Line::from(vec![Span::styled(bot, marker)]));
    out
}

fn trim_common_leading_spaces(lines: &[String]) -> Vec<String> {
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

fn border_line(left: char, mid: char, right: char, widths: &[usize]) -> String {
    let mut s = String::new();
    s.push(left);
    for (i, w) in widths.iter().enumerate() {
        s.push_str(&"─".repeat(*w + 2));
        if i + 1 == widths.len() {
            s.push(right);
        } else {
            s.push(mid);
        }
    }
    s
}

fn align_cell(text: &str, width: usize, align: pulldown_cmark::Alignment) -> String {
    use unicode_width::UnicodeWidthStr;
    let cur = UnicodeWidthStr::width(text);
    if cur >= width {
        return text.to_string();
    }
    let pad = width - cur;
    match align {
        pulldown_cmark::Alignment::Center => {
            let l = pad / 2;
            let r = pad - l;
            format!("{}{}{}", " ".repeat(l), text, " ".repeat(r))
        }
        pulldown_cmark::Alignment::Right => format!("{}{}", " ".repeat(pad), text),
        _ => format!("{}{}", text, " ".repeat(pad)),
    }
}

fn extract_front_matter(markdown: &str) -> (Option<FrontMatter>, String) {
    let mut lines = markdown.lines();
    let Some(first) = lines.next() else {
        return (None, markdown.to_string());
    };
    if first.trim() != "---" {
        return (None, markdown.to_string());
    }
    let mut fm_buf = String::new();
    let mut consumed = first.len() + 1;
    let mut closed = false;
    for line in markdown[first.len() + 1..].lines() {
        consumed += line.len() + 1;
        if line.trim() == "---" {
            closed = true;
            break;
        }
        fm_buf.push_str(line);
        fm_buf.push('\n');
    }
    if !closed {
        return (None, markdown.to_string());
    }

    let mut fm = FrontMatter::default();
    for raw in fm_buf.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_ascii_lowercase();
            let val = v.trim().trim_matches('"').trim_matches('\'').to_string();
            match key.as_str() {
                "title" => fm.title = Some(val),
                "description" => fm.description = Some(val),
                _ => {}
            }
        }
    }

    let body = markdown.get(consumed..).unwrap_or("").to_string();
    (Some(fm), body)
}

fn preprocess_block_title_attributes(markdown: &str) -> String {
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
                    if i + 1 < lines.len() {
                        if let Some(title) = parse_jekyll_block_title(lines[i + 1]) {
                            block.insert(1, format!("{BLOCK_TITLE_MARKER}{title}"));
                            i += 1;
                        }
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

fn parse_fence_start(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let ch = trimmed.chars().next()?;
    if ch != '`' && ch != '~' {
        return None;
    }
    let count = trimmed.chars().take_while(|c| *c == ch).count();
    if count >= 3 { Some((ch, count)) } else { None }
}

fn is_fence_end(line: &str, fence_char: char, fence_count: usize) -> bool {
    let trimmed = line.trim_start();
    let run = trimmed.chars().take_while(|c| *c == fence_char).count();
    if run < fence_count {
        return false;
    }
    trimmed[run..].trim().is_empty()
}

fn parse_jekyll_block_title(line: &str) -> Option<String> {
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

fn extract_block_title_marker(line: &str) -> Option<&str> {
    line.strip_prefix(BLOCK_TITLE_MARKER)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn split_block_title_marker(code: &str) -> (Option<String>, String) {
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

fn heading_prefix(level: Option<HeadingLevel>) -> String {
    let n = match level {
        Some(HeadingLevel::H1) => 1,
        Some(HeadingLevel::H2) => 2,
        Some(HeadingLevel::H3) => 3,
        Some(HeadingLevel::H4) => 4,
        Some(HeadingLevel::H5) => 5,
        Some(HeadingLevel::H6) => 6,
        None => 0,
    };
    if n == 0 {
        String::new()
    } else {
        format!("{} ", "#".repeat(n))
    }
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

fn exact_semantic_theme(app_theme: &AppTheme) -> Theme {
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

    let mut settings = ThemeSettings::default();
    settings.foreground = Some(to_syn_color(p.white));
    settings.background = app_theme.code.bg.map(to_syn_color);
    settings.caret = app_theme.cursor_line.bg.map(to_syn_color);

    Theme {
        name: Some("calci-exact-semantic".to_string()),
        author: Some("calci".to_string()),
        settings,
        scopes,
    }
}

fn theme_item(scope_selector: &str, color: Color, italic: bool, bold: bool) -> ThemeItem {
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

fn to_syn_color(c: Color) -> SynColor {
    let (r, g, b) = color_to_rgb(c);
    SynColor { r, g, b, a: 0xFF }
}

fn highlight_code_block(
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

fn plain_code_lines(code: &str, style: Style) -> Vec<Line<'static>> {
    let mut out = code
        .lines()
        .map(|line| Line::from(vec![Span::styled(line.to_string(), style)]))
        .collect::<Vec<_>>();
    if code.ends_with('\n') {
        out.push(Line::from(vec![Span::styled(String::new(), style)]));
    }
    if out.is_empty() {
        out.push(Line::from(vec![Span::styled(String::new(), style)]));
    }
    out
}

struct BlockRenderLine {
    line: Line<'static>,
    is_code_content: bool,
}

fn pad_block_lines(
    mut lines: Vec<Line<'static>>,
    title: Option<&str>,
    pad_style: Style,
    title_style: Style,
) -> Vec<BlockRenderLine> {
    if let Some(title) = title {
        if !title.trim().is_empty() {
            lines.insert(
                0,
                Line::from(vec![Span::styled(title.to_string(), title_style)]),
            );
        }
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

fn line_display_width(line: &Line<'_>) -> usize {
    line.spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

fn color_to_rgb(color: ratatui::style::Color) -> (u8, u8, u8) {
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

fn normalize_loose_inline_math(text: &str) -> String {
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

fn looks_like_math(s: &str) -> bool {
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
    use super::*;
    use crate::theme::{AppTheme, ThemeName};

    fn settings() -> RenderSettings {
        RenderSettings {
            width: 80,
            theme: AppTheme::from_name(ThemeName::Dark),
        }
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
        assert!(joined.contains("x²"));
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
        let mut theme = AppTheme::from_name(ThemeName::Dark);
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
        assert!(doc.links.iter().any(|l| l == "https://example.com"));
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
    fn math_code_block_is_rendered_with_calcifer() {
        let md = "```math\n\\frac{a}{b}\n```";
        let doc = render_markdown(md, &settings()).expect("render");
        let has_frac_bar = doc.lines.iter().any(|l| {
            l.line
                .spans
                .iter()
                .any(|s| s.content.as_ref().contains('─'))
        });
        assert!(has_frac_bar);
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
    fn display_math_is_left_trimmed_consistently() {
        let md = "$$\na = b\nm^{e^{d}} = m\n$$";
        let doc = render_markdown(md, &settings()).expect("render");
        let math_lines: Vec<String> = doc
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
        assert!(!math_lines.is_empty());
        let min_indent = math_lines
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.chars().take_while(|c| *c == ' ').count())
            .min()
            .unwrap_or(0);
        assert_eq!(min_indent, BLOCK_PAD_X);
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
    fn list_items_render_on_separate_lines() {
        let doc = render_markdown("- a\n- b", &settings()).expect("render");
        let bullet_lines = doc
            .lines
            .iter()
            .filter(|l| {
                l.line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .starts_with("• ")
            })
            .count();
        assert_eq!(bullet_lines, 2);
    }

    #[test]
    fn front_matter_is_extracted() {
        let md = "---\ntitle: My title\ndescription: Desc\n---\n# Body";
        let (fm, body) = extract_front_matter(md);
        let fm = fm.expect("fm");
        assert_eq!(fm.title.as_deref(), Some("My title"));
        assert_eq!(fm.description.as_deref(), Some("Desc"));
        assert!(body.contains("# Body"));
    }
}

//! Visual layout: wraps rendered markdown lines into screen rows once per
//! width/document change. Drawing, scrolling and hit-testing then work on the
//! precomputed rows, so per-frame cost is proportional to the viewport, not to
//! the document.

use crate::util::char_widths;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::renderer::{LineKind, LinkRange, RenderLine};
use crate::theme::AppTheme;

pub const LINE_NUMBER_COLS: usize = 6;
pub const QUOTE_PREFIX: &str = "│ ";
pub const QUOTE_PREFIX_WIDTH: usize = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutOptions {
    pub width: usize,
    pub wrap: bool,
    pub line_numbers: bool,
    pub center_blocks: bool,
    /// Reserve two rows (at double width) for H2 so it can be drawn at 2x
    /// with kitty's text sizing protocol.
    pub heading_scale: bool,
}

/// One screen row of a rendered line. The row is drawn as:
/// `[line number col] [lead_pad] [quote bar] [hang_pad] content`.
#[derive(Clone, Debug)]
pub struct VisualRow {
    pub line_idx: usize,
    pub row_in_line: usize,
    /// Centering padding, placed before the quote bar.
    pub lead_pad: usize,
    pub quote_bar: bool,
    /// Hanging indent of wrapped list items, placed after the quote bar.
    pub hang_pad: usize,
    /// Columns before `content` starts (line numbers, pads and quote bar).
    pub prefix_width: usize,
    pub content: Vec<Span<'static>>,
    /// For each content span, the index into the source line's `link_ranges`.
    pub content_links: Vec<Option<usize>>,
    /// First row of a 2x-scaled heading; the next row is reserved for it.
    pub scaled: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DocLayout {
    pub width: usize,
    pub rows: Vec<VisualRow>,
    /// `line_start[i]` is the first row of line `i`; has `lines + 1` entries.
    pub line_start: Vec<usize>,
}

impl DocLayout {
    pub fn build(lines: &[RenderLine], opts: LayoutOptions, theme: &AppTheme) -> Self {
        let mut rows = Vec::with_capacity(lines.len());
        let mut line_start = Vec::with_capacity(lines.len() + 1);
        for (idx, line) in lines.iter().enumerate() {
            line_start.push(rows.len());
            rows.extend(layout_line(line, idx, opts, theme));
        }
        line_start.push(rows.len());
        Self {
            width: opts.width,
            rows,
            line_start,
        }
    }

    /// Whether this layout was built for `lines_len` lines at `width`.
    pub fn is_current(&self, lines_len: usize, width: usize) -> bool {
        self.width == width && self.line_start.len() == lines_len + 1
    }

    pub fn line_rows(&self, line_idx: usize) -> &[VisualRow] {
        let last = self.line_start.len().saturating_sub(1);
        let start = self
            .line_start
            .get(line_idx.min(last))
            .copied()
            .unwrap_or(0);
        let end = self
            .line_start
            .get((line_idx + 1).min(last))
            .copied()
            .unwrap_or(start);
        &self.rows[start..end]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WrapMode {
    /// Word wrap (prose).
    Word,
    /// Character wrap, for preformatted blocks.
    Char,
    /// Never wrap (images).
    None,
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    width: usize,
    style: Style,
    link: Option<usize>,
}

pub fn layout_line(
    line: &RenderLine,
    line_idx: usize,
    opts: LayoutOptions,
    theme: &AppTheme,
) -> Vec<VisualRow> {
    let width = opts.width.max(1);
    let number_cols = if opts.line_numbers {
        LINE_NUMBER_COLS
    } else {
        0
    };
    let quote_bar = line.kind == LineKind::Quote;
    let bar_cols = if quote_bar { QUOTE_PREFIX_WIDTH } else { 0 };
    let avail = width.saturating_sub(number_cols + bar_cols).max(1);
    let spans = styled_spans(line, theme);
    let total_width: usize = spans.iter().map(Span::width).sum();

    let mode = match line.kind {
        LineKind::Image => WrapMode::None,
        LineKind::Code | LineKind::Ascii | LineKind::Math | LineKind::Table => WrapMode::Char,
        LineKind::Normal | LineKind::Heading | LineKind::Quote => WrapMode::Word,
    };
    let fits = !opts.wrap || mode == WrapMode::None || total_width <= avail;

    let make_row = |row_in_line: usize,
                    lead_pad: usize,
                    hang_pad: usize,
                    content: Vec<Span<'static>>,
                    content_links: Vec<Option<usize>>| VisualRow {
        line_idx,
        row_in_line,
        lead_pad,
        quote_bar,
        hang_pad,
        prefix_width: number_cols + lead_pad + bar_cols + hang_pad,
        content,
        content_links,
        scaled: false,
    };

    let scale_heading = opts.heading_scale
        && line.kind == LineKind::Heading
        && line.heading_level == Some(2)
        && line.link_ranges.is_empty()
        && !spans
            .iter()
            .any(|s| crate::placeholder::is_placeholder(&s.content))
        && number_cols + 2 * total_width <= width;
    if scale_heading {
        let mut first = make_row(0, 0, 0, spans.clone(), vec![None; spans.len()]);
        first.scaled = true;
        let reserved = make_row(1, 0, 0, Vec::new(), Vec::new());
        return vec![first, reserved];
    }

    if fits {
        let lead_pad = center_pad(line.kind, opts, bar_cols + total_width, width);
        let (content, content_links) = if line.link_ranges.is_empty() {
            let links = vec![None; spans.len()];
            (spans, links)
        } else {
            cells_to_spans(&to_cells(&spans, &line.link_ranges))
        };
        return vec![make_row(0, lead_pad, 0, content, content_links)];
    }

    let cells = to_cells(&spans, &line.link_ranges);
    let hang = line.hang_indent.min(avail.saturating_sub(1));
    let split_at = column_index(&cells, hang);
    let (head, body) = cells.split_at(split_at);
    let body_avail = avail - hang;

    let mut rows = match mode {
        WrapMode::Word => word_wrap(body, body_avail),
        _ => char_wrap(trim_trailing_spaces(body), body_avail),
    };
    let justify = matches!(line.kind, LineKind::Normal | LineKind::Quote) && hang == 0;
    if justify && rows.len() > 1 {
        let last = rows.len() - 1;
        for row in &mut rows[..last] {
            justify_cells(row, body_avail);
        }
    }

    rows.into_iter()
        .enumerate()
        .map(|(i, row_cells)| {
            if i == 0 {
                let mut first = head.to_vec();
                first.extend(row_cells);
                let (content, links) = cells_to_spans(&first);
                make_row(0, 0, 0, content, links)
            } else {
                let (content, links) = cells_to_spans(&row_cells);
                make_row(i, 0, hang, content, links)
            }
        })
        .collect()
}

fn styled_spans(line: &RenderLine, theme: &AppTheme) -> Vec<Span<'static>> {
    let mut spans = line.line.spans.clone();
    match line.kind {
        LineKind::Heading => {
            let heading_style = match line.heading_level.unwrap_or(6) {
                1 => theme.heading_h1,
                2 => theme.heading_h2,
                3 => theme.heading_h3,
                4 => theme.heading_h4,
                _ => theme.heading_h5,
            };
            for s in spans
                .iter_mut()
                .filter(|s| !crate::placeholder::is_placeholder(&s.content))
            {
                s.style = s.style.patch(heading_style);
            }
        }
        LineKind::Quote => {
            for s in &mut spans {
                s.style = s.style.add_modifier(Modifier::ITALIC);
            }
        }
        _ => {}
    }
    spans
}

fn center_pad(kind: LineKind, opts: LayoutOptions, used: usize, width: usize) -> usize {
    // Quotes are never centered: their bar must stay in one column.
    let centerable = matches!(
        kind,
        LineKind::Table | LineKind::Math | LineKind::Ascii | LineKind::Image
    );
    if opts.center_blocks && !opts.line_numbers && centerable && used < width {
        (width - used) / 2
    } else {
        0
    }
}

fn to_cells(spans: &[Span<'static>], link_ranges: &[LinkRange]) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut col = 0usize;
    for span in spans {
        for (ch, width) in char_widths(&span.content) {
            let link = link_ranges
                .iter()
                .position(|lr| col >= lr.start && col < lr.end);
            cells.push(Cell {
                ch,
                width,
                style: span.style,
                link,
            });
            col += width;
        }
    }
    cells
}

/// One row of word-wrapped styled text, with link ranges in row columns.
#[derive(Clone, Debug, Default)]
pub struct WrappedText {
    pub spans: Vec<Span<'static>>,
    pub links: Vec<LinkRange>,
    pub width: usize,
}

/// Word-wrap styled text to `width` columns (hard-breaking longer words),
/// keeping styles and remapping link ranges onto each row. Used for table
/// cells, which are laid out by the renderer.
pub fn wrap_styled(spans: &[Span<'static>], links: &[LinkRange], width: usize) -> Vec<WrappedText> {
    let cells = to_cells(spans, links);
    word_wrap(&cells, width.max(1))
        .into_iter()
        .map(|row| {
            let row = trim_trailing_spaces(&row);
            let mut row_links: Vec<LinkRange> = Vec::new();
            let mut col = 0usize;
            for cell in row {
                if let Some(idx) = cell.link {
                    match row_links.last_mut() {
                        Some(last)
                            if last.end == col
                                && last.url == links[idx].url
                                && last.kind == links[idx].kind =>
                        {
                            last.end += cell.width;
                        }
                        _ => row_links.push(LinkRange {
                            start: col,
                            end: col + cell.width,
                            url: links[idx].url.clone(),
                            kind: links[idx].kind,
                        }),
                    }
                }
                col += cell.width;
            }
            let (spans, _) = cells_to_spans(row);
            WrappedText {
                spans,
                links: row_links,
                width: col,
            }
        })
        .collect()
}

/// Index of the first cell starting at or after display column `col`.
fn column_index(cells: &[Cell], col: usize) -> usize {
    let mut acc = 0usize;
    for (i, cell) in cells.iter().enumerate() {
        if acc >= col {
            return i;
        }
        acc += cell.width;
    }
    cells.len()
}

fn cells_width(cells: &[Cell]) -> usize {
    cells.iter().map(|c| c.width).sum()
}

fn trim_trailing_spaces(cells: &[Cell]) -> &[Cell] {
    let end = cells.iter().rposition(|c| c.ch != ' ').map_or(0, |i| i + 1);
    &cells[..end]
}

fn word_wrap(cells: &[Cell], avail: usize) -> Vec<Vec<Cell>> {
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut cur_w = 0usize;
    let mut pending: Vec<Cell> = Vec::new();
    let mut pending_w = 0usize;
    let mut i = 0usize;

    while i < cells.len() {
        let start = i;
        let is_space = cells[i].ch == ' ';
        while i < cells.len() && (cells[i].ch == ' ') == is_space {
            i += 1;
        }
        let run = &cells[start..i];
        let run_w = cells_width(run);

        if is_space {
            if rows.is_empty() && cur.is_empty() {
                // Leading spaces of the line are content (indentation).
                cur.extend_from_slice(run);
                cur_w += run_w;
            } else {
                pending.extend_from_slice(run);
                pending_w += run_w;
            }
            continue;
        }

        let has_text = cur.iter().any(|c| c.ch != ' ');
        if has_text && cur_w + pending_w + run_w <= avail {
            cur.append(&mut pending);
            cur.extend_from_slice(run);
            cur_w += pending_w + run_w;
        } else if !has_text && cur_w + run_w <= avail {
            cur.extend_from_slice(run);
            cur_w += run_w;
        } else {
            if has_text {
                rows.push(std::mem::take(&mut cur));
            } else {
                // Leading indentation alone can't hold the word: drop it.
                cur.clear();
            }
            if run_w <= avail {
                cur = run.to_vec();
                cur_w = run_w;
            } else {
                let mut chunks = char_wrap(run, avail);
                cur = chunks.pop().unwrap_or_default();
                cur_w = cells_width(&cur);
                rows.extend(chunks);
            }
        }
        pending.clear();
        pending_w = 0;
    }

    if !cur.is_empty() || rows.is_empty() {
        rows.push(cur);
    }
    rows
}

fn char_wrap(cells: &[Cell], avail: usize) -> Vec<Vec<Cell>> {
    let mut rows = Vec::new();
    let mut cur: Vec<Cell> = Vec::new();
    let mut cur_w = 0usize;
    for cell in cells {
        if cell.width > 0 && cur_w + cell.width > avail && !cur.is_empty() {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(*cell);
        cur_w += cell.width;
    }
    if !cur.is_empty() || rows.is_empty() {
        rows.push(cur);
    }
    rows
}

/// Stretch inter-word gaps so the row fills `target` columns. Rows that would
/// need more than two extra spaces per gap (or have fewer than two gaps) are
/// left ragged, since they look broken when stretched.
fn justify_cells(row: &mut Vec<Cell>, target: usize) {
    let width = cells_width(row);
    if width >= target {
        return;
    }
    let (Some(first), Some(last)) = (
        row.iter().position(|c| c.ch != ' '),
        row.iter().rposition(|c| c.ch != ' '),
    ) else {
        return;
    };
    // Gap = end index (exclusive) of each space run strictly inside the text.
    let mut gap_ends = Vec::new();
    let mut i = first;
    while i < last {
        if row[i].ch == ' ' {
            while row[i].ch == ' ' {
                i += 1;
            }
            gap_ends.push(i);
        } else {
            i += 1;
        }
    }
    let extra = target - width;
    if gap_ends.len() < 2 || extra > gap_ends.len() * 2 {
        return;
    }
    let base = extra / gap_ends.len();
    let remainder = extra % gap_ends.len();
    let mut out = Vec::with_capacity(row.len() + extra);
    let mut gap = 0usize;
    for (idx, cell) in row.iter().enumerate() {
        if gap < gap_ends.len() && idx == gap_ends[gap] {
            let filler = Cell {
                ch: ' ',
                width: 1,
                ..row[idx - 1]
            };
            let add = base + usize::from(gap < remainder);
            out.extend(std::iter::repeat_n(filler, add));
            gap += 1;
        }
        out.push(*cell);
    }
    *row = out;
}

fn cells_to_spans(cells: &[Cell]) -> (Vec<Span<'static>>, Vec<Option<usize>>) {
    let mut spans = Vec::new();
    let mut links = Vec::new();
    let mut buf = String::new();
    let mut current: Option<(Style, Option<usize>)> = None;
    for cell in cells {
        let key = (cell.style, cell.link);
        if current.is_some_and(|c| c != key) {
            let (style, link) = current.unwrap_or_default();
            spans.push(Span::styled(std::mem::take(&mut buf), style));
            links.push(link);
        }
        current = Some(key);
        buf.push(cell.ch);
    }
    if let Some((style, link)) = current {
        spans.push(Span::styled(buf, style));
        links.push(link);
    }
    (spans, links)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::{LinkRange, RangeKind};
    use ratatui::text::Line;
    use unicode_width::UnicodeWidthStr;

    fn opts(width: usize) -> LayoutOptions {
        LayoutOptions {
            width,
            wrap: true,
            line_numbers: false,
            center_blocks: false,
            heading_scale: false,
        }
    }

    fn line(kind: LineKind, spans: Vec<Span<'static>>) -> RenderLine {
        RenderLine {
            line: Line::from(spans),
            kind,
            link_ranges: vec![],
            code_block_index: None,
            image_index: None,
            heading_level: None,
            hang_indent: 0,
            task: None,
        }
    }

    fn row_text(row: &VisualRow) -> String {
        row.content.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn short_line_is_one_unjustified_row() {
        let l = line(LineKind::Normal, vec![Span::raw("short line")]);
        let rows = layout_line(&l, 0, opts(24), &AppTheme::default());
        assert_eq!(rows.len(), 1);
        assert_eq!(row_text(&rows[0]), "short line");
    }

    #[test]
    fn wrapped_paragraph_is_justified_except_last_row() {
        let text = "This is a very long line that should wrap and justify nicely";
        let l = line(LineKind::Normal, vec![Span::raw(text)]);
        let rows = layout_line(&l, 0, opts(24), &AppTheme::default());
        assert!(rows.len() > 1);
        for row in &rows[..rows.len() - 1] {
            assert_eq!(UnicodeWidthStr::width(row_text(row).as_str()), 24);
        }
        let last = row_text(rows.last().unwrap());
        assert!(UnicodeWidthStr::width(last.as_str()) <= 24);
        assert!(!last.contains("  "));
    }

    #[test]
    fn justification_keeps_mixed_styles_and_links() {
        let bold = Style::default().add_modifier(Modifier::BOLD);
        let mut l = line(
            LineKind::Normal,
            vec![
                Span::raw("alpha beta gamma "),
                Span::styled("delta", bold),
                Span::raw(" epsilon zeta eta theta iota kappa"),
            ],
        );
        l.link_ranges = vec![LinkRange {
            start: 6,
            end: 10,
            url: "https://beta.test".into(),
            kind: RangeKind::Url,
        }];
        let rows = layout_line(&l, 0, opts(30), &AppTheme::default());
        assert!(rows.len() > 1);
        assert_eq!(UnicodeWidthStr::width(row_text(&rows[0]).as_str()), 30);
        let first = &rows[0];
        let link_span = first
            .content
            .iter()
            .zip(&first.content_links)
            .find(|(_, link)| link.is_some())
            .map(|(s, _)| s.content.to_string());
        assert_eq!(link_span.as_deref(), Some("beta"));
        assert!(
            rows.iter()
                .flat_map(|r| &r.content)
                .any(|s| s.content == "delta" && s.style == bold)
        );
    }

    #[test]
    fn overstretched_rows_stay_ragged() {
        let l = line(
            LineKind::Normal,
            vec![Span::raw("alpha beta averyveryverylongword")],
        );
        let rows = layout_line(&l, 0, opts(24), &AppTheme::default());
        assert_eq!(row_text(&rows[0]), "alpha beta");
    }

    #[test]
    fn quote_rows_word_wrap_and_carry_the_bar() {
        let l = line(LineKind::Quote, vec![Span::raw("one two three four")]);
        let rows = layout_line(&l, 0, opts(12), &AppTheme::default());
        assert!(rows.len() > 1);
        for row in &rows {
            assert!(row.quote_bar);
            assert_eq!(row.prefix_width, QUOTE_PREFIX_WIDTH);
            assert!(!row_text(row).starts_with(' '));
        }
        assert_eq!(row_text(&rows[0]).trim_end(), "one two");
    }

    #[test]
    fn long_words_are_hard_broken() {
        let l = line(LineKind::Quote, vec![Span::raw("abcdefghij")]);
        let rows = layout_line(&l, 0, opts(6), &AppTheme::default());
        let texts: Vec<_> = rows.iter().map(row_text).collect();
        assert_eq!(texts, vec!["abcd", "efgh", "ij"]);
    }

    #[test]
    fn list_items_get_hanging_indent_and_no_justification() {
        let mut l = line(
            LineKind::Normal,
            vec![
                Span::raw("  "),
                Span::raw("• "),
                Span::raw("one two three four five six"),
            ],
        );
        l.hang_indent = 4;
        let rows = layout_line(&l, 0, opts(16), &AppTheme::default());
        assert!(rows.len() > 1);
        assert_eq!(row_text(&rows[0]), "  • one two");
        for row in &rows[1..] {
            assert_eq!(row.hang_pad, 4);
            assert_eq!(row.prefix_width, 4);
            assert!(!row_text(row).contains("  "));
        }
    }

    #[test]
    fn line_numbers_reserve_columns_on_every_row() {
        let l = line(LineKind::Normal, vec![Span::raw("aaa bbb ccc ddd")]);
        let mut o = opts(14);
        o.line_numbers = true;
        let rows = layout_line(&l, 0, o, &AppTheme::default());
        assert!(rows.len() > 1);
        assert!(rows.iter().all(|r| r.prefix_width == LINE_NUMBER_COLS));
    }

    #[test]
    fn short_blocks_are_centered_when_enabled() {
        let l = line(LineKind::Table, vec![Span::raw("abcd")]);
        let mut o = opts(20);
        o.center_blocks = true;
        let rows = layout_line(&l, 0, o, &AppTheme::default());
        assert_eq!(rows[0].lead_pad, 8);
        assert_eq!(rows[0].prefix_width, 8);
    }

    #[test]
    fn no_wrap_keeps_one_row_per_line() {
        let l = line(LineKind::Normal, vec![Span::raw("aaa bbb ccc ddd eee")]);
        let mut o = opts(8);
        o.wrap = false;
        assert_eq!(layout_line(&l, 0, o, &AppTheme::default()).len(), 1);
    }

    #[test]
    fn scaled_h2_reserves_two_rows_only_when_it_fits_doubled() {
        let mut l = line(LineKind::Heading, vec![Span::raw("Section")]);
        l.heading_level = Some(2);
        let mut o = opts(20);
        o.heading_scale = true;
        let rows = layout_line(&l, 0, o, &AppTheme::default());
        assert_eq!(rows.len(), 2);
        assert!(rows[0].scaled && !rows[1].scaled);
        assert!(rows[1].content.is_empty());
        // 2 * 7 > 10 columns: falls back to a normal heading row.
        o.width = 10;
        let rows = layout_line(&l, 0, o, &AppTheme::default());
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].scaled);
        // Only H2 is scaled.
        l.heading_level = Some(3);
        o.width = 40;
        assert_eq!(layout_line(&l, 0, o, &AppTheme::default()).len(), 1);
    }

    #[test]
    fn doc_layout_indexes_rows_per_line() {
        let lines = vec![
            line(LineKind::Normal, vec![Span::raw("aaa bbb ccc ddd eee fff")]),
            line(LineKind::Normal, vec![Span::raw("x")]),
        ];
        let layout = DocLayout::build(&lines, opts(8), &AppTheme::default());
        let first_rows = layout.line_rows(0).len();
        assert!(first_rows > 1);
        assert_eq!(layout.line_start, vec![0, first_rows, first_rows + 1]);
        assert_eq!(layout.line_rows(1).len(), 1);
        assert!(layout.is_current(2, 8));
        assert!(!layout.is_current(2, 9));
    }
}

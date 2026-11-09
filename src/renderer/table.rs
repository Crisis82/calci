//! Tables: styled cells, column fitting to the window and soft-wrapped rows.

use super::*;
use crate::util::char_widths;

/// Styled content of one table cell, with link ranges in cell columns.
#[derive(Clone, Debug, Default)]
pub(super) struct TableCell {
    pub(super) spans: Vec<Span<'static>>,
    pub(super) links: Vec<LinkRange>,
}

impl TableCell {
    pub(super) fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    pub(super) fn push(&mut self, text: String, style: Style) {
        if !text.is_empty() {
            self.spans.push(Span::styled(text, style));
        }
    }

    /// Push text that opens `target` (link) or copies it (inline code).
    pub(super) fn push_range(&mut self, text: String, style: Style, target: &str, kind: RangeKind) {
        let start = self.width();
        let width = UnicodeWidthStr::width(text.as_str());
        self.push(text, style);
        if width > 0 {
            self.links.push(LinkRange {
                start,
                end: start + width,
                url: target.to_string(),
                kind,
            });
        }
    }
}

#[derive(Default)]
pub(super) struct TableState {
    pub(super) alignments: Vec<pulldown_cmark::Alignment>,
    pub(super) rows: Vec<Vec<TableCell>>,
    pub(super) current_row: Vec<TableCell>,
    pub(super) current_cell: TableCell,
    pub(super) header_rows: usize,
}

impl TableState {
    pub(super) fn new(alignments: Vec<pulldown_cmark::Alignment>) -> Self {
        Self {
            alignments,
            ..Default::default()
        }
    }
}

/// Narrowest a column gets when even unbroken words can't fit.
pub(super) const TABLE_MIN_SQUEEZED_COL: usize = 3;

/// Lay a table out within `settings.width` columns: columns keep their
/// natural width when possible, otherwise shrink and soft-wrap cell text.
pub(super) fn render_table(table: &TableState, settings: &RenderSettings) -> Vec<RenderLine> {
    let cols = table.rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return vec![];
    }
    let marker = settings.theme.list_marker;
    let normal = settings.theme.normal;

    let words: Vec<Vec<CellWords>> = table
        .rows
        .iter()
        .map(|row| {
            (0..cols)
                .map(|c| row.get(c).map(CellWords::of).unwrap_or_default())
                .collect()
        })
        .collect();
    let widths = fit_column_widths(&words, settings.width as usize);

    let empty = TableCell::default();
    let wrapped: Vec<Vec<Vec<crate::layout::WrappedText>>> = table
        .rows
        .iter()
        .map(|row| {
            (0..cols)
                .map(|c| {
                    let cell = row.get(c).unwrap_or(&empty);
                    crate::layout::wrap_styled(&cell.spans, &cell.links, widths[c])
                })
                .collect()
        })
        .collect();

    let header_rows = if table.header_rows == 0 {
        1
    } else {
        table.header_rows.min(table.rows.len())
    };
    let any_wrapped = wrapped.iter().flatten().any(|lines| lines.len() > 1);
    let separate_body_rows = match settings.table_row_separator {
        TableRowSeparator::Never => false,
        TableRowSeparator::Wrapped => any_wrapped,
        TableRowSeparator::Always => true,
    };

    let border = |left, mid, right| {
        table_line(
            vec![Span::styled(border_line(left, mid, right, &widths), marker)],
            vec![],
        )
    };
    let mut out = vec![border('┌', '┬', '┐')];
    for (idx, row) in wrapped.iter().enumerate() {
        let height = row.iter().map(Vec::len).max().unwrap_or(1).max(1);
        for li in 0..height {
            let mut spans = vec![Span::styled("│ ".to_string(), marker)];
            let mut links = Vec::new();
            let mut col = 2usize;
            for (c, cell_lines) in row.iter().enumerate() {
                let piece = cell_lines.get(li);
                let piece_width = piece.map_or(0, |p| p.width);
                let pad = widths[c].saturating_sub(piece_width);
                let left = match table.alignments.get(c) {
                    Some(pulldown_cmark::Alignment::Center) => pad / 2,
                    Some(pulldown_cmark::Alignment::Right) => pad,
                    _ => 0,
                };
                if left > 0 {
                    spans.push(Span::styled(" ".repeat(left), normal));
                }
                col += left;
                if let Some(piece) = piece {
                    links.extend(piece.links.iter().map(|lr| LinkRange {
                        start: lr.start + col,
                        end: lr.end + col,
                        url: lr.url.clone(),
                        kind: lr.kind,
                    }));
                    spans.extend(piece.spans.iter().cloned());
                }
                col += piece_width;
                if pad > left {
                    spans.push(Span::styled(" ".repeat(pad - left), normal));
                }
                col += pad - left;
                let sep = if c + 1 == cols { " │" } else { " │ " };
                spans.push(Span::styled(sep.to_string(), marker));
                col += UnicodeWidthStr::width(sep);
            }
            out.push(table_line(spans, links));
        }
        let is_last = idx + 1 == wrapped.len();
        let header_end = idx + 1 == header_rows;
        if !is_last && (header_end || (separate_body_rows && idx >= header_rows)) {
            out.push(border('├', '┼', '┤'));
        }
    }
    out.push(border('└', '┴', '┘'));
    out
}

pub(super) fn table_line(spans: Vec<Span<'static>>, links: Vec<LinkRange>) -> RenderLine {
    RenderLine {
        line: Line::from(spans),
        kind: LineKind::Table,
        link_ranges: links,
        code_block_index: None,
        image_index: None,
        heading_level: None,
        hang_indent: 0,
        task: None,
    }
}

pub(super) fn border_line(left: char, mid: char, right: char, widths: &[usize]) -> String {
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

/// Word layout of one cell, used to predict wrapping without building spans.
#[derive(Clone, Debug, Default)]
pub(super) struct CellWords {
    /// (width of the spaces before the word, width of the word)
    words: Vec<(usize, usize)>,
    width: usize,
}

impl CellWords {
    pub(super) fn of(cell: &TableCell) -> Self {
        let text: String = cell.spans.iter().map(|s| s.content.as_ref()).collect();
        let mut words = Vec::new();
        let mut space = 0usize;
        let mut word = 0usize;
        for (ch, w) in char_widths(&text) {
            if ch == ' ' {
                if word > 0 {
                    words.push((space, word));
                    space = 0;
                    word = 0;
                }
                space += w;
            } else {
                word += w;
            }
        }
        if word > 0 {
            words.push((space, word));
        }
        if let Some(first) = words.first_mut() {
            first.0 = 0;
        }
        Self {
            words,
            width: cell.width(),
        }
    }

    fn longest_word(&self) -> usize {
        self.words.iter().map(|&(_, w)| w).max().unwrap_or(0)
    }

    /// Lines this cell takes when word-wrapped to `width` columns; mirrors
    /// `layout::wrap_styled` (greedy word wrap, hard-breaking long words).
    pub(super) fn lines_at(&self, width: usize) -> usize {
        let width = width.max(1);
        let mut lines = 1usize;
        let mut cur = 0usize;
        for &(space, word) in &self.words {
            if cur > 0 && cur + space + word <= width {
                cur += space + word;
                continue;
            }
            if cur > 0 {
                lines += 1;
            }
            if word <= width {
                cur = word;
            } else {
                let chunks = word.div_ceil(width);
                lines += chunks - 1;
                cur = word - (chunks - 1) * width;
            }
        }
        lines
    }
}

/// Column widths for a table that must fit in `total` columns (borders and
/// padding included).
///
/// Columns start at their natural width. While the table is too wide, one
/// column loses one cell of width at a time: the column whose shrinking adds
/// the fewest lines to the table (then the fewest wrapped cell lines, then
/// the widest column). Whole words are kept intact first; only if they
/// can't all fit are columns squeezed to `TABLE_MIN_SQUEEZED_COL` and, if
/// still needed, to a single column, breaking words.
/// Cost key (height increase, wrapped-line increase, -width), the column,
/// and that column's per-row line counts after shrinking it by one.
type ShrinkCandidate = ((usize, usize, isize), usize, Vec<usize>);

pub(super) fn fit_column_widths(rows: &[Vec<CellWords>], total: usize) -> Vec<usize> {
    let n = rows.iter().map(Vec::len).max().unwrap_or(0);
    if n == 0 {
        return Vec::new();
    }
    let column = |c: usize| rows.iter().filter_map(move |row| row.get(c));
    let natural: Vec<usize> = (0..n)
        .map(|c| column(c).map(|cell| cell.width).max().unwrap_or(0).max(1))
        .collect();
    let overhead = 3 * n + 1;
    let avail = total.saturating_sub(overhead).max(n);
    if natural.iter().sum::<usize>() <= avail {
        return natural;
    }

    let mut widths = natural.clone();
    // lines[r][c]: lines of cell (r, c) at the current width.
    let mut lines: Vec<Vec<usize>> = rows
        .iter()
        .map(|row| {
            (0..n)
                .map(|c| row.get(c).map_or(1, |cell| cell.lines_at(widths[c])))
                .collect()
        })
        .collect();

    let whole_words: Vec<usize> = (0..n)
        .map(|c| {
            column(c)
                .map(CellWords::longest_word)
                .max()
                .unwrap_or(0)
                .clamp(1, natural[c])
        })
        .collect();
    let squeezed: Vec<usize> = natural
        .iter()
        .map(|w| (*w).clamp(1, TABLE_MIN_SQUEEZED_COL))
        .collect();

    for floor in [whole_words, squeezed, vec![1; n]] {
        while widths.iter().sum::<usize>() > avail {
            let mut best: Option<ShrinkCandidate> = None;
            for c in (0..n).filter(|&c| widths[c] > floor[c]) {
                let candidate: Vec<usize> = rows
                    .iter()
                    .map(|row| row.get(c).map_or(1, |cell| cell.lines_at(widths[c] - 1)))
                    .collect();
                let mut height_delta = 0usize;
                let mut lines_delta = 0usize;
                for (r, new) in candidate.iter().enumerate() {
                    let old_height = lines[r].iter().copied().max().unwrap_or(1);
                    let others = (0..n)
                        .filter(|&o| o != c)
                        .map(|o| lines[r][o])
                        .max()
                        .unwrap_or(1);
                    height_delta += (*new).max(others).saturating_sub(old_height);
                    lines_delta += new.saturating_sub(lines[r][c]);
                }
                let key = (height_delta, lines_delta, -(widths[c] as isize));
                if best.as_ref().is_none_or(|(k, _, _)| key < *k) {
                    best = Some((key, c, candidate));
                }
            }
            let Some((_, c, candidate)) = best else {
                break;
            };
            widths[c] -= 1;
            for (r, new) in candidate.into_iter().enumerate() {
                lines[r][c] = new;
            }
        }
    }
    widths
}

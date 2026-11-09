//! Drawing the pager: body rows, status line and popups.

use super::*;

pub fn draw(frame: &mut Frame<'_>, state: &AppState) {
    let area = frame.area();
    let has_top_title = state.top_frontmatter_title().is_some();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if has_top_title { 1 } else { 0 }),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    let top_area = chunks[0];
    let content_chunk = chunks[1];
    let status_chunk = chunks[2];

    if has_top_title {
        draw_top_title(frame, top_area, state);
    }
    let body_area = Rect {
        x: content_chunk.x.saturating_add(WINDOW_PAD_X),
        y: content_chunk.y.saturating_add(WINDOW_PAD_TOP),
        width: content_chunk.width.saturating_sub(WINDOW_PAD_X * 2),
        height: content_chunk.height.saturating_sub(WINDOW_PAD_TOP),
    };
    let status_area = Rect {
        x: status_chunk.x.saturating_add(WINDOW_PAD_X),
        y: status_chunk.y,
        width: status_chunk.width.saturating_sub(WINDOW_PAD_X * 2),
        height: status_chunk.height,
    };
    draw_body(frame, body_area, state);
    draw_status(frame, status_area, body_area.height as usize, state);
    draw_overlay(frame, area, state);
}

pub(super) fn draw_top_title(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let Some(title) = state.top_frontmatter_title() else {
        return;
    };
    let centered = centered_text(title, area.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            centered,
            state.theme.heading_h1,
        )])),
        area,
    );
}

pub(super) fn draw_body(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let height = area.height as usize;
    let rows = &state.layout.rows;
    let start = state.offset.min(rows.len());
    let end = (start + height).min(rows.len());
    let lines: Vec<Line> = rows[start..end]
        .iter()
        .map(|row| compose_visual_row(state, row))
        .collect();
    frame.render_widget(Paragraph::new(lines).style(state.theme.normal), area);

    // Leave the cells of 2x headings to the terminal (see
    // `render_scaled_headings`). The sentinel symbol guarantees ratatui
    // repaints them once they stop being skipped.
    let buf = frame.buffer_mut();
    for (x, y, text, _) in state.visible_scaled_headings(area) {
        let cols = 2 * UnicodeWidthStr::width(text.as_str()) as u16;
        for dy in 0..2 {
            for dx in 0..cols {
                let pos = (x + dx, y + dy);
                if area.contains(pos.into()) {
                    let cell = &mut buf[pos];
                    cell.set_symbol(SKIPPED_CELL_SENTINEL);
                    cell.skip = true;
                }
            }
        }
    }
}

/// Stored in skipped cells; never printed. Any real content differs from it,
/// so ratatui's diff always repaints a cell after it stops being skipped.
const SKIPPED_CELL_SENTINEL: &str = "\u{FFFF}";

/// Build the on-screen line for one layout row, applying the per-frame state
/// (hover, cursor line, search) that the cached layout doesn't contain.
pub(super) fn compose_visual_row(state: &AppState, row: &VisualRow) -> Line<'static> {
    let Some(line) = state.doc.lines.get(row.line_idx) else {
        return Line::default();
    };
    let selected = state.line_highlight && row.line_idx == state.selected_line;
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(row.content.len() + 4);

    if state.line_numbers {
        let number = if row.row_in_line == 0 {
            format!("{:>5} ", row.line_idx + 1)
        } else {
            " ".repeat(LINE_NUMBER_COLS)
        };
        spans.push(Span::styled(number, state.theme.line_number));
    }
    if row.lead_pad > 0 {
        spans.push(Span::raw(" ".repeat(row.lead_pad)));
    }
    if row.quote_bar {
        let mut bar_style = state.theme.list_marker.remove_modifier(Modifier::BOLD);
        if selected {
            bar_style = bar_style.patch(state.theme.cursor_line);
        }
        spans.push(Span::styled(QUOTE_PREFIX.to_string(), bar_style));
    }
    if row.hang_pad > 0 {
        spans.push(Span::raw(" ".repeat(row.hang_pad)));
    }

    let mut content = row.content.clone();
    match state.hover_target {
        Some((hover_line, HoverTarget::Link(link_idx))) if hover_line == row.line_idx => {
            let bright = brighten_color(
                state
                    .theme
                    .link
                    .fg
                    .unwrap_or(state.theme.normal.fg.unwrap_or(Color::White)),
            );
            for (span, link) in content.iter_mut().zip(&row.content_links) {
                if *link == Some(link_idx) && !crate::placeholder::is_placeholder(&span.content) {
                    span.style = span.style.fg(bright);
                }
            }
        }
        Some((hover_line, HoverTarget::InlineCode(link_idx))) if hover_line == row.line_idx => {
            if let Some(tint) = state.theme.cursor_line.bg {
                for (span, link) in content.iter_mut().zip(&row.content_links) {
                    if *link == Some(link_idx) {
                        span.style = span.style.bg(tint);
                    }
                }
            }
        }
        Some((
            _,
            HoverTarget::Code {
                lines: (start, end),
                ..
            },
        )) if (start..end).contains(&row.line_idx) => {
            if let Some(tint) = state.theme.cursor_line.bg {
                for span in &mut content {
                    span.style = span.style.bg(tint);
                }
            }
        }
        _ => {}
    }
    if selected {
        for span in &mut content {
            span.style = span.style.patch(state.theme.cursor_line);
        }
    }
    if !state.search_query.is_empty() && line.kind != LineKind::Image {
        let mark = if state.search_hits.get(state.search_index) == Some(&row.line_idx) {
            state.theme.search_current
        } else {
            state.theme.search_hit
        };
        content = highlight_spans_for_search(&content, &state.search_query, mark);
    }
    spans.extend(content);
    Line::from(spans)
}

pub(super) fn draw_status(frame: &mut Frame<'_>, area: Rect, body_height: usize, state: &AppState) {
    let width = area.width as usize;
    if width == 0 {
        return;
    }
    let (left, style) = if state.mode == Mode::Search {
        let left = if state.search_hits.is_empty() {
            format!("/{}", state.search_query)
        } else {
            format!(
                "/{}  {}/{}",
                state.search_query,
                state.search_index + 1,
                state.search_hits.len()
            )
        };
        (left, state.theme.status)
    } else if state.status_is_error && !state.status.is_empty() {
        (state.status.clone(), state.theme.status_error)
    } else if !state.status.is_empty() {
        (state.status.clone(), state.theme.status)
    } else if let Some(hint) = state.hover_hint() {
        (hint, state.theme.status)
    } else {
        let source = state
            .source_path
            .as_ref()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "[stdin]".to_string());
        (source, state.theme.status)
    };
    let center = status_progress_text(state, body_height.max(1));
    let line = compose_status_line_with_center(&left, &center, "Help ?", width);
    let style = if state.status_is_error && !state.status.is_empty() {
        style
    } else {
        style.patch(state.theme.line_number)
    };
    frame.render_widget(Paragraph::new(line).style(style), area);
}

pub(super) fn status_progress_text(state: &AppState, body_height: usize) -> String {
    let content_rows = state
        .content_visual_rows(state.viewport_width.max(1))
        .max(1);
    let visible_bottom = state.offset + body_height.saturating_sub(1);
    let current_row = visible_bottom.min(content_rows.saturating_sub(1)) + 1;
    let pct = (current_row * 100 / content_rows).min(100);
    if state.line_highlight {
        let cur_line = state.selected_line.saturating_add(1);
        let total_lines = state.doc.lines.len().max(1);
        format!("Ln {cur_line}/{total_lines}  {pct:>3}%")
    } else {
        format!("{pct:>3}%")
    }
}

pub(super) fn compose_status_line(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let right_w = UnicodeWidthStr::width(right);
    if right_w >= width {
        return truncate_with_ellipsis(right, width);
    }
    let left_budget = width.saturating_sub(right_w + 1);
    let left_fit = truncate_with_ellipsis(left, left_budget);
    let left_w = UnicodeWidthStr::width(left_fit.as_str());
    let gap = width.saturating_sub(left_w + right_w);
    format!("{left_fit}{}{}", " ".repeat(gap), right)
}

pub(super) fn compose_status_line_with_center(
    left: &str,
    center: &str,
    right: &str,
    width: usize,
) -> String {
    let base = compose_status_line(left, right, width);
    if width == 0 {
        return base;
    }
    let center_fit = truncate_with_ellipsis(center, width);
    let center_chars: Vec<char> = center_fit.chars().collect();
    if center_chars.is_empty() {
        return base;
    }
    let mut cells: Vec<char> = base.chars().collect();
    if cells.len() < width {
        cells.extend(std::iter::repeat_n(' ', width - cells.len()));
    } else if cells.len() > width {
        cells.truncate(width);
    }
    let center_len = center_chars.len().min(width);
    let center_start = (width.saturating_sub(center_len)) / 2;
    for (i, ch) in center_chars.into_iter().take(center_len).enumerate() {
        cells[center_start + i] = ch;
    }
    cells.into_iter().collect()
}

pub(super) fn draw_overlay(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let Some(overlay) = state.overlay() else {
        return;
    };
    let (title, mut body, popup_w, popup_h): (&str, Vec<Line>, u16, u16) = match overlay {
        Overlay::Help => {
            let mut lines = vec![];
            lines.extend(vec![
                kb_line(state, "q", "quit"),
                kb_line(state, "j/k, arrows", "move"),
                kb_line(state, "PgDn/PgUp, space", "page"),
                kb_line(state, "/", "search"),
                kb_line(state, "n/N", "next/prev match"),
                kb_line(state, "y", "copy code block"),
                kb_line(state, "o", "open link"),
                kb_line(state, "e", "open editor"),
                kb_line(state, "r", "reload"),
                kb_line(state, "x", "toggle task checkbox"),
                kb_line(state, "m", "toggle selection mode"),
                kb_line(state, "?", "toggle help"),
                kb_line(state, "Esc/Enter", "close popup"),
            ]);
            ("KEYBINDINGS", lines, 54, 17)
        }
        Overlay::LinkConfirm(url) => {
            let url_w = UnicodeWidthStr::width(url.as_str()).min(64) as u16 + 8;
            (
                "OPEN LINK",
                vec![
                    Line::from(vec![Span::styled(url.to_string(), state.theme.link)]),
                    link_actions_line(state),
                ],
                url_w.clamp(50, 72),
                9,
            )
        }
    };
    let popup = centered_rect_size(popup_w, popup_h, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .style(state.theme.normal),
        popup,
    );
    let inner = Rect {
        x: popup.x + 1,
        y: popup.y + 1,
        width: popup.width.saturating_sub(2),
        height: popup.height.saturating_sub(2),
    };
    let top_title = Line::from(vec![Span::styled(
        centered_text(title.to_uppercase().as_str(), inner.width as usize),
        state
            .theme
            .popup_title
            .add_modifier(ratatui::style::Modifier::BOLD),
    )]);
    frame.render_widget(
        Paragraph::new(vec![top_title]).style(state.theme.normal),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        },
    );
    let body_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: inner.height.saturating_sub(1),
    };
    if matches!(overlay, Overlay::LinkConfirm(_)) {
        body = center_link_popup_lines(body, body_area.width as usize, body_area.height as usize);
    } else {
        body = center_block_lines(body, body_area.width as usize, body_area.height as usize);
    }
    frame.render_widget(
        Paragraph::new(body)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false })
            .style(state.theme.normal),
        body_area,
    );
}

pub(super) fn kb_line(state: &AppState, key: &str, desc: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<18}", key),
            state.theme.popup_key.remove_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(desc.to_string(), state.theme.normal),
    ])
}

pub(super) fn brighten_color(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(
            r.saturating_add(28),
            g.saturating_add(28),
            b.saturating_add(28),
        ),
        Color::Cyan => Color::LightCyan,
        Color::Blue => Color::LightBlue,
        Color::Magenta => Color::LightMagenta,
        other => other,
    }
}

pub(super) fn center_block_lines(
    mut lines: Vec<Line<'static>>,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let max_w = lines
        .iter()
        .map(|line| {
            let txt = line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            UnicodeWidthStr::width(txt.as_str())
        })
        .max()
        .unwrap_or(0)
        .min(width);
    let left_pad = width.saturating_sub(max_w) / 2;
    if left_pad > 0 {
        for line in &mut lines {
            line.spans.insert(0, Span::raw(" ".repeat(left_pad)));
        }
    }
    if lines.len() < height {
        let rem = height - lines.len();
        let pad_top = rem / 2;
        let pad_bottom = rem - pad_top;
        for _ in 0..pad_top {
            lines.insert(0, Line::from(""));
        }
        for _ in 0..pad_bottom {
            lines.push(Line::from(""));
        }
    }
    lines
}

pub(super) fn link_actions_line(state: &AppState) -> Line<'static> {
    let left = "Yes (";
    let mid = ")         No (";
    let right = ")";
    Line::from(vec![
        Span::styled(left.to_string(), state.theme.normal),
        Span::styled("Enter".to_string(), state.theme.popup_key),
        Span::styled(mid.to_string(), state.theme.normal),
        Span::styled("ESC".to_string(), state.theme.popup_key),
        Span::styled(right.to_string(), state.theme.normal),
    ])
}

pub(super) fn center_link_popup_lines(
    lines: Vec<Line<'static>>,
    width: usize,
    height: usize,
) -> Vec<Line<'static>> {
    let mut out = vec![Line::from(""); height];
    if lines.is_empty() || height == 0 {
        return out;
    }
    // Template:
    // row 0: blank
    // row 1: centered URL
    // row 2: blank
    // row 3: blank
    // row 4: centered actions
    // row 5+: blank
    let first_row = 1usize.min(height.saturating_sub(1));
    let action_row = 4usize.min(height.saturating_sub(1));
    let mut insert_centered = |row: usize, line: &Line<'static>| {
        if row >= out.len() {
            return;
        }
        let txt = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        let w = UnicodeWidthStr::width(txt.as_str());
        let left = width.saturating_sub(w) / 2;
        let mut spans = vec![Span::raw(" ".repeat(left))];
        spans.extend(line.spans.clone());
        out[row] = Line::from(spans);
    };
    insert_centered(first_row, &lines[0]);
    if lines.len() > 1 {
        insert_centered(action_row, &lines[1]);
    }
    out
}

pub(super) fn highlight_spans_for_search(
    spans: &[Span<'static>],
    query: &str,
    mark: ratatui::style::Style,
) -> Vec<Span<'static>> {
    let q = query.trim();
    if q.is_empty() {
        return spans.to_vec();
    }
    let needle = q.to_lowercase();
    let mut out = Vec::new();
    for sp in spans {
        let text = sp.content.as_ref();
        // Lowercasing can change byte length (e.g. 'İ' -> "i̇"), so search in the
        // lowered copy but map every match back to char boundaries of `text`.
        let mut lower = String::with_capacity(text.len());
        let mut lower_to_orig: Vec<usize> = Vec::with_capacity(text.len() + 1);
        for (byte_idx, ch) in text.char_indices() {
            for lc in ch.to_lowercase() {
                lower.push(lc);
            }
            lower_to_orig.resize(lower.len(), byte_idx);
        }
        let orig_char_end =
            |byte_idx: usize| byte_idx + text[byte_idx..].chars().next().map_or(0, char::len_utf8);

        let mut cursor = 0usize;
        let mut search_from = 0usize;
        while let Some(pos) = lower[search_from..].find(&needle) {
            let lower_start = search_from + pos;
            let lower_end = lower_start + needle.len();
            search_from = lower_end;
            let start = lower_to_orig[lower_start];
            let end = orig_char_end(lower_to_orig[lower_end - 1]);
            if start < cursor {
                continue;
            }
            if start > cursor {
                out.push(Span::styled(text[cursor..start].to_string(), sp.style));
            }
            out.push(Span::styled(
                text[start..end].to_string(),
                sp.style.patch(mark),
            ));
            cursor = end;
        }
        if cursor < text.len() {
            out.push(Span::styled(text[cursor..].to_string(), sp.style));
        }
    }
    out
}

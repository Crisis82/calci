//! Image loading/caching (background worker) and everything drawn with
//! kitty's graphics and text sizing protocols.

use super::*;
use crate::placeholder;
use crate::renderer::RangeKind;

pub(super) const IMAGE_MAX_WIDTH_RATIO: f32 = 0.40;
pub(super) const IMAGE_MIN_WIDTH: usize = 8;
pub(super) const KITTY_CHUNK_SIZE: usize = 4096;
pub(super) const REMOTE_IMAGE_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const REMOTE_IMAGE_MAX_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct CachedImage {
    pub(super) pixel_size: Option<(u32, u32)>,
    pub(super) png_data: Option<Vec<u8>>,
    pub(super) kitty_image_id: u32,
    pub(super) uploaded: bool,
    /// Still being decoded/fetched by the background loader.
    pub(super) loading: bool,
}

pub(super) type ImagePayload = Result<(Vec<u8>, u32, u32), String>;

/// Decodes (and, if enabled, fetches) images on a worker thread so the pager
/// never blocks on image IO. Results are drained with `AppState::poll_images`.
pub(super) struct ImageLoader {
    requests: mpsc::Sender<(String, ResolvedImageSource)>,
    results: mpsc::Receiver<(String, ImagePayload)>,
}

impl ImageLoader {
    fn spawn() -> Option<Self> {
        let (req_tx, req_rx) = mpsc::channel::<(String, ResolvedImageSource)>();
        let (res_tx, res_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("calci-images".to_string())
            .spawn(move || {
                for (key, source) in req_rx {
                    let payload = load_kitty_image_payload(&source);
                    if res_tx.send((key, payload)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Self {
            requests: req_tx,
            results: res_rx,
        })
    }
}

impl std::fmt::Debug for ImageLoader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ImageLoader")
    }
}

pub(super) enum ImageState {
    Loading,
    Missing,
    Ready(ImageLayout),
}

#[derive(Clone, Debug)]
pub(super) enum ResolvedImageSource {
    Local(PathBuf),
    Url(String),
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ImageLayout {
    pub(super) cols: usize,
    pub(super) rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VisibleImagePlacement {
    image_index: usize,
    cols: usize,
    total_rows: usize,
    visible_rows: usize,
    hidden_rows_top: usize,
    x: usize,
    y: usize,
}

#[derive(Clone, Debug)]
pub(super) struct KittyUpload {
    image_id: u32,
    png_data: Vec<u8>,
}

impl AppState {
    /// Drain finished background image loads. Returns true when the document
    /// layout changed and a redraw is needed.
    pub fn poll_images(&mut self) -> bool {
        let Some(loader) = &self.image_loader else {
            return false;
        };
        let finished: Vec<_> = loader.results.try_iter().collect();
        if finished.is_empty() {
            return false;
        }
        for (key, payload) in finished {
            self.apply_image_payload(&key, payload);
        }
        self.rebuild_after_image_load();
        true
    }

    #[cfg(test)]
    pub(super) fn wait_for_images(&mut self) {
        while self.image_cache.values().any(|e| e.loading) {
            let Some(loader) = &self.image_loader else {
                return;
            };
            let Ok((key, payload)) = loader.results.recv_timeout(Duration::from_secs(10)) else {
                return;
            };
            self.apply_image_payload(&key, payload);
        }
        self.rebuild_after_image_load();
    }

    pub(super) fn apply_image_payload(&mut self, key: &str, payload: ImagePayload) {
        let Some(entry) = self.image_cache.get_mut(key) else {
            return;
        };
        entry.loading = false;
        match payload {
            Ok((png, width, height)) => {
                entry.pixel_size = Some((width, height));
                entry.png_data = Some(png);
            }
            Err(_) => {
                entry.pixel_size = None;
                entry.png_data = None;
            }
        }
    }

    /// Re-materialize image rows after loads complete, keeping the viewport
    /// anchored on the same text line so content doesn't jump as images
    /// above it pop in.
    pub(super) fn rebuild_after_image_load(&mut self) {
        let width = self.viewport_width.max(1);
        let (top_line, top_inner) = self.line_and_inner_row_at_visual_row(self.offset, width);
        let top_anchor = non_image_ordinal(&self.doc.lines, top_line);
        let selected_anchor = non_image_ordinal(&self.doc.lines, self.selected_line);

        self.doc.lines = self.rendered_lines.clone();
        self.materialize_images(width);
        self.relayout();

        if let Some(ord) = top_anchor {
            let line = line_for_non_image_ordinal(&self.doc.lines, ord);
            self.offset = self.visual_row_of_line(line, width) + top_inner;
        }
        if let Some(ord) = selected_anchor {
            self.selected_line = line_for_non_image_ordinal(&self.doc.lines, ord);
        }
        self.selected_line = self
            .selected_line
            .min(self.doc.lines.len().saturating_sub(1));
        self.offset = self
            .offset
            .min(self.total_visual_rows(width).saturating_sub(1));
        self.search_hits = self.collect_search_hits();
        self.search_index = self
            .search_index
            .min(self.search_hits.len().saturating_sub(1));
    }

    pub(super) fn materialize_images(&mut self, content_width: usize) {
        if self.doc.images.is_empty() {
            return;
        }
        let width = content_width.max(1);
        let images = self.doc.images.clone();
        let original = std::mem::take(&mut self.doc.lines);
        let mut expanded = Vec::with_capacity(original.len());

        for line in original {
            if line.kind != LineKind::Image {
                expanded.push(line);
                continue;
            }
            let Some(image_idx) = line.image_index else {
                expanded.push(line);
                continue;
            };
            let Some(image) = images.get(image_idx) else {
                expanded.push(line);
                continue;
            };
            expanded.extend(self.render_image_block(image, image_idx, width));
        }

        self.doc.lines = expanded;
    }

    /// Transmit math images whose placeholder cells are on screen and that
    /// the terminal doesn't have yet.
    pub(super) fn upload_visible_math_images(&mut self, body_height: usize) -> std::io::Result<()> {
        let Some(store) = self.math_images.clone() else {
            return Ok(());
        };
        let mut store = store.borrow_mut();
        let start = self.offset.min(self.layout.rows.len());
        let end = (self.offset + body_height).min(self.layout.rows.len());
        let mut stdout = std::io::stdout();
        let mut sent = false;
        for span in self.layout.rows[start..end]
            .iter()
            .flat_map(|row| &row.content)
        {
            if !placeholder::is_placeholder(&span.content) {
                continue;
            }
            let Some(math) = span
                .style
                .fg
                .and_then(placeholder::color_id)
                .and_then(|id| store.get_mut(id))
            else {
                continue;
            };
            if !math.uploaded {
                let image = &math.image;
                placeholder::transmit_virtual(
                    &mut stdout,
                    math.id,
                    &image.png,
                    image.cols,
                    image.rows,
                )?;
                math.uploaded = true;
                sent = true;
            }
        }
        if sent {
            stdout.flush()?;
        }
        Ok(())
    }

    pub(super) fn render_image_block(
        &mut self,
        image: &RenderImage,
        image_idx: usize,
        content_width: usize,
    ) -> Vec<RenderLine> {
        let image_link =
            if image.source.starts_with("http://") || image.source.starts_with("https://") {
                Some(image.source.clone())
            } else {
                None
            };
        if image_link.is_some() && !self.remote_images {
            let label = format!("[{}]", remote_image_file_name(&image.source));
            return self.render_missing_image_block(image, image_link, Some(label));
        }
        if !self.graphics {
            // No kitty graphics: don't decode or send anything, show the alt text.
            return self.render_missing_image_block(image, image_link, None);
        }
        let layout = match self.cached_image_state(image, content_width) {
            ImageState::Ready(layout) => layout,
            ImageState::Missing => return self.render_missing_image_block(image, image_link, None),
            ImageState::Loading => {
                let label = if image.alt.trim().is_empty() {
                    "[loading image…]".to_string()
                } else {
                    format!("[loading {}…]", image.alt.trim())
                };
                return self.render_missing_image_block(image, image_link, Some(label));
            }
        };

        let placeholder = " ".repeat(layout.cols.max(1));
        let mut lines = Vec::with_capacity(layout.rows.saturating_add(2));
        for _ in 0..layout.rows {
            let row_width = UnicodeWidthStr::width(placeholder.as_str());
            let link_ranges = image_link
                .as_ref()
                .filter(|_| row_width > 0)
                .map(|url| {
                    vec![LinkRange {
                        start: 0,
                        end: row_width,
                        url: url.clone(),
                        kind: RangeKind::Url,
                    }]
                })
                .unwrap_or_default();
            lines.push(RenderLine {
                line: Line::from(vec![Span::styled(placeholder.clone(), self.theme.normal)]),
                kind: LineKind::Image,
                link_ranges,
                code_block_index: None,
                image_index: Some(image_idx),
                heading_level: None,
                hang_indent: 0,
                task: None,
            });
        }

        if let Some(title) = image
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            lines.push(RenderLine {
                line: Line::from(vec![Span::styled(String::new(), self.theme.normal)]),
                kind: LineKind::Image,
                link_ranges: Vec::new(),
                code_block_index: None,
                image_index: None,
                heading_level: None,
                hang_indent: 0,
                task: None,
            });
            let title_style = self.theme.line_number.add_modifier(Modifier::ITALIC);
            let title_width = UnicodeWidthStr::width(title);
            let link_ranges = image_link
                .as_ref()
                .filter(|_| title_width > 0)
                .map(|url| {
                    vec![LinkRange {
                        start: 0,
                        end: title_width,
                        url: url.clone(),
                        kind: RangeKind::Url,
                    }]
                })
                .unwrap_or_default();
            lines.push(RenderLine {
                line: Line::from(vec![Span::styled(title.to_string(), title_style)]),
                kind: LineKind::Image,
                link_ranges,
                code_block_index: None,
                image_index: None,
                heading_level: None,
                hang_indent: 0,
                task: None,
            });
        }

        lines
    }

    pub(super) fn render_missing_image_block(
        &self,
        image: &RenderImage,
        image_link: Option<String>,
        label_override: Option<String>,
    ) -> Vec<RenderLine> {
        let placeholder_label = if let Some(label) = label_override {
            label
        } else if image.alt.trim().is_empty() {
            "[image]".to_string()
        } else {
            format!("[{}]", image.alt.trim())
        };
        let placeholder_style = self.theme.line_number;
        let placeholder_width = UnicodeWidthStr::width(placeholder_label.as_str());
        let placeholder_links = image_link
            .as_ref()
            .filter(|_| placeholder_width > 0)
            .map(|url| {
                vec![LinkRange {
                    start: 0,
                    end: placeholder_width,
                    url: url.clone(),
                    kind: RangeKind::Url,
                }]
            })
            .unwrap_or_default();

        let mut lines = Vec::with_capacity(4);
        lines.push(RenderLine {
            line: Line::from(vec![Span::styled(String::new(), self.theme.normal)]),
            kind: LineKind::Image,
            link_ranges: Vec::new(),
            code_block_index: None,
            image_index: None,
            heading_level: None,
            hang_indent: 0,
            task: None,
        });
        lines.push(RenderLine {
            line: Line::from(vec![Span::styled(placeholder_label, placeholder_style)]),
            kind: LineKind::Image,
            link_ranges: placeholder_links,
            code_block_index: None,
            image_index: None,
            heading_level: None,
            hang_indent: 0,
            task: None,
        });
        lines.push(RenderLine {
            line: Line::from(vec![Span::styled(String::new(), self.theme.normal)]),
            kind: LineKind::Image,
            link_ranges: Vec::new(),
            code_block_index: None,
            image_index: None,
            heading_level: None,
            hang_indent: 0,
            task: None,
        });
        if let Some(title) = image
            .title
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            let title_style = self.theme.line_number.add_modifier(Modifier::ITALIC);
            let title_width = UnicodeWidthStr::width(title);
            let link_ranges = image_link
                .as_ref()
                .filter(|_| title_width > 0)
                .map(|url| {
                    vec![LinkRange {
                        start: 0,
                        end: title_width,
                        url: url.clone(),
                        kind: RangeKind::Url,
                    }]
                })
                .unwrap_or_default();
            lines.push(RenderLine {
                line: Line::from(vec![Span::styled(title.to_string(), title_style)]),
                kind: LineKind::Image,
                link_ranges,
                code_block_index: None,
                image_index: None,
                heading_level: None,
                hang_indent: 0,
                task: None,
            });
        }

        lines
    }

    pub(super) fn cached_image_state(
        &mut self,
        image: &RenderImage,
        content_width: usize,
    ) -> ImageState {
        let entry = self.cached_image_entry(image);
        if entry.loading {
            return ImageState::Loading;
        }
        let Some((pixel_width, pixel_height)) = entry.pixel_size else {
            return ImageState::Missing;
        };
        ImageState::Ready(compute_image_layout(
            content_width,
            pixel_width,
            pixel_height,
            image.width_percent,
        ))
    }

    pub(super) fn cached_image_entry(&mut self, image: &RenderImage) -> &mut CachedImage {
        let resolved = resolve_image_source(&image.source, self.source_path.as_deref());
        let key = resolved_image_cache_key(&resolved);
        match self.image_cache.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let kitty_image_id = self.next_kitty_image_id.max(1);
                self.next_kitty_image_id = self.next_kitty_image_id.saturating_add(1);
                if self.image_loader.is_none() {
                    self.image_loader = ImageLoader::spawn();
                }
                let queued = self.image_loader.as_ref().is_some_and(|loader| {
                    loader
                        .requests
                        .send((entry.key().clone(), resolved.clone()))
                        .is_ok()
                });
                // Fall back to loading inline if the worker is unavailable.
                let (pixel_size, png_data) = if queued {
                    (None, None)
                } else {
                    match load_kitty_image_payload(&resolved) {
                        Ok((png, width, height)) => (Some((width, height)), Some(png)),
                        Err(_) => (None, None),
                    }
                };
                entry.insert(CachedImage {
                    pixel_size,
                    png_data,
                    kitty_image_id,
                    uploaded: false,
                    loading: queued,
                })
            }
        }
    }

    pub fn render_kitty_images(&mut self, terminal_size: Rect) -> anyhow::Result<()> {
        let body_area = pager_body_area(terminal_size, self.top_frontmatter_title().is_some());
        if self.overlay.is_none() && self.layout_is_current(body_area.width as usize) {
            self.upload_visible_math_images(body_area.height as usize)?;
        }
        let placements = self.visible_image_placements(body_area);
        let uploads = self.collect_pending_kitty_uploads(&placements);
        if uploads.is_empty() && self.last_kitty_placements.as_ref() == Some(&placements) {
            return Ok(());
        }
        if placements.is_empty() && !self.kitty_placed {
            // Nothing to show and nothing shown: send no graphics escapes at
            // all (terminals without the protocol may print them as text).
            self.last_kitty_placements = Some(placements);
            return Ok(());
        }
        self.kitty_placed = !placements.is_empty();

        let mut stdout = std::io::stdout();
        kitty_delete_all_placements(&mut stdout)?;
        for upload in uploads {
            kitty_transmit_png(&mut stdout, upload.image_id, &upload.png_data)?;
        }
        for placement in placements.iter().copied() {
            let Some((image_id, pixel_height)) = self.kitty_image_meta(placement.image_index)
            else {
                continue;
            };
            let src_y = ((placement.hidden_rows_top as u64 * pixel_height as u64)
                / placement.total_rows as u64) as u32;
            let mut src_h = ((placement.visible_rows as u64 * pixel_height as u64)
                / placement.total_rows as u64) as u32;
            if src_h == 0 {
                src_h = 1;
            }
            kitty_move_cursor(&mut stdout, placement.y + 1, placement.x + 1)?;
            kitty_place_image(
                &mut stdout,
                image_id,
                placement.cols,
                placement.visible_rows,
                src_y,
                src_h,
            )?;
        }
        stdout.flush()?;
        self.last_kitty_placements = Some(placements);
        Ok(())
    }

    /// Visible 2x headings as (x, y, text, style), in screen cells. Only
    /// headings whose two rows are both on screen are scaled; a heading cut
    /// by the viewport edge shows its normal-size fallback instead.
    pub(super) fn visible_scaled_headings(
        &self,
        body_area: Rect,
    ) -> Vec<(u16, u16, String, Style)> {
        if !self.heading_scale || self.overlay.is_some() {
            return Vec::new();
        }
        if !self.layout_is_current(body_area.width as usize) {
            return Vec::new();
        }
        let rows = &self.layout.rows;
        let start = self.offset.min(rows.len());
        let end = (self.offset + body_area.height as usize).min(rows.len());
        let mut out = Vec::new();
        for (r, row) in rows
            .iter()
            .enumerate()
            .take(end.saturating_sub(1))
            .skip(start)
        {
            if !row.scaled {
                continue;
            }
            let text: String = row
                .content
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
                .chars()
                .filter(|c| !c.is_control())
                .collect();
            let style = row.content.first().map(|s| s.style).unwrap_or_default();
            out.push((
                body_area.x + row.prefix_width as u16,
                body_area.y + (r - start) as u16,
                text,
                style,
            ));
        }
        out
    }

    /// Draw visible H2 headings at 2x with kitty's text sizing protocol
    /// (OSC 66). `draw` marks those cells as skipped so ratatui never paints
    /// over them.
    pub fn render_scaled_headings(&self, terminal_size: Rect) -> anyhow::Result<()> {
        let body_area = pager_body_area(terminal_size, self.top_frontmatter_title().is_some());
        let headings = self.visible_scaled_headings(body_area);
        if headings.is_empty() {
            return Ok(());
        }
        let mut stdout = std::io::stdout();
        for (x, y, text, style) in headings {
            kitty_move_cursor(&mut stdout, y as usize + 1, x as usize + 1)?;
            write!(
                stdout,
                "{}\x1b]66;s=2;{text}\x1b\\\x1b[0m",
                ansi_style_prefix(style)
            )?;
        }
        stdout.flush()?;
        Ok(())
    }

    pub fn clear_kitty_images(&mut self) -> anyhow::Result<()> {
        self.last_kitty_placements = None;
        let mut freed = false;
        let mut stdout = std::io::stdout();
        if let Some(store) = &self.math_images {
            for math in store.borrow_mut().iter_mut() {
                if std::mem::take(&mut math.uploaded) {
                    placeholder::delete_image(&mut stdout, math.id)?;
                    freed = true;
                }
            }
        }
        if freed {
            stdout.flush()?;
        }
        if !std::mem::take(&mut self.kitty_placed) {
            return Ok(());
        }
        let mut stdout = std::io::stdout();
        kitty_delete_all_placements(&mut stdout)?;
        stdout.flush()?;
        Ok(())
    }

    pub(super) fn invalidate_kitty_upload_state(&mut self) {
        self.last_kitty_placements = None;
        if let Some(store) = &self.math_images {
            for math in store.borrow_mut().iter_mut() {
                math.uploaded = false;
            }
        }
        for entry in self.image_cache.values_mut() {
            entry.uploaded = false;
        }
    }

    pub(super) fn visible_image_placements(&self, body_area: Rect) -> Vec<VisibleImagePlacement> {
        if self.overlay.is_some() || body_area.width == 0 || body_area.height == 0 {
            return Vec::new();
        }
        if !self.layout_is_current(body_area.width as usize) {
            return Vec::new();
        }
        let rows = &self.layout.rows;
        let view_top = self.offset.min(rows.len());
        let view_bottom = (self.offset + body_area.height as usize).min(rows.len());
        let lines = &self.doc.lines;
        let image_of = |line_idx: usize| {
            lines
                .get(line_idx)
                .filter(|l| l.kind == LineKind::Image)
                .and_then(|l| l.image_index)
        };

        let mut placements = Vec::new();
        let mut r = view_top;
        while r < view_bottom {
            let row = &rows[r];
            let Some(image_index) = image_of(row.line_idx) else {
                r += 1;
                continue;
            };
            // Image rows never wrap: one line == one row.
            let mut first = row.line_idx;
            while first > 0 && image_of(first - 1) == Some(image_index) {
                first -= 1;
            }
            let mut last = row.line_idx;
            while image_of(last + 1) == Some(image_index) {
                last += 1;
            }
            let total_rows = last - first + 1;
            let hidden_rows_top = row.line_idx - first;
            let visible_rows = (last - row.line_idx + 1).min(view_bottom - r);
            let cols = row.content.iter().map(Span::width).sum::<usize>();
            placements.push(VisibleImagePlacement {
                image_index,
                cols: cols.max(1),
                total_rows,
                visible_rows,
                hidden_rows_top,
                x: body_area.x as usize + row.prefix_width,
                y: body_area.y as usize + (r - view_top),
            });
            r += visible_rows;
        }
        placements
    }

    pub(super) fn collect_pending_kitty_uploads(
        &mut self,
        placements: &[VisibleImagePlacement],
    ) -> Vec<KittyUpload> {
        let mut uploads = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for placement in placements {
            if !seen.insert(placement.image_index) {
                continue;
            }
            let Some(image) = self.doc.images.get(placement.image_index).cloned() else {
                continue;
            };
            let entry = self.cached_image_entry(&image);
            if !entry.uploaded
                && let Some(png_data) = entry.png_data.clone()
            {
                uploads.push(KittyUpload {
                    image_id: entry.kitty_image_id,
                    png_data,
                });
                entry.uploaded = true;
            }
        }
        uploads
    }

    pub(super) fn kitty_image_meta(&mut self, image_index: usize) -> Option<(u32, u32)> {
        let image = self.doc.images.get(image_index).cloned()?;
        let entry = self.cached_image_entry(&image);
        let (_, pixel_height) = entry.pixel_size?;
        Some((entry.kitty_image_id, pixel_height))
    }
}

/// Position of `line_idx` among lines that are not image rows. Image rows are
/// the only lines whose count changes when an image finishes loading, so this
/// ordinal is stable across such rebuilds.
pub(super) fn non_image_ordinal(lines: &[RenderLine], line_idx: usize) -> Option<usize> {
    let line_idx = (line_idx..lines.len()).find(|&i| lines[i].kind != LineKind::Image)?;
    Some(
        lines[..line_idx]
            .iter()
            .filter(|l| l.kind != LineKind::Image)
            .count(),
    )
}

pub(super) fn line_for_non_image_ordinal(lines: &[RenderLine], ordinal: usize) -> usize {
    lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind != LineKind::Image)
        .nth(ordinal)
        .map_or(lines.len().saturating_sub(1), |(i, _)| i)
}

pub(super) fn resolve_image_source(raw: &str, markdown_path: Option<&Path>) -> ResolvedImageSource {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        return ResolvedImageSource::Url(raw.to_string());
    }

    let path = Path::new(raw);
    let local = if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(base) = markdown_path.and_then(Path::parent) {
        base.join(path)
    } else {
        path.to_path_buf()
    };
    let canonical = local.canonicalize().unwrap_or(local);
    ResolvedImageSource::Local(canonical)
}

pub(super) fn resolved_image_cache_key(source: &ResolvedImageSource) -> String {
    match source {
        ResolvedImageSource::Local(path) => format!("file:{}", path.display()),
        ResolvedImageSource::Url(url) => format!("url:{url}"),
    }
}

pub(super) fn compute_image_layout(
    content_width: usize,
    pixel_width: u32,
    pixel_height: u32,
    width_percent: Option<u8>,
) -> ImageLayout {
    let width_ratio = width_percent
        .map(|percent| (percent as f32 / 100.0).clamp(0.01, 1.0))
        .unwrap_or(IMAGE_MAX_WIDTH_RATIO);
    let max_width = ((content_width as f32) * width_ratio).floor().max(1.0) as usize;
    let cols = max_width
        .max(IMAGE_MIN_WIDTH)
        .min(content_width.max(1))
        .min(pixel_width.max(1) as usize)
        .max(1);
    let aspect = pixel_height.max(1) as f32 / pixel_width.max(1) as f32;
    let rows = ((cols as f32 * aspect) * 0.5).round().max(1.0) as usize;
    ImageLayout { cols, rows }
}

pub(super) fn load_kitty_image_payload(
    source: &ResolvedImageSource,
) -> Result<(Vec<u8>, u32, u32), String> {
    let decoded = load_dynamic_image(source)?;
    let width = decoded.width().max(1);
    let height = decoded.height().max(1);
    let mut png_data = Vec::new();
    decoded
        .write_to(&mut Cursor::new(&mut png_data), image::ImageFormat::Png)
        .map_err(|err| format!("failed encoding image payload: {err}"))?;
    Ok((png_data, width, height))
}

pub(super) fn load_dynamic_image(source: &ResolvedImageSource) -> Result<DynamicImage, String> {
    let (bytes, content_type) = match source {
        ResolvedImageSource::Local(path) => std::fs::read(path)
            .map(|bytes| (bytes, None))
            .map_err(|err| format!("failed reading {}: {err}", path.display()))?,
        ResolvedImageSource::Url(url) => fetch_remote_image(url)?,
    };

    match image::load_from_memory(&bytes) {
        Ok(decoded) => Ok(decoded),
        Err(raster_err) => {
            let maybe_svg = source_is_svg_hint(source)
                || content_type_is_svg(content_type.as_deref())
                || bytes_look_like_svg(&bytes);
            if maybe_svg {
                return decode_svg_to_image(&bytes).map_err(|svg_err| {
                    format!("failed decoding image: {raster_err}; svg fallback failed: {svg_err}")
                });
            }
            Err(format!("failed decoding image: {raster_err}"))
        }
    }
}

/// Fetch a remote image with a hard timeout and body size cap so a slow or
/// oversized response can neither hang the pager nor exhaust memory.
pub(super) fn fetch_remote_image(url: &str) -> Result<(Vec<u8>, Option<String>), String> {
    use std::io::Read as _;
    let client = reqwest::blocking::Client::builder()
        .timeout(REMOTE_IMAGE_TIMEOUT)
        .build()
        .map_err(|err| format!("failed building http client: {err}"))?;
    let response = client
        .get(url)
        .send()
        .map_err(|err| format!("failed fetching {url}: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("http {} for {url}", response.status()));
    }
    if response
        .content_length()
        .is_some_and(|len| len > REMOTE_IMAGE_MAX_BYTES)
    {
        return Err(format!("image too large: {url}"));
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.to_string());
    let mut bytes = Vec::new();
    response
        .take(REMOTE_IMAGE_MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("failed reading response body for {url}: {err}"))?;
    if bytes.len() as u64 > REMOTE_IMAGE_MAX_BYTES {
        return Err(format!("image too large: {url}"));
    }
    Ok((bytes, content_type))
}

/// Last path segment of an image URL (query/fragment stripped), used as the
/// placeholder label when remote image loading is disabled.
pub(super) fn remote_image_file_name(url: &str) -> String {
    let without_suffix = url.split(['?', '#']).next().unwrap_or(url);
    let after_scheme = without_suffix
        .split_once("://")
        .map_or(without_suffix, |(_, rest)| rest);
    after_scheme
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(after_scheme)
        .to_string()
}

pub(super) fn source_is_svg_hint(source: &ResolvedImageSource) -> bool {
    match source {
        ResolvedImageSource::Local(path) => path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("svg")),
        ResolvedImageSource::Url(url) => url
            .split('#')
            .next()
            .unwrap_or(url)
            .split('?')
            .next()
            .unwrap_or(url)
            .rsplit('.')
            .next()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("svg")),
    }
}

pub(super) fn content_type_is_svg(content_type: Option<&str>) -> bool {
    content_type
        .map(|value| value.to_ascii_lowercase().contains("image/svg+xml"))
        .unwrap_or(false)
}

pub(super) fn bytes_look_like_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    head.to_ascii_lowercase().contains("<svg")
}

/// Scanning system fonts is slow (often 100ms+); do it once per process.
pub(super) fn svg_font_database() -> std::sync::Arc<usvg::fontdb::Database> {
    static FONTS: std::sync::OnceLock<std::sync::Arc<usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            std::sync::Arc::new(db)
        })
        .clone()
}

pub(super) fn decode_svg_to_image(bytes: &[u8]) -> Result<DynamicImage, String> {
    let options = usvg::Options {
        fontdb: svg_font_database(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|err| err.to_string())?;
    let size = tree.size().to_int_size();
    let mut pixmap = tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| "failed allocating pixmap for svg".to_string())?;
    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    let rgba = RgbaImage::from_raw(size.width(), size.height(), pixmap.data().to_vec())
        .ok_or_else(|| "failed converting rendered svg into image buffer".to_string())?;
    Ok(DynamicImage::ImageRgba8(rgba))
}

pub(super) fn kitty_delete_all_placements(stdout: &mut impl Write) -> std::io::Result<()> {
    stdout.write_all(b"\x1b_Ga=d,d=a,q=2\x1b\\")
}

pub(super) fn kitty_transmit_png(
    stdout: &mut impl Write,
    image_id: u32,
    png_data: &[u8],
) -> std::io::Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(png_data);
    let bytes = encoded.as_bytes();
    let mut offset = 0usize;
    if bytes.is_empty() {
        return Ok(());
    }
    while offset < bytes.len() {
        let end = (offset + KITTY_CHUNK_SIZE).min(bytes.len());
        let chunk = &bytes[offset..end];
        let has_more = end < bytes.len();
        if offset == 0 {
            write!(
                stdout,
                "\x1b_Gf=100,q=2,i={},m={};",
                image_id,
                if has_more { 1 } else { 0 }
            )?;
        } else {
            write!(stdout, "\x1b_Gm={};", if has_more { 1 } else { 0 })?;
        }
        stdout.write_all(chunk)?;
        stdout.write_all(b"\x1b\\")?;
        offset = end;
    }
    Ok(())
}

pub(super) fn kitty_move_cursor(
    stdout: &mut impl Write,
    row: usize,
    col: usize,
) -> std::io::Result<()> {
    write!(stdout, "\x1b[{};{}H", row, col)
}

pub(super) fn kitty_place_image(
    stdout: &mut impl Write,
    image_id: u32,
    cols: usize,
    rows: usize,
    src_y: u32,
    src_h: u32,
) -> std::io::Result<()> {
    write!(
        stdout,
        "\x1b_Ga=p,q=2,i={},c={},r={},C=1,y={},h={}\x1b\\",
        image_id, cols, rows, src_y, src_h
    )
}

/// Terminals implementing the kitty graphics protocol (kitty, Ghostty).
pub fn terminal_supports_kitty_graphics() -> bool {
    terminal_is_kitty()
        || std::env::var("TERM").is_ok_and(|t| t.contains("ghostty"))
        || std::env::var("TERM_PROGRAM").is_ok_and(|p| p.eq_ignore_ascii_case("ghostty"))
}

pub(super) fn terminal_is_kitty() -> bool {
    std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var("TERM").is_ok_and(|t| t.contains("kitty"))
}

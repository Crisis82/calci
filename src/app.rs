use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::Context;
use base64::Engine as _;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use image::DynamicImage;
use image::RgbaImage;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use reqwest::header::CONTENT_TYPE;
use resvg::{tiny_skia, usvg};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::renderer::{
    LineKind, LinkRange, RenderDoc, RenderImage, RenderLine, RenderSettings, open_in_editor,
    preprocess_math, render_markdown,
};
use crate::theme::AppTheme;

const END_PADDING_ROWS: usize = 3;
pub const WINDOW_PAD_X: u16 = 2;
pub const WINDOW_PAD_TOP: u16 = 1;
const LINE_NUMBER_COLS: usize = 6;
const QUOTE_PREFIX: &str = "│ ";
const QUOTE_PREFIX_WIDTH: usize = 2;
const IMAGE_MAX_WIDTH_RATIO: f32 = 0.40;
const IMAGE_MIN_WIDTH: usize = 8;
const KITTY_CHUNK_SIZE: usize = 4096;

#[derive(Clone, Debug)]
struct CachedImage {
    pixel_size: Option<(u32, u32)>,
    png_data: Option<Vec<u8>>,
    kitty_image_id: u32,
    uploaded: bool,
}

#[derive(Clone, Debug)]
enum ResolvedImageSource {
    Local(PathBuf),
    Url(String),
}

#[derive(Clone, Copy, Debug)]
struct ImageLayout {
    cols: usize,
    rows: usize,
}

#[derive(Clone, Copy, Debug)]
struct VisibleImagePlacement {
    image_index: usize,
    cols: usize,
    total_rows: usize,
    visible_rows: usize,
    hidden_rows_top: usize,
    x: usize,
    y: usize,
}

#[derive(Clone, Debug)]
struct KittyUpload {
    image_id: u32,
    png_data: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub input_path: Option<PathBuf>,
    pub line_numbers: bool,
    pub line_highlight: bool,
    pub start_in_pager: bool,
    pub mouse: bool,
    pub wrap: bool,
    pub smooth_scroll: usize,
    pub math: bool,
    pub center_blocks: bool,
    pub link_confirm: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            input_path: None,
            line_numbers: false,
            line_highlight: false,
            start_in_pager: true,
            mouse: true,
            wrap: true,
            smooth_scroll: 3,
            math: true,
            center_blocks: true,
            link_confirm: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Pager,
    Search,
}

#[derive(Clone, Debug)]
enum Overlay {
    Help,
    LinkConfirm(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum HoverTarget {
    Link(usize),
    Code(usize),
}

#[derive(Clone, Debug)]
enum ClickTarget {
    Link { url: String, link_idx: usize },
    Code(usize),
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub doc: RenderDoc,
    pub source_markdown: String,
    pub source_path: Option<PathBuf>,
    pub offset: usize,
    pub selected_line: usize,
    pub should_quit: bool,
    mode: Mode,
    pub search_query: String,
    pub search_hits: Vec<usize>,
    pub search_index: usize,
    pub status: String,
    pub status_is_error: bool,
    pub theme: AppTheme,
    pub line_numbers: bool,
    pub line_highlight: bool,
    pub wrap: bool,
    mouse_allowed: bool,
    pub mouse: bool,
    pub smooth_scroll: usize,
    pub math_enabled: bool,
    pub center_blocks: bool,
    pub link_confirm: bool,
    overlay: Option<Overlay>,
    hover_target: Option<(usize, HoverTarget)>,
    viewport_width: usize,
    image_cache: HashMap<String, CachedImage>,
    next_kitty_image_id: u32,
    last_status_at: Instant,
    force_redraw: bool,
    return_to_dashboard_on_esc: bool,
    should_return_to_dashboard: bool,
}

impl AppState {
    pub fn top_frontmatter_title(&self) -> Option<&str> {
        self.doc
            .front_matter
            .as_ref()
            .and_then(|fm| fm.title.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }

    pub fn from_markdown(
        markdown: String,
        source_path: Option<PathBuf>,
        theme: AppTheme,
        line_numbers: bool,
        line_highlight: bool,
        wrap: bool,
        mouse: bool,
        smooth_scroll: usize,
        math_enabled: bool,
        center_blocks: bool,
        link_confirm: bool,
        width: u16,
    ) -> anyhow::Result<Self> {
        let preprocessed = if math_enabled {
            preprocess_math(&markdown)
        } else {
            markdown.clone()
        };
        let settings = RenderSettings {
            width,
            theme: theme.clone(),
        };
        let doc = render_markdown(&preprocessed, &settings)?;
        let mut state = Self {
            doc,
            source_markdown: markdown,
            source_path,
            offset: 0,
            selected_line: 0,
            should_quit: false,
            mode: Mode::Pager,
            search_query: String::new(),
            search_hits: Vec::new(),
            search_index: 0,
            status: String::new(),
            status_is_error: false,
            theme,
            line_numbers,
            line_highlight,
            wrap,
            mouse_allowed: mouse,
            mouse,
            smooth_scroll,
            math_enabled,
            center_blocks,
            link_confirm,
            overlay: None,
            hover_target: None,
            viewport_width: width as usize,
            image_cache: HashMap::new(),
            next_kitty_image_id: 1,
            last_status_at: Instant::now(),
            force_redraw: false,
            return_to_dashboard_on_esc: false,
            should_return_to_dashboard: false,
        };
        state.materialize_images(width as usize);
        Ok(state)
    }

    pub fn set_return_to_dashboard_on_esc(&mut self, enabled: bool) {
        self.return_to_dashboard_on_esc = enabled;
    }

    pub fn take_return_to_dashboard(&mut self) -> bool {
        std::mem::take(&mut self.should_return_to_dashboard)
    }

    pub fn reload(&mut self, width: u16) -> anyhow::Result<()> {
        self.rerender(width, true)
    }

    pub fn rerender_for_width(&mut self, width: u16) -> anyhow::Result<()> {
        self.rerender(width, false)
    }

    fn rerender(&mut self, width: u16, announce: bool) -> anyhow::Result<()> {
        let preprocessed = if self.math_enabled {
            preprocess_math(&self.source_markdown)
        } else {
            self.source_markdown.clone()
        };
        let settings = RenderSettings {
            width,
            theme: self.theme.clone(),
        };
        self.doc = render_markdown(&preprocessed, &settings)?;
        self.materialize_images(width as usize);
        self.recompute_search_hits();
        self.selected_line = self
            .selected_line
            .min(self.doc.lines.len().saturating_sub(1));
        self.viewport_width = width as usize;
        let total_rows = self
            .total_visual_rows(self.viewport_width)
            .saturating_sub(1);
        self.offset = self.offset.min(total_rows);
        let selected_top = self.visual_row_of_line(self.selected_line, self.viewport_width);
        if self.offset > selected_top {
            self.offset = selected_top;
        }
        if announce {
            self.set_status("reloaded".to_string(), false);
        }
        Ok(())
    }

    fn materialize_images(&mut self, content_width: usize) {
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

    fn render_image_block(
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
        let Some(layout) = self.cached_image_layout(image, content_width) else {
            return self.render_missing_image_block(image, image_link);
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
                    }]
                })
                .unwrap_or_default();
            lines.push(RenderLine {
                line: Line::from(vec![Span::styled(placeholder.clone(), self.theme.normal)]),
                kind: LineKind::Image,
                link_url: image_link.clone(),
                link_ranges,
                code_block_index: None,
                image_index: Some(image_idx),
                heading_level: None,
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
                link_url: None,
                link_ranges: Vec::new(),
                code_block_index: None,
                image_index: None,
                heading_level: None,
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
                    }]
                })
                .unwrap_or_default();
            lines.push(RenderLine {
                line: Line::from(vec![Span::styled(title.to_string(), title_style)]),
                kind: LineKind::Image,
                link_url: image_link,
                link_ranges,
                code_block_index: None,
                image_index: None,
                heading_level: None,
            });
        }

        lines
    }

    fn render_missing_image_block(
        &self,
        image: &RenderImage,
        image_link: Option<String>,
    ) -> Vec<RenderLine> {
        let placeholder_label = if image.alt.trim().is_empty() {
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
                }]
            })
            .unwrap_or_default();

        let mut lines = Vec::with_capacity(4);
        lines.push(RenderLine {
            line: Line::from(vec![Span::styled(String::new(), self.theme.normal)]),
            kind: LineKind::Image,
            link_url: None,
            link_ranges: Vec::new(),
            code_block_index: None,
            image_index: None,
            heading_level: None,
        });
        lines.push(RenderLine {
            line: Line::from(vec![Span::styled(placeholder_label, placeholder_style)]),
            kind: LineKind::Image,
            link_url: image_link.clone(),
            link_ranges: placeholder_links,
            code_block_index: None,
            image_index: None,
            heading_level: None,
        });
        lines.push(RenderLine {
            line: Line::from(vec![Span::styled(String::new(), self.theme.normal)]),
            kind: LineKind::Image,
            link_url: None,
            link_ranges: Vec::new(),
            code_block_index: None,
            image_index: None,
            heading_level: None,
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
                    }]
                })
                .unwrap_or_default();
            lines.push(RenderLine {
                line: Line::from(vec![Span::styled(title.to_string(), title_style)]),
                kind: LineKind::Image,
                link_url: image_link,
                link_ranges,
                code_block_index: None,
                image_index: None,
                heading_level: None,
            });
        }

        lines
    }

    fn cached_image_layout(
        &mut self,
        image: &RenderImage,
        content_width: usize,
    ) -> Option<ImageLayout> {
        let entry = self.cached_image_entry(image);
        let (pixel_width, pixel_height) = entry.pixel_size?;
        Some(compute_image_layout(
            content_width,
            pixel_width,
            pixel_height,
            image.width_percent,
        ))
    }

    fn cached_image_entry(&mut self, image: &RenderImage) -> &mut CachedImage {
        let resolved = resolve_image_source(&image.source, self.source_path.as_deref());
        let key = resolved_image_cache_key(&resolved);
        match self.image_cache.entry(key) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let kitty_image_id = self.next_kitty_image_id.max(1);
                self.next_kitty_image_id = self.next_kitty_image_id.saturating_add(1);
                let loaded = load_kitty_image_payload(&resolved);
                let (pixel_size, png_data) = match loaded {
                    Ok((png, width, height)) => (Some((width, height)), Some(png)),
                    Err(_err) => (None, None),
                };
                entry.insert(CachedImage {
                    pixel_size,
                    png_data,
                    kitty_image_id,
                    uploaded: false,
                })
            }
        }
    }

    pub fn render_kitty_images(&mut self, terminal_size: Rect) -> anyhow::Result<()> {
        let body_area = pager_body_area(terminal_size, self.top_frontmatter_title().is_some());
        let placements = self.visible_image_placements(body_area);
        let uploads = self.collect_pending_kitty_uploads(&placements);

        let mut stdout = std::io::stdout();
        kitty_delete_all_placements(&mut stdout)?;
        for upload in uploads {
            kitty_transmit_png(&mut stdout, upload.image_id, &upload.png_data)?;
        }
        for placement in placements {
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
        Ok(())
    }

    pub fn clear_kitty_images(&mut self) -> anyhow::Result<()> {
        let mut stdout = std::io::stdout();
        kitty_delete_all_placements(&mut stdout)?;
        stdout.flush()?;
        Ok(())
    }

    fn invalidate_kitty_upload_state(&mut self) {
        for entry in self.image_cache.values_mut() {
            entry.uploaded = false;
        }
    }

    fn visible_image_placements(&self, body_area: Rect) -> Vec<VisibleImagePlacement> {
        if self.overlay.is_some() {
            return Vec::new();
        }
        if body_area.width == 0 || body_area.height == 0 {
            return Vec::new();
        }
        let content_width = body_area.width as usize;
        let viewport_rows = body_area.height as usize;
        let view_top = self.offset;
        let view_bottom = self.offset + viewport_rows;
        let mut placements = Vec::new();
        let mut idx = 0usize;

        while idx < self.doc.lines.len() {
            let line = &self.doc.lines[idx];
            let Some(image_index) = line.image_index else {
                idx += 1;
                continue;
            };
            if line.kind != LineKind::Image {
                idx += 1;
                continue;
            }
            if idx > 0
                && self.doc.lines[idx - 1].kind == LineKind::Image
                && self.doc.lines[idx - 1].image_index == Some(image_index)
            {
                idx += 1;
                continue;
            }

            let mut total_rows = 0usize;
            while idx + total_rows < self.doc.lines.len()
                && self.doc.lines[idx + total_rows].kind == LineKind::Image
                && self.doc.lines[idx + total_rows].image_index == Some(image_index)
            {
                total_rows += 1;
            }
            if total_rows == 0 {
                idx += 1;
                continue;
            }

            let top_visual = self.visual_row_of_line(idx, content_width.max(1));
            let bottom_visual = top_visual + total_rows;
            let visible_top = top_visual.max(view_top);
            let visible_bottom = bottom_visual.min(view_bottom);
            if visible_top < visible_bottom {
                let hidden_rows_top = visible_top - top_visual;
                let visible_rows = visible_bottom - visible_top;
                let cols = UnicodeWidthStr::width(
                    line.line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .as_str(),
                );
                let x = body_area.x as usize + self.image_line_x_offset(cols, content_width);
                let y = body_area.y as usize + (visible_top - view_top);
                placements.push(VisibleImagePlacement {
                    image_index,
                    cols: cols.max(1),
                    total_rows,
                    visible_rows,
                    hidden_rows_top,
                    x,
                    y,
                });
            }

            idx += total_rows;
        }

        placements
    }

    fn image_line_x_offset(&self, line_width: usize, content_width: usize) -> usize {
        let mut x = if self.line_numbers {
            LINE_NUMBER_COLS
        } else {
            0
        };
        if self.center_blocks && !self.line_numbers && line_width < content_width {
            x += (content_width - line_width) / 2;
        }
        x
    }

    fn collect_pending_kitty_uploads(
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
            if !entry.uploaded {
                if let Some(png_data) = entry.png_data.clone() {
                    uploads.push(KittyUpload {
                        image_id: entry.kitty_image_id,
                        png_data,
                    });
                    entry.uploaded = true;
                }
            }
        }
        uploads
    }

    fn kitty_image_meta(&mut self, image_index: usize) -> Option<(u32, u32)> {
        let image = self.doc.images.get(image_index).cloned()?;
        let entry = self.cached_image_entry(&image);
        let (_, pixel_height) = entry.pixel_size?;
        Some((entry.kitty_image_id, pixel_height))
    }

    pub fn set_status(&mut self, message: String, is_error: bool) {
        self.status = message;
        self.status_is_error = is_error;
        self.last_status_at = Instant::now();
    }

    pub fn on_tick(&mut self) {
        if self.last_status_at.elapsed() > Duration::from_secs(5) && !self.status.is_empty() {
            self.status.clear();
            self.status_is_error = false;
        }
    }

    fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    pub fn take_force_redraw(&mut self) -> bool {
        std::mem::take(&mut self.force_redraw)
    }

    pub fn on_event(&mut self, event: Event, viewport_height: usize, width: u16) {
        self.viewport_width = width as usize;
        let top_inset =
            WINDOW_PAD_TOP as usize + usize::from(self.top_frontmatter_title().is_some());
        match event {
            Event::Key(key) => self.on_key(key, viewport_height, width),
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollDown => {
                    self.move_down(self.smooth_scroll.max(1), viewport_height)
                }
                MouseEventKind::ScrollUp => {
                    self.move_up(self.smooth_scroll.max(1), viewport_height)
                }
                MouseEventKind::Moved => {
                    if !self.mouse {
                        self.hover_target = None;
                        return;
                    }
                    let row = mouse.row as usize;
                    let col = mouse.column as usize;
                    let pad_x = WINDOW_PAD_X as usize;
                    if row < top_inset || row >= top_inset + viewport_height {
                        self.hover_target = None;
                        return;
                    }
                    if col < pad_x || col >= pad_x + width as usize {
                        self.hover_target = None;
                        return;
                    }
                    let inner_row = row - top_inset;
                    let inner_col = col - pad_x;
                    let vis_row = self.offset + inner_row;
                    let (idx, row_in_line) =
                        self.line_and_inner_row_at_visual_row(vis_row, width as usize);
                    let target =
                        self.detect_click_target(idx, inner_col, width as usize, row_in_line);
                    self.hover_target = target.map(|t| {
                        let hover = match t {
                            ClickTarget::Link { link_idx, .. } => HoverTarget::Link(link_idx),
                            ClickTarget::Code(block_idx) => HoverTarget::Code(block_idx),
                        };
                        (vis_row, hover)
                    });
                }
                MouseEventKind::Down(MouseButton::Left) => {
                    if !self.mouse {
                        return;
                    }
                    let row = mouse.row as usize;
                    let col = mouse.column as usize;
                    let pad_x = WINDOW_PAD_X as usize;
                    if row < top_inset || row >= top_inset + viewport_height {
                        return;
                    }
                    if col < pad_x || col >= pad_x + width as usize {
                        return;
                    }
                    let inner_row = row - top_inset;
                    let inner_col = col - pad_x;
                    let vis_row = self.offset + inner_row;
                    let (idx, row_in_line) =
                        self.line_and_inner_row_at_visual_row(vis_row, width as usize);
                    self.selected_line = idx;
                    self.ensure_visible(viewport_height);
                    match self.detect_click_target(idx, inner_col, width as usize, row_in_line) {
                        Some(ClickTarget::Link { url, .. }) => {
                            if let Err(err) = self.prompt_open_selected_link(url) {
                                self.set_status(format!("Open link error: {err}"), true);
                            }
                        }
                        Some(ClickTarget::Code(block_idx)) => {
                            if let Err(err) = self.copy_code_block_by_index(block_idx) {
                                self.set_status(format!("Copy failed: {err}"), true);
                            }
                        }
                        None => {}
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn on_key(&mut self, key: KeyEvent, viewport_height: usize, width: u16) {
        if self.handle_overlay_key(key) {
            return;
        }
        match self.mode {
            Mode::Search => self.on_search_key(key, viewport_height),
            Mode::Pager => self.on_pager_key(key, viewport_height, width),
        }
    }

    fn on_search_key(&mut self, key: KeyEvent, viewport_height: usize) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Pager;
            }
            KeyCode::Enter => {
                self.mode = Mode::Pager;
                self.find_next(viewport_height);
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.recompute_search_hits();
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.search_query.push(c);
                self.recompute_search_hits();
            }
            _ => {}
        }
    }

    fn on_pager_key(&mut self, key: KeyEvent, viewport_height: usize, width: u16) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                self.should_quit = true;
                return;
            }
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc if self.return_to_dashboard_on_esc => {
                self.should_return_to_dashboard = true;
                self.should_quit = true;
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_down(1, viewport_height),
            KeyCode::Up | KeyCode::Char('k') => self.move_up(1, viewport_height),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                self.move_down(viewport_height.saturating_sub(1), viewport_height)
            }
            KeyCode::PageUp => self.move_up(viewport_height.saturating_sub(1), viewport_height),
            KeyCode::Home | KeyCode::Char('g') => self.goto_top(),
            KeyCode::End | KeyCode::Char('G') => self.goto_bottom(viewport_height),
            KeyCode::Char('/') => {
                self.mode = Mode::Search;
                self.search_query.clear();
                self.search_hits.clear();
                self.search_index = 0;
                self.status.clear();
                self.status_is_error = false;
            }
            KeyCode::Char('n') => self.find_next(viewport_height),
            KeyCode::Char('N') => self.find_prev(viewport_height),
            KeyCode::Char('?') => {
                self.overlay = Some(Overlay::Help);
            }
            KeyCode::Char('e') => {
                if let Err(err) = self.open_editor_and_reload(width) {
                    self.set_status(format!("Editor error: {err}"), true);
                }
            }
            KeyCode::Char('o') => {
                if let Err(err) = self.prompt_open_selected_line_link() {
                    self.set_status(format!("Open link error: {err}"), true);
                }
            }
            KeyCode::Char('y') => {
                if let Err(err) = self.copy_selected_code_block() {
                    self.set_status(format!("Copy failed: {err}"), true);
                }
            }
            KeyCode::Char('r') => {
                if let Err(err) = self.reload(width) {
                    self.set_status(format!("Reload failed: {err}"), true);
                }
            }
            KeyCode::Char('m') => {
                if let Err(err) = self.toggle_mouse_capture_mode() {
                    self.set_status(format!("Mouse mode error: {err}"), true);
                }
            }
            _ => {}
        }
    }

    fn toggle_mouse_capture_mode(&mut self) -> anyhow::Result<()> {
        if !self.mouse_allowed {
            return Err(anyhow::anyhow!("mouse support disabled in config"));
        }
        let mut stdout = std::io::stdout();
        if self.mouse {
            execute!(stdout, DisableMouseCapture).context("disable mouse capture")?;
            self.mouse = false;
            self.hover_target = None;
            self.set_status("selection mode on".to_string(), false);
        } else {
            execute!(stdout, EnableMouseCapture).context("enable mouse capture")?;
            self.mouse = true;
            self.set_status("selection mode off".to_string(), false);
        }
        Ok(())
    }

    fn move_down(&mut self, n: usize, viewport_height: usize) {
        if self.line_highlight {
            let last = self.doc.lines.len().saturating_sub(1);
            self.selected_line = (self.selected_line + n).min(last);
            self.ensure_visible(viewport_height);
            self.update_active_search_match();
        } else {
            self.scroll_down(n.max(1), viewport_height);
        }
    }

    fn move_up(&mut self, n: usize, viewport_height: usize) {
        if self.line_highlight {
            self.selected_line = self.selected_line.saturating_sub(n);
            self.ensure_visible(viewport_height);
            self.update_active_search_match();
        } else {
            self.scroll_up(n.max(1), viewport_height);
        }
    }

    fn goto_top(&mut self) {
        self.offset = 0;
        if self.line_highlight {
            self.selected_line = 0;
        } else {
            self.sync_selected_with_offset();
        }
        self.update_active_search_match();
    }

    fn goto_bottom(&mut self, viewport_height: usize) {
        if self.line_highlight {
            let last = self.doc.lines.len().saturating_sub(1);
            self.selected_line = last;
            self.ensure_visible(viewport_height);
        } else {
            self.offset = self.max_offset(viewport_height);
            self.sync_selected_with_offset();
        }
        self.update_active_search_match();
    }

    fn scroll_down(&mut self, n: usize, viewport_height: usize) {
        self.offset = (self.offset + n).min(self.max_offset(viewport_height));
        self.sync_selected_with_offset();
    }

    fn scroll_up(&mut self, n: usize, _viewport_height: usize) {
        self.offset = self.offset.saturating_sub(n);
        self.sync_selected_with_offset();
    }

    fn max_offset(&self, viewport_height: usize) -> usize {
        let total = self.total_visual_rows(self.viewport_width.max(1));
        total.saturating_sub(viewport_height.max(1))
    }

    fn sync_selected_with_offset(&mut self) {
        if self.doc.lines.is_empty() {
            self.selected_line = 0;
            return;
        }
        let (idx, _) =
            self.line_and_inner_row_at_visual_row(self.offset, self.viewport_width.max(1));
        self.selected_line = idx;
        self.update_active_search_match();
    }

    fn ensure_visible(&mut self, viewport_height: usize) {
        if self.doc.lines.is_empty() {
            self.offset = 0;
            self.selected_line = 0;
            return;
        }
        let width = self.viewport_width.max(1);
        let selected_top = self.visual_row_of_line(self.selected_line, width);
        let selected_height = self.line_visual_height(self.selected_line, width);
        let selected_bottom = selected_top + selected_height.saturating_sub(1);
        if selected_top < self.offset {
            self.offset = selected_top;
            return;
        }
        let viewport_bottom = self.offset + viewport_height.saturating_sub(1);
        if selected_bottom > viewport_bottom {
            self.offset = selected_bottom.saturating_sub(viewport_height.saturating_sub(1));
        }
    }

    fn recompute_search_hits(&mut self) {
        self.search_hits.clear();
        self.search_index = 0;
        let q = self.search_query.trim();
        if q.is_empty() {
            return;
        }
        let needle = q.to_lowercase();
        for (i, line) in self.doc.lines.iter().enumerate() {
            if matches!(line.kind, LineKind::Code | LineKind::Math | LineKind::Image) {
                continue;
            }
            let s = line
                .line
                .spans
                .iter()
                .map(|sp| sp.content.as_ref())
                .collect::<String>();
            if s.to_lowercase().contains(&needle) {
                self.search_hits.push(i);
            }
        }
        if !self.search_hits.is_empty() {
            self.selected_line = self.search_hits[0];
            self.search_index = 0;
            self.update_active_search_match();
        }
    }

    fn update_active_search_match(&mut self) {
        if self.search_hits.is_empty() {
            return;
        }
        if let Some((idx, _)) = self
            .search_hits
            .iter()
            .enumerate()
            .find(|(_, line)| **line == self.selected_line)
        {
            self.search_index = idx;
            self.set_status(
                format!("{}/{}", self.search_index + 1, self.search_hits.len()),
                false,
            );
        }
    }

    fn find_next(&mut self, viewport_height: usize) {
        if self.search_hits.is_empty() {
            self.set_status("No matches".to_string(), true);
            return;
        }
        self.search_index = (self.search_index + 1) % self.search_hits.len();
        self.selected_line = self.search_hits[self.search_index];
        self.ensure_visible(viewport_height);
        self.set_status(
            format!("{}/{}", self.search_index + 1, self.search_hits.len()),
            false,
        );
    }

    fn find_prev(&mut self, viewport_height: usize) {
        if self.search_hits.is_empty() {
            self.set_status("No matches".to_string(), true);
            return;
        }
        if self.search_index == 0 {
            self.search_index = self.search_hits.len() - 1;
        } else {
            self.search_index -= 1;
        }
        self.selected_line = self.search_hits[self.search_index];
        self.ensure_visible(viewport_height);
        self.set_status(
            format!("{}/{}", self.search_index + 1, self.search_hits.len()),
            false,
        );
    }

    fn open_editor_and_reload(&mut self, width: u16) -> anyhow::Result<()> {
        let Some(path) = self.source_path.clone() else {
            return Err(anyhow::anyhow!(
                "no file path available; open editor is file-only"
            ));
        };
        // Fully suspend TUI before spawning editor, then restore like glow.
        disable_raw_mode().ok();
        let mut stdout = std::io::stdout();
        execute!(stdout, DisableMouseCapture, LeaveAlternateScreen).ok();
        let editor_result = open_in_editor(&path);
        enable_raw_mode().context("re-enable raw mode after editor")?;
        execute!(stdout, EnterAlternateScreen).context("re-enter alternate screen")?;
        self.invalidate_kitty_upload_state();
        if self.mouse {
            execute!(stdout, EnableMouseCapture).ok();
        }
        editor_result?;
        self.source_markdown = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        self.reload(width)?;
        self.set_status("reloaded".to_string(), false);
        self.force_redraw = true;
        Ok(())
    }

    fn copy_selected_code_block(&mut self) -> anyhow::Result<()> {
        let idx = self
            .resolve_selected_code_block_index()
            .ok_or_else(|| anyhow::anyhow!("selected line is not part of a code block"))?;
        self.copy_code_block_by_index(idx)
    }

    fn copy_code_block_by_index(&mut self, idx: usize) -> anyhow::Result<()> {
        let code = self
            .doc
            .code_blocks
            .get(idx)
            .ok_or_else(|| anyhow::anyhow!("code block index out of bounds"))?;
        if !write_via_clipboard_cmd("wl-copy", &[], code)? {
            return Err(anyhow::anyhow!(
                "clipboard unavailable; install wl-copy for copy support"
            ));
        }
        self.set_status("copied".to_string(), false);
        Ok(())
    }

    fn resolve_selected_code_block_index(&self) -> Option<usize> {
        if let Some(idx) = self
            .doc
            .lines
            .get(self.selected_line)
            .and_then(|l| l.code_block_index)
        {
            return Some(idx);
        }
        if self.selected_line > 0 {
            if let Some(idx) = self
                .doc
                .lines
                .get(self.selected_line - 1)
                .and_then(|l| l.code_block_index)
            {
                return Some(idx);
            }
        }
        self.doc
            .lines
            .get(self.selected_line + 1)
            .and_then(|l| l.code_block_index)
    }

    fn prompt_open_selected_line_link(&mut self) -> anyhow::Result<()> {
        let line = self
            .doc
            .lines
            .get(self.selected_line)
            .ok_or_else(|| anyhow::anyhow!("no selected line"))?;
        let url = line
            .link_ranges
            .first()
            .map(|r| r.url.clone())
            .or_else(|| line.link_url.clone())
            .ok_or_else(|| anyhow::anyhow!("no link found on selected line"))?;
        self.prompt_open_selected_link(url)
    }

    fn prompt_open_selected_link(&mut self, url: String) -> anyhow::Result<()> {
        if self.link_confirm {
            self.overlay = Some(Overlay::LinkConfirm(url.clone()));
            self.set_status("link ready".to_string(), false);
            Ok(())
        } else {
            webbrowser::open(&url).context("failed opening browser")?;
            self.set_status(format!("opened {url}"), false);
            Ok(())
        }
    }

    fn handle_overlay_key(&mut self, key: KeyEvent) -> bool {
        let Some(overlay) = self.overlay.clone() else {
            return false;
        };
        match overlay {
            Overlay::Help => match key.code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter => {
                    self.overlay = None;
                    true
                }
                _ => true,
            },
            Overlay::LinkConfirm(url) => match key.code {
                KeyCode::Esc => {
                    self.overlay = None;
                    self.status.clear();
                    self.status_is_error = false;
                    true
                }
                KeyCode::Enter => {
                    self.overlay = None;
                    match webbrowser::open(&url) {
                        Ok(_) => self.set_status(format!("opened {url}"), false),
                        Err(err) => self.set_status(format!("Open link error: {err}"), true),
                    }
                    true
                }
                _ => true,
            },
        }
    }

    fn detect_click_target(
        &self,
        line_idx: usize,
        col: usize,
        total_width: usize,
        row_in_line: usize,
    ) -> Option<ClickTarget> {
        let line = self.doc.lines.get(line_idx)?;
        let logical_col = col + row_in_line.saturating_mul(total_width.max(1));
        let mut x = if self.line_numbers {
            LINE_NUMBER_COLS
        } else {
            0
        };
        if matches!(line.kind, LineKind::Quote) {
            x += QUOTE_PREFIX_WIDTH;
        }
        if self.center_blocks
            && !self.line_numbers
            && matches!(
                line.kind,
                LineKind::Table
                    | LineKind::Quote
                    | LineKind::Math
                    | LineKind::Ascii
                    | LineKind::Image
            )
        {
            let content = line
                .line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            let width = UnicodeWidthStr::width(content.as_str());
            if width < total_width {
                x += (total_width - width) / 2;
            }
        }

        if !line.link_ranges.is_empty() {
            for (idx, lr) in line.link_ranges.iter().enumerate() {
                let start = x + lr.start;
                let end = x + lr.end;
                if logical_col >= start && logical_col < end {
                    return Some(ClickTarget::Link {
                        url: lr.url.clone(),
                        link_idx: idx,
                    });
                }
            }
        }

        if let Some(block_idx) = line.code_block_index {
            let text = line
                .line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            let leading = &text[..text.len().saturating_sub(text.trim_start().len())];
            let trimmed = text.trim_end();
            let start = x + UnicodeWidthStr::width(leading);
            let end = x + UnicodeWidthStr::width(trimmed);
            if end > start && logical_col >= start && logical_col < end {
                return Some(ClickTarget::Code(block_idx));
            }
        }
        None
    }

    fn line_display_width(&self, line_idx: usize, area_width: usize) -> usize {
        let Some(line) = self.doc.lines.get(line_idx) else {
            return 0;
        };
        let mut width = line
            .line
            .spans
            .iter()
            .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
            .sum::<usize>();
        if matches!(line.kind, LineKind::Quote) {
            width += QUOTE_PREFIX_WIDTH;
        }
        if self.line_numbers {
            width += LINE_NUMBER_COLS;
        }
        if self.center_blocks
            && !self.line_numbers
            && matches!(
                line.kind,
                LineKind::Table
                    | LineKind::Quote
                    | LineKind::Math
                    | LineKind::Ascii
                    | LineKind::Image
            )
            && width < area_width
        {
            width += (area_width - width) / 2;
        }
        width.max(1)
    }

    fn line_visual_height(&self, line_idx: usize, area_width: usize) -> usize {
        if !self.wrap || area_width == 0 {
            return 1;
        }
        if self.should_prefix_wrapped_quote_line(line_idx, area_width) {
            let text = self.doc.lines[line_idx]
                .line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            return wrap_text_for_quote_fragments(
                &text,
                area_width.saturating_sub(QUOTE_PREFIX_WIDTH).max(1),
            )
            .len()
            .max(1);
        }
        if self.should_justify_wrapped_line(line_idx, area_width) {
            let text = self.doc.lines[line_idx]
                .line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            return wrap_text_for_justified_fragments(&text, area_width)
                .len()
                .max(1);
        }
        let width = self.line_display_width(line_idx, area_width);
        width.div_ceil(area_width).max(1)
    }

    fn should_justify_wrapped_line(&self, line_idx: usize, area_width: usize) -> bool {
        if !self.wrap || self.line_numbers || area_width == 0 {
            return false;
        }
        let Some(line) = self.doc.lines.get(line_idx) else {
            return false;
        };
        if line.kind != LineKind::Normal || !line.link_ranges.is_empty() {
            return false;
        }
        let Some(first) = line.line.spans.first() else {
            return false;
        };
        if line.line.spans.iter().any(|s| s.style != first.style) {
            return false;
        }
        let text = line
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        if text.trim().is_empty() || is_list_like_text(&text) {
            return false;
        }
        UnicodeWidthStr::width(text.as_str()) > area_width
    }

    fn should_prefix_wrapped_quote_line(&self, line_idx: usize, area_width: usize) -> bool {
        if !self.wrap || self.line_numbers || area_width <= QUOTE_PREFIX_WIDTH {
            return false;
        }
        let Some(line) = self.doc.lines.get(line_idx) else {
            return false;
        };
        if line.kind != LineKind::Quote || !line.link_ranges.is_empty() {
            return false;
        }
        let text = line
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        UnicodeWidthStr::width(text.as_str()) > area_width.saturating_sub(QUOTE_PREFIX_WIDTH)
    }

    fn visual_row_of_line(&self, line_idx: usize, area_width: usize) -> usize {
        let mut rows = 0usize;
        for i in 0..line_idx.min(self.doc.lines.len()) {
            rows += self.line_visual_height(i, area_width);
        }
        rows
    }

    fn line_and_inner_row_at_visual_row(&self, row: usize, area_width: usize) -> (usize, usize) {
        if self.doc.lines.is_empty() {
            return (0, 0);
        }
        let mut cur = 0usize;
        for i in 0..self.doc.lines.len() {
            let h = self.line_visual_height(i, area_width.max(1));
            if row < cur + h {
                return (i, row - cur);
            }
            cur += h;
        }
        let last = self.doc.lines.len().saturating_sub(1);
        (
            last,
            self.line_visual_height(last, area_width.max(1))
                .saturating_sub(1),
        )
    }

    fn total_visual_rows(&self, area_width: usize) -> usize {
        self.content_visual_rows(area_width) + END_PADDING_ROWS
    }

    fn content_visual_rows(&self, area_width: usize) -> usize {
        if self.doc.lines.is_empty() {
            return 0;
        }
        (0..self.doc.lines.len())
            .map(|i| self.line_visual_height(i, area_width.max(1)))
            .sum()
    }
}

fn resolve_image_source(raw: &str, markdown_path: Option<&Path>) -> ResolvedImageSource {
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

fn resolved_image_cache_key(source: &ResolvedImageSource) -> String {
    match source {
        ResolvedImageSource::Local(path) => format!("file:{}", path.display()),
        ResolvedImageSource::Url(url) => format!("url:{url}"),
    }
}

fn compute_image_layout(
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

fn pager_body_area(area: Rect, has_top_title: bool) -> Rect {
    let top_height = u16::from(has_top_title);
    let content_chunk = Rect {
        x: area.x,
        y: area.y.saturating_add(top_height),
        width: area.width,
        height: area.height.saturating_sub(top_height.saturating_add(1)),
    };
    Rect {
        x: content_chunk.x.saturating_add(WINDOW_PAD_X),
        y: content_chunk.y.saturating_add(WINDOW_PAD_TOP),
        width: content_chunk.width.saturating_sub(WINDOW_PAD_X * 2),
        height: content_chunk.height.saturating_sub(WINDOW_PAD_TOP),
    }
}

fn load_kitty_image_payload(source: &ResolvedImageSource) -> Result<(Vec<u8>, u32, u32), String> {
    let decoded = load_dynamic_image(source)?;
    let width = decoded.width().max(1);
    let height = decoded.height().max(1);
    let mut png_data = Vec::new();
    decoded
        .write_to(&mut Cursor::new(&mut png_data), image::ImageFormat::Png)
        .map_err(|err| format!("failed encoding image payload: {err}"))?;
    Ok((png_data, width, height))
}

fn load_dynamic_image(source: &ResolvedImageSource) -> Result<DynamicImage, String> {
    let (bytes, content_type) = match source {
        ResolvedImageSource::Local(path) => std::fs::read(path)
            .map(|bytes| (bytes, None))
            .map_err(|err| format!("failed reading {}: {err}", path.display()))?,
        ResolvedImageSource::Url(url) => {
            let response = reqwest::blocking::get(url)
                .map_err(|err| format!("failed fetching {url}: {err}"))?;
            if !response.status().is_success() {
                return Err(format!("http {} for {url}", response.status()));
            }
            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(|value| value.to_string());
            let bytes = response
                .bytes()
                .map_err(|err| format!("failed reading response body for {url}: {err}"))?
                .to_vec();
            (bytes, content_type)
        }
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

fn source_is_svg_hint(source: &ResolvedImageSource) -> bool {
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

fn content_type_is_svg(content_type: Option<&str>) -> bool {
    content_type
        .map(|value| value.to_ascii_lowercase().contains("image/svg+xml"))
        .unwrap_or(false)
}

fn bytes_look_like_svg(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]);
    head.to_ascii_lowercase().contains("<svg")
}

fn decode_svg_to_image(bytes: &[u8]) -> Result<DynamicImage, String> {
    let mut options = usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
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

fn kitty_delete_all_placements(stdout: &mut impl Write) -> std::io::Result<()> {
    stdout.write_all(b"\x1b_Ga=d,d=a,q=2\x1b\\")
}

fn kitty_transmit_png(
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

fn kitty_move_cursor(stdout: &mut impl Write, row: usize, col: usize) -> std::io::Result<()> {
    write!(stdout, "\x1b[{};{}H", row, col)
}

fn kitty_place_image(
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

fn draw_top_title(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
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

fn draw_body(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let mut lines: Vec<Line> = Vec::with_capacity(state.doc.lines.len());

    for idx in 0..state.doc.lines.len() {
        let RenderLine {
            line,
            kind,
            link_url: _,
            link_ranges: _,
            code_block_index: _,
            image_index: _,
            heading_level,
        } = &state.doc.lines[idx];

        let mut spans = line.spans.clone();
        if matches!(kind, LineKind::Heading) {
            let heading_style = match heading_level.unwrap_or(6) {
                1 => state.theme.heading_h1,
                2 => state.theme.heading_h2,
                _ => state.theme.heading_h3,
            };
            for s in &mut spans {
                s.style = s.style.patch(heading_style);
            }
        }
        if matches!(kind, LineKind::Quote) {
            for s in &mut spans {
                s.style = s.style.add_modifier(Modifier::ITALIC);
            }
        }
        if let Some((hover_row, target)) = &state.hover_target {
            let row_start = state.visual_row_of_line(idx, area.width as usize);
            let row_h = state.line_visual_height(idx, area.width as usize);
            let hovered_this_line = *hover_row >= row_start && *hover_row < row_start + row_h;
            match target {
                HoverTarget::Link(link_idx)
                    if hovered_this_line
                        && state.doc.lines[idx].link_ranges.get(*link_idx).is_some() =>
                {
                    let lr = &state.doc.lines[idx].link_ranges[*link_idx];
                    let mut cur = 0usize;
                    for s in &mut spans {
                        let w = UnicodeWidthStr::width(s.content.as_ref());
                        let span_start = cur;
                        let span_end = cur + w;
                        let overlap = span_start < lr.end && span_end > lr.start;
                        if overlap {
                            let bright = brighten_color(
                                state
                                    .theme
                                    .link
                                    .fg
                                    .unwrap_or(state.theme.normal.fg.unwrap_or(Color::White)),
                            );
                            s.style = s.style.fg(bright);
                        }
                        cur += w;
                    }
                }
                HoverTarget::Code(hover_block_idx)
                    if state.doc.lines[idx].code_block_index == Some(*hover_block_idx) =>
                {
                    for s in &mut spans {
                        let base = s
                            .style
                            .fg
                            .unwrap_or(state.theme.normal.fg.unwrap_or(Color::White));
                        s.style = s.style.fg(brighten_color(base));
                    }
                }
                _ => {}
            }
        }
        if idx == state.selected_line && state.line_highlight {
            for s in &mut spans {
                s.style = s.style.patch(state.theme.cursor_line);
            }
        }

        if state.should_prefix_wrapped_quote_line(idx, area.width as usize) {
            let base_style = spans.first().map(|s| s.style).unwrap_or(state.theme.normal);
            let text = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
            let wrapped_rows = wrap_text_for_quote_fragments(
                &text,
                (area.width as usize)
                    .saturating_sub(QUOTE_PREFIX_WIDTH)
                    .max(1),
            );
            let search_style = if state.search_hits.get(state.search_index).copied() == Some(idx) {
                state.theme.search_current
            } else {
                state.theme.search_hit
            };
            let mut bar_style = state.theme.list_marker.remove_modifier(Modifier::BOLD);
            if idx == state.selected_line && state.line_highlight {
                bar_style = bar_style.patch(state.theme.cursor_line);
            }
            for row in wrapped_rows {
                let mut row_text_spans = vec![Span::styled(row, base_style)];
                if !state.search_query.is_empty() {
                    row_text_spans = highlight_spans_for_search(
                        &row_text_spans,
                        &state.search_query,
                        search_style,
                    );
                }
                let mut row_spans = vec![Span::styled(QUOTE_PREFIX.to_string(), bar_style)];
                row_spans.extend(row_text_spans);
                lines.push(Line::from(row_spans));
            }
            continue;
        }

        if state.should_justify_wrapped_line(idx, area.width as usize) {
            let base_style = spans.first().map(|s| s.style).unwrap_or(state.theme.normal);
            let text = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
            let wrapped_rows = wrap_text_for_justified_fragments(&text, area.width as usize);
            let search_style = if state.search_hits.get(state.search_index).copied() == Some(idx) {
                state.theme.search_current
            } else {
                state.theme.search_hit
            };
            for row in wrapped_rows {
                let mut row_spans = vec![Span::styled(row, base_style)];
                if !state.search_query.is_empty() {
                    row_spans =
                        highlight_spans_for_search(&row_spans, &state.search_query, search_style);
                }
                lines.push(Line::from(row_spans));
            }
            continue;
        }

        if !state.search_query.is_empty() && !matches!(kind, LineKind::Image) {
            spans = highlight_spans_for_search(
                &spans,
                &state.search_query,
                if state.search_hits.get(state.search_index).copied() == Some(idx) {
                    state.theme.search_current
                } else {
                    state.theme.search_hit
                },
            );
        }

        if state.line_numbers {
            let ln = format!("{:>5} ", idx + 1);
            spans.insert(0, Span::styled(ln, state.theme.line_number));
        }
        if matches!(kind, LineKind::Quote) {
            let bar_style = state.theme.list_marker.remove_modifier(Modifier::BOLD);
            let insert_at = if state.line_numbers { 1 } else { 0 };
            spans.insert(
                insert_at.min(spans.len()),
                Span::styled(QUOTE_PREFIX.to_string(), bar_style),
            );
        }
        if state.center_blocks
            && !state.line_numbers
            && matches!(
                kind,
                LineKind::Table
                    | LineKind::Quote
                    | LineKind::Math
                    | LineKind::Ascii
                    | LineKind::Image
            )
        {
            let content = spans.iter().map(|s| s.content.as_ref()).collect::<String>();
            let width = UnicodeWidthStr::width(content.as_str());
            let avail = area.width as usize;
            if width < avail {
                let pad = (avail - width) / 2;
                if pad > 0 {
                    spans.insert(0, Span::raw(" ".repeat(pad)));
                }
            }
        }
        lines.push(Line::from(spans));
    }
    for _ in 0..END_PADDING_ROWS {
        lines.push(Line::from(""));
    }

    let y_scroll = state.offset.min(u16::MAX as usize) as u16;
    let widget = Paragraph::new(lines)
        .block(Block::default().borders(Borders::NONE))
        .scroll((y_scroll, 0))
        .wrap(Wrap { trim: !state.wrap })
        .style(state.theme.normal);
    frame.render_widget(widget, area);

    // no side pointer: hover feedback is applied directly to hovered link spans
}

fn wrap_text_for_justified_fragments(text: &str, target_width: usize) -> Vec<String> {
    if target_width == 0 {
        return vec![text.to_string()];
    }

    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return vec![String::new()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in words {
        for chunk in split_word_chunks(word, target_width) {
            let chunk_width = UnicodeWidthStr::width(chunk.as_str());
            if current.is_empty() {
                current = chunk;
                current_width = chunk_width;
            } else if current_width + 1 + chunk_width <= target_width {
                current.push(' ');
                current.push_str(&chunk);
                current_width += 1 + chunk_width;
            } else {
                lines.push(current);
                current = chunk;
                current_width = chunk_width;
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.len() > 1 {
        let last_idx = lines.len() - 1;
        for line in lines.iter_mut().take(last_idx) {
            if let Some(justified) = justify_text_to_width(line, target_width) {
                *line = justified;
            }
        }
    }

    lines
}

fn wrap_text_for_quote_fragments(text: &str, target_width: usize) -> Vec<String> {
    if text.is_empty() || target_width == 0 {
        return vec![text.to_string()];
    }

    let mut rows = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + ch_width > target_width && !current.is_empty() {
            rows.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(ch);
        current_width += ch_width;
    }

    if !current.is_empty() {
        rows.push(current);
    }
    if rows.is_empty() {
        rows.push(String::new());
    }

    rows
}

fn split_word_chunks(word: &str, target_width: usize) -> Vec<String> {
    if word.is_empty() || target_width == 0 {
        return vec![word.to_string()];
    }
    if UnicodeWidthStr::width(word) <= target_width {
        return vec![word.to_string()];
    }

    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut width = 0usize;

    for ch in word.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if ch_width > 0 && width + ch_width > target_width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            width = 0;
        }
        current.push(ch);
        width += ch_width;
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

fn is_list_like_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with("• ") || trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        return true;
    }
    let mut chars = trimmed.chars().peekable();
    let mut saw_digit = false;
    while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
        saw_digit = true;
        chars.next();
    }
    saw_digit && chars.next() == Some('.') && chars.next() == Some(' ')
}

fn justify_text_to_width(text: &str, target_width: usize) -> Option<String> {
    let current_width = UnicodeWidthStr::width(text);
    if current_width >= target_width {
        return None;
    }

    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return None;
    }

    let mut gaps: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].1 != ' ' {
            i += 1;
            continue;
        }
        let run_start = chars[i].0;
        let mut j = i + 1;
        while j < chars.len() && chars[j].1 == ' ' {
            j += 1;
        }
        let run_end = if j < chars.len() {
            chars[j].0
        } else {
            text.len()
        };
        let has_left_word = i > 0 && chars[i - 1].1 != ' ';
        let has_right_word = j < chars.len() && chars[j].1 != ' ';
        if has_left_word && has_right_word {
            gaps.push((run_start, run_end));
        }
        i = j;
    }

    if gaps.is_empty() {
        return None;
    }

    let extra = target_width - current_width;
    // Avoid over-stretched rows that look visually broken.
    if gaps.len() < 2 || extra > gaps.len() * 2 {
        return None;
    }
    let base_add = extra / gaps.len();
    let remainder = extra % gaps.len();
    let mut out = String::with_capacity(text.len() + extra);
    let mut cursor = 0usize;

    for (idx, (start, end)) in gaps.into_iter().enumerate() {
        out.push_str(&text[cursor..start]);
        out.push_str(&text[start..end]);
        let add = base_add + usize::from(idx < remainder);
        if add > 0 {
            out.push_str(&" ".repeat(add));
        }
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    Some(out)
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, body_height: usize, state: &AppState) {
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

fn status_progress_text(state: &AppState, body_height: usize) -> String {
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

fn compose_status_line(left: &str, right: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let right_w = UnicodeWidthStr::width(right);
    if right_w >= width {
        return truncate_to_width(right, width);
    }
    let left_budget = width.saturating_sub(right_w + 1);
    let left_fit = truncate_to_width(left, left_budget);
    let left_w = UnicodeWidthStr::width(left_fit.as_str());
    let gap = width.saturating_sub(left_w + right_w);
    format!("{left_fit}{}{}", " ".repeat(gap), right)
}

fn compose_status_line_with_center(left: &str, center: &str, right: &str, width: usize) -> String {
    let base = compose_status_line(left, right, width);
    if width == 0 {
        return base;
    }
    let center_fit = truncate_to_width(center, width);
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

fn truncate_to_width(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 3 {
        let mut out = String::new();
        let mut w = 0usize;
        for ch in text.chars() {
            let cw = ch.width().unwrap_or(0);
            if w + cw > max_width {
                break;
            }
            out.push(ch);
            w += cw;
        }
        return out;
    }
    let target = max_width - 3;
    let mut out = String::new();
    let mut w = 0usize;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if w + cw > target {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out.push_str("...");
    out
}

fn draw_overlay(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
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
                kb_line(state, "m", "toggle selection mode"),
                kb_line(state, "?", "toggle help"),
                kb_line(state, "Esc/Enter", "close popup"),
            ]);
            ("KEYBINDINGS", lines, 54, 16)
        }
        Overlay::LinkConfirm(url) => {
            let url_w = UnicodeWidthStr::width(url.as_str()).min(64) as u16 + 8;
            (
                "OPEN LINK",
                vec![
                    Line::from(vec![Span::styled(url.to_string(), state.theme.link)]),
                    link_actions_line(state),
                ],
                url_w.max(50).min(72),
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

fn centered_rect_size(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(3);
    let height = height.min(area.height.saturating_sub(2)).max(3);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn kb_line(state: &AppState, key: &str, desc: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<18}", key),
            state.theme.popup_key.remove_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(desc.to_string(), state.theme.normal),
    ])
}

fn brighten_color(color: Color) -> Color {
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

fn centered_text(text: &str, width: usize) -> String {
    let w = UnicodeWidthStr::width(text);
    if w >= width {
        return text.to_string();
    }
    let pad = (width - w) / 2;
    format!("{}{}", " ".repeat(pad), text)
}

fn center_block_lines(
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

fn link_actions_line(state: &AppState) -> Line<'static> {
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

fn center_link_popup_lines(
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

fn write_via_clipboard_cmd(cmd: &str, args: &[&str], text: &str) -> anyhow::Result<bool> {
    let spawn = std::process::Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = spawn else {
        return Ok(false);
    };
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin
            .write_all(text.as_bytes())
            .with_context(|| format!("failed writing to {cmd}"))?;
    }
    let status = child
        .wait()
        .with_context(|| format!("failed waiting for {cmd}"))?;
    Ok(status.success())
}

fn highlight_spans_for_search(
    spans: &[Span<'static>],
    query: &str,
    mark: ratatui::style::Style,
) -> Vec<Span<'static>> {
    let q = query.trim();
    if q.is_empty() {
        return spans.to_vec();
    }
    let mut out = Vec::new();
    for sp in spans {
        let text = sp.content.to_string();
        let lower = text.to_lowercase();
        let needle = q.to_lowercase();
        let mut start = 0usize;
        while let Some(pos) = lower[start..].find(&needle) {
            let abs = start + pos;
            if abs > start {
                out.push(Span::styled(text[start..abs].to_string(), sp.style));
            }
            let end = abs + needle.len();
            out.push(Span::styled(
                text[abs..end].to_string(),
                sp.style.patch(mark),
            ));
            start = end;
        }
        if start < text.len() {
            out.push(Span::styled(text[start..].to_string(), sp.style));
        }
    }
    out
}

pub fn read_markdown_input(path: Option<&Path>) -> anyhow::Result<(String, Option<PathBuf>)> {
    match path {
        Some(p) => {
            let s = std::fs::read_to_string(p)
                .with_context(|| format!("failed reading '{}'", p.display()))?;
            Ok((s, Some(p.to_path_buf())))
        }
        None => {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("failed reading stdin")?;
            Ok((buf, None))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::AppTheme;
    use image::{DynamicImage, GrayImage, ImageFormat, Luma, Rgb, RgbImage};
    use tempfile::tempdir;
    use unicode_width::UnicodeWidthStr;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn justify_text_expands_internal_gaps_to_target_width() {
        let input = "alpha beta gamma";
        let out = super::justify_text_to_width(input, 20).expect("justified");
        assert_eq!(UnicodeWidthStr::width(out.as_str()), 20);
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
        assert!(out.contains("gamma"));
    }

    #[test]
    fn justify_text_skips_list_like_lines() {
        assert!(super::is_list_like_text("• item one"));
        assert!(super::is_list_like_text("  12. ordered item"));
        assert!(!super::is_list_like_text("plain paragraph line"));
    }

    #[test]
    fn justify_text_skips_overstretched_rows() {
        assert!(super::justify_text_to_width("alpha beta", 24).is_none());
    }

    #[test]
    fn wrapped_fragments_are_justified_except_last() {
        let rows = super::wrap_text_for_justified_fragments(
            "This is a very long line that should wrap and justify nicely",
            24,
        );
        assert!(rows.len() > 1);
        for row in rows.iter().take(rows.len() - 1) {
            assert_eq!(UnicodeWidthStr::width(row.as_str()), 24);
        }
        let last_width = UnicodeWidthStr::width(rows.last().expect("last row").as_str());
        assert!(last_width <= 24);
    }

    #[test]
    fn short_line_remains_single_row_without_justification() {
        let rows = super::wrap_text_for_justified_fragments("short line", 24);
        assert_eq!(rows, vec!["short line".to_string()]);
    }

    #[test]
    fn wrapped_quote_fragments_split_to_target_width() {
        let rows = super::wrap_text_for_quote_fragments("abcdefghij", 4);
        assert_eq!(
            rows,
            vec!["abcd".to_string(), "efgh".to_string(), "ij".to_string()]
        );
    }

    #[test]
    fn wrapped_quote_visual_height_counts_all_prefixed_rows() {
        let mut state = AppState::from_markdown(
            "> abcdefghij".to_string(),
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            false,
            true,
            80,
        )
        .expect("state");
        state.viewport_width = 80;
        let quote_idx = state
            .doc
            .lines
            .iter()
            .position(|l| l.kind == LineKind::Quote)
            .expect("quote line");
        assert_eq!(state.line_visual_height(quote_idx, 6), 3);
    }

    #[test]
    fn image_lines_are_materialized_with_italic_gray_title() {
        let dir = tempdir().expect("tempdir");
        let image_path = dir.path().join("preview.png");
        let markdown_path = dir.path().join("doc.md");

        let mut img = GrayImage::new(16, 8);
        for y in 0..8 {
            for x in 0..16 {
                img.put_pixel(x, y, Luma([((x * 16) as u8).saturating_add((y * 8) as u8)]));
            }
        }
        img.save(&image_path).expect("save image");

        let md = format!("![preview]({} \"Figure A\")", image_path.display());
        let state = AppState::from_markdown(
            md,
            Some(markdown_path),
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");

        let title_line = state
            .doc
            .lines
            .iter()
            .find(|l| {
                l.kind == LineKind::Image
                    && l.line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .contains("Figure A")
            })
            .expect("image title line");
        let style = title_line.line.spans[0].style;
        assert!(style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(style.fg, state.theme.line_number.fg);

        let title_idx = state
            .doc
            .lines
            .iter()
            .position(|l| {
                l.kind == LineKind::Image
                    && l.line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .contains("Figure A")
            })
            .expect("title index");
        let spacer_line = state
            .doc
            .lines
            .get(title_idx.saturating_sub(1))
            .expect("spacer line");
        assert!(spacer_line.image_index.is_none());
        let spacer_text = spacer_line
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(spacer_text.trim().is_empty());
        let image_before_spacer = state
            .doc
            .lines
            .get(title_idx.saturating_sub(2))
            .expect("image line before spacer");
        assert!(image_before_spacer.image_index.is_some());

        let image_row = state
            .doc
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Image && l.image_index.is_some())
            .expect("image content row");
        let row_text = image_row
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(row_text.trim().is_empty());
    }

    #[test]
    fn image_width_is_capped_to_forty_percent_of_content_width() {
        let dir = tempdir().expect("tempdir");
        let image_path = dir.path().join("wide.png");

        let mut img = GrayImage::new(220, 20);
        for y in 0..20 {
            for x in 0..220 {
                img.put_pixel(x, y, Luma([(x % 255) as u8]));
            }
        }
        img.save(&image_path).expect("save image");

        let md = format!("![wide]({})", image_path.display());
        let state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            100,
        )
        .expect("state");

        let max_image_width = state
            .doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Image)
            .map(|l| {
                UnicodeWidthStr::width(
                    l.line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .as_str(),
                )
            })
            .max()
            .unwrap_or(0);
        assert!(max_image_width <= 40);
    }

    #[test]
    fn image_width_attribute_overrides_default_width_ratio() {
        let dir = tempdir().expect("tempdir");
        let image_path = dir.path().join("wide.png");

        let mut img = GrayImage::new(260, 20);
        for y in 0..20 {
            for x in 0..260 {
                img.put_pixel(x, y, Luma([(x % 255) as u8]));
            }
        }
        img.save(&image_path).expect("save image");

        let md = format!("![wide]({}){{width=70%}}", image_path.display());
        let state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            100,
        )
        .expect("state");

        let max_image_width = state
            .doc
            .lines
            .iter()
            .filter(|l| l.kind == LineKind::Image && l.image_index.is_some())
            .map(|l| {
                UnicodeWidthStr::width(
                    l.line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .as_str(),
                )
            })
            .max()
            .unwrap_or(0);
        assert!(max_image_width <= 70);
        assert!(max_image_width > 40);
    }

    #[test]
    fn image_cache_is_reused_across_rerenders() {
        let dir = tempdir().expect("tempdir");
        let image_path = dir.path().join("cache.png");

        let mut img = GrayImage::new(32, 12);
        for y in 0..12 {
            for x in 0..32 {
                img.put_pixel(x, y, Luma([((x + y) % 255) as u8]));
            }
        }
        img.save(&image_path).expect("save image");

        let md = format!("![cache]({})", image_path.display());
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        assert_eq!(state.image_cache.len(), 1);
        state.rerender_for_width(120).expect("rerender");
        assert_eq!(state.image_cache.len(), 1);
    }

    #[test]
    fn kitty_upload_state_can_be_invalidated_after_screen_reset() {
        let dir = tempdir().expect("tempdir");
        let image_path = dir.path().join("upload.png");
        let mut img = GrayImage::new(20, 10);
        for y in 0..10 {
            for x in 0..20 {
                img.put_pixel(x, y, Luma([((x + y) % 255) as u8]));
            }
        }
        img.save(&image_path).expect("save image");

        let md = format!("![upload]({})", image_path.display());
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let placements = state.visible_image_placements(area);
        assert!(!placements.is_empty());
        let first_uploads = state.collect_pending_kitty_uploads(&placements);
        assert!(!first_uploads.is_empty());
        let second_uploads = state.collect_pending_kitty_uploads(&placements);
        assert!(second_uploads.is_empty());

        state.invalidate_kitty_upload_state();
        let third_uploads = state.collect_pending_kitty_uploads(&placements);
        assert!(!third_uploads.is_empty());
    }

    #[test]
    fn visible_image_placements_are_hidden_while_overlay_is_open() {
        let dir = tempdir().expect("tempdir");
        let image_path = dir.path().join("overlay-image.png");
        let mut img = GrayImage::new(20, 10);
        for y in 0..10 {
            for x in 0..20 {
                img.put_pixel(x, y, Luma([((x + y) % 255) as u8]));
            }
        }
        img.save(&image_path).expect("save image");

        let md = format!("![overlay]({})", image_path.display());
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");

        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        assert!(!state.visible_image_placements(area).is_empty());
        state.overlay = Some(Overlay::Help);
        assert!(state.visible_image_placements(area).is_empty());
    }

    #[test]
    fn local_svg_image_is_decoded_successfully() {
        let dir = tempdir().expect("tempdir");
        let image_path = dir.path().join("vector.svg");
        std::fs::write(
            &image_path,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40" viewBox="0 0 120 40">
  <rect x="0" y="0" width="120" height="40" fill="#111111"/>
  <circle cx="60" cy="20" r="14" fill="#f2f2f2"/>
</svg>"##,
        )
        .expect("write svg");

        let md = format!("![vector]({})", image_path.display());
        let state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");

        let cache_entry = state.image_cache.values().next().expect("cached image");
        assert!(cache_entry.pixel_size.is_some());
        let has_failure_placeholder = state.doc.lines.iter().any(|line| {
            line.kind == LineKind::Image
                && line
                    .line
                    .spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    .contains("[image load failed:")
        });
        assert!(!has_failure_placeholder);
    }

    #[test]
    fn missing_image_uses_bracketed_placeholder_text_and_title() {
        let dir = tempdir().expect("tempdir");
        let missing_path = dir.path().join("missing.png");
        let md = format!(
            "![missing alt]({} \"Missing Title\")",
            missing_path.display()
        );
        let state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");

        let placeholder_idx = state
            .doc
            .lines
            .iter()
            .position(|line| {
                line.kind == LineKind::Image
                    && line
                        .line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .contains("[missing alt]")
            })
            .expect("placeholder line index");
        let placeholder = state
            .doc
            .lines
            .get(placeholder_idx)
            .expect("placeholder line");
        assert!(placeholder.image_index.is_none());
        let placeholder_style = placeholder
            .line
            .spans
            .first()
            .expect("placeholder span")
            .style;
        assert!(!placeholder_style.add_modifier.contains(Modifier::ITALIC));
        let top_spacing = state
            .doc
            .lines
            .get(placeholder_idx.saturating_sub(1))
            .expect("top spacing line")
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(top_spacing.trim().is_empty());
        let bottom_spacing = state
            .doc
            .lines
            .get(placeholder_idx + 1)
            .expect("bottom spacing line")
            .line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(bottom_spacing.trim().is_empty());

        let title = state
            .doc
            .lines
            .iter()
            .find(|line| {
                line.kind == LineKind::Image
                    && line
                        .line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .contains("Missing Title")
            })
            .expect("title line");
        assert!(title.image_index.is_none());
    }

    #[test]
    fn missing_image_without_alt_uses_image_placeholder_label() {
        let dir = tempdir().expect("tempdir");
        let missing_path = dir.path().join("missing-no-alt.png");
        let md = format!("![]({})", missing_path.display());
        let state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");

        let placeholder = state
            .doc
            .lines
            .iter()
            .find(|line| {
                line.kind == LineKind::Image
                    && line
                        .line
                        .spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        .contains("[image]")
            })
            .expect("placeholder line");
        assert!(placeholder.image_index.is_none());
    }

    #[test]
    fn common_raster_formats_decode_consistently() {
        let dir = tempdir().expect("tempdir");
        let mut base = RgbImage::new(24, 12);
        for y in 0..12 {
            for x in 0..24 {
                base.put_pixel(
                    x,
                    y,
                    Rgb([
                        ((x * 7 + y * 11) % 255) as u8,
                        ((x * 13 + y * 5) % 255) as u8,
                        ((x * 3 + y * 17) % 255) as u8,
                    ]),
                );
            }
        }
        let dynamic = DynamicImage::ImageRgb8(base);
        let formats = [
            ("png", ImageFormat::Png),
            ("jpg", ImageFormat::Jpeg),
            ("gif", ImageFormat::Gif),
            ("webp", ImageFormat::WebP),
            ("bmp", ImageFormat::Bmp),
            ("tiff", ImageFormat::Tiff),
        ];

        for (ext, format) in formats {
            let path = dir.path().join(format!("sample.{ext}"));
            dynamic
                .save_with_format(&path, format)
                .expect("save test image");
            let source = ResolvedImageSource::Local(path);
            let (png_payload, width, height) =
                load_kitty_image_payload(&source).expect("decode+encode payload");
            assert!(!png_payload.is_empty());
            assert_eq!((width, height), (24, 12));
        }
    }

    #[test]
    fn search_hits_work() {
        let md = "hello\nworld\nhello world".to_string();
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        state.mode = Mode::Search;
        state.search_query = "hello".to_string();
        state.recompute_search_hits();
        assert_eq!(state.search_hits.len(), 2);
    }

    #[test]
    fn status_is_empty_when_idle() {
        let md = "text".to_string();
        let state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        assert!(state.status.is_empty());
    }

    #[test]
    fn copied_status_message_is_expected() {
        let md = "```rust\nfn main() {}\n```".to_string();
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        state.selected_line = state
            .doc
            .lines
            .iter()
            .position(|l| l.code_block_index.is_some())
            .expect("code line");
        let _ = state.copy_selected_code_block();
        if state.status.is_empty() {
            // Clipboard may be unavailable in CI; at least ensure no unrelated message.
            assert!(state.status.is_empty());
        } else {
            assert_eq!(state.status, "copied");
        }
    }

    #[test]
    fn resolve_code_block_index_from_neighbor_line() {
        let md = "```rust\nfn main() {}\n```\n".to_string();
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        let fence_blank = state
            .doc
            .lines
            .iter()
            .position(|l| l.code_block_index.is_none())
            .unwrap_or(0);
        state.selected_line = fence_blank;
        assert!(state.resolve_selected_code_block_index().is_some());
    }

    #[test]
    fn copy_command_wrapper_handles_missing_binary() {
        let out =
            super::write_via_clipboard_cmd("definitely-not-a-real-clipboard-tool", &[], "abc")
                .expect("wrapper should not error");
        assert!(!out);
    }

    #[test]
    fn visual_row_mapping_handles_wrapped_lines() {
        let md = "A very very very long line that should wrap in narrow view\nshort".to_string();
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            20,
        )
        .expect("state");
        state.viewport_width = 20;
        let first_h = state.line_visual_height(0, 20);
        assert!(first_h >= 2);
        let (idx0, inner0) = state.line_and_inner_row_at_visual_row(0, 20);
        assert_eq!((idx0, inner0), (0, 0));
        let (idx1, inner1) = state.line_and_inner_row_at_visual_row(1, 20);
        assert_eq!(idx1, 0);
        assert_eq!(inner1, 1);
        let (idx_next, inner_next) = state.line_and_inner_row_at_visual_row(first_h, 20);
        assert_eq!(idx_next, 1);
        assert_eq!(inner_next, 0);
    }

    #[test]
    fn detect_click_target_picks_correct_link_in_same_line() {
        use crate::renderer::{LinkRange, RenderLine};
        let mut state = AppState::from_markdown(
            "dummy".to_string(),
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        state.doc.lines = vec![RenderLine {
            line: Line::from(vec![Span::raw("A B".to_string())]),
            kind: LineKind::Normal,
            link_url: Some("https://a.test".to_string()),
            link_ranges: vec![
                LinkRange {
                    start: 0,
                    end: 1,
                    url: "https://a.test".to_string(),
                },
                LinkRange {
                    start: 2,
                    end: 3,
                    url: "https://b.test".to_string(),
                },
            ],
            code_block_index: None,
            image_index: None,
            heading_level: None,
        }];
        let first = state.detect_click_target(0, 0, 80, 0);
        let second = state.detect_click_target(0, 2, 80, 0);
        assert!(
            matches!(first, Some(ClickTarget::Link { ref url, .. }) if url == "https://a.test")
        );
        assert!(
            matches!(second, Some(ClickTarget::Link { ref url, .. }) if url == "https://b.test")
        );
    }

    #[test]
    fn quote_line_display_width_includes_accent_bar() {
        let mut state = AppState::from_markdown(
            "> hello".to_string(),
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            false,
            true,
            80,
        )
        .expect("state");
        state.viewport_width = 80;
        let quote_idx = state
            .doc
            .lines
            .iter()
            .position(|l| l.kind == LineKind::Quote)
            .expect("quote line");
        assert_eq!(
            state.line_display_width(quote_idx, 80),
            QUOTE_PREFIX_WIDTH + UnicodeWidthStr::width("hello")
        );
    }

    #[test]
    fn detect_click_target_for_quote_link_accounts_for_prefix() {
        use crate::renderer::{LinkRange, RenderLine};
        let mut state = AppState::from_markdown(
            "dummy".to_string(),
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            false,
            true,
            80,
        )
        .expect("state");
        state.doc.lines = vec![RenderLine {
            line: Line::from(vec![Span::raw("A B".to_string())]),
            kind: LineKind::Quote,
            link_url: Some("https://a.test".to_string()),
            link_ranges: vec![
                LinkRange {
                    start: 0,
                    end: 1,
                    url: "https://a.test".to_string(),
                },
                LinkRange {
                    start: 2,
                    end: 3,
                    url: "https://b.test".to_string(),
                },
            ],
            code_block_index: None,
            image_index: None,
            heading_level: None,
        }];
        let first = state.detect_click_target(0, QUOTE_PREFIX_WIDTH, 80, 0);
        let second = state.detect_click_target(0, QUOTE_PREFIX_WIDTH + 2, 80, 0);
        assert!(
            matches!(first, Some(ClickTarget::Link { ref url, .. }) if url == "https://a.test")
        );
        assert!(
            matches!(second, Some(ClickTarget::Link { ref url, .. }) if url == "https://b.test")
        );
    }

    #[test]
    fn no_line_highlight_scrolling_is_offset_based() {
        let md = (0..60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        let viewport_h = 12usize;
        let initial_line = state.selected_line;
        state.move_down(1, viewport_h);
        assert_eq!(state.offset, 1);
        assert_eq!(state.selected_line, 1);
        state.move_down(5, viewport_h);
        assert_eq!(state.offset, 6);
        assert_eq!(state.selected_line, 6);
        state.move_up(2, viewport_h);
        assert_eq!(state.offset, 4);
        assert_eq!(state.selected_line, 4);
        assert_ne!(state.selected_line, initial_line);
    }

    #[test]
    fn status_line_composes_left_and_right_sections() {
        let line = super::compose_status_line("README.md", " 42%", 20);
        assert_eq!(UnicodeWidthStr::width(line.as_str()), 20);
        assert!(line.contains("README"));
        assert!(line.ends_with(" 42%"));
    }

    #[test]
    fn status_line_with_center_contains_all_sections() {
        let line = super::compose_status_line_with_center("README.md", "42%", "Help ?", 30);
        assert_eq!(UnicodeWidthStr::width(line.as_str()), 30);
        assert!(line.contains("README"));
        assert!(line.contains("42%"));
        assert!(line.contains("Help ?"));
    }

    #[test]
    fn status_line_with_center_layout_snapshot() {
        let line = super::compose_status_line_with_center("README.md", "42%", "Help ?", 30);
        assert_eq!(line, "README.md    42%        Help ?");
    }

    #[test]
    fn pager_help_line_layout_snapshot() {
        let state = AppState::from_markdown(
            "text".to_string(),
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        let line = super::kb_line(&state, "PgDn/PgUp, space", "page");
        assert_eq!(line_text(&line), "PgDn/PgUp, space    page");
    }

    #[test]
    fn pager_help_center_layout_snapshot() {
        let state = AppState::from_markdown(
            "text".to_string(),
            None,
            AppTheme::default(),
            false,
            false,
            true,
            true,
            3,
            true,
            true,
            true,
            80,
        )
        .expect("state");
        let rows = vec![super::kb_line(&state, "q", "quit")];
        let centered = super::center_block_lines(rows, 30, 4);
        let texts = centered.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(texts, vec!["", "   q                   quit", "", ""]);
    }
}

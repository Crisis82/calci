use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;
use std::sync::mpsc;
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
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use reqwest::header::CONTENT_TYPE;
use resvg::{tiny_skia, usvg};
use unicode_width::UnicodeWidthStr;

use crate::config::TableRowSeparator;
use crate::tex::MathImages;
mod draw;
mod images;

pub use draw::draw;
pub use images::terminal_supports_kitty_graphics;
use images::*;

use crate::layout::{DocLayout, LINE_NUMBER_COLS, LayoutOptions, QUOTE_PREFIX, VisualRow};
use crate::renderer::{
    LineKind, LinkRange, RangeKind, RenderDoc, RenderImage, RenderLine, RenderSettings,
    open_in_editor, preprocess_math, render_markdown, toggle_task_marker,
};
use crate::theme::AppTheme;
use crate::util::{ansi_style_prefix, centered_rect_size, centered_text, truncate_with_ellipsis};

const END_PADDING_ROWS: usize = 3;
pub const WINDOW_PAD_X: u16 = 2;
pub const WINDOW_PAD_TOP: u16 = 1;
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
    pub remote_images: bool,
    pub table_row_separator: TableRowSeparator,
    pub checkbox_toggle: bool,
    pub heading_scale: bool,
    pub math_font: crate::config::MathFont,
    /// Terminal supports the kitty graphics protocol (images are drawn).
    pub graphics: bool,
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
            remote_images: false,
            table_row_separator: TableRowSeparator::Never,
            checkbox_toggle: false,
            heading_scale: false,
            math_font: crate::config::MathFont::Katex,
            graphics: true,
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
    /// Index into the hovered line's `link_ranges`.
    Link(usize),
    /// Code block index and the block's line range (start, end exclusive),
    /// padding and title rows included.
    Code { block: usize, lines: (usize, usize) },
    /// Task checkbox (only when `checkbox_toggle` is on).
    Task,
    /// Inline code (index into the hovered line's `link_ranges`).
    InlineCode(usize),
}

impl HoverTarget {
    /// Mouse pointer shape (OSC 22 / CSS name) shown over this target.
    fn pointer_shape(&self) -> &'static str {
        match self {
            HoverTarget::Link(_) | HoverTarget::Task => "pointer",
            HoverTarget::Code { .. } | HoverTarget::InlineCode(_) => "copy",
        }
    }
}

#[derive(Clone, Debug)]
enum ClickTarget {
    Link {
        url: String,
        link_idx: usize,
    },
    Code(usize),
    /// Inline code: copy its text.
    CopyText {
        text: String,
        link_idx: usize,
    },
    /// Task checkbox on the given line.
    Task(usize),
}

#[derive(Debug)]
pub struct AppState {
    pub doc: RenderDoc,
    /// Renderer output before image rows are materialized; kept so images
    /// that finish loading can be laid out without re-parsing the markdown.
    rendered_lines: Vec<RenderLine>,
    layout: DocLayout,
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
    remote_images: bool,
    table_row_separator: TableRowSeparator,
    checkbox_toggle: bool,
    /// 2x H2 via kitty's text sizing protocol (config on + running in kitty).
    heading_scale: bool,
    graphics: bool,
    /// Whether any kitty image placement may be on screen.
    kitty_placed: bool,
    overlay: Option<Overlay>,
    hover_target: Option<(usize, HoverTarget)>,
    /// Pointer shape currently pushed onto the terminal's OSC 22 stack.
    pointer_shape: Option<&'static str>,
    viewport_width: usize,
    image_cache: HashMap<String, CachedImage>,
    /// Math drawn as kitty images; `None` without kitty graphics (math then
    /// shows as LaTeX source).
    math_images: Option<Rc<RefCell<MathImages>>>,
    image_loader: Option<ImageLoader>,
    last_kitty_placements: Option<Vec<VisibleImagePlacement>>,
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
        config: &AppConfig,
        width: u16,
    ) -> anyhow::Result<Self> {
        let math_enabled = config.math;
        let preprocessed = if math_enabled {
            preprocess_math(&markdown)
        } else {
            markdown.clone()
        };
        let math_images = config.graphics.then(|| {
            Rc::new(RefCell::new(
                MathImages::for_terminal(config.math_font)
                    .unwrap_or_else(|| MathImages::new((10, 20), config.math_font)),
            ))
        });
        let settings = RenderSettings {
            width: content_render_width(width, config.line_numbers),
            theme: theme.clone(),
            table_row_separator: config.table_row_separator,
            promote_h1_title: true,
            math_images: math_images.clone(),
        };
        let doc = render_markdown(&preprocessed, &settings)?;
        let mut state = Self {
            doc: RenderDoc::default(),
            rendered_lines: Vec::new(),
            layout: DocLayout::default(),
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
            line_numbers: config.line_numbers,
            line_highlight: config.line_highlight,
            wrap: config.wrap,
            mouse_allowed: config.mouse,
            mouse: config.mouse,
            smooth_scroll: config.smooth_scroll,
            math_enabled,
            center_blocks: config.center_blocks,
            link_confirm: config.link_confirm,
            remote_images: config.remote_images,
            table_row_separator: config.table_row_separator,
            checkbox_toggle: config.checkbox_toggle,
            heading_scale: config.heading_scale && terminal_is_kitty(),
            graphics: config.graphics,
            kitty_placed: false,
            overlay: None,
            hover_target: None,
            pointer_shape: None,
            viewport_width: width as usize,
            image_cache: HashMap::new(),
            math_images,
            image_loader: None,
            last_kitty_placements: None,
            next_kitty_image_id: 1,
            last_status_at: Instant::now(),
            force_redraw: false,
            return_to_dashboard_on_esc: false,
            should_return_to_dashboard: false,
        };
        state.set_rendered_doc(doc);
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
            width: content_render_width(width, self.line_numbers),
            theme: self.theme.clone(),
            table_row_separator: self.table_row_separator,
            promote_h1_title: true,
            math_images: self.math_images.clone(),
        };
        self.viewport_width = width as usize;
        self.set_rendered_doc(render_markdown(&preprocessed, &settings)?);
        self.recompute_search_hits();
        self.selected_line = self
            .selected_line
            .min(self.doc.lines.len().saturating_sub(1));
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

    /// Install fresh renderer output: materialize image rows and lay it out.
    fn set_rendered_doc(&mut self, doc: RenderDoc) {
        self.rendered_lines = doc.lines.clone();
        self.doc = doc;
        self.materialize_images(self.viewport_width);
        self.relayout();
    }

    fn layout_options(&self) -> LayoutOptions {
        LayoutOptions {
            width: self.viewport_width.max(1),
            wrap: self.wrap,
            line_numbers: self.line_numbers,
            center_blocks: self.center_blocks,
            heading_scale: self.heading_scale,
        }
    }

    pub(crate) fn relayout(&mut self) {
        self.layout = DocLayout::build(&self.doc.lines, self.layout_options(), &self.theme);
    }

    pub fn set_status(&mut self, message: String, is_error: bool) {
        self.status = message;
        self.status_is_error = is_error;
        self.last_status_at = Instant::now();
    }

    /// Periodic housekeeping. Returns true when something visible changed.
    pub fn on_tick(&mut self) -> bool {
        if self.last_status_at.elapsed() > Duration::from_secs(5) && !self.status.is_empty() {
            self.status.clear();
            self.status_is_error = false;
            return true;
        }
        false
    }

    fn overlay(&self) -> Option<&Overlay> {
        self.overlay.as_ref()
    }

    pub fn take_force_redraw(&mut self) -> bool {
        let forced = std::mem::take(&mut self.force_redraw);
        if forced {
            // A cleared screen drops image placements; place them again.
            self.last_kitty_placements = None;
        }
        forced
    }

    pub fn on_event(&mut self, event: Event, viewport_height: usize, width: u16) {
        if self.viewport_width != width as usize {
            self.viewport_width = width as usize;
            self.relayout();
        }
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
                        self.set_hover(None);
                        return;
                    }
                    let row = mouse.row as usize;
                    let col = mouse.column as usize;
                    let pad_x = WINDOW_PAD_X as usize;
                    if row < top_inset || row >= top_inset + viewport_height {
                        self.set_hover(None);
                        return;
                    }
                    if col < pad_x || col >= pad_x + width as usize {
                        self.set_hover(None);
                        return;
                    }
                    let inner_row = row - top_inset;
                    let inner_col = col - pad_x;
                    let vis_row = self.offset + inner_row;
                    let (idx, row_in_line) =
                        self.line_and_inner_row_at_visual_row(vis_row, width as usize);
                    let target =
                        self.detect_click_target(idx, inner_col, width as usize, row_in_line);
                    let hover = target.and_then(|t| {
                        let hover = match t {
                            ClickTarget::Link { link_idx, .. } => HoverTarget::Link(link_idx),
                            ClickTarget::Code(block) => HoverTarget::Code {
                                block,
                                lines: self.block_line_range(idx),
                            },
                            ClickTarget::CopyText { link_idx, .. } => {
                                HoverTarget::InlineCode(link_idx)
                            }
                            ClickTarget::Task(_) if self.checkbox_toggle => HoverTarget::Task,
                            ClickTarget::Task(_) => return None,
                        };
                        Some((idx, hover))
                    });
                    self.set_hover(hover);
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
                        Some(ClickTarget::CopyText { text, .. }) => {
                            if let Err(err) = self.copy_to_clipboard(&text) {
                                self.set_status(format!("Copy failed: {err}"), true);
                            }
                        }
                        Some(ClickTarget::Task(line_idx)) => {
                            if let Err(err) = self.toggle_task_on_line(line_idx) {
                                self.set_status(format!("Toggle failed: {err}"), true);
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
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
            KeyCode::Char('x') => {
                if let Err(err) = self.toggle_task_on_line(self.selected_line) {
                    self.set_status(format!("Toggle failed: {err}"), true);
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
            self.set_hover(None);
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
        self.search_hits = self.collect_search_hits();
        if !self.search_hits.is_empty() {
            self.selected_line = self.search_hits[0];
            self.search_index = 0;
            self.update_active_search_match();
        }
    }

    fn collect_search_hits(&self) -> Vec<usize> {
        let q = self.search_query.trim();
        if q.is_empty() {
            return Vec::new();
        }
        let needle = q.to_lowercase();
        self.doc
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| {
                !matches!(line.kind, LineKind::Code | LineKind::Math | LineKind::Image)
            })
            .filter(|(_, line)| {
                line.line
                    .spans
                    .iter()
                    .map(|sp| sp.content.as_ref())
                    .collect::<String>()
                    .to_lowercase()
                    .contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
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
        self.set_hover(None);
        // Fully suspend TUI before spawning editor, then restore like glow.
        let mut stdout = std::io::stdout();
        execute!(stdout, DisableMouseCapture, LeaveAlternateScreen).ok();
        // Queued mouse reports would otherwise be typed into the editor.
        crate::util::drain_pending_input();
        disable_raw_mode().ok();
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

    /// Flip the task checkbox on `line_idx` in the source file (opt-in via
    /// `checkbox_toggle`), then re-render from the updated file.
    fn toggle_task_on_line(&mut self, line_idx: usize) -> anyhow::Result<()> {
        let task = self
            .doc
            .lines
            .get(line_idx)
            .and_then(|l| l.task)
            .ok_or_else(|| anyhow::anyhow!("no task on the selected line"))?;
        if !self.checkbox_toggle {
            return Err(anyhow::anyhow!(
                "set checkbox_toggle = true in config.toml to enable"
            ));
        }
        let path = self
            .source_path
            .clone()
            .ok_or_else(|| anyhow::anyhow!("only files can be edited, not stdin"))?;
        let on_disk = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        if on_disk != self.source_markdown {
            return Err(anyhow::anyhow!("file changed on disk; press r to reload"));
        }
        let total = self.doc.lines.iter().filter(|l| l.task.is_some()).count();
        let updated = toggle_task_marker(&on_disk, task.ordinal, task.checked, total)?;
        write_file_atomically(&path, &updated)?;
        self.source_markdown = updated;
        self.rerender(self.viewport_width as u16, false)?;
        let status = if task.checked {
            "task unchecked"
        } else {
            "task checked"
        };
        self.set_status(status.to_string(), false);
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
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("code block index out of bounds"))?;
        self.copy_to_clipboard(&code)
    }

    fn copy_to_clipboard(&mut self, text: &str) -> anyhow::Result<()> {
        if !write_via_clipboard_cmd("wl-copy", &[], text)? {
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
        if self.selected_line > 0
            && let Some(idx) = self
                .doc
                .lines
                .get(self.selected_line - 1)
                .and_then(|l| l.code_block_index)
        {
            return Some(idx);
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
            .iter()
            .find(|r| r.kind == RangeKind::Url)
            .map(|r| r.url.clone())
            .ok_or_else(|| anyhow::anyhow!("no link found on selected line"))?;
        self.prompt_open_selected_link(url)
    }

    fn prompt_open_selected_link(&mut self, url: String) -> anyhow::Result<()> {
        let trusted_scheme = is_trusted_link_scheme(&url);
        if self.link_confirm || !trusted_scheme {
            self.overlay = Some(Overlay::LinkConfirm(url.clone()));
            let status = if trusted_scheme {
                "link ready"
            } else {
                "non-web link: confirm to open"
            };
            self.set_status(status.to_string(), false);
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

    /// Rows of `line_idx` at `width`: borrowed from the cached layout when it
    /// matches, otherwise laid out on the fly.
    /// Update the hovered target and, when the pointer shape it implies
    /// changes, tell the terminal (OSC 22 push/pop; ignored by terminals
    /// without pointer-shape support).
    fn set_hover(&mut self, hover: Option<(usize, HoverTarget)>) {
        let shape = hover.as_ref().map(|(_, h)| h.pointer_shape());
        self.hover_target = hover;
        if shape == self.pointer_shape {
            return;
        }
        let mut out = String::new();
        if self.pointer_shape.is_some() {
            out.push_str("\x1b]22;<\x1b\\");
        }
        if let Some(shape) = shape {
            out.push_str(&format!("\x1b]22;>{shape}\x1b\\"));
        }
        self.pointer_shape = shape;
        if cfg!(test) {
            return;
        }
        let mut stdout = std::io::stdout();
        let _ = stdout
            .write_all(out.as_bytes())
            .and_then(|_| stdout.flush());
    }

    /// Restore the terminal's pointer shape (call before leaving the pager).
    pub fn reset_pointer_shape(&mut self) {
        self.set_hover(None);
    }

    /// Line range (start, end exclusive) of the code block containing
    /// `line_idx`: the contiguous run of code/ASCII lines around it.
    fn block_line_range(&self, line_idx: usize) -> (usize, usize) {
        let lines = &self.doc.lines;
        let is_block = |i: usize| {
            lines
                .get(i)
                .is_some_and(|l| matches!(l.kind, LineKind::Code | LineKind::Ascii))
        };
        let mut start = line_idx;
        while start > 0 && is_block(start - 1) {
            start -= 1;
        }
        let mut end = line_idx + 1;
        while is_block(end) {
            end += 1;
        }
        (start, end)
    }

    /// Status-bar hint for the hovered element.
    fn hover_hint(&self) -> Option<String> {
        let (line_idx, hover) = self.hover_target.as_ref()?;
        match hover {
            HoverTarget::Link(link_idx) => {
                let url = &self
                    .doc
                    .lines
                    .get(*line_idx)?
                    .link_ranges
                    .get(*link_idx)?
                    .url;
                Some(format!("↗ {url}"))
            }
            HoverTarget::Code { .. } => Some("click to copy code block".to_string()),
            HoverTarget::Task => Some("click to toggle task".to_string()),
            HoverTarget::InlineCode(idx) => {
                let text = &self.doc.lines.get(*line_idx)?.link_ranges.get(*idx)?.url;
                Some(format!("click to copy `{text}`"))
            }
        }
    }

    fn rows_for_line(&self, line_idx: usize, width: usize) -> std::borrow::Cow<'_, [VisualRow]> {
        if self.layout.is_current(self.doc.lines.len(), width.max(1)) {
            return std::borrow::Cow::Borrowed(self.layout.line_rows(line_idx));
        }
        let Some(line) = self.doc.lines.get(line_idx) else {
            return std::borrow::Cow::Owned(Vec::new());
        };
        let mut opts = self.layout_options();
        opts.width = width.max(1);
        std::borrow::Cow::Owned(crate::layout::layout_line(
            line,
            line_idx,
            opts,
            &self.theme,
        ))
    }

    fn layout_is_current(&self, width: usize) -> bool {
        self.layout.is_current(self.doc.lines.len(), width.max(1))
    }

    fn detect_click_target(
        &self,
        line_idx: usize,
        col: usize,
        total_width: usize,
        row_in_line: usize,
    ) -> Option<ClickTarget> {
        let line = self.doc.lines.get(line_idx)?;
        let rows = self.rows_for_line(line_idx, total_width);
        let row = rows.get(row_in_line)?;

        if let Some(task) = line.task.filter(|_| row_in_line == 0) {
            let start = row.prefix_width + task.start;
            if col >= start && col < row.prefix_width + task.end {
                return Some(ClickTarget::Task(line_idx));
            }
        }

        let mut x = row.prefix_width;
        for (span, link) in row.content.iter().zip(&row.content_links) {
            let w = span.width();
            if col >= x && col < x + w {
                if let Some(link_idx) = *link {
                    let lr = line.link_ranges.get(link_idx)?;
                    return Some(match lr.kind {
                        RangeKind::Url => ClickTarget::Link {
                            url: lr.url.clone(),
                            link_idx,
                        },
                        RangeKind::Copy => ClickTarget::CopyText {
                            text: lr.url.clone(),
                            link_idx,
                        },
                    });
                }
                break;
            }
            x += w;
        }

        if let Some(block_idx) = line.code_block_index {
            let text = row
                .content
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            let leading = &text[..text.len() - text.trim_start().len()];
            let start = row.prefix_width + UnicodeWidthStr::width(leading);
            let end = row.prefix_width + UnicodeWidthStr::width(text.trim_end());
            if end > start && col >= start && col < end {
                return Some(ClickTarget::Code(block_idx));
            }
        }
        None
    }

    fn line_visual_height(&self, line_idx: usize, area_width: usize) -> usize {
        self.rows_for_line(line_idx, area_width).len().max(1)
    }

    fn visual_row_of_line(&self, line_idx: usize, area_width: usize) -> usize {
        if self.layout_is_current(area_width) {
            let last = self.layout.line_start.len() - 1;
            return self.layout.line_start[line_idx.min(last)];
        }
        (0..line_idx.min(self.doc.lines.len()))
            .map(|i| self.line_visual_height(i, area_width))
            .sum()
    }

    fn line_and_inner_row_at_visual_row(&self, row: usize, area_width: usize) -> (usize, usize) {
        if self.doc.lines.is_empty() {
            return (0, 0);
        }
        if self.layout_is_current(area_width) && !self.layout.rows.is_empty() {
            let r = &self.layout.rows[row.min(self.layout.rows.len() - 1)];
            return (r.line_idx, r.row_in_line);
        }
        let mut cur = 0usize;
        for i in 0..self.doc.lines.len() {
            let h = self.line_visual_height(i, area_width);
            if row < cur + h {
                return (i, row - cur);
            }
            cur += h;
        }
        let last = self.doc.lines.len() - 1;
        (last, self.line_visual_height(last, area_width) - 1)
    }

    fn total_visual_rows(&self, area_width: usize) -> usize {
        self.content_visual_rows(area_width) + END_PADDING_ROWS
    }

    fn content_visual_rows(&self, area_width: usize) -> usize {
        if self.layout_is_current(area_width) {
            return self.layout.rows.len();
        }
        (0..self.doc.lines.len())
            .map(|i| self.line_visual_height(i, area_width))
            .sum()
    }
}

/// Replace a file's contents via a temp file + rename in the same directory,
/// so a crash mid-write never leaves a truncated document. Symlinks are
/// followed (the link itself is kept) and the original permissions are kept.
fn write_file_atomically(path: &Path, contents: &str) -> anyhow::Result<()> {
    let target = std::fs::canonicalize(path)
        .with_context(|| format!("failed to resolve '{}'", path.display()))?;
    let dir = target
        .parent()
        .ok_or_else(|| anyhow::anyhow!("'{}' has no parent directory", target.display()))?;
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.calci-{}.tmp", std::process::id()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("failed to create '{}'", tmp.display()))?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
        if let Ok(meta) = std::fs::metadata(&target) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, &target)
            .with_context(|| format!("failed to replace '{}'", target.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Columns the renderer may use: the body width minus the line number gutter.
fn content_render_width(width: u16, line_numbers: bool) -> u16 {
    if line_numbers {
        width.saturating_sub(LINE_NUMBER_COLS as u16).max(1)
    } else {
        width.max(1)
    }
}

/// Schemes that may be handed to the system opener without confirmation.
/// Anything else (file://, ssh://, custom app handlers, relative paths) always
/// goes through the confirm popup.
fn is_trusted_link_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.trim().split_once(':') else {
        return false;
    };
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "mailto"
    )
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
    use super::draw::*;
    use super::*;
    use crate::layout::QUOTE_PREFIX_WIDTH;
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
    fn wrapped_quote_visual_height_counts_all_prefixed_rows() {
        let mut state = AppState::from_markdown(
            "> abcdefghij".to_string(),
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: false,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
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
        let mut state = AppState::from_markdown(
            md,
            Some(markdown_path),
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.wait_for_images();

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
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            100,
        )
        .expect("state");
        state.wait_for_images();

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
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            100,
        )
        .expect("state");
        state.wait_for_images();

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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.wait_for_images();
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.wait_for_images();

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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.wait_for_images();

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
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.wait_for_images();

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
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.wait_for_images();

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
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.wait_for_images();

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
    fn search_highlight_handles_case_folding_that_changes_byte_length() {
        // 'İ' (2 bytes) lowercases to "i̇" (3 bytes); offsets from the lowered
        // string must not be used to slice the original text.
        let spans = vec![Span::raw("İx İX")];
        let mark = ratatui::style::Style::default().add_modifier(Modifier::BOLD);
        let out = highlight_spans_for_search(&spans, "x", mark);
        let joined = out.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert_eq!(joined, "İx İX");
        let marked = out
            .iter()
            .filter(|s| s.style == mark)
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(marked, vec!["x", "X"]);
        let out = highlight_spans_for_search(&spans, "i", mark);
        let joined = out.iter().map(|s| s.content.as_ref()).collect::<String>();
        assert_eq!(joined, "İx İX");
    }

    #[test]
    fn only_web_and_mail_schemes_skip_link_confirmation() {
        assert!(is_trusted_link_scheme("https://example.com"));
        assert!(is_trusted_link_scheme("HTTP://example.com"));
        assert!(is_trusted_link_scheme("mailto:me@example.com"));
        assert!(!is_trusted_link_scheme("file:///etc/passwd"));
        assert!(!is_trusted_link_scheme("ssh://host"));
        assert!(!is_trusted_link_scheme("vscode://open?x"));
        assert!(!is_trusted_link_scheme("notes/other.md"));
        assert!(!is_trusted_link_scheme("#anchor"));
    }

    #[test]
    fn non_web_link_click_always_opens_confirm_popup() {
        let mut state = AppState::from_markdown(
            "x".to_string(),
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: false,
                link_confirm: false,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state
            .prompt_open_selected_link("file:///etc/passwd".to_string())
            .expect("prompt");
        assert!(matches!(state.overlay, Some(Overlay::LinkConfirm(_))));
    }

    #[test]
    fn remote_image_file_name_strips_query_and_fragment() {
        assert_eq!(
            remote_image_file_name("https://example.com/a/plot.png?x=1#f"),
            "plot.png"
        );
        assert_eq!(
            remote_image_file_name("https://example.com/"),
            "example.com"
        );
    }

    #[test]
    fn remote_images_disabled_render_clickable_file_name_placeholder() {
        let url = "https://example.com/img/plot.png";
        let mut state = AppState::from_markdown(
            format!("![a plot]({url})"),
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: false,
                link_confirm: false,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.wait_for_images();
        let placeholder = state
            .doc
            .lines
            .iter()
            .find(|l| l.kind == LineKind::Image && !l.link_ranges.is_empty())
            .expect("placeholder line");
        assert_eq!(line_text(&placeholder.line), "[plot.png]");
        assert_eq!(placeholder.link_ranges[0].url, url);
        assert!(state.image_cache.is_empty(), "nothing must be fetched");
    }

    #[test]
    fn draw_renders_wrapped_rows_with_quote_bar_and_justification() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let md = "Some **bold** text with a [link](https://a.test) that wraps across several rows of the narrow pager.\n\n> a quoted sentence that is long enough to wrap\n\n- a list item that is long enough to wrap onto a second row".to_string();
        let width = 36u16;
        let state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: false,
                link_confirm: false,
                remote_images: false,
                ..AppConfig::default()
            },
            width - WINDOW_PAD_X * 2,
        )
        .expect("state");
        let mut terminal = Terminal::new(TestBackend::new(width, 20)).expect("terminal");
        terminal.draw(|f| draw(f, &state)).expect("draw");
        let buf = terminal.backend().buffer();
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        eprintln!("{}", rows.join("\n"));
        let body: Vec<&str> = rows.iter().map(|r| &r[..]).collect();
        // Justified paragraph rows end exactly at the right edge of the body.
        let first = body[1].trim_end();
        assert_eq!(
            UnicodeWidthStr::width(first),
            (width - WINDOW_PAD_X) as usize
        );
        // Every wrapped quote row carries the bar.
        let quote_rows: Vec<_> = body.iter().filter(|r| r.contains("│ ")).collect();
        assert!(quote_rows.len() >= 2, "{quote_rows:?}");
        // List continuation rows are indented under the item text.
        let item = body
            .iter()
            .position(|r| r.contains("• a list"))
            .expect("item");
        assert!(body[item + 1].starts_with("      "), "{:?}", body[item + 1]);
    }

    #[test]
    fn links_inside_centered_tables_are_clickable() {
        let md = "| a | b |\n|---|---|\n| x | [site](https://s.test) |\n".to_string();
        let state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                center_blocks: true,
                ..AppConfig::default()
            },
            60,
        )
        .expect("state");
        let idx = state
            .doc
            .lines
            .iter()
            .position(|l| !l.link_ranges.is_empty())
            .expect("table row with link");
        let row = &state.rows_for_line(idx, 60)[0];
        assert!(row.lead_pad > 0, "table should be centered");
        let col = row.prefix_width + state.doc.lines[idx].link_ranges[0].start;
        let target = state.detect_click_target(idx, col, 60, 0);
        assert!(
            matches!(target, Some(ClickTarget::Link { ref url, .. }) if url == "https://s.test")
        );
        assert!(
            state
                .detect_click_target(idx, row.prefix_width, 60, 0)
                .is_none()
        );
    }

    fn task_state(path: &Path, toggle: bool) -> AppState {
        let md = std::fs::read_to_string(path).expect("read");

        AppState::from_markdown(
            md,
            Some(path.to_path_buf()),
            AppTheme::default(),
            &AppConfig {
                checkbox_toggle: toggle,
                ..AppConfig::default()
            },
            60,
        )
        .expect("state")
    }

    fn task_line(state: &AppState, ordinal: usize) -> usize {
        state
            .doc
            .lines
            .iter()
            .position(|l| l.task.is_some_and(|t| t.ordinal == ordinal))
            .expect("task line")
    }

    #[test]
    fn x_key_and_click_toggle_tasks_in_the_file() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("todo.md");
        std::fs::write(&path, "# Todo\n\n- [ ] first\n- [x] second\n").expect("write");
        let mut state = task_state(&path, true);

        state.selected_line = task_line(&state, 0);
        state
            .toggle_task_on_line(state.selected_line)
            .expect("toggle");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Todo\n\n- [x] first\n- [x] second\n"
        );
        assert!(state.doc.lines[task_line(&state, 0)].task.unwrap().checked);

        // Click exactly on the second checkbox glyph.
        let idx = task_line(&state, 1);
        let row = &state.rows_for_line(idx, 60)[0];
        let col = row.prefix_width + state.doc.lines[idx].task.unwrap().start;
        let target = state.detect_click_target(idx, col, 60, 0);
        assert!(matches!(target, Some(ClickTarget::Task(i)) if i == idx));
        // ...but not on the item text.
        assert!(state.detect_click_target(idx, col + 4, 60, 0).is_none());
        state.toggle_task_on_line(idx).expect("toggle");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# Todo\n\n- [x] first\n- [ ] second\n"
        );
    }

    #[test]
    fn task_toggle_is_opt_in_and_refuses_stale_files() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("todo.md");
        std::fs::write(&path, "- [ ] first\n").expect("write");

        let mut disabled = task_state(&path, false);
        assert!(
            disabled
                .toggle_task_on_line(task_line(&disabled, 0))
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "- [ ] first\n");

        let mut state = task_state(&path, true);
        std::fs::write(&path, "- [ ] first\n- [ ] added elsewhere\n").expect("write");
        let err = state.toggle_task_on_line(task_line(&state, 0)).unwrap_err();
        assert!(err.to_string().contains("changed on disk"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "- [ ] first\n- [ ] added elsewhere\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn task_toggle_through_symlink_keeps_the_link() {
        let dir = tempdir().expect("tempdir");
        let real = dir.path().join("real.md");
        let link = dir.path().join("link.md");
        std::fs::write(&real, "- [ ] first\n").expect("write");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let mut state = task_state(&link, true);
        state
            .toggle_task_on_line(task_line(&state, 0))
            .expect("toggle");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "- [x] first\n");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains("calci"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");
    }

    #[test]
    fn scaled_h2_cells_are_skipped_by_ratatui_and_payload_is_sanitized() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut state = AppState::from_markdown(
            "intro\n\n## Sec\u{1b}tion\n\ntext".to_string(),
            None,
            AppTheme::default(),
            &AppConfig::default(),
            36,
        )
        .expect("state");
        state.heading_scale = true;
        state.relayout();
        let body = Rect::new(WINDOW_PAD_X, WINDOW_PAD_TOP, 36, 20);
        let headings = state.visible_scaled_headings(body);
        assert_eq!(headings.len(), 1);
        let (x, y, text, _) = &headings[0];
        assert_eq!(text, "Section", "control chars must be stripped");

        let render = |state: &AppState| {
            let mut terminal = Terminal::new(TestBackend::new(40, 22)).expect("terminal");
            terminal.draw(|f| draw(f, state)).expect("draw");
            terminal.backend().buffer().clone()
        };
        // The backend only receives cells ratatui actually writes: skipped
        // cells stay untouched (no fallback text, no sentinel)...
        let buf = render(&state);
        for dx in 0..14 {
            for dy in 0..2 {
                assert_eq!(buf[(*x + dx, *y + dy)].symbol(), " ");
            }
        }
        // ...whereas without scaling the heading text is painted normally.
        state.heading_scale = false;
        state.relayout();
        assert_eq!(render(&state)[(*x, *y)].symbol(), "S");
        state.heading_scale = true;
        state.relayout();

        // Hidden while a popup is open, so the popup isn't drawn over.
        state.overlay = Some(Overlay::Help);
        assert!(state.visible_scaled_headings(body).is_empty());
    }

    fn hover_at(state: &mut AppState, line_idx: usize, col_in_line: usize) {
        use crossterm::event::{MouseEvent, MouseEventKind};
        let width = 60usize;
        let row = state.visual_row_of_line(line_idx, width) - state.offset;
        let prefix = state.rows_for_line(line_idx, width)[0].prefix_width;
        let event = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: (WINDOW_PAD_X as usize + prefix + col_in_line) as u16,
            row: (WINDOW_PAD_TOP as usize + row) as u16,
            modifiers: KeyModifiers::NONE,
        });
        state.on_event(event, 30, width as u16);
    }

    #[test]
    fn hovering_sets_pointer_shape_hint_and_code_tint() {
        let md = "see [docs](https://d.test) here\n\n```rust\nfn main() {}\n```\n\n- [ ] task\n"
            .to_string();
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                checkbox_toggle: true,
                ..AppConfig::default()
            },
            60,
        )
        .expect("state");
        let link_line = state
            .doc
            .lines
            .iter()
            .position(|l| !l.link_ranges.is_empty())
            .unwrap();
        let link_col = state.doc.lines[link_line].link_ranges[0].start;
        hover_at(&mut state, link_line, link_col);
        assert_eq!(state.pointer_shape, Some("pointer"));
        assert_eq!(state.hover_hint().as_deref(), Some("↗ https://d.test"));

        let code_line = state
            .doc
            .lines
            .iter()
            .position(|l| l.code_block_index.is_some())
            .unwrap();
        hover_at(&mut state, code_line, 3);
        assert_eq!(state.pointer_shape, Some("copy"));
        assert_eq!(
            state.hover_hint().as_deref(),
            Some("click to copy code block")
        );
        // The whole block (padding rows included) is tinted.
        let Some((
            _,
            HoverTarget::Code {
                lines: (start, end),
                ..
            },
        )) = state.hover_target.clone()
        else {
            panic!("code hover expected");
        };
        assert!(
            start < code_line && end > code_line + 1,
            "padding rows belong to the block"
        );
        let tint = state.theme.cursor_line.bg;
        for line in start..end {
            let row = &state.rows_for_line(line, 60)[0];
            let composed = draw::compose_visual_row(&state, row);
            assert!(
                composed
                    .spans
                    .iter()
                    .filter(|s| !s.content.is_empty())
                    .any(|s| s.style.bg == tint)
            );
        }

        let task_line = state
            .doc
            .lines
            .iter()
            .position(|l| l.task.is_some())
            .unwrap();
        let task_col = state.doc.lines[task_line].task.unwrap().start;
        hover_at(&mut state, task_line, task_col);
        assert_eq!(state.pointer_shape, Some("pointer"));
        assert_eq!(state.hover_hint().as_deref(), Some("click to toggle task"));

        // Plain text: pointer restored, no hint.
        hover_at(&mut state, link_line, 0);
        assert_eq!(state.pointer_shape, None);
        assert!(state.hover_hint().is_none());
    }

    #[test]
    fn inline_code_click_and_hover_copy_its_text() {
        let md = "run `fdisk -l` now".to_string();
        let mut state =
            AppState::from_markdown(md, None, AppTheme::default(), &AppConfig::default(), 60)
                .expect("state");
        let range = state.doc.lines[0].link_ranges[0].clone();
        assert_eq!(range.kind, RangeKind::Copy);
        let target = state.detect_click_target(0, range.start, 60, 0);
        assert!(
            matches!(target, Some(ClickTarget::CopyText { ref text, .. }) if text == "fdisk -l")
        );

        hover_at(&mut state, 0, range.start + 1);
        assert_eq!(state.pointer_shape, Some("copy"));
        assert_eq!(
            state.hover_hint().as_deref(),
            Some("click to copy `fdisk -l`")
        );
        let row = &state.rows_for_line(0, 60)[0];
        let composed = draw::compose_visual_row(&state, row);
        let tint = state.theme.cursor_line.bg;
        let tinted: String = composed
            .spans
            .iter()
            .filter(|s| s.style.bg == tint && tint.is_some())
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(tinted, "fdisk -l");
        // `o` only opens real links.
        assert!(state.prompt_open_selected_line_link().is_err());
    }

    #[test]
    fn without_kitty_graphics_images_show_alt_text_and_nothing_is_loaded() {
        let dir = tempdir().expect("tempdir");
        let image_path = dir.path().join("pic.png");
        GrayImage::new(4, 4).save(&image_path).expect("save");
        let mut state = AppState::from_markdown(
            format!("![a picture]({})", image_path.display()),
            None,
            AppTheme::default(),
            &AppConfig {
                graphics: false,
                ..AppConfig::default()
            },
            60,
        )
        .expect("state");
        state.wait_for_images();
        assert!(state.image_cache.is_empty());
        assert!(
            state
                .doc
                .lines
                .iter()
                .any(|l| line_text(&l.line) == "[a picture]")
        );
        assert!(!state.kitty_placed);
    }

    fn math_state(markdown: &str) -> AppState {
        AppState::from_markdown(
            markdown.to_string(),
            None,
            AppTheme::default(),
            &AppConfig {
                graphics: true,
                ..AppConfig::default()
            },
            60,
        )
        .expect("state")
    }

    fn placeholder_ids(state: &AppState) -> Vec<u32> {
        state
            .layout
            .rows
            .iter()
            .flat_map(|row| &row.content)
            .filter(|s| s.content.starts_with(crate::placeholder::PLACEHOLDER))
            .filter_map(|s| s.style.fg.and_then(crate::placeholder::color_id))
            .collect()
    }

    #[test]
    fn math_images_upload_when_visible_and_survive_rewrap() {
        let mut state = math_state("before $x^2$ text\n\n$$\\frac{a}{b}$$\n\nafter");
        state.relayout();
        let ids = placeholder_ids(&state);
        assert!(ids.len() >= 3, "inline run + display rows: {ids:?}");
        state.upload_visible_math_images(50).expect("upload");
        let store = state.math_images.clone().expect("store");
        for id in &ids {
            assert!(store.borrow_mut().get_mut(*id).expect("known id").uploaded);
        }
        state.rerender_for_width(40).expect("rerender");
        assert_eq!(placeholder_ids(&state).len(), ids.len());
    }

    #[test]
    fn without_graphics_math_is_latex_source() {
        let state = AppState::from_markdown(
            "a $x^2$ b".to_string(),
            None,
            AppTheme::default(),
            &AppConfig {
                graphics: false,
                ..AppConfig::default()
            },
            60,
        )
        .expect("state");
        assert!(state.math_images.is_none());
        assert!(line_text(&state.doc.lines[0].line).contains("$x^2$"));
    }

    #[test]
    fn search_hits_work() {
        let md = "hello\nworld\nhello world".to_string();
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.doc.lines = vec![RenderLine {
            line: Line::from(vec![Span::raw("A B".to_string())]),
            kind: LineKind::Normal,
            link_ranges: vec![
                LinkRange {
                    start: 0,
                    end: 1,
                    url: "https://a.test".to_string(),
                    kind: RangeKind::Url,
                },
                LinkRange {
                    start: 2,
                    end: 3,
                    url: "https://b.test".to_string(),
                    kind: RangeKind::Url,
                },
            ],
            code_block_index: None,
            image_index: None,
            heading_level: None,
            hang_indent: 0,
            task: None,
        }];
        state.relayout();
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: false,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
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
        let row = &state.rows_for_line(quote_idx, 80)[0];
        let content_width: usize = row.content.iter().map(Span::width).sum();
        assert_eq!(
            row.prefix_width + content_width,
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: false,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        state.doc.lines = vec![RenderLine {
            line: Line::from(vec![Span::raw("A B".to_string())]),
            kind: LineKind::Quote,
            link_ranges: vec![
                LinkRange {
                    start: 0,
                    end: 1,
                    url: "https://a.test".to_string(),
                    kind: RangeKind::Url,
                },
                LinkRange {
                    start: 2,
                    end: 3,
                    url: "https://b.test".to_string(),
                    kind: RangeKind::Url,
                },
            ],
            code_block_index: None,
            image_index: None,
            heading_level: None,
            hang_indent: 0,
            task: None,
        }];
        state.relayout();
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
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
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
        let line = compose_status_line("README.md", " 42%", 20);
        assert_eq!(UnicodeWidthStr::width(line.as_str()), 20);
        assert!(line.contains("README"));
        assert!(line.ends_with(" 42%"));
    }

    #[test]
    fn status_line_with_center_contains_all_sections() {
        let line = compose_status_line_with_center("README.md", "42%", "Help ?", 30);
        assert_eq!(UnicodeWidthStr::width(line.as_str()), 30);
        assert!(line.contains("README"));
        assert!(line.contains("42%"));
        assert!(line.contains("Help ?"));
    }

    #[test]
    fn status_line_with_center_layout_snapshot() {
        let line = compose_status_line_with_center("README.md", "42%", "Help ?", 30);
        assert_eq!(line, "README.md    42%        Help ?");
    }

    #[test]
    fn pager_help_line_layout_snapshot() {
        let state = AppState::from_markdown(
            "text".to_string(),
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        let line = kb_line(&state, "PgDn/PgUp, space", "page");
        assert_eq!(line_text(&line), "PgDn/PgUp, space    page");
    }

    #[test]
    fn pager_help_center_layout_snapshot() {
        let state = AppState::from_markdown(
            "text".to_string(),
            None,
            AppTheme::default(),
            &AppConfig {
                line_numbers: false,
                line_highlight: false,
                wrap: true,
                mouse: true,
                smooth_scroll: 3,
                math: true,
                center_blocks: true,
                link_confirm: true,
                remote_images: false,
                ..AppConfig::default()
            },
            80,
        )
        .expect("state");
        let rows = vec![kb_line(&state, "q", "quit")];
        let centered = center_block_lines(rows, 30, 4);
        let texts = centered.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(texts, vec!["", "   q                   quit", "", ""]);
    }
}

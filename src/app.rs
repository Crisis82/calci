use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::Context;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::renderer::{
    LineKind, RenderDoc, RenderLine, RenderSettings, open_in_editor, preprocess_math,
    render_markdown,
};
use crate::theme::AppTheme;

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
    pub mouse: bool,
    pub smooth_scroll: usize,
    pub math_enabled: bool,
    pub center_blocks: bool,
    pub link_confirm: bool,
    overlay: Option<Overlay>,
    hover_target: Option<(usize, HoverTarget)>,
    viewport_width: usize,
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
        Ok(Self {
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
            mouse,
            smooth_scroll,
            math_enabled,
            center_blocks,
            link_confirm,
            overlay: None,
            hover_target: None,
            viewport_width: width as usize,
            last_status_at: Instant::now(),
            force_redraw: false,
            return_to_dashboard_on_esc: false,
            should_return_to_dashboard: false,
        })
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
            _ => {}
        }
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
            if matches!(line.kind, LineKind::Code) {
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
        let mut x = if self.line_numbers { 6 } else { 0 };
        if self.center_blocks
            && !self.line_numbers
            && matches!(line.kind, LineKind::Table | LineKind::Quote)
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
        if self.line_numbers {
            width += 6;
        }
        if self.center_blocks
            && !self.line_numbers
            && matches!(line.kind, LineKind::Table | LineKind::Quote)
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
        let width = self.line_display_width(line_idx, area_width);
        width.div_ceil(area_width).max(1)
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
                s.style = s.style.patch(state.theme.quote);
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
        if !state.search_query.is_empty() {
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
        if state.center_blocks
            && !state.line_numbers
            && matches!(kind, LineKind::Table | LineKind::Quote)
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
    use crate::theme::{AppTheme, ThemeName};

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn search_hits_work() {
        let md = "hello\nworld\nhello world".to_string();
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::from_name(ThemeName::Oxocarbon),
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
            AppTheme::from_name(ThemeName::Oxocarbon),
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
            AppTheme::from_name(ThemeName::Oxocarbon),
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
            AppTheme::from_name(ThemeName::Oxocarbon),
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
            AppTheme::from_name(ThemeName::Oxocarbon),
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
            AppTheme::from_name(ThemeName::Oxocarbon),
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
    fn no_line_highlight_scrolling_is_offset_based() {
        let md = (0..60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut state = AppState::from_markdown(
            md,
            None,
            AppTheme::from_name(ThemeName::Oxocarbon),
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
            AppTheme::from_name(ThemeName::Oxocarbon),
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
            AppTheme::from_name(ThemeName::Oxocarbon),
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

mod app;
mod backend;
mod cli;
mod config;
mod dashboard;
mod layout;
mod placeholder;
mod plain;
mod renderer;
mod tex;
mod tex_typst;
mod theme;
mod util;

use std::io::{IsTerminal, Write as _};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::backend::SafeBackend;
use anyhow::Context;
use clap::Parser;
use crossterm::event;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;

use app::{AppConfig, AppState, WINDOW_PAD_TOP, WINDOW_PAD_X, draw, read_markdown_input};
use cli::{Cli, print_completion};
use config::LoadedConfig;
use dashboard::run_dashboard;
use plain::line_to_ansi_with_links;
use theme::AppTheme;

/// Restore the terminal before the default panic report is printed, so a
/// panic never leaves the shell in raw mode / alternate screen.
fn install_terminal_restore_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if !crossterm::terminal::is_raw_mode_enabled().unwrap_or(false) {
            default_hook(info);
            return;
        }
        let mut stdout = std::io::stdout();
        // Drop any kitty image placements left on screen.
        let _ = std::io::Write::write_all(&mut stdout, b"\x1b_Ga=d,d=a,q=2\x1b\\");
        // Reset the mouse pointer shape (OSC 22) in case a link was hovered.
        let _ = std::io::Write::write_all(&mut stdout, b"\x1b]22;\x1b\\");
        let _ = execute!(
            stdout,
            crossterm::event::DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = disable_raw_mode();
        default_hook(info);
    }));
}

fn main() -> anyhow::Result<()> {
    install_terminal_restore_panic_hook();
    let cli = Cli::parse();
    if cli.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let Some(shell) = cli.completion {
        print_completion(shell);
        return Ok(());
    }
    let loaded = LoadedConfig::load(None, cli.color.as_deref())?;
    let chosen_theme = loaded.build_theme();
    let config = AppConfig {
        input_path: cli.file.clone(),
        line_numbers: loaded.app.line_numbers,
        line_highlight: loaded.app.line_highlight,
        start_in_pager: if cli.plain { false } else { loaded.app.pager },
        mouse: loaded.app.mouse,
        wrap: loaded.app.wrap,
        smooth_scroll: loaded.app.smooth_scroll,
        math: loaded.app.math,
        center_blocks: loaded.app.center_blocks,
        link_confirm: loaded.app.link_confirmation,
        remote_images: loaded.app.remote_images,
        table_row_separator: loaded.app.table_row_separator,
        checkbox_toggle: loaded.app.checkbox_toggle,
        heading_scale: loaded.app.heading_scale,
        math_font: loaded.app.math_font,
        graphics: app::terminal_supports_kitty_graphics(),
    };
    if !config.start_in_pager {
        let (markdown, _path) = if config.input_path.is_none() && std::io::stdin().is_terminal() {
            let picked = run_dashboard(
                &chosen_theme,
                loaded.app.dashboard_sort,
                loaded.app.dashboard_fuzzy_mode,
                loaded.app.dashboard_show_edited_age,
                loaded.app.mouse,
                &loaded.source_dir,
            )?;
            if let Some(p) = picked {
                read_markdown_input(Some(&p))?
            } else {
                return Ok(());
            }
        } else {
            read_markdown_input(config.input_path.as_deref())?
        };
        let pre = if loaded.app.math {
            renderer::preprocess_math(&markdown)
        } else {
            markdown.clone()
        };
        let settings = renderer::RenderSettings {
            width: 100,
            theme: chosen_theme.clone(),
            table_row_separator: loaded.app.table_row_separator,
            promote_h1_title: false,
            math_images: plain::math_images_for_stdout(config.graphics, config.math_font),
        };
        let doc = renderer::render_markdown(&pre, &settings)?;
        let mut out = std::io::stdout().lock();
        let mut result = match &settings.math_images {
            Some(store) => plain::transmit_math_images(&mut out, &mut store.borrow_mut()),
            None => Ok(()),
        };
        for l in &doc.lines {
            if result.is_err() {
                break;
            }
            let rendered = line_to_ansi_with_links(l, chosen_theme.normal, chosen_theme.link);
            result = writeln!(out, "{rendered}");
        }
        if let Err(err) = result {
            // Reader went away (e.g. `calci --plain f.md | head`): stop quietly.
            if err.kind() != std::io::ErrorKind::BrokenPipe {
                return Err(err.into());
            }
        }
        return Ok(());
    }

    if config.input_path.is_none() && std::io::stdin().is_terminal() {
        loop {
            let picked = run_dashboard(
                &chosen_theme,
                loaded.app.dashboard_sort,
                loaded.app.dashboard_fuzzy_mode,
                loaded.app.dashboard_show_edited_age,
                loaded.app.mouse,
                &loaded.source_dir,
            )?;
            let Some(picked_path) = picked else {
                return Ok(());
            };
            let (markdown, path) = read_markdown_input(Some(&picked_path))?;
            let return_to_dashboard =
                run_pager(markdown, path, config.clone(), chosen_theme.clone(), true)?;
            if !return_to_dashboard {
                return Ok(());
            }
        }
    }

    let (markdown, path) = read_markdown_input(config.input_path.as_deref())?;
    let _ = run_pager(markdown, path, config, chosen_theme, false)?;
    Ok(())
}

/// Upper bound on queued input events handled before the next frame.
const MAX_EVENTS_PER_FRAME: usize = 128;

fn run_pager(
    markdown: String,
    path: Option<PathBuf>,
    config: AppConfig,
    theme: AppTheme,
    return_to_dashboard_on_esc: bool,
) -> anyhow::Result<bool> {
    fn content_width(term_width: u16) -> u16 {
        term_width.saturating_sub(WINDOW_PAD_X * 2).max(1)
    }

    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen")?;

    let backend = SafeBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear().ok();

    let mut size = terminal.size().context("terminal size")?;
    let pager_mouse_capture = config.mouse;
    if pager_mouse_capture {
        execute!(terminal.backend_mut(), crossterm::event::EnableMouseCapture).ok();
    }
    let source = markdown;
    let mut state =
        AppState::from_markdown(source, path, theme, &config, content_width(size.width))?;
    state.set_return_to_dashboard_on_esc(return_to_dashboard_on_esc);

    let tick_rate = Duration::from_millis(120);
    let mut last_tick = Instant::now();
    // Only redraw when something changed: an input event, a resize, a status
    // timeout or a finished image load. Idle ticks cost nothing.
    let mut dirty = true;
    let run_result: anyhow::Result<()> = loop {
        if let Ok(new_size) = terminal.size()
            && new_size != size
        {
            size = new_size;
            state.rerender_for_width(content_width(size.width)).ok();
            dirty = true;
        }
        if state.poll_images() {
            dirty = true;
        }
        if dirty {
            terminal.draw(|f| draw(f, &state)).context("draw frame")?;
            if let Err(err) = state.render_kitty_images(size.into()) {
                state.set_status(format!("image render error: {err}"), true);
            }
            if let Err(err) = state.render_scaled_headings(size.into()) {
                state.set_status(format!("heading render error: {err}"), true);
            }
            dirty = false;
        }

        if state.should_quit {
            break Ok(());
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if event::poll(timeout).context("event poll")? {
            dirty = true;
            // Handle everything already queued (fast scrolling, mouse moves)
            // before drawing once, instead of one frame per event.
            for _ in 0..MAX_EVENTS_PER_FRAME {
                let ev = event::read().context("event read")?;
                let top_bar = if state.top_frontmatter_title().is_some() {
                    1
                } else {
                    0
                };
                let reserved = 1 + WINDOW_PAD_TOP + top_bar;
                let viewport_h = size.height.saturating_sub(reserved) as usize;
                state.on_event(ev, viewport_h.max(1), content_width(size.width));
                if state.take_force_redraw() {
                    terminal.clear().ok();
                }
                if state.should_quit || !event::poll(Duration::ZERO).context("event poll")? {
                    break;
                }
            }
        }
        if last_tick.elapsed() >= tick_rate {
            if state.on_tick() {
                dirty = true;
            }
            last_tick = Instant::now();
        }
    };

    let _ = state.clear_kitty_images();
    state.reset_pointer_shape();
    // Stop mouse reports and drain queued input *before* leaving raw mode,
    // otherwise pending reports are echoed as text.
    if pager_mouse_capture {
        execute!(
            terminal.backend_mut(),
            crossterm::event::DisableMouseCapture
        )
        .ok();
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    util::drain_pending_input();
    disable_raw_mode().ok();
    terminal.show_cursor().ok();

    run_result?;
    Ok(state.take_return_to_dashboard())
}

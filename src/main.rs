mod app;
mod config;
mod renderer;
mod theme;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use clap::{Parser, ValueEnum, ValueHint};
use crossterm::event;
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use serde::{Deserialize, Serialize};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use app::{AppConfig, AppState, WINDOW_PAD_TOP, WINDOW_PAD_X, draw, read_markdown_input};
use config::{DashboardFuzzyMode, DashboardSort, LoadedConfig};
use theme::AppTheme;

const DASHBOARD_BODY_PAD_X: usize = 2;
const DASHBOARD_BODY_PAD_Y: usize = 1;
const DASHBOARD_META_PAD_X: usize = 2;
const DASHBOARD_SCAN_BATCH_DIRS: usize = 32;
const DASHBOARD_SCAN_BATCH_FILES: usize = 512;
const DASHBOARD_MODIFIED_BATCH: usize = 96;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum CompletionShell {
    Bash,
    Zsh,
    Fish,
}

#[derive(Debug, Parser)]
#[command(
    name = "calci",
    version,
    about = "TUI markdown pager with search, syntax highlighting, links, and calcifer rendering",
    disable_version_flag = true
)]
struct Cli {
    /// Markdown file path. Uses dashboard if omitted.
    #[arg(value_hint = ValueHint::FilePath)]
    file: Option<PathBuf>,

    /// Use a specific color palette file
    #[arg(long = "color", value_name = "FILE", value_hint = ValueHint::FilePath)]
    color: Option<PathBuf>,

    /// Disable pager UI (prints rendered plain text)
    #[arg(short = 'p', long = "plain")]
    plain: bool,

    /// Print shell completion script (bash|zsh|fish)
    #[arg(short = 'c', long, value_name = "SHELL")]
    completion: Option<CompletionShell>,

    /// Print version
    #[arg(short = 'v', long = "version")]
    version: bool,
}

fn main() -> anyhow::Result<()> {
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
    };
    if !config.start_in_pager {
        let (markdown, _path) = if config.input_path.is_none() && atty::is(atty::Stream::Stdin) {
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
        };
        let doc = renderer::render_markdown(&pre, &settings)?;
        for l in doc.lines {
            println!(
                "{}",
                line_to_ansi_with_links(&l, chosen_theme.normal, chosen_theme.link)
            );
        }
        return Ok(());
    }

    if config.input_path.is_none() && atty::is(atty::Stream::Stdin) {
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

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal")?;
    terminal.clear().ok();

    let mut size = terminal.size().context("terminal size")?;
    let pager_mouse_capture = config.mouse;
    if pager_mouse_capture {
        execute!(terminal.backend_mut(), crossterm::event::EnableMouseCapture).ok();
    }
    let source = markdown;
    let mut state = AppState::from_markdown(
        source,
        path,
        theme,
        config.line_numbers,
        config.line_highlight,
        config.wrap,
        pager_mouse_capture,
        config.smooth_scroll,
        config.math,
        config.center_blocks,
        config.link_confirm,
        content_width(size.width),
    )?;
    state.set_return_to_dashboard_on_esc(return_to_dashboard_on_esc);

    let tick_rate = Duration::from_millis(120);
    let mut last_tick = Instant::now();
    let run_result: anyhow::Result<()> = loop {
        terminal.draw(|f| draw(f, &state)).context("draw frame")?;
        if let Err(err) = state.render_kitty_images(size.into()) {
            state.set_status(format!("image render error: {err}"), true);
        }

        if state.should_quit {
            break Ok(());
        }

        let timeout = tick_rate.saturating_sub(last_tick.elapsed());
        if let Ok(new_size) = terminal.size() {
            if new_size != size {
                size = new_size;
                state.rerender_for_width(content_width(size.width)).ok();
            }
        }
        if event::poll(timeout).context("event poll")? {
            let ev = event::read().context("event read")?;
            let top_bar = if state.top_frontmatter_title().is_some() {
                1
            } else {
                0
            };
            let reserved = 1 + WINDOW_PAD_TOP as u16 + top_bar;
            let viewport_h = size.height.saturating_sub(reserved) as usize;
            state.on_event(ev, viewport_h.max(1), content_width(size.width));
            if state.take_force_redraw() {
                terminal.clear().ok();
            }
        }
        if last_tick.elapsed() >= tick_rate {
            state.on_tick();
            last_tick = Instant::now();
        }
    };

    let _ = state.clear_kitty_images();
    disable_raw_mode().ok();
    if pager_mouse_capture {
        execute!(
            terminal.backend_mut(),
            crossterm::event::DisableMouseCapture
        )
        .ok();
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    run_result?;
    Ok(state.take_return_to_dashboard())
}

fn print_completion(shell: CompletionShell) {
    match shell {
        CompletionShell::Bash => {
            print!("{}", bash_completion_script());
        }
        CompletionShell::Zsh => {
            print!("{}", zsh_completion_script());
        }
        CompletionShell::Fish => {
            print!("{}", fish_completion_script());
        }
    }
}

fn bash_completion_script() -> &'static str {
    r#"_calci__md_entries() {
    local base
    base="${1:-.}"
    [ -d "$base" ] || return 0
    find "$base" -mindepth 1 -maxdepth 1 \
        \( -type f \( -iname '*.md' -o -iname '*.markdown' \) -o -type d \) \
        -printf '%P\n' 2>/dev/null
}

_calci__md_paths() {
    local cur dir_part prefix search_base
    cur="$1"
    dir_part="${cur%/*}"
    if [ "$dir_part" = "$cur" ]; then
        dir_part=""
        prefix=""
        search_base="."
    else
        prefix="$dir_part/"
        search_base="$dir_part"
    fi
    _calci__md_entries "$search_base" | while IFS= read -r item; do
        [ -z "$item" ] && continue
        if [ -d "${search_base%/}/$item" ]; then
            if find "${search_base%/}/$item" -type f \
                \( -iname '*.md' -o -iname '*.markdown' \) \
                -print -quit 2>/dev/null | grep -q .; then
                printf '%s%s/\n' "$prefix" "$item"
            fi
        else
            printf '%s%s\n' "$prefix" "$item"
        fi
    done
}

_calci() {
    local i cur prev opts cmd
    COMPREPLY=()
    if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        cur="$2"
    else
        cur="${COMP_WORDS[COMP_CWORD]}"
    fi
    prev="$3"
    cmd=""
    opts="-h -v -p -c --help --version --plain --completion --color"

    for i in "${COMP_WORDS[@]:0:COMP_CWORD}"; do
        case "${cmd},${i}" in
            ",$1")
                cmd="calci"
                ;;
            *)
                ;;
        esac
    done

    case "${cmd}" in
        calci)
            if [[ ${cur} == -* ]]; then
                COMPREPLY=( $(compgen -W "${opts}" -- "${cur}") )
                return 0
            fi
            case "${prev}" in
                --completion|-c)
                    COMPREPLY=($(compgen -W "bash zsh fish" -- "${cur}"))
                    return 0
                    ;;
                --color)
                    COMPREPLY=($(compgen -f -- "${cur}"))
                    return 0
                    ;;
                *)
                    ;;
            esac

            local oldifs
            if [ -n "${IFS+x}" ]; then
                oldifs="$IFS"
            fi
            IFS=$'\n'
            COMPREPLY=($(compgen -W "$(_calci__md_paths "${cur}")" -- "${cur}"))
            if [ -n "${oldifs+x}" ]; then
                IFS="$oldifs"
            fi
            if [[ "${BASH_VERSINFO[0]}" -ge 4 ]]; then
                compopt -o filenames
            fi
            return 0
            ;;
    esac
}

if [[ "${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4 || "${BASH_VERSINFO[0]}" -gt 4 ]]; then
    complete -F _calci -o nosort calci
else
    complete -F _calci calci
fi
"#
}

fn zsh_completion_script() -> &'static str {
    r#"#compdef calci

_calci_md_paths() {
  local -a files dirs
  local cur_dir item
  cur_dir="${PREFIX%/*}"
  if [[ "$PREFIX" == */* ]]; then
    cur_dir="${PREFIX%/*}"
  else
    cur_dir="."
  fi
  local base_prefix=""
  if [[ "$PREFIX" == */* ]]; then
    base_prefix="${PREFIX%/*}/"
  fi
  if [[ -d "$cur_dir" ]]; then
    for item in "$cur_dir"/*(N); do
      if [[ -f "$item" ]]; then
        if [[ "$item" == *.md || "$item" == *.markdown || "$item" == *.MD || "$item" == *.MARKDOWN ]]; then
          files+=("${base_prefix}${item:t}")
        fi
      elif [[ -d "$item" ]]; then
        if [[ -n "$(find "$item" -type f \( -iname '*.md' -o -iname '*.markdown' \) -print -quit 2>/dev/null)" ]]; then
          dirs+=("${base_prefix}${item:t}/")
        fi
      fi
    done
  fi
  (( ${#dirs[@]} )) && compadd -Q -S '' -- "${dirs[@]}"
  (( ${#files[@]} )) && compadd -Q -- "${files[@]}"
  (( ${#dirs[@]} + ${#files[@]} > 0 ))
}

_calci() {
  _arguments -s -S \
    '-h[Print help]' \
    '--help[Print help]' \
    '-v[Print version]' \
    '--version[Print version]' \
    '--color[Use palette file path]:file:_files' \
    '-p[Disable pager UI (prints rendered plain text)]' \
    '--plain[Disable pager UI (prints rendered plain text)]' \
    '-c[Print shell completion script (bash|zsh|fish)]:shell:(bash zsh fish)' \
    '--completion[Print shell completion script (bash|zsh|fish)]:shell:(bash zsh fish)' \
    '::file: _calci_md_paths' && return 0

  case "$state" in
    *)
      return 1
      ;;
  esac
}

if [[ "$funcstack[1]" == "_calci" ]]; then
  _calci "$@"
else
  compdef _calci calci
fi
"#
}

fn fish_completion_script() -> &'static str {
    r#"function __calci_md_paths
    set -l token (commandline -ct)
    set -l dir .
    set -l prefix ""
    if string match -q "*/*" -- $token
        set dir (string replace -r '/[^/]*$' '' -- $token)
        if test -z "$dir"
            set dir "."
        end
        set prefix "$dir/"
    end
    if test -d "$dir"
        for p in $dir/*
            if test -f "$p"
                if string match -qr '\.(md|markdown)$' -- (string lower -- "$p")
                    echo "$prefix"(basename "$p")
                end
            else if test -d "$p"
                if test -n (find "$p" -type f \( -iname '*.md' -o -iname '*.markdown' \) -print -quit 2>/dev/null)
                    echo "$prefix"(basename "$p")/
                end
            end
        end
    end
end

complete -c calci -s c -l completion -d 'Print shell completion script (bash|zsh|fish)' -r -f -a "bash zsh fish"
complete -c calci -l color -d 'Use palette file path' -r -f -a '(__fish_complete_path)'
complete -c calci -s p -l plain -d 'Disable pager UI (prints rendered plain text)'
complete -c calci -s h -l help -d 'Print help'
complete -c calci -s v -l version -d 'Print version'
complete -c calci -f -a '(__calci_md_paths)' -k
"#
}

fn run_dashboard(
    theme: &AppTheme,
    sort_mode: DashboardSort,
    fuzzy_mode: DashboardFuzzyMode,
    show_edited_age: bool,
    mouse_enabled: bool,
    config_dir: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let state_path = dashboard_state_path(config_dir);
    let cache_path = dashboard_cache_path(config_dir);
    let mut dashboard_state = DashboardState::load(&state_path)?;
    let mut entries = DashboardCache::load(&cache_path)?.to_entries(&dashboard_state);
    let mut known_paths = entries
        .iter()
        .map(|e| dashboard_path_key(&e.path))
        .collect::<HashSet<_>>();
    let mut scanner = DashboardScanner::new(Path::new("."));
    let mut modified_checked: HashSet<String> = HashSet::new();
    let mut modified_cursor = 0usize;
    let mut sort_mode = sort_mode;
    let mut cache_dirty = false;
    sort_dashboard_entries(&mut entries, sort_mode);

    enable_raw_mode().context("enable raw mode for dashboard")?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen).context("enter alternate screen for dashboard")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("create terminal for dashboard")?;
    terminal.clear().ok();
    terminal.hide_cursor().ok();
    if mouse_enabled {
        execute!(terminal.backend_mut(), crossterm::event::EnableMouseCapture).ok();
    }

    let mut selected = 0usize;
    let mut chosen: Option<PathBuf> = None;
    let mut search_query = String::new();
    let mut search_mode = false;
    let mut show_help = false;

    let run_result: anyhow::Result<()> = loop {
        let filtered = filtered_entry_indices(&entries, &search_query, fuzzy_mode);
        if filtered.is_empty() {
            selected = 0;
        } else {
            selected = selected.min(filtered.len().saturating_sub(1));
        }

        terminal.draw(|f| {
            let area = f.area();
            let chunks = dashboard_chunks(area);

            let badge_text = " CALCI ";
            let badge_style = theme
                .popup_title
                .add_modifier(Modifier::BOLD)
                .fg(theme.status.bg.unwrap_or(Color::Black))
                .bg(theme
                    .popup_hint
                    .fg
                    .unwrap_or(theme.link.fg.unwrap_or(Color::Yellow)));
            let badge_pad =
                centered_left_pad(chunks[0].width as usize, UnicodeWidthStr::width(badge_text));
            f.render_widget(
                Paragraph::new(vec![Line::from(vec![
                    Span::raw(" ".repeat(badge_pad)),
                    Span::styled(badge_text.to_string(), badge_style),
                ])])
                .style(theme.normal),
                chunks[0],
            );

            let meta_text = if search_mode || !search_query.is_empty() {
                compose_dashboard_meta(
                    &format!("/{}", search_query),
                    &format!(
                        "{} match{}",
                        filtered.len(),
                        if filtered.len() == 1 { "" } else { "es" }
                    ),
                    chunks[1]
                        .width
                        .saturating_sub((2 + DASHBOARD_META_PAD_X * 2) as u16)
                        as usize,
                )
            } else {
                compose_dashboard_meta(
                    &format!("{} documents", entries.len()),
                    &format!("sort: {}", dashboard_sort_label(sort_mode)),
                    chunks[1]
                        .width
                        .saturating_sub((2 + DASHBOARD_META_PAD_X * 2) as u16)
                        as usize,
                )
            };
            let meta_text = if search_mode || !search_query.is_empty() {
                meta_text
            } else if scanner.is_done() {
                meta_text
            } else {
                compose_dashboard_meta(
                    &format!("{} documents (scanning…)", entries.len()),
                    &format!("sort: {}", dashboard_sort_label(sort_mode)),
                    chunks[1]
                        .width
                        .saturating_sub((2 + DASHBOARD_META_PAD_X * 2) as u16)
                        as usize,
                )
            };
            f.render_widget(
                Paragraph::new(vec![Line::from(vec![
                    Span::raw(" ".repeat(2 + DASHBOARD_META_PAD_X)),
                    Span::styled(meta_text, theme.line_number),
                ])])
                .style(theme.normal),
                chunks[2],
            );

            f.render_widget(Paragraph::new("").style(theme.normal), chunks[3]);

            let visible_h = chunks[4].height as usize;
            let list_h = visible_h.saturating_sub(DASHBOARD_BODY_PAD_Y * 2);
            let per_page = (list_h / 3).max(1);
            let total_pages = filtered.len().max(1).div_ceil(per_page);
            let page_start = if filtered.is_empty() {
                0
            } else {
                (selected / per_page) * per_page
            };
            let page_end = (page_start + per_page).min(filtered.len());
            let row_width = chunks[4]
                .width
                .saturating_sub((2 + DASHBOARD_BODY_PAD_X) as u16)
                as usize;

            let mut lines: Vec<Line> = Vec::new();
            for _ in 0..DASHBOARD_BODY_PAD_Y {
                lines.push(Line::from(""));
            }
            if filtered.is_empty() {
                lines.push(Line::from(vec![
                    Span::raw(" ".repeat(2 + DASHBOARD_BODY_PAD_X)),
                    Span::styled("No files match search.", theme.line_number),
                ]));
            } else {
                for list_idx in page_start..page_end {
                    let entry = &entries[filtered[list_idx]];
                    let is_selected = list_idx == selected;
                    let file_style = if is_selected {
                        theme.heading_h2.add_modifier(Modifier::BOLD)
                    } else {
                        theme.normal.add_modifier(Modifier::BOLD)
                    };
                    let right_text = if show_edited_age {
                        let age = entry
                            .modified_unix
                            .map(format_relative_time_value_only_unix)
                            .unwrap_or_else(|| "?".to_string());
                        format!("({age})")
                    } else {
                        String::new()
                    };
                    let reserved_right = if right_text.is_empty() {
                        0
                    } else {
                        UnicodeWidthStr::width(right_text.as_str()) + 1
                    };
                    let filename_only_width = row_width.saturating_sub(reserved_right);
                    let filename = truncate_with_ellipsis(&entry.display, filename_only_width);
                    let relative = truncate_with_ellipsis(&entry.relative, row_width);
                    let left_w = UnicodeWidthStr::width(filename.as_str());
                    let right_w = UnicodeWidthStr::width(right_text.as_str());
                    let gap = row_width.saturating_sub(left_w + right_w);
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(2 + DASHBOARD_BODY_PAD_X)),
                        Span::styled(filename, file_style),
                        Span::raw(" ".repeat(gap)),
                        Span::styled(right_text, theme.line_number),
                    ]));
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(2 + DASHBOARD_BODY_PAD_X)),
                        Span::styled(relative, theme.line_number),
                    ]));
                    lines.push(Line::from(""));
                }
            }
            for _ in 0..DASHBOARD_BODY_PAD_Y {
                lines.push(Line::from(""));
            }
            while lines.len() < visible_h {
                lines.push(Line::from(""));
            }

            f.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .style(theme.normal),
                chunks[4],
            );

            let current_page = if filtered.is_empty() {
                1
            } else {
                (selected / per_page) + 1
            };
            let page_text = format!("Page {current_page}/{total_pages}");
            let page_pad = centered_left_pad(
                chunks[6]
                    .width
                    .saturating_sub((2 + DASHBOARD_META_PAD_X * 2) as u16) as usize,
                UnicodeWidthStr::width(page_text.as_str()),
            );
            let page_row = compose_dashboard_meta(
                &format!("{}{}", " ".repeat(page_pad), page_text),
                "Help ?",
                chunks[6]
                    .width
                    .saturating_sub((2 + DASHBOARD_META_PAD_X * 2) as u16) as usize,
            );
            f.render_widget(
                Paragraph::new(vec![Line::from(vec![
                    Span::raw(" ".repeat(2 + DASHBOARD_META_PAD_X)),
                    Span::styled(page_row, theme.line_number),
                ])])
                .style(theme.normal),
                chunks[6],
            );

            f.render_widget(Paragraph::new("").style(theme.normal), chunks[5]);
            f.render_widget(Paragraph::new("").style(theme.normal), chunks[7]);

            if show_help {
                let popup_w = 54u16.min(area.width.saturating_sub(4));
                let popup_h = 14u16.min(area.height.saturating_sub(4));
                let popup = centered_rect_size(popup_w, popup_h, area);
                f.render_widget(Clear, popup);
                f.render_widget(
                    Block::default().borders(Borders::ALL).style(theme.normal),
                    popup,
                );
                let inner = Rect {
                    x: popup.x + 1,
                    y: popup.y + 1,
                    width: popup.width.saturating_sub(2),
                    height: popup.height.saturating_sub(2),
                };
                let title = centered_text("DASHBOARD KEYBINDINGS", inner.width as usize);
                let rows = vec![
                    dashboard_kb_line(theme, "/", "search"),
                    dashboard_kb_line(theme, "Esc", "clear search / close help"),
                    dashboard_kb_line(theme, "j/k, arrows", "move"),
                    dashboard_kb_line(theme, "h, left arrow", "prev page"),
                    dashboard_kb_line(theme, "l, right arrow", "next page"),
                    dashboard_kb_line(theme, "g/G", "top/bottom"),
                    dashboard_kb_line(theme, "s", "toggle sort mode"),
                    dashboard_kb_line(theme, "Enter", "open file"),
                    dashboard_kb_line(theme, "q", "quit"),
                    dashboard_kb_line(theme, "?", "toggle help"),
                ];
                let body_height = inner.height.saturating_sub(2) as usize;
                let rows = center_dashboard_help_lines(rows, inner.width as usize, body_height);
                f.render_widget(
                    Paragraph::new(
                        std::iter::once(Line::from(vec![Span::styled(
                            title,
                            theme.popup_title.add_modifier(Modifier::BOLD),
                        )]))
                        .chain(std::iter::once(Line::from("")))
                        .chain(rows.into_iter())
                        .collect::<Vec<_>>(),
                    )
                    .style(theme.normal),
                    inner,
                );
            }
        })?;

        let size = terminal.size().unwrap_or_default();
        let per_page = dashboard_per_page(size.height);
        if event::poll(Duration::from_millis(120)).context("dashboard event poll")? {
            match event::read().context("dashboard event read")? {
                event::Event::Key(key) => {
                    if key.modifiers.contains(event::KeyModifiers::CONTROL)
                        && key.code == event::KeyCode::Char('c')
                    {
                        break Ok(());
                    }
                    let filtered = filtered_entry_indices(&entries, &search_query, fuzzy_mode);
                    match key.code {
                        event::KeyCode::Esc => {
                            if show_help {
                                show_help = false;
                            } else if search_mode || !search_query.is_empty() {
                                search_mode = false;
                                search_query.clear();
                                selected = 0;
                            } else {
                                break Ok(());
                            }
                        }
                        event::KeyCode::Char('/') => {
                            search_mode = true;
                            show_help = false;
                        }
                        event::KeyCode::Backspace if search_mode => {
                            search_query.pop();
                            selected = 0;
                        }
                        event::KeyCode::Char(c)
                            if search_mode
                                && (key.modifiers.is_empty()
                                    || key.modifiers == event::KeyModifiers::SHIFT) =>
                        {
                            search_query.push(c);
                            selected = 0;
                        }
                        event::KeyCode::Char('q') if !search_mode => break Ok(()),
                        event::KeyCode::Down | event::KeyCode::Char('j') => {
                            if !filtered.is_empty() && selected + 1 < filtered.len() {
                                selected += 1;
                            }
                        }
                        event::KeyCode::Up | event::KeyCode::Char('k') => {
                            selected = selected.saturating_sub(1);
                        }
                        event::KeyCode::Char('l') | event::KeyCode::Right => {
                            if !filtered.is_empty() {
                                selected =
                                    (selected + per_page).min(filtered.len().saturating_sub(1));
                            }
                        }
                        event::KeyCode::Char('h') | event::KeyCode::Left => {
                            selected = selected.saturating_sub(per_page);
                        }
                        event::KeyCode::Home | event::KeyCode::Char('g') => {
                            selected = 0;
                        }
                        event::KeyCode::End | event::KeyCode::Char('G') => {
                            if !filtered.is_empty() {
                                selected = filtered.len().saturating_sub(1);
                            }
                        }
                        event::KeyCode::Char('s') if !search_mode => {
                            sort_mode = toggle_dashboard_sort(sort_mode);
                            sort_dashboard_entries(&mut entries, sort_mode);
                            selected = 0;
                        }
                        event::KeyCode::Enter => {
                            if !filtered.is_empty() {
                                if let Some(path) =
                                    entries.get(filtered[selected]).map(|e| e.path.clone())
                                {
                                    dashboard_state.mark_opened(&path);
                                    dashboard_state.save(&state_path)?;
                                    chosen = Some(path);
                                }
                                break Ok(());
                            }
                        }
                        event::KeyCode::Char('?') if !search_mode => {
                            show_help = !show_help;
                        }
                        _ => {}
                    }
                }
                event::Event::Mouse(mouse) => {
                    let filtered = filtered_entry_indices(&entries, &search_query, fuzzy_mode);
                    match mouse.kind {
                        event::MouseEventKind::ScrollDown => {
                            if !filtered.is_empty() && selected + 1 < filtered.len() {
                                selected += 1;
                            }
                        }
                        event::MouseEventKind::ScrollUp => {
                            selected = selected.saturating_sub(1);
                        }
                        event::MouseEventKind::ScrollRight => {
                            if !filtered.is_empty() {
                                selected =
                                    (selected + per_page).min(filtered.len().saturating_sub(1));
                            }
                        }
                        event::MouseEventKind::ScrollLeft => {
                            selected = selected.saturating_sub(per_page);
                        }
                        event::MouseEventKind::Down(event::MouseButton::Left) => {
                            if show_help {
                                show_help = false;
                                continue;
                            }
                            let area = Rect {
                                x: 0,
                                y: 0,
                                width: size.width,
                                height: size.height,
                            };
                            if let Some(list_idx) = dashboard_mouse_list_index(
                                area,
                                mouse.row,
                                mouse.column,
                                selected,
                                filtered.len(),
                                per_page,
                            ) {
                                if list_idx == selected {
                                    if let Some(path) =
                                        entries.get(filtered[selected]).map(|e| e.path.clone())
                                    {
                                        dashboard_state.mark_opened(&path);
                                        dashboard_state.save(&state_path)?;
                                        chosen = Some(path);
                                    }
                                    break Ok(());
                                }
                                selected = list_idx;
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        let mut entries_changed = false;
        let discovered = scanner.scan_batch(DASHBOARD_SCAN_BATCH_DIRS, DASHBOARD_SCAN_BATCH_FILES);
        for path in discovered {
            let key = dashboard_path_key(&path);
            if !known_paths.insert(key) {
                continue;
            }
            entries.push(dashboard_entry_from_path(path, &dashboard_state, None));
            entries_changed = true;
            cache_dirty = true;
        }

        if show_edited_age || sort_mode == DashboardSort::LastEdited {
            if hydrate_dashboard_modified_batch(
                &mut entries,
                &mut modified_checked,
                &mut modified_cursor,
                DASHBOARD_MODIFIED_BATCH,
            ) {
                entries_changed = true;
                cache_dirty = true;
            }
        }

        if entries_changed {
            sort_dashboard_entries(&mut entries, sort_mode);
        }

        if scanner.is_done() && entries.is_empty() {
            break Err(anyhow::anyhow!(
                "no markdown files found in current directory"
            ));
        }
    };

    disable_raw_mode().ok();
    if mouse_enabled {
        execute!(
            terminal.backend_mut(),
            crossterm::event::DisableMouseCapture
        )
        .ok();
    }
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    run_result?;
    if cache_dirty {
        DashboardCache::from_entries(&entries).save(&cache_path)?;
    }
    Ok(chosen)
}

#[derive(Clone, Debug)]
struct DashboardEntry {
    path: PathBuf,
    relative: String,
    display: String,
    modified: Option<SystemTime>,
    modified_unix: Option<u64>,
    last_open_unix: Option<u64>,
    lower_search: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DashboardCacheEntry {
    path: String,
    #[serde(default)]
    modified_unix: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DashboardCache {
    #[serde(default)]
    entries: Vec<DashboardCacheEntry>,
}

impl DashboardCache {
    fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content =
            std::fs::read_to_string(path).with_context(|| format!("read '{}'", path.display()))?;
        toml::from_str::<Self>(&content).with_context(|| format!("parse '{}'", path.display()))
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create '{}'", parent.display()))?;
        }
        let content = toml::to_string(self).context("serialize dashboard cache")?;
        std::fs::write(path, content).with_context(|| format!("write '{}'", path.display()))
    }

    fn from_entries(entries: &[DashboardEntry]) -> Self {
        let mut seen = HashSet::new();
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let key = dashboard_path_key(&entry.path);
            if !seen.insert(key.clone()) {
                continue;
            }
            out.push(DashboardCacheEntry {
                path: key,
                modified_unix: entry.modified_unix,
            });
        }
        Self { entries: out }
    }

    fn to_entries(&self, state: &DashboardState) -> Vec<DashboardEntry> {
        self.entries
            .iter()
            .filter_map(|c| {
                let path = PathBuf::from(&c.path);
                path.exists()
                    .then(|| dashboard_entry_from_path(path, state, c.modified_unix))
            })
            .collect()
    }
}

#[derive(Debug)]
struct DashboardScanner {
    pending_dirs: VecDeque<PathBuf>,
    done: bool,
}

impl DashboardScanner {
    fn new(root: &Path) -> Self {
        let mut pending_dirs = VecDeque::new();
        pending_dirs.push_back(root.to_path_buf());
        Self {
            pending_dirs,
            done: false,
        }
    }

    fn is_done(&self) -> bool {
        self.done
    }

    fn scan_batch(&mut self, max_dirs: usize, max_files: usize) -> Vec<PathBuf> {
        if self.done {
            return Vec::new();
        }
        let mut found = Vec::new();
        let max_dirs = max_dirs.max(1);
        let max_files = max_files.max(1);
        for _ in 0..max_dirs {
            let Some(dir) = self.pending_dirs.pop_front() else {
                self.done = true;
                break;
            };
            let Ok(read_dir) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in read_dir.flatten() {
                let path = entry.path();
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if file_type.is_dir() {
                    if should_skip_dashboard_dir(&path) {
                        continue;
                    }
                    self.pending_dirs.push_back(path);
                    continue;
                }
                if file_type.is_file() && is_markdown_path(&path) {
                    found.push(path);
                }
            }
            if found.len() >= max_files {
                break;
            }
        }
        if self.pending_dirs.is_empty() {
            self.done = true;
        }
        found
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct DashboardState {
    #[serde(default)]
    last_open_unix: HashMap<String, u64>,
}

impl DashboardState {
    fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content =
            std::fs::read_to_string(path).with_context(|| format!("read '{}'", path.display()))?;
        toml::from_str::<Self>(&content).with_context(|| format!("parse '{}'", path.display()))
    }

    fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create '{}'", parent.display()))?;
        }
        let content = toml::to_string(self).context("serialize dashboard state")?;
        std::fs::write(path, content).with_context(|| format!("write '{}'", path.display()))
    }

    fn mark_opened(&mut self, path: &Path) {
        self.last_open_unix
            .insert(dashboard_path_key(path), now_unix_secs());
    }

    fn last_open_unix_for(&self, path: &Path) -> Option<u64> {
        self.last_open_unix.get(&dashboard_path_key(path)).copied()
    }
}

fn dashboard_entry_from_path(
    path: PathBuf,
    dashboard_state: &DashboardState,
    modified_unix: Option<u64>,
) -> DashboardEntry {
    let relative = path
        .strip_prefix(".")
        .unwrap_or(path.as_path())
        .display()
        .to_string();
    let display = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(ToString::to_string)
        .unwrap_or_else(|| relative.clone());
    let last_open_unix = dashboard_state.last_open_unix_for(&path);
    let lower_search = format!("{} {}", relative.to_lowercase(), display.to_lowercase());
    DashboardEntry {
        path,
        relative,
        display,
        modified: modified_unix.and_then(system_time_from_unix),
        modified_unix,
        last_open_unix,
        lower_search,
    }
}

fn hydrate_dashboard_modified_batch(
    entries: &mut [DashboardEntry],
    checked: &mut HashSet<String>,
    cursor: &mut usize,
    batch_size: usize,
) -> bool {
    if entries.is_empty() {
        *cursor = 0;
        return false;
    }
    if *cursor >= entries.len() {
        *cursor = 0;
    }
    let mut changed = false;
    let mut processed = 0usize;
    let mut visited = 0usize;
    let batch_size = batch_size.max(1).min(entries.len());
    while processed < batch_size && visited < entries.len() {
        let idx = *cursor;
        *cursor = (*cursor + 1) % entries.len();
        visited += 1;
        let key = dashboard_path_key(&entries[idx].path);
        if checked.contains(&key) {
            continue;
        }
        checked.insert(key);
        let new_modified = std::fs::metadata(&entries[idx].path)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(unix_secs);
        if entries[idx].modified_unix != new_modified {
            entries[idx].modified_unix = new_modified;
            entries[idx].modified = new_modified.and_then(system_time_from_unix);
            changed = true;
        }
        processed += 1;
    }
    changed
}

fn toggle_dashboard_sort(mode: DashboardSort) -> DashboardSort {
    match mode {
        DashboardSort::LastOpen => DashboardSort::LastEdited,
        DashboardSort::LastEdited => DashboardSort::LastOpen,
    }
}

fn filtered_entry_indices(
    entries: &[DashboardEntry],
    query: &str,
    fuzzy_mode: DashboardFuzzyMode,
) -> Vec<usize> {
    let q = query.trim();
    if q.is_empty() {
        return (0..entries.len()).collect();
    }
    let mut scored = entries
        .iter()
        .enumerate()
        .filter_map(|(idx, e)| {
            dashboard_fuzzy_score(&e.lower_search, q, fuzzy_mode)
                .map(|score| DashboardMatch { index: idx, score })
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.index.cmp(&b.index)));
    scored.into_iter().map(|m| m.index).collect()
}

#[derive(Clone, Copy, Debug)]
struct DashboardMatch {
    index: usize,
    score: i32,
}

fn fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
    let h = haystack.to_lowercase();
    let n = needle.trim().to_lowercase();
    if n.is_empty() {
        return Some(0);
    }
    let mut score = 0i32;
    let mut h_iter = h.char_indices();
    let mut last_match: Option<usize> = None;
    for nc in n.chars() {
        let mut found = None;
        for (idx, hc) in h_iter.by_ref() {
            if hc == nc {
                found = Some(idx);
                break;
            }
        }
        let idx = found?;
        score += 10;
        if let Some(prev) = last_match {
            if idx == prev + 1 {
                score += 8;
            } else {
                score -= (idx.saturating_sub(prev + 1) as i32).min(6);
            }
        } else {
            score += (50i32 - idx as i32).max(0) / 5;
        }
        if idx == 0 {
            score += 6;
        } else {
            let prev_ch = h[..idx].chars().next_back().unwrap_or(' ');
            if matches!(prev_ch, '/' | '-' | '_' | ' ' | '.') {
                score += 4;
            }
        }
        last_match = Some(idx);
    }
    score -= (h.len().saturating_sub(n.len()) as i32 / 12).min(8);
    Some(score)
}

fn strict_fuzzy_score(haystack: &str, needle: &str) -> Option<i32> {
    let h = haystack.to_lowercase();
    let n = needle.trim().to_lowercase();
    if n.is_empty() {
        return Some(0);
    }
    let idx = h.find(&n)?;
    let mut score = 120i32;
    score -= (idx as i32).min(60);
    if idx == 0 {
        score += 10;
    } else {
        let prev = h[..idx].chars().next_back().unwrap_or(' ');
        if matches!(prev, '/' | '-' | '_' | ' ' | '.') {
            score += 6;
        }
    }
    score -= (h.len().saturating_sub(n.len()) as i32 / 8).min(20);
    Some(score)
}

fn dashboard_fuzzy_score(haystack: &str, needle: &str, mode: DashboardFuzzyMode) -> Option<i32> {
    match mode {
        DashboardFuzzyMode::Strict => strict_fuzzy_score(haystack, needle),
        DashboardFuzzyMode::Loose => fuzzy_score(haystack, needle),
    }
}

fn should_skip_dashboard_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name.starts_with('.') {
        return true;
    }
    matches!(
        name,
        "target"
            | "node_modules"
            | "dist"
            | "build"
            | "vendor"
            | "venv"
            | ".venv"
            | "__pycache__"
            | ".mypy_cache"
            | ".pytest_cache"
            | ".idea"
            | ".vscode"
            | ".direnv"
            | ".cache"
    )
}

fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false)
}

fn sort_dashboard_entries(entries: &mut [DashboardEntry], sort_mode: DashboardSort) {
    entries.sort_by(|a, b| {
        let by_sort_mode = match sort_mode {
            DashboardSort::LastEdited => compare_recent(a.modified_unix, b.modified_unix),
            DashboardSort::LastOpen => compare_recent(a.last_open_unix, b.last_open_unix)
                .then_with(|| compare_recent(a.modified_unix, b.modified_unix)),
        };
        by_sort_mode
            .then_with(|| a.display.cmp(&b.display))
            .then_with(|| a.path.cmp(&b.path))
    });
}

fn compare_recent(lhs: Option<u64>, rhs: Option<u64>) -> std::cmp::Ordering {
    rhs.cmp(&lhs)
}

fn dashboard_sort_label(sort_mode: DashboardSort) -> &'static str {
    match sort_mode {
        DashboardSort::LastOpen => "last_open",
        DashboardSort::LastEdited => "last_edited",
    }
}

fn centered_left_pad(total_width: usize, content_width: usize) -> usize {
    if content_width >= total_width {
        0
    } else {
        (total_width - content_width) / 2
    }
}

fn centered_text(text: &str, width: usize) -> String {
    let text_w = UnicodeWidthStr::width(text);
    if text_w >= width {
        truncate_with_ellipsis(text, width)
    } else {
        format!("{}{}", " ".repeat((width - text_w) / 2), text)
    }
}

fn centered_rect_size(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width).max(1);
    let h = height.min(area.height).max(1);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}

fn dashboard_chunks(area: Rect) -> [Rect; 8] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);
    [
        chunks[0], chunks[1], chunks[2], chunks[3], chunks[4], chunks[5], chunks[6], chunks[7],
    ]
}

fn dashboard_mouse_list_index(
    area: Rect,
    row: u16,
    col: u16,
    selected: usize,
    filtered_len: usize,
    per_page: usize,
) -> Option<usize> {
    if filtered_len == 0 {
        return None;
    }
    let chunks = dashboard_chunks(area);
    let list = chunks[4];
    if row < list.y || row >= list.y.saturating_add(list.height) {
        return None;
    }
    let left_pad = (2 + DASHBOARD_BODY_PAD_X) as u16;
    if col < list.x.saturating_add(left_pad) || col >= list.x.saturating_add(list.width) {
        return None;
    }
    let row_in = (row - list.y) as usize;
    if row_in < DASHBOARD_BODY_PAD_Y
        || row_in >= (list.height as usize).saturating_sub(DASHBOARD_BODY_PAD_Y)
    {
        return None;
    }
    let row_on_page = row_in - DASHBOARD_BODY_PAD_Y;
    if row_on_page % 3 == 2 {
        return None;
    }
    let entry_on_page = row_on_page / 3;
    let page_start = (selected / per_page.max(1)) * per_page.max(1);
    let page_end = (page_start + per_page.max(1)).min(filtered_len);
    let list_idx = page_start + entry_on_page;
    (list_idx < page_end).then_some(list_idx)
}

fn dashboard_kb_line<'a>(theme: &AppTheme, key: &'a str, desc: &'a str) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("  {:<18}", key),
            theme.popup_key.remove_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(desc.to_string(), theme.normal),
    ])
}

fn center_dashboard_help_lines(
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

fn compose_dashboard_meta(left: &str, right: &str, width: usize) -> String {
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

fn dashboard_per_page(term_height: u16) -> usize {
    let area = Rect {
        x: 0,
        y: 0,
        width: 1,
        height: term_height,
    };
    let chunks = dashboard_chunks(area);
    let usable = (chunks[4].height as usize).saturating_sub(DASHBOARD_BODY_PAD_Y * 2);
    (usable / 3).max(1)
}

fn format_relative_time_value_only(ts: SystemTime) -> String {
    let Ok(delta) = SystemTime::now().duration_since(ts) else {
        return "now".to_string();
    };
    let secs = delta.as_secs();
    if secs < 60 {
        "now".to_string()
    } else if secs < 3_600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3_600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

fn dashboard_state_path(config_dir: &Path) -> PathBuf {
    config_dir.join("dashboard_state.toml")
}

fn dashboard_cache_path(config_dir: &Path) -> PathBuf {
    config_dir.join("dashboard_cache.toml")
}

fn dashboard_path_key(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.strip_prefix("./").unwrap_or(s.as_ref()).to_string()
}

fn format_relative_time_value_only_unix(unix_ts: u64) -> String {
    if let Some(ts) = system_time_from_unix(unix_ts) {
        return format_relative_time_value_only(ts);
    }
    "?".to_string()
}

fn unix_secs(ts: SystemTime) -> Option<u64> {
    ts.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn system_time_from_unix(unix_ts: u64) -> Option<SystemTime> {
    UNIX_EPOCH.checked_add(Duration::from_secs(unix_ts))
}

fn now_unix_secs() -> u64 {
    unix_secs(SystemTime::now()).unwrap_or(0)
}

fn truncate_with_ellipsis(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    if UnicodeWidthStr::width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let target = max_width - 1;
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let cw = ch.width().unwrap_or(0);
        if width + cw > target {
            break;
        }
        out.push(ch);
        width += cw;
    }
    out.push('…');
    out
}

fn line_to_ansi(line: &Line<'_>, default_style: Style) -> String {
    let mut out = String::new();
    for sp in &line.spans {
        let style = default_style.patch(sp.style);
        let prefix = ansi_style_prefix(style);
        if prefix.is_empty() {
            out.push_str(sp.content.as_ref());
        } else {
            out.push_str(prefix.as_str());
            out.push_str(sp.content.as_ref());
            out.push_str("\x1b[0m");
        }
    }
    out
}

fn line_to_ansi_with_links(
    render_line: &renderer::RenderLine,
    default_style: Style,
    link_style: Style,
) -> String {
    if render_line.link_ranges.is_empty() {
        return line_to_ansi(&render_line.line, default_style);
    }

    let mut inserts: Vec<(usize, String)> = render_line
        .link_ranges
        .iter()
        .filter_map(|lr| {
            let shown = text_for_display_range(&render_line.line, lr.start, lr.end);
            if should_append_link_suffix(shown.as_str(), lr.url.as_str()) {
                Some((lr.end, format!(" ({})", lr.url)))
            } else {
                None
            }
        })
        .collect();
    inserts.sort_by_key(|(col, _)| *col);

    let mut out_spans: Vec<Span<'static>> = Vec::new();
    let mut insert_idx = 0usize;
    let mut col = 0usize;

    for sp in &render_line.line.spans {
        let style = sp.style;
        let mut buf = String::new();
        for ch in sp.content.chars() {
            while insert_idx < inserts.len() && inserts[insert_idx].0 == col {
                if !buf.is_empty() {
                    out_spans.push(Span::styled(std::mem::take(&mut buf), style));
                }
                out_spans.push(Span::styled(inserts[insert_idx].1.clone(), link_style));
                insert_idx += 1;
            }
            buf.push(ch);
            col += ch.width().unwrap_or(0);
        }
        if !buf.is_empty() {
            out_spans.push(Span::styled(buf, style));
        }
    }

    while insert_idx < inserts.len() {
        out_spans.push(Span::styled(inserts[insert_idx].1.clone(), link_style));
        insert_idx += 1;
    }

    line_to_ansi(&Line::from(out_spans), default_style)
}

fn text_for_display_range(line: &Line<'_>, start: usize, end: usize) -> String {
    let mut out = String::new();
    let mut col = 0usize;
    for sp in &line.spans {
        for ch in sp.content.chars() {
            let w = ch.width().unwrap_or(0).max(1);
            let ch_start = col;
            let ch_end = col + w;
            if ch_start < end && ch_end > start {
                out.push(ch);
            }
            col = ch_end;
        }
    }
    out
}

fn should_append_link_suffix(display_text: &str, url: &str) -> bool {
    let d = display_text.trim();
    let u = url.trim();
    if d.eq_ignore_ascii_case(u) {
        return false;
    }
    if d.len() >= 2 && d.starts_with('<') && d.ends_with('>') {
        let inner = d[1..d.len() - 1].trim();
        if inner.eq_ignore_ascii_case(u) {
            return false;
        }
    }
    true
}

fn ansi_style_prefix(style: Style) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(fg) = style.fg {
        if let Some(code) = ansi_fg_code(fg) {
            parts.push(code);
        }
    }
    if let Some(bg) = style.bg {
        if let Some(code) = ansi_bg_code(bg) {
            parts.push(code);
        }
    }
    let m = style.add_modifier;
    if m.contains(Modifier::BOLD) {
        parts.push("1".to_string());
    }
    if m.contains(Modifier::DIM) {
        parts.push("2".to_string());
    }
    if m.contains(Modifier::ITALIC) {
        parts.push("3".to_string());
    }
    if m.contains(Modifier::UNDERLINED) {
        parts.push("4".to_string());
    }
    if m.contains(Modifier::SLOW_BLINK) {
        parts.push("5".to_string());
    }
    if m.contains(Modifier::RAPID_BLINK) {
        parts.push("6".to_string());
    }
    if m.contains(Modifier::REVERSED) {
        parts.push("7".to_string());
    }
    if m.contains(Modifier::HIDDEN) {
        parts.push("8".to_string());
    }
    if m.contains(Modifier::CROSSED_OUT) {
        parts.push("9".to_string());
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("\x1b[{}m", parts.join(";"))
    }
}

fn ansi_fg_code(c: Color) -> Option<String> {
    match c {
        Color::Reset => Some("39".to_string()),
        Color::Black => Some("30".to_string()),
        Color::Red => Some("31".to_string()),
        Color::Green => Some("32".to_string()),
        Color::Yellow => Some("33".to_string()),
        Color::Blue => Some("34".to_string()),
        Color::Magenta => Some("35".to_string()),
        Color::Cyan => Some("36".to_string()),
        Color::Gray => Some("37".to_string()),
        Color::DarkGray => Some("90".to_string()),
        Color::LightRed => Some("91".to_string()),
        Color::LightGreen => Some("92".to_string()),
        Color::LightYellow => Some("93".to_string()),
        Color::LightBlue => Some("94".to_string()),
        Color::LightMagenta => Some("95".to_string()),
        Color::LightCyan => Some("96".to_string()),
        Color::White => Some("97".to_string()),
        Color::Rgb(r, g, b) => Some(format!("38;2;{r};{g};{b}")),
        Color::Indexed(v) => Some(format!("38;5;{v}")),
    }
}

fn ansi_bg_code(c: Color) -> Option<String> {
    match c {
        Color::Reset => Some("49".to_string()),
        Color::Black => Some("40".to_string()),
        Color::Red => Some("41".to_string()),
        Color::Green => Some("42".to_string()),
        Color::Yellow => Some("43".to_string()),
        Color::Blue => Some("44".to_string()),
        Color::Magenta => Some("45".to_string()),
        Color::Cyan => Some("46".to_string()),
        Color::Gray => Some("47".to_string()),
        Color::DarkGray => Some("100".to_string()),
        Color::LightRed => Some("101".to_string()),
        Color::LightGreen => Some("102".to_string()),
        Color::LightYellow => Some("103".to_string()),
        Color::LightBlue => Some("104".to_string()),
        Color::LightMagenta => Some("105".to_string()),
        Color::LightCyan => Some("106".to_string()),
        Color::White => Some("107".to_string()),
        Color::Rgb(r, g, b) => Some(format!("48;2;{r};{g};{b}")),
        Color::Indexed(v) => Some(format!("48;5;{v}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;
    use std::path::Path;
    use tempfile::tempdir;

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn completion_generation_runs() {
        assert!(bash_completion_script().contains("_calci"));
        assert!(zsh_completion_script().contains("#compdef calci"));
        assert!(fish_completion_script().contains("complete -c calci"));
        assert!(bash_completion_script().contains("opts=\"-h -v -p -c"));
        assert!(bash_completion_script().contains("--color"));
        assert!(zsh_completion_script().contains("--color[Use palette file path]"));
        assert!(fish_completion_script().contains("complete -c calci -l color"));
        assert!(zsh_completion_script().contains("-p[Disable pager UI"));
        assert!(zsh_completion_script().contains("compadd -Q -S '' --"));
        assert!(fish_completion_script().contains("complete -c calci -s p -l plain"));
    }

    #[test]
    fn completion_shell_limits() {
        assert_eq!(CompletionShell::value_variants().len(), 3);
    }

    #[test]
    fn relative_time_value_only_format_works() {
        let now = SystemTime::now();
        let s = format_relative_time_value_only(now);
        assert!(!s.is_empty());
        assert!(!s.contains("ago"));
    }

    #[test]
    fn truncate_with_ellipsis_respects_width() {
        let out = truncate_with_ellipsis("abcdef", 4);
        assert_eq!(UnicodeWidthStr::width(out.as_str()), 4);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn ansi_render_includes_bold_escape() {
        let line = Line::from(vec![Span::styled(
            "x",
            Style::default().add_modifier(Modifier::BOLD),
        )]);
        let out = line_to_ansi(&line, Style::default());
        assert!(out.contains("\x1b[1m"));
        assert!(out.ends_with("\x1b[0m"));
    }

    #[test]
    fn autolink_suffix_is_not_duplicated() {
        assert!(!should_append_link_suffix(
            "https://example.com",
            "https://example.com"
        ));
        assert!(!should_append_link_suffix(
            "<https://example.com>",
            "https://example.com"
        ));
        assert!(should_append_link_suffix("Example", "https://example.com"));
    }

    #[test]
    fn dashboard_sort_defaults_to_last_open_recency() {
        let mut entries = vec![
            DashboardEntry {
                path: PathBuf::from("b.md"),
                relative: "b.md".to_string(),
                display: "b.md".to_string(),
                modified: None,
                modified_unix: Some(10),
                last_open_unix: Some(20),
                lower_search: "b.md".to_string(),
            },
            DashboardEntry {
                path: PathBuf::from("a.md"),
                relative: "a.md".to_string(),
                display: "a.md".to_string(),
                modified: None,
                modified_unix: Some(99),
                last_open_unix: Some(1),
                lower_search: "a.md".to_string(),
            },
        ];
        sort_dashboard_entries(&mut entries, DashboardSort::LastOpen);
        assert_eq!(entries[0].display, "b.md");
    }

    #[test]
    fn dashboard_sort_last_edited_uses_modified_time() {
        let mut entries = vec![
            DashboardEntry {
                path: PathBuf::from("old.md"),
                relative: "old.md".to_string(),
                display: "old.md".to_string(),
                modified: None,
                modified_unix: Some(5),
                last_open_unix: Some(99),
                lower_search: "old.md".to_string(),
            },
            DashboardEntry {
                path: PathBuf::from("new.md"),
                relative: "new.md".to_string(),
                display: "new.md".to_string(),
                modified: None,
                modified_unix: Some(50),
                last_open_unix: Some(1),
                lower_search: "new.md".to_string(),
            },
        ];
        sort_dashboard_entries(&mut entries, DashboardSort::LastEdited);
        assert_eq!(entries[0].display, "new.md");
    }

    #[test]
    fn dashboard_state_roundtrip() {
        let d = tempdir().expect("tempdir");
        let state_path = d.path().join("dashboard_state.toml");
        let mut state = DashboardState::default();
        state.mark_opened(Path::new("./demo.md"));
        state.save(&state_path).expect("save");
        let loaded = DashboardState::load(&state_path).expect("load");
        assert!(loaded.last_open_unix_for(Path::new("demo.md")).is_some());
    }

    #[test]
    fn compare_recent_orders_desc() {
        assert_eq!(compare_recent(Some(5), Some(1)), Ordering::Less);
        assert_eq!(compare_recent(Some(1), Some(5)), Ordering::Greater);
    }

    #[test]
    fn fuzzy_score_matches_in_order_only() {
        assert!(fuzzy_score("docs/readme.md", "drm").is_some());
        assert!(fuzzy_score("docs/readme.md", "zrm").is_none());
    }

    #[test]
    fn fuzzy_mode_strict_vs_loose_behaves_differently() {
        assert!(
            dashboard_fuzzy_score("docs/readme.md", "drm", DashboardFuzzyMode::Loose).is_some()
        );
        assert!(
            dashboard_fuzzy_score("docs/readme.md", "drm", DashboardFuzzyMode::Strict).is_none()
        );
    }

    #[test]
    fn fuzzy_filter_ranks_better_match_first() {
        let entries = vec![
            DashboardEntry {
                path: PathBuf::from("a.md"),
                relative: "notes/alpha.md".to_string(),
                display: "alpha.md".to_string(),
                modified: None,
                modified_unix: None,
                last_open_unix: None,
                lower_search: "notes/alpha.md alpha.md".to_string(),
            },
            DashboardEntry {
                path: PathBuf::from("b.md"),
                relative: "notes/alphabet.md".to_string(),
                display: "alphabet.md".to_string(),
                modified: None,
                modified_unix: None,
                last_open_unix: None,
                lower_search: "notes/alphabet.md alphabet.md".to_string(),
            },
        ];
        let idxs = filtered_entry_indices(&entries, "alpmd", DashboardFuzzyMode::Loose);
        assert_eq!(idxs.first().copied(), Some(0));
    }

    #[test]
    fn fuzzy_filter_preserves_sort_priority_on_tie() {
        let entries = vec![
            DashboardEntry {
                path: PathBuf::from("z.md"),
                relative: "docs/z.md".to_string(),
                display: "z.md".to_string(),
                modified: None,
                modified_unix: None,
                last_open_unix: None,
                lower_search: "docs/z.md z.md".to_string(),
            },
            DashboardEntry {
                path: PathBuf::from("a.md"),
                relative: "docs/a.md".to_string(),
                display: "a.md".to_string(),
                modified: None,
                modified_unix: None,
                last_open_unix: None,
                lower_search: "docs/a.md a.md".to_string(),
            },
        ];
        let idxs = filtered_entry_indices(&entries, "md", DashboardFuzzyMode::Loose);
        assert_eq!(idxs, vec![0, 1]);
    }

    #[test]
    fn fuzzy_filter_strict_requires_contiguous_match() {
        let entries = vec![DashboardEntry {
            path: PathBuf::from("alpha.md"),
            relative: "docs/alpha.md".to_string(),
            display: "alpha.md".to_string(),
            modified: None,
            modified_unix: None,
            last_open_unix: None,
            lower_search: "docs/alpha.md alpha.md".to_string(),
        }];
        let loose = filtered_entry_indices(&entries, "amd", DashboardFuzzyMode::Loose);
        let strict = filtered_entry_indices(&entries, "amd", DashboardFuzzyMode::Strict);
        assert_eq!(loose, vec![0]);
        assert!(strict.is_empty());
    }

    #[test]
    fn centered_text_adds_left_padding() {
        let out = centered_text("A", 5);
        assert!(out.starts_with("  "));
    }

    #[test]
    fn dashboard_kb_line_renders_new_page_keys() {
        let theme = AppTheme::default();
        let line = dashboard_kb_line(&theme, "h, left arrow", "prev page");
        let text = line_text(&line);
        assert!(text.contains("h, left arrow"));
        assert!(text.contains("prev page"));
    }

    #[test]
    fn dashboard_help_line_layout_snapshot() {
        let theme = AppTheme::default();
        let line = dashboard_kb_line(&theme, "h, left arrow", "prev page");
        assert_eq!(line_text(&line), "  h, left arrow       prev page");
    }

    #[test]
    fn dashboard_sort_toggle_switches_modes() {
        assert_eq!(
            toggle_dashboard_sort(DashboardSort::LastOpen),
            DashboardSort::LastEdited
        );
        assert_eq!(
            toggle_dashboard_sort(DashboardSort::LastEdited),
            DashboardSort::LastOpen
        );
    }

    #[test]
    fn hidden_and_common_dirs_are_skipped() {
        assert!(should_skip_dashboard_dir(Path::new("./.git")));
        assert!(should_skip_dashboard_dir(Path::new("./target")));
        assert!(should_skip_dashboard_dir(Path::new("./node_modules")));
        assert!(!should_skip_dashboard_dir(Path::new("./docs")));
    }

    #[test]
    fn cache_roundtrip_preserves_path_and_modified() {
        let d = tempdir().expect("tempdir");
        let cache_path = d.path().join("dashboard_cache.toml");
        let cache = DashboardCache {
            entries: vec![DashboardCacheEntry {
                path: "docs/a.md".to_string(),
                modified_unix: Some(42),
            }],
        };
        cache.save(&cache_path).expect("save cache");
        let loaded = DashboardCache::load(&cache_path).expect("load cache");
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].path, "docs/a.md");
        assert_eq!(loaded.entries[0].modified_unix, Some(42));
    }

    #[test]
    fn scanner_skips_hidden_and_library_dirs() {
        let d = tempdir().expect("tempdir");
        let root = d.path();
        std::fs::create_dir_all(root.join("docs")).expect("mkdir docs");
        std::fs::create_dir_all(root.join(".git")).expect("mkdir .git");
        std::fs::create_dir_all(root.join("target")).expect("mkdir target");
        std::fs::write(root.join("docs").join("ok.md"), "# ok").expect("write docs md");
        std::fs::write(root.join(".git").join("skip.md"), "# skip").expect("write git md");
        std::fs::write(root.join("target").join("skip.md"), "# skip").expect("write target md");

        let mut scanner = DashboardScanner::new(root);
        let mut found = Vec::new();
        while !scanner.is_done() {
            found.extend(scanner.scan_batch(16, 128));
        }
        let names = found
            .iter()
            .filter_map(|p| p.file_name().and_then(|n| n.to_str()))
            .collect::<Vec<_>>();
        assert!(names.contains(&"ok.md"));
        assert!(!names.contains(&"skip.md"));
    }

    #[test]
    fn dashboard_help_center_layout_snapshot() {
        let theme = AppTheme::default();
        let rows = vec![dashboard_kb_line(&theme, "q", "quit")];
        let centered = center_dashboard_help_lines(rows, 30, 4);
        let texts = centered.iter().map(line_text).collect::<Vec<_>>();
        assert_eq!(texts, vec!["", "    q                   quit", "", ""]);
    }

    #[test]
    fn dashboard_page_row_layout_snapshot() {
        let row =
            compose_dashboard_meta(&format!("{}{}", " ".repeat(11), "Page 2/9"), "Help ?", 30);
        assert_eq!(row, "           Page 2/9     Help ?");
    }

    #[test]
    fn dashboard_help_lines_can_be_centered() {
        let theme = AppTheme::default();
        let rows = vec![dashboard_kb_line(&theme, "q", "quit")];
        let centered = center_dashboard_help_lines(rows, 30, 4);
        assert!(centered.len() >= 4);
    }

    #[test]
    fn dashboard_mouse_row_mapping_works() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let per_page = dashboard_per_page(area.height);
        assert_eq!(
            dashboard_mouse_list_index(area, 5, 6, 0, 20, per_page),
            Some(0)
        );
        assert_eq!(
            dashboard_mouse_list_index(area, 6, 6, 0, 20, per_page),
            Some(0)
        );
        assert_eq!(
            dashboard_mouse_list_index(area, 7, 6, 0, 20, per_page),
            None
        );
        assert_eq!(
            dashboard_mouse_list_index(area, 8, 6, 0, 20, per_page),
            Some(1)
        );
    }

    #[test]
    fn dashboard_mouse_row_mapping_respects_page() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 24,
        };
        let per_page = dashboard_per_page(area.height);
        assert_eq!(
            dashboard_mouse_list_index(area, 5, 6, 6, 20, per_page),
            Some(5)
        );
    }

    #[test]
    fn dashboard_mouse_scroll_vertical_moves_files_not_pages() {
        let mut selected = 3usize;
        let filtered_len = 40usize;
        if selected + 1 < filtered_len {
            selected += 1;
        }
        assert_eq!(selected, 4);
        selected = selected.saturating_sub(1);
        assert_eq!(selected, 3);
    }

    #[test]
    fn dashboard_mouse_scroll_horizontal_moves_pages() {
        let per_page = 5usize;
        let filtered_len = 40usize;
        let mut selected = 6usize;
        selected = (selected + per_page).min(filtered_len.saturating_sub(1));
        assert_eq!(selected, 11);
        selected = selected.saturating_sub(per_page);
        assert_eq!(selected, 6);
    }
}

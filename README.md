# calci

Rust TUI markdown pager inspired by [Glow](https://github.com/charmbracelet/glow) and heavily modified on my personal needs with LaTeX math rendering via [RaTeX](https://crates.io/crates/ratex-render).

![pager first image](./assets/screenshots/pager1.png)
![pager second image](./assets/screenshots/pager2.png "Pager")
![dashboard image](./assets/screenshots/dashboard.png "Dashboard view")
![plain mode image](./assets/screenshots/plain.png "Plain mode")

## Features

- Dashboard, pager and plain mode
- Text/File search (`/`, `n`, `N`)
- Direct editing (`e`) and reload (`r`)
- Configurable behavior via `config.toml` and colors via `color.toml`
- Sticky title and distinct heading levels
- Code block rendering with syntax highlighting and title support
- LaTeX math (`$...$`, `$$...$$`, `math`/`latex` code blocks) drawn as images in kitty, with math font set with `math_font`.
- Images rendering via Kitty graphics protocol with width support, loaded in the background (remote images opt-in)
- Copy entire code block to clipboard with `y` or mouse click, and inline `code` with a click
- Link opening with `o` or mouse click
- Task lists with checkbox icons that can be toggled with `x` or a click (`checkbox_toggle`)
- Markdown formatting support
- Typographic smart quotes
- Toggable mouse mode to allow text selection
- Hover feedback: link URL / copy / toggle hints in the status bar

## Build

```bash
cargo build --release
```

## Usage

```bash
# Pager mode (default)
calci README.md

# Non-pager mode (render to stdout)
calci --plain README.md

# Generate shell completion
calci -c zsh > _calci
```

Completion offers also positional tab suggestion of Markdown files and relative directories that contain Markdown files.

### Config files

Defaults:

- app config is read from `~/.config/calci/config.toml` when present
- colors are read from `~/.config/calci/color.toml` when present
- dashboard open history: `~/.config/calci/dashboard_state.toml`
- dashboard file cache: `~/.config/calci/dashboard_cache.toml`
- `--color <FILE>` overrides the palette file path for the current run

`config.toml` supports:
- `dashboard_sort = "last_open"` (default) or `"last_edited"`
- `dashboard_fuzzy_mode = "loose"` (default) or `"strict"`
- `dashboard_show_edited_age = false` (default)
- `mouse = true` enables dashboard click + wheel gestures and pager mouse actions by default
- `remote_images = false` (default): `http(s)` images are not fetched and show a clickable `[file-name]` placeholder; when `true` they are fetched with a 5s timeout and a 20 MB size cap
- `table_row_separator = "never"` (default), `"wrapped"` (separator lines between body rows only in tables where a row wraps) or `"always"`; tables always shrink to the window width, wrapping cell text as needed
- `checkbox_toggle = false` (default): when `true`, `x` or clicking a task checkbox rewrites `[ ]` ↔ `[x]` in the file (refused if the file changed on disk since it was loaded); checkboxes use Nerd Font glyphs
- `heading_scale = false` (default): when `true` and running in kitty 0.40+, `##` headings are drawn at 2x size (text sizing protocol); elsewhere headings are styled only
- `link_confirmation = false` (default): `http`, `https` and `mailto` links open directly; any other scheme or relative path always asks for confirmation
- dashboard hotkey `s` toggles sort mode (`last_open`/`last_edited`) at runtime

## Keybindings

- `q`: quit
- `j/k`, `↓/↑`: move
- `PgDn/PgUp`, `Space`: page scroll
- `/`: search mode
- `n` / `N`: next / previous match
- `y`: copy selected code block
- `o`: open link on selected line (shows confirm popup)
- mouse click on line with link: open link (when selection mode is off)
- mouse click on code line: copy code block (when selection mode is off)
- `e`: open editor (`$EDITOR`, default `vi`)
- `r`: reload content
- `m`: toggle selection mode (off by default)
- `x` / click on a task checkbox: toggle `[ ]` ↔ `[x]` in the file (requires `checkbox_toggle = true`)
- `?`: keybindings popup

### Dashboard search modes

- `loose` (default): in-order fuzzy match with gaps allowed.
  Example: `drm` matches `docs/readme.md`.
- `strict`: contiguous substring only.
  Example: `read` matches `docs/readme.md`, but `drm` does not.

### color.toml format

`Calci` uses a `[base16]` color palette that auto-maps defaults for pager/search/code
For explicit overrides, the following sections can be adjusted:
  - `[pager]` with `text`, `heading`, `quote`, `list_marker`, `link`, `status_fg`, `status_bg`, `cursor_line_bg`, `line_number_fg`
  - `[search]` with `hit_fg`, `hit_bg`, `current_fg`, `current_bg`
  - `[code]` with `inline`, `black`, `grey`, `white`, `purple`, `pink`, `blue`, `cyan`, `green`, `red`, `yellow`, `orange`

## Notes

- Search ignores code/math
- Image decoding/fetching is pager-only and needs a terminal with the kitty graphics protocol (kitty, Ghostty); elsewhere the alt text is shown

# calci

Rust TUI markdown pager inspired by Glow, with integrated math rendering via `calcifer`.

## Features

- Pager-first TUI interface (default mode) + no-arg dashboard mode
- Smooth scrolling (`j/k`, arrows, page up/down, mouse wheel)
- Search in text (`/`, `n`, `N`) — skips code blocks and math/ascii sections
- Open source file in editor (`e`) and reload (`r`)
- Theme support (`--theme oxocarbon|darkhorizon|dark|light|dracula|solarized-dark`)
- Configurable behavior via `config.toml` and colors via `colors.toml`
- Jekyll-like front matter header (`--- title: ... ---`) with sticky top title
- Markdown + code block rendering with syntax highlighting
- Math rendering through `calcifer` (`$...$`, `$$...$$`, ` ```math ` blocks)
- Copy current code block to clipboard only (`y` or click on code line)
- Open links (`o`) or mouse click on link lines, with URL confirm popup
- Centering only for tables and block quotes (math blocks are left-aligned)
- Reactive status bar and `?` keybindings popup
- Search highlights matched words; current active match uses a distinct color
- Optional current-line highlight via config (`line_highlight = false` by default)
- Clipboard copy uses `wl-copy`
- Shell completion generation (`--completion bash|zsh|fish`)
- Glow-like interactive dashboard when launched with no file argument
- Markdown formatting support: headings, bold, italic, strikethrough, links, block quotes, lists, code fences, tables

## Build

```bash
cargo build --release
```

## Usage

```bash
# Pager mode (default)
./target/release/calci README.md

# Read from stdin
cat README.md | ./target/release/calci

# Non-pager mode (render to stdout)
./target/release/calci --no-pager README.md

# Theme and line numbers
./target/release/calci --theme oxocarbon --line-numbers README.md

# Custom config/colors path
./target/release/calci --config ./config.toml --colors ./colors.toml README.md

# Generate shell completion
./target/release/calci --completion zsh > _calci
```

### Config files

Defaults:

- `~/.config/calci/config.toml`
- `~/.config/calci/colors.toml`
- dashboard open history: `~/.config/calci/dashboard_state.toml`

Project-local examples are included:

- `config.toml`
- `colors.toml`
- override-ready files:
  - `overrides/config/default.toml`
  - `overrides/colorschemes/oxocarbon.toml`
  - `overrides/colorschemes/darkhorizon.toml`
  - `overrides/colorschemes/oxocarbon-base10.toml`
  - `overrides/colorschemes/darkhorizon-base10.toml`

`config.toml` supports:
- `dashboard_sort = "last_open"` (default) or `"last_edited"`
- `dashboard_fuzzy_mode = "loose"` (default) or `"strict"`
- `dashboard_show_edited_age = false` (default)
- `mouse = true` enables dashboard click + wheel gestures (up/down: files, left/right: pages)

You can also use nested dashboard keys:
- `[dashboard] sort = "last_open" | "last_edited"`
- `[dashboard] fuzzy_mode = "loose" | "strict"`
- `[dashboard] show_edited_age = true | false`

## Keybindings

- `q`: quit
- `j/k`, `↓/↑`: move
- `PgDn/PgUp`, `Space`: page scroll
- `/`: search mode
- `n` / `N`: next / previous match
- `y`: copy selected code block
- `o`: open link on selected line (shows confirm popup)
- mouse click on line with link: open link
- mouse click on code line: copy code block
- `e`: open editor (`$EDITOR`, default `vi`)
- `r`: reload content
- `?`: keybindings popup

### Dashboard search modes

- `loose` (default): in-order fuzzy match with gaps allowed.
  Example: `drm` matches `docs/readme.md`.
- `strict`: contiguous substring only.
  Example: `read` matches `docs/readme.md`, but `drm` does not.

### colors.toml format

`calci` uses a strict nested palette format:

- `[base16]` with lowercase keys `base00..base0f`
- `[base10]` with `black, grey, white, green, cyan, blue, purple, pink, red, yellow`
- Nested sections:
  - `[pager]` with `text`, `heading`, `quote`, `list_marker`, `link`, `status_fg`, `status_bg`, `cursor_line_bg`, `line_number_fg`
  - `[search]` with `hit_fg`, `hit_bg`, `current_fg`, `current_bg`
  - `[code]` with `inline`, `black`, `grey`, `white`, `purple`, `pink`, `blue`, `cyan`, `green`, `red`, `yellow`, `orange`

`[base16]` or `[base10]` auto-maps defaults for pager/search/code. `[pager]`, `[search]`, and `[code]` act as explicit overrides.

Fenced-code syntax highlighting is exact semantic scope mapping from `[code]` (no blending and no nearest-color bucketing).

The bundled default `colors.toml` is the refined Oxocarbon mapping (including stronger semantic scope separation for fenced code).

When a `colors.toml` is provided, calci derives the whole UI palette from it (pager, dashboard, popups, status, headings, search, and code), with only fallback defaults used when no colors file is provided.

To test a maintained scheme file directly:

```bash
./target/release/calci --colors overrides/colorschemes/oxocarbon.toml README.md
./target/release/calci --colors overrides/colorschemes/darkhorizon.toml README.md
./target/release/calci --colors overrides/colorschemes/oxocarbon-base10.toml README.md
./target/release/calci --colors overrides/colorschemes/darkhorizon-base10.toml README.md
```

## Notes

- Search ignores code lines by design.
- `array` follows LaTeX semantics (no implicit delimiters).
- For `e` to work, open from a file path (not stdin).
- Default theme is `oxocarbon`.
- Inline code renders highlighted text without literal backtick glyphs.
- Dashboard and status bar are always enabled.
- Theme selection is CLI-only (`--theme`), while `colors.toml` controls color overrides.

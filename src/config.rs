use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

use crate::theme::{AppTheme, ThemeName};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DashboardSort {
    #[default]
    LastOpen,
    LastEdited,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DashboardFuzzyMode {
    Strict,
    #[default]
    Loose,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AppConfigFile {
    pub pager: bool,
    pub math: bool,
    pub line_numbers: bool,
    pub line_highlight: bool,
    pub mouse: bool,
    pub wrap: bool,
    pub smooth_scroll: usize,
    pub center_blocks: bool,
    pub link_confirmation: bool,
    pub dashboard_sort: DashboardSort,
    pub dashboard_fuzzy_mode: DashboardFuzzyMode,
    pub dashboard_show_edited_age: bool,
}

impl Default for AppConfigFile {
    fn default() -> Self {
        Self {
            pager: true,
            math: true,
            line_numbers: false,
            line_highlight: false,
            mouse: true,
            wrap: true,
            smooth_scroll: 3,
            center_blocks: true,
            link_confirmation: false,
            dashboard_sort: DashboardSort::LastOpen,
            dashboard_fuzzy_mode: DashboardFuzzyMode::Loose,
            dashboard_show_edited_age: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct DashboardConfigSection {
    sort: Option<DashboardSort>,
    fuzzy_mode: Option<DashboardFuzzyMode>,
    show_edited_age: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct RawAppConfigFile {
    pager: Option<bool>,
    math: Option<bool>,
    line_numbers: Option<bool>,
    line_highlight: Option<bool>,
    mouse: Option<bool>,
    wrap: Option<bool>,
    smooth_scroll: Option<usize>,
    center_blocks: Option<bool>,
    link_confirmation: Option<bool>,
    dashboard_sort: Option<DashboardSort>,
    dashboard_fuzzy_mode: Option<DashboardFuzzyMode>,
    dashboard_show_edited_age: Option<bool>,
    dashboard: Option<DashboardConfigSection>,
}

impl RawAppConfigFile {
    fn into_app_config(self) -> AppConfigFile {
        let mut out = AppConfigFile::default();
        if let Some(v) = self.pager {
            out.pager = v;
        }
        if let Some(v) = self.math {
            out.math = v;
        }
        if let Some(v) = self.line_numbers {
            out.line_numbers = v;
        }
        if let Some(v) = self.line_highlight {
            out.line_highlight = v;
        }
        if let Some(v) = self.mouse {
            out.mouse = v;
        }
        if let Some(v) = self.wrap {
            out.wrap = v;
        }
        if let Some(v) = self.smooth_scroll {
            out.smooth_scroll = v;
        }
        if let Some(v) = self.center_blocks {
            out.center_blocks = v;
        }
        if let Some(v) = self.link_confirmation {
            out.link_confirmation = v;
        }
        if let Some(d) = self.dashboard {
            if let Some(v) = d.sort {
                out.dashboard_sort = v;
            }
            if let Some(v) = d.fuzzy_mode {
                out.dashboard_fuzzy_mode = v;
            }
            if let Some(v) = d.show_edited_age {
                out.dashboard_show_edited_age = v;
            }
        }
        if let Some(v) = self.dashboard_sort {
            out.dashboard_sort = v;
        }
        if let Some(v) = self.dashboard_fuzzy_mode {
            out.dashboard_fuzzy_mode = v;
        }
        if let Some(v) = self.dashboard_show_edited_age {
            out.dashboard_show_edited_age = v;
        }
        out
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ColorsFile {
    pub normal_fg: Option<String>,
    pub heading_fg: Option<String>,
    pub quote_fg: Option<String>,
    pub list_marker_fg: Option<String>,
    pub inline_code_fg: Option<String>,
    pub link_fg: Option<String>,
    pub status_fg: Option<String>,
    pub status_bg: Option<String>,
    pub search_hit_fg: Option<String>,
    pub search_hit_bg: Option<String>,
    pub search_current_fg: Option<String>,
    pub search_current_bg: Option<String>,
    pub cursor_line_bg: Option<String>,
    pub line_number_fg: Option<String>,
    pub code_black: Option<String>,
    pub code_grey: Option<String>,
    pub code_white: Option<String>,
    pub code_purple: Option<String>,
    pub code_pink: Option<String>,
    pub code_blue: Option<String>,
    pub code_cyan: Option<String>,
    pub code_green: Option<String>,
    pub code_red: Option<String>,
    pub code_yellow: Option<String>,
    pub code_orange: Option<String>,
}

impl Default for ColorsFile {
    fn default() -> Self {
        Self {
            normal_fg: None,
            heading_fg: None,
            quote_fg: None,
            list_marker_fg: None,
            inline_code_fg: None,
            link_fg: None,
            status_fg: None,
            status_bg: None,
            search_hit_fg: None,
            search_hit_bg: None,
            search_current_fg: None,
            search_current_bg: None,
            cursor_line_bg: None,
            line_number_fg: None,
            code_black: None,
            code_grey: None,
            code_white: None,
            code_purple: None,
            code_pink: None,
            code_blue: None,
            code_cyan: None,
            code_green: None,
            code_red: None,
            code_yellow: None,
            code_orange: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct PagerColorsSection {
    text: Option<String>,
    heading: Option<String>,
    quote: Option<String>,
    list_marker: Option<String>,
    link: Option<String>,
    status_fg: Option<String>,
    status_bg: Option<String>,
    cursor_line_bg: Option<String>,
    line_number_fg: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct SearchColorsSection {
    hit_fg: Option<String>,
    hit_bg: Option<String>,
    current_fg: Option<String>,
    current_bg: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct CodeColorsSection {
    inline: Option<String>,
    black: Option<String>,
    grey: Option<String>,
    white: Option<String>,
    purple: Option<String>,
    pink: Option<String>,
    blue: Option<String>,
    cyan: Option<String>,
    green: Option<String>,
    red: Option<String>,
    yellow: Option<String>,
    orange: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Base16Section {
    base00: Option<String>,
    base01: Option<String>,
    base02: Option<String>,
    base03: Option<String>,
    base04: Option<String>,
    base05: Option<String>,
    base06: Option<String>,
    base07: Option<String>,
    base08: Option<String>,
    base09: Option<String>,
    base0a: Option<String>,
    base0b: Option<String>,
    base0c: Option<String>,
    base0d: Option<String>,
    base0e: Option<String>,
    base0f: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Base10Section {
    black: Option<String>,
    grey: Option<String>,
    white: Option<String>,
    green: Option<String>,
    cyan: Option<String>,
    blue: Option<String>,
    purple: Option<String>,
    pink: Option<String>,
    red: Option<String>,
    yellow: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawColorsFile {
    base10: Option<Base10Section>,
    base16: Option<Base16Section>,
    pager: Option<PagerColorsSection>,
    search: Option<SearchColorsSection>,
    code: Option<CodeColorsSection>,
}

impl RawColorsFile {
    fn into_colors(self) -> ColorsFile {
        let mut out = ColorsFile::default();
        if let Some(b) = self.base10 {
            out.normal_fg = b.white.clone();
            out.heading_fg = b.purple.clone();
            out.quote_fg = b.grey.clone();
            out.list_marker_fg = b.blue.clone();
            out.inline_code_fg = b.cyan.clone();
            out.link_fg = b.cyan.clone();
            out.status_fg = b.white.clone();
            out.status_bg = b.black.clone();
            out.search_hit_fg = b.black.clone();
            out.search_hit_bg = b.blue.clone();
            out.search_current_fg = b.black.clone();
            out.search_current_bg = b.green.clone();
            out.cursor_line_bg = b.black.clone();
            out.line_number_fg = b.grey.clone();
            out.code_black = b.black.clone();
            out.code_grey = b.grey.clone();
            out.code_white = b.white.clone();
            out.code_purple = b.purple.clone();
            out.code_pink = b.pink.clone();
            out.code_blue = b.blue.clone();
            out.code_cyan = b.cyan.clone();
            out.code_green = b.green.clone();
            out.code_red = b.red.clone();
            out.code_yellow = b.yellow.clone();
            out.code_orange = b.blue.clone();
        }
        if let Some(b) = self.base16 {
            out.normal_fg = b.base05.clone();
            out.heading_fg = b.base0e.clone();
            out.quote_fg = b.base03.clone();
            out.list_marker_fg = b.base09.clone();
            out.inline_code_fg = b.base0b.clone();
            out.link_fg = b.base0c.clone();
            out.status_fg = b.base05.clone();
            out.status_bg = b.base01.clone();
            out.search_hit_fg = b.base00.clone();
            out.search_hit_bg = b.base0d.clone();
            out.search_current_fg = b.base00.clone();
            out.search_current_bg = b.base0b.clone();
            out.cursor_line_bg = b.base02.clone();
            out.line_number_fg = b.base03.clone();
            out.code_black = b.base00.clone();
            out.code_grey = b.base03.clone();
            out.code_white = b.base05.clone();
            out.code_purple = b.base0e.clone();
            out.code_pink = b.base0a.clone();
            out.code_blue = b.base09.clone();
            out.code_cyan = b.base0c.clone();
            out.code_green = b.base0d.clone();
            out.code_red = b.base08.clone();
            out.code_yellow = b.base06.clone();
            out.code_orange = b.base0f.clone();
        }
        if let Some(p) = self.pager {
            if let Some(v) = p.text {
                out.normal_fg = Some(v);
            }
            if let Some(v) = p.heading {
                out.heading_fg = Some(v);
            }
            if let Some(v) = p.quote {
                out.quote_fg = Some(v);
            }
            if let Some(v) = p.list_marker {
                out.list_marker_fg = Some(v);
            }
            if let Some(v) = p.link {
                out.link_fg = Some(v);
            }
            if let Some(v) = p.status_fg {
                out.status_fg = Some(v);
            }
            if let Some(v) = p.status_bg {
                out.status_bg = Some(v);
            }
            if let Some(v) = p.cursor_line_bg {
                out.cursor_line_bg = Some(v);
            }
            if let Some(v) = p.line_number_fg {
                out.line_number_fg = Some(v);
            }
        }
        if let Some(s) = self.search {
            if let Some(v) = s.hit_fg {
                out.search_hit_fg = Some(v);
            }
            if let Some(v) = s.hit_bg {
                out.search_hit_bg = Some(v);
            }
            if let Some(v) = s.current_fg {
                out.search_current_fg = Some(v);
            }
            if let Some(v) = s.current_bg {
                out.search_current_bg = Some(v);
            }
        }
        if let Some(c) = self.code {
            if let Some(v) = c.inline {
                out.inline_code_fg = Some(v);
            }
            if let Some(v) = c.black {
                out.code_black = Some(v);
            }
            if let Some(v) = c.grey {
                out.code_grey = Some(v);
            }
            if let Some(v) = c.white {
                out.code_white = Some(v);
            }
            if let Some(v) = c.purple {
                out.code_purple = Some(v);
            }
            if let Some(v) = c.pink {
                out.code_pink = Some(v);
            }
            if let Some(v) = c.blue {
                out.code_blue = Some(v);
            }
            if let Some(v) = c.cyan {
                out.code_cyan = Some(v);
            }
            if let Some(v) = c.green {
                out.code_green = Some(v);
            }
            if let Some(v) = c.red {
                out.code_red = Some(v);
            }
            if let Some(v) = c.yellow {
                out.code_yellow = Some(v);
            }
            if let Some(v) = c.orange {
                out.code_orange = Some(v);
            }
        }
        out
    }
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub app: AppConfigFile,
    pub colors: ColorsFile,
    pub source_dir: PathBuf,
}

impl LoadedConfig {
    pub fn load(config_path: Option<&Path>, colors_path: Option<&Path>) -> anyhow::Result<Self> {
        let cfg_path = config_path
            .map(PathBuf::from)
            .unwrap_or_else(default_config_path);
        let clr_path = colors_path
            .map(PathBuf::from)
            .unwrap_or_else(default_colors_path);

        let app = if cfg_path.exists() {
            let content = fs::read_to_string(&cfg_path)
                .with_context(|| format!("read '{}'", cfg_path.display()))?;
            toml::from_str::<RawAppConfigFile>(&content)
                .with_context(|| format!("parse '{}'", cfg_path.display()))?
                .into_app_config()
        } else {
            AppConfigFile::default()
        };

        let colors = if clr_path.exists() {
            let content = fs::read_to_string(&clr_path)
                .with_context(|| format!("read '{}'", clr_path.display()))?;
            toml::from_str::<RawColorsFile>(&content)
                .with_context(|| format!("parse '{}'", clr_path.display()))?
                .into_colors()
        } else {
            ColorsFile::default()
        };

        let source_dir = cfg_path
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        std::fs::create_dir_all(&source_dir).ok();

        Ok(Self {
            app,
            colors,
            source_dir,
        })
    }

    pub fn build_theme(&self, cli_theme: Option<&str>) -> AppTheme {
        let name = cli_theme
            .map(ThemeName::parse)
            .unwrap_or(ThemeName::Oxocarbon);
        let mut theme = AppTheme::from_name(name);
        theme.apply_overrides(&self.colors);
        theme
    }
}

fn default_config_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("calci").join("config.toml");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("calci")
            .join("config.toml");
    }
    PathBuf::from("config.toml")
}

fn default_colors_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("calci").join("colors.toml");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".config")
            .join("calci")
            .join("colors.toml");
    }
    PathBuf::from("colors.toml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn defaults_load_when_files_missing() {
        let d = tempdir().expect("tempdir");
        let cfg = d.path().join("config.toml");
        let clr = d.path().join("colors.toml");
        let loaded = LoadedConfig::load(Some(&cfg), Some(&clr)).expect("load");
        assert!(loaded.app.pager);
        assert!(loaded.app.math);
        assert!(loaded.app.mouse);
        assert!(!loaded.app.line_highlight);
        assert!(loaded.app.center_blocks);
        assert!(!loaded.app.link_confirmation);
        assert_eq!(loaded.app.dashboard_sort, DashboardSort::LastOpen);
        assert_eq!(loaded.app.dashboard_fuzzy_mode, DashboardFuzzyMode::Loose);
        assert!(!loaded.app.dashboard_show_edited_age);
    }

    #[test]
    fn parse_config_values() {
        let d = tempdir().expect("tempdir");
        let cfg = d.path().join("config.toml");
        let clr = d.path().join("colors.toml");
        fs::write(
            &cfg,
            r#"
pager = false
math = false
line_numbers = true
line_highlight = true
center_blocks = false
link_confirmation = false
dashboard_sort = "last_edited"
dashboard_fuzzy_mode = "strict"
dashboard_show_edited_age = true
"#,
        )
        .expect("write cfg");
        fs::write(
            &clr,
            r##"
[pager]
heading = "#FF00FF"
"##,
        )
        .expect("write clr");
        let loaded = LoadedConfig::load(Some(&cfg), Some(&clr)).expect("load");
        assert!(!loaded.app.pager);
        assert!(!loaded.app.math);
        assert!(loaded.app.line_numbers);
        assert!(loaded.app.line_highlight);
        assert!(!loaded.app.center_blocks);
        assert!(!loaded.app.link_confirmation);
        assert_eq!(loaded.app.dashboard_sort, DashboardSort::LastEdited);
        assert_eq!(loaded.app.dashboard_fuzzy_mode, DashboardFuzzyMode::Strict);
        assert!(loaded.app.dashboard_show_edited_age);
        assert_eq!(loaded.colors.heading_fg.as_deref(), Some("#FF00FF"));
    }

    #[test]
    fn parse_nested_dashboard_config_values() {
        let d = tempdir().expect("tempdir");
        let cfg = d.path().join("config.toml");
        let clr = d.path().join("colors.toml");
        fs::write(
            &cfg,
            r#"
pager = false
mouse = true
[dashboard]
sort = "last_edited"
fuzzy_mode = "strict"
show_edited_age = true
"#,
        )
        .expect("write cfg");
        fs::write(&clr, "").expect("write clr");
        let loaded = LoadedConfig::load(Some(&cfg), Some(&clr)).expect("load");
        assert!(!loaded.app.pager);
        assert!(loaded.app.mouse);
        assert_eq!(loaded.app.dashboard_sort, DashboardSort::LastEdited);
        assert_eq!(loaded.app.dashboard_fuzzy_mode, DashboardFuzzyMode::Strict);
        assert!(loaded.app.dashboard_show_edited_age);
    }

    #[test]
    fn parse_nested_colors_sections_values() {
        let d = tempdir().expect("tempdir");
        let cfg = d.path().join("config.toml");
        let clr = d.path().join("colors.toml");
        fs::write(&cfg, "").expect("write cfg");
        fs::write(
            &clr,
            r##"
[pager]
text = "#111111"
heading = "#222222"
quote = "#333333"
list_marker = "#444444"
link = "#555555"
status_fg = "#666666"
status_bg = "#777777"
cursor_line_bg = "#888888"
line_number_fg = "#999999"

[search]
hit_fg = "#aaaaaa"
hit_bg = "#bbbbbb"
current_fg = "#cccccc"
current_bg = "#dddddd"

[code]
inline = "#eeeeee"
black = "#000001"
grey = "#000002"
white = "#000003"
purple = "#000004"
pink = "#000005"
blue = "#000006"
cyan = "#000007"
green = "#000008"
red = "#000009"
yellow = "#00000a"
orange = "#00000b"
"##,
        )
        .expect("write colors");
        let loaded = LoadedConfig::load(Some(&cfg), Some(&clr)).expect("load");
        assert_eq!(loaded.colors.normal_fg.as_deref(), Some("#111111"));
        assert_eq!(loaded.colors.heading_fg.as_deref(), Some("#222222"));
        assert_eq!(loaded.colors.quote_fg.as_deref(), Some("#333333"));
        assert_eq!(loaded.colors.list_marker_fg.as_deref(), Some("#444444"));
        assert_eq!(loaded.colors.link_fg.as_deref(), Some("#555555"));
        assert_eq!(loaded.colors.status_fg.as_deref(), Some("#666666"));
        assert_eq!(loaded.colors.status_bg.as_deref(), Some("#777777"));
        assert_eq!(loaded.colors.cursor_line_bg.as_deref(), Some("#888888"));
        assert_eq!(loaded.colors.line_number_fg.as_deref(), Some("#999999"));
        assert_eq!(loaded.colors.search_hit_fg.as_deref(), Some("#aaaaaa"));
        assert_eq!(loaded.colors.search_hit_bg.as_deref(), Some("#bbbbbb"));
        assert_eq!(loaded.colors.search_current_fg.as_deref(), Some("#cccccc"));
        assert_eq!(loaded.colors.search_current_bg.as_deref(), Some("#dddddd"));
        assert_eq!(loaded.colors.inline_code_fg.as_deref(), Some("#eeeeee"));
        assert_eq!(loaded.colors.code_black.as_deref(), Some("#000001"));
        assert_eq!(loaded.colors.code_grey.as_deref(), Some("#000002"));
        assert_eq!(loaded.colors.code_white.as_deref(), Some("#000003"));
        assert_eq!(loaded.colors.code_purple.as_deref(), Some("#000004"));
        assert_eq!(loaded.colors.code_pink.as_deref(), Some("#000005"));
        assert_eq!(loaded.colors.code_blue.as_deref(), Some("#000006"));
        assert_eq!(loaded.colors.code_cyan.as_deref(), Some("#000007"));
        assert_eq!(loaded.colors.code_green.as_deref(), Some("#000008"));
        assert_eq!(loaded.colors.code_red.as_deref(), Some("#000009"));
        assert_eq!(loaded.colors.code_yellow.as_deref(), Some("#00000a"));
        assert_eq!(loaded.colors.code_orange.as_deref(), Some("#00000b"));
    }

    #[test]
    fn parse_base16_section_auto_maps_defaults() {
        let d = tempdir().expect("tempdir");
        let cfg = d.path().join("config.toml");
        let clr = d.path().join("colors.toml");
        fs::write(&cfg, "").expect("write cfg");
        fs::write(
            &clr,
            r##"
[base16]
base00 = "#000000"
base03 = "#030303"
base05 = "#050505"
base08 = "#080808"
base0b = "#0b0b0b"
base0d = "#0d0d0d"
base0e = "#0e0e0e"

[pager]
heading = "#222222" # explicit section overrides base16
"##,
        )
        .expect("write colors");
        let loaded = LoadedConfig::load(Some(&cfg), Some(&clr)).expect("load");
        assert_eq!(loaded.colors.normal_fg.as_deref(), Some("#050505"));
        assert_eq!(loaded.colors.search_hit_fg.as_deref(), Some("#000000"));
        assert_eq!(loaded.colors.search_current_bg.as_deref(), Some("#0b0b0b"));
        assert_eq!(loaded.colors.search_hit_bg.as_deref(), Some("#0d0d0d"));
        assert_eq!(loaded.colors.code_red.as_deref(), Some("#080808"));
        assert_eq!(loaded.colors.line_number_fg.as_deref(), Some("#030303"));
        assert_eq!(loaded.colors.heading_fg.as_deref(), Some("#222222"));
    }

    #[test]
    fn parse_base10_section_auto_maps_defaults() {
        let d = tempdir().expect("tempdir");
        let cfg = d.path().join("config.toml");
        let clr = d.path().join("colors.toml");
        fs::write(&cfg, "").expect("write cfg");
        fs::write(
            &clr,
            r##"
[base10]
black = "#101010"
grey = "#202020"
white = "#f0f0f0"
green = "#00aa00"
cyan = "#00bbbb"
blue = "#0000cc"
purple = "#7700dd"
pink = "#cc33aa"
red = "#dd2244"
yellow = "#eebb66"
"##,
        )
        .expect("write colors");
        let loaded = LoadedConfig::load(Some(&cfg), Some(&clr)).expect("load");
        assert_eq!(loaded.colors.normal_fg.as_deref(), Some("#f0f0f0"));
        assert_eq!(loaded.colors.heading_fg.as_deref(), Some("#7700dd"));
        assert_eq!(loaded.colors.quote_fg.as_deref(), Some("#202020"));
        assert_eq!(loaded.colors.list_marker_fg.as_deref(), Some("#0000cc"));
        assert_eq!(loaded.colors.inline_code_fg.as_deref(), Some("#00bbbb"));
        assert_eq!(loaded.colors.link_fg.as_deref(), Some("#00bbbb"));
        assert_eq!(loaded.colors.search_hit_fg.as_deref(), Some("#101010"));
        assert_eq!(loaded.colors.search_hit_bg.as_deref(), Some("#0000cc"));
        assert_eq!(loaded.colors.search_current_bg.as_deref(), Some("#00aa00"));
        assert_eq!(loaded.colors.code_orange.as_deref(), Some("#0000cc"));
        assert_eq!(loaded.colors.code_red.as_deref(), Some("#dd2244"));
    }

    #[test]
    fn flat_color_keys_are_rejected() {
        let d = tempdir().expect("tempdir");
        let cfg = d.path().join("config.toml");
        let clr = d.path().join("colors.toml");
        fs::write(&cfg, "").expect("write cfg");
        fs::write(&clr, "heading_fg = \"#FF00FF\"").expect("write colors");
        let err = LoadedConfig::load(Some(&cfg), Some(&clr)).expect_err("must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("unknown field"));
        assert!(msg.contains("heading_fg"));
    }
}

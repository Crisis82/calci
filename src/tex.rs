//! LaTeX math rendered for kitty's Unicode placeholders, with RaTeX (KaTeX
//! fonts, `math_font = "katex"`, the default) or Typst + mitex with an
//! OpenType math font (any other `math_font`, `src/tex_typst.rs`).
//!
//! [`MathImages`] is the per-document store: the renderer asks it for a
//! formula and gets back an image id and a size in cells, then emits the
//! placeholder text; the pager / plain output transmit the PNGs to the
//! terminal. Nothing here panics outward: RaTeX bounds parse depth, macro
//! expansion and layout work, and any panic that slips through becomes an
//! error (the caller then shows the LaTeX source).

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use ratex_layout::{LayoutOptions, layout, to_display_list, validate_layout};
use ratex_render::{RenderOptions, render_to_png};
use ratex_types::color::Color;
use ratex_types::display_item::DisplayList;
use ratex_types::math_style::MathStyle;

use crate::config::MathFont;

/// TeX em relative to the cell height: about the size of terminal text
/// (a monospace font's em is ~0.86 of its line height).
const EM_PER_CELL_HEIGHT: f32 = 0.85;

/// Where terminal text sits on the cell: its baseline, as a fraction of the
/// cell height from the top (ascent / line height of common monospace
/// fonts). Inline math is drawn on this baseline.
const BASELINE_PER_CELL_HEIGHT: f32 = 0.78;

/// Largest image a placeholder can address, in cells per side.
const MAX_CELLS: usize = crate::placeholder::MAX_CELLS;

/// A formula rendered to a PNG that covers exactly `cols` x `rows` cells.
#[derive(Clone)]
pub struct CellImage {
    pub png: Vec<u8>,
    pub cols: usize,
    pub rows: usize,
}

/// A rendered formula and whether the terminal has it yet.
pub struct MathImage {
    pub id: u32,
    pub image: CellImage,
    pub uploaded: bool,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    latex: String,
    display: bool,
    color: (u8, u8, u8),
    max_cols: usize,
}

/// Math images of one document, rendered once per formula / colour / width
/// for a terminal whose cells are `cell` = (width, height) pixels.
pub struct MathImages {
    cell: (u32, u32),
    font: MathFont,
    images: HashMap<u32, MathImage>,
    /// `None` caches a formula that failed to render.
    by_key: HashMap<Key, Option<u32>>,
    used_ids: HashSet<u32>,
}

impl std::fmt::Debug for MathImages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MathImages")
            .field("cell", &self.cell)
            .field("images", &self.images.len())
            .finish()
    }
}

impl MathImages {
    /// A store for the terminal on stdout, or `None` when its cell size in
    /// pixels is unknown (not a terminal, or one that doesn't report it).
    pub fn for_terminal(font: MathFont) -> Option<Self> {
        let ws = crossterm::terminal::window_size().ok()?;
        if ws.columns == 0 || ws.rows == 0 || ws.width == 0 || ws.height == 0 {
            return None;
        }
        Some(Self::new(
            (
                u32::from(ws.width / ws.columns),
                u32::from(ws.height / ws.rows),
            ),
            font,
        ))
    }

    pub fn new(cell: (u32, u32), font: MathFont) -> Self {
        Self {
            cell: (cell.0.max(1), cell.1.max(1)),
            font,
            images: HashMap::new(),
            by_key: HashMap::new(),
            used_ids: HashSet::new(),
        }
    }

    /// Display math: image id and (cols, rows), at most `max_cols` wide.
    pub fn display(
        &mut self,
        latex: &str,
        color: (u8, u8, u8),
        max_cols: usize,
    ) -> Option<(u32, usize, usize)> {
        let key = Key {
            latex: latex.to_string(),
            display: true,
            color,
            max_cols: max_cols.clamp(1, MAX_CELLS),
        };
        let (cell, font) = (self.cell, self.font);
        let id = self.get_or_render(key, |key| {
            with_katex_backup(font, |font| {
                render_display(&key.latex, key.color, cell, key.max_cols, font)
            })
        })?;
        let image = &self.images[&id].image;
        Some((id, image.cols, image.rows))
    }

    /// Inline math, one row tall on the text baseline: image id and cols.
    pub fn inline(
        &mut self,
        latex: &str,
        color: (u8, u8, u8),
        max_cols: usize,
    ) -> Option<(u32, usize)> {
        let key = Key {
            latex: latex.to_string(),
            display: false,
            color,
            max_cols: max_cols.clamp(1, MAX_CELLS),
        };
        let (cell, font) = (self.cell, self.font);
        let id = self.get_or_render(key, |key| {
            with_katex_backup(font, |font| {
                render_inline(&key.latex, key.color, cell, key.max_cols, font)
            })
        })?;
        Some((id, self.images[&id].image.cols))
    }

    fn get_or_render(
        &mut self,
        key: Key,
        render: impl FnOnce(&Key) -> Result<CellImage, String>,
    ) -> Option<u32> {
        if let Some(cached) = self.by_key.get(&key) {
            return *cached;
        }
        let id = render(&key).ok().map(|image| {
            let id = self.allocate_id(&image.png);
            self.images.insert(
                id,
                MathImage {
                    id,
                    image,
                    uploaded: false,
                },
            );
            id
        });
        self.by_key.insert(key, id);
        id
    }

    /// Ids are derived from the image content: output printed in plain mode
    /// stays in the terminal's scrollback, and a later run must not reuse an
    /// id for a different image (that would change the older output). The
    /// same image gets the same id, which is harmless.
    fn allocate_id(&mut self, png: &[u8]) -> u32 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        png.hash(&mut hasher);
        let mut id = (hasher.finish() as u32) & 0x00ff_ffff;
        while id == 0 || !self.used_ids.insert(id) {
            id = (id + 1) & 0x00ff_ffff;
        }
        id
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut MathImage> {
        self.images.get_mut(&id)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut MathImage> {
        self.images.values_mut()
    }
}

/// Render with `font`; if that is a Typst font and Typst fails (a LaTeX
/// construct mitex can't convert, too deep nesting...), render with RaTeX
/// and the KaTeX fonts instead. The caller shows the LaTeX source only when
/// both fail.
fn with_katex_backup(
    font: MathFont,
    render: impl Fn(MathFont) -> Result<CellImage, String>,
) -> Result<CellImage, String> {
    match render(font) {
        Err(_) if font != MathFont::Katex => render(MathFont::Katex),
        result => result,
    }
}

/// A typeset formula: its image (transparent background, no padding) and
/// the distance from the image top to the baseline, in pixels.
struct Typeset {
    image: image::RgbaImage,
    ascent: f32,
}

/// Typeset `latex` at `font_px` pixels per em: RaTeX for the KaTeX fonts,
/// Typst for the OpenType math fonts.
fn typeset(
    latex: &str,
    display: bool,
    color: (u8, u8, u8),
    font_px: f32,
    font: MathFont,
) -> Result<Typeset, String> {
    if font == MathFont::Katex {
        let list = layout_formula(latex, display, color)?;
        let image = rasterize(&list, font_px)?;
        return Ok(Typeset {
            image,
            ascent: list.height as f32 * font_px,
        });
    }
    let typeset = crate::tex_typst::typeset(latex, display, color, font_px, font)?;
    Ok(Typeset {
        image: typeset.image,
        ascent: typeset.ascent,
    })
}

/// Parse and lay out `latex` (sizes in em, independent of pixels).
fn layout_formula(latex: &str, display: bool, color: (u8, u8, u8)) -> Result<DisplayList, String> {
    let latex = latex.to_owned();
    std::panic::catch_unwind(move || {
        let ast = ratex_parser::parser::parse(&latex).map_err(|err| err.to_string())?;
        let (r, g, b) = color;
        let color = Color::rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        let style = if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let options = LayoutOptions::default().with_style(style).with_color(color);
        let layout_box = layout(&ast, &options);
        validate_layout(&layout_box)?;
        Ok(to_display_list(&layout_box))
    })
    .unwrap_or_else(|_| Err("math layout panicked".to_string()))
}

/// Rasterize at `font_px` pixels per em, transparent background, no padding.
fn rasterize(list: &DisplayList, font_px: f32) -> Result<image::RgbaImage, String> {
    let options = RenderOptions {
        font_size: font_px,
        padding: 0.0,
        background_color: Color::new(0.0, 0.0, 0.0, 0.0),
        font_dir: String::new(),
        device_pixel_ratio: 1.0,
    };
    let png = std::panic::catch_unwind(|| render_to_png(list, &options))
        .unwrap_or_else(|_| Err("math renderer panicked".to_string()))?;
    image::load_from_memory(&png)
        .map(|decoded| decoded.to_rgba8())
        .map_err(|err| format!("failed decoding math image: {err}"))
}

/// Draw `formula` onto a canvas of whole cells at (x, y) and encode it.
fn on_cells(
    formula: &image::RgbaImage,
    (cell_w, cell_h): (u32, u32),
    cols: usize,
    rows: usize,
    x: u32,
    y: u32,
) -> Result<CellImage, String> {
    let mut canvas = image::RgbaImage::new(cols as u32 * cell_w, rows as u32 * cell_h);
    image::imageops::overlay(&mut canvas, formula, x as i64, y as i64);
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(canvas)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|err| format!("failed encoding math image: {err}"))?;
    Ok(CellImage { png, cols, rows })
}

fn em_px(cell_h: u32) -> f32 {
    cell_h as f32 * EM_PER_CELL_HEIGHT
}

/// Display math, left-aligned and vertically centred on whole cells; drawn
/// smaller when wider than `max_cols` (or taller than a placeholder can
/// address).
pub fn render_display(
    latex: &str,
    color: (u8, u8, u8),
    cell: (u32, u32),
    max_cols: usize,
    font: MathFont,
) -> Result<CellImage, String> {
    let (cell_w, cell_h) = cell;
    let em = em_px(cell_h);
    let mut formula = typeset(latex, true, color, em, font)?;
    let scale = 1f32
        .min(max_cols as f32 * cell_w as f32 / (formula.image.width() as f32).max(1.0))
        .min(MAX_CELLS as f32 * cell_h as f32 / (formula.image.height() as f32).max(1.0));
    if scale < 1.0 {
        formula = typeset(latex, true, color, em * scale, font)?;
    }
    let image = &formula.image;
    let cols = image
        .width()
        .div_ceil(cell_w)
        .clamp(1, max_cols.max(1) as u32) as usize;
    let rows = image.height().div_ceil(cell_h).clamp(1, MAX_CELLS as u32) as usize;
    let y = (rows as u32 * cell_h).saturating_sub(image.height()) / 2;
    on_cells(image, cell, cols, rows, 0, y)
}

/// Inline math in one row, left-aligned. A formula that fits sits on the
/// text baseline at full size. One that would have to shrink to stay on the
/// baseline (fractions, big operators) is instead scaled to fill the whole
/// row and centred vertically: about 1.3-1.4x larger, close to how TeX
/// centres fractions on the math axis, and the line height never changes.
/// (`\displaystyle` gains nothing here: its taller layout is squeezed into
/// the same row.)
pub fn render_inline(
    latex: &str,
    color: (u8, u8, u8),
    cell: (u32, u32),
    max_cols: usize,
    font: MathFont,
) -> Result<CellImage, String> {
    let (cell_w, cell_h) = cell;
    let row_h = cell_h as f32;
    let em = em_px(cell_h);
    let baseline = row_h * BASELINE_PER_CELL_HEIGHT;
    let mut formula = typeset(latex, false, color, em, font)?;
    let height = formula.ascent;
    let depth = formula.image.height() as f32 - formula.ascent;
    let fit_width = max_cols as f32 * cell_w as f32 / (formula.image.width() as f32).max(1.0);

    let on_baseline = (baseline / height.max(0.01)).min((row_h - baseline) / depth.max(0.01));
    let (scale, centred) = if on_baseline >= 1.0 {
        (1f32.min(fit_width), false)
    } else {
        let fill_row = row_h / (height + depth).max(0.01);
        (1f32.min(fill_row).min(fit_width), true)
    };
    if scale < 1.0 {
        formula = typeset(latex, false, color, em * scale, font)?;
    }

    let image = &formula.image;
    let cols = image
        .width()
        .div_ceil(cell_w)
        .clamp(1, max_cols.max(1) as u32) as usize;
    let free = cell_h.saturating_sub(image.height());
    let y = if centred {
        free / 2
    } else {
        ((baseline - formula.ascent).round().max(0.0) as u32).min(free)
    };
    // Left-aligned, so punctuation right after the formula sits close to it.
    on_cells(image, cell, cols, 1, 0, y)
}

/// Height in pixels of the formula drawn inside an inline image (rows of
/// non-transparent pixels), for tests.
#[cfg(test)]
fn ink_height(image: &CellImage) -> u32 {
    let decoded = image::load_from_memory(&image.png).unwrap().to_rgba8();
    let rows: Vec<u32> = (0..decoded.height())
        .filter(|&y| (0..decoded.width()).any(|x| decoded.get_pixel(x, y)[3] > 0))
        .collect();
    rows.last().map_or(0, |last| last - rows[0] + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const WHITE: (u8, u8, u8) = (255, 255, 255);
    const CELL: (u32, u32) = (10, 20);

    const SAMPLES: &[&str] = &[
        r"x^2 + y^2 = z^2",
        r"\frac{-b \pm \sqrt{b^2 - 4ac}}{2a}",
        r"\sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}",
        r"\int_0^1 x\,dx = \tfrac12",
        r"\lim_{x \to 0} \frac{\sin x}{x} = 1",
        r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
        r"f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}",
        r"\left( \frac{a}{b} \right)^{n}",
        r"\mathbb{R}^n \to \mathcal{H},\ \nabla \cdot \vec{E} = \frac{\rho}{\varepsilon_0}",
        r"\begin{aligned} a &= b + c \\ &= d \end{aligned}",
    ];

    const MALFORMED: &[&str] = &[
        r"\left( a } \right)",
        r"\frac{a}",
        r"{{{",
        r"}}}",
        r"\begin{matrix} a & b",
        r"\undefinedmacro{x}",
        r"x^^2",
        r"\def\a{\a}\a",
        r"\sqrt[",
        r"\left(",
        "",
    ];

    fn size(image: &CellImage) -> (u32, u32) {
        let decoded = image::load_from_memory(&image.png).unwrap();
        (decoded.width(), decoded.height())
    }

    /// Every test below runs once per renderer: `katex` (RaTeX) and
    /// `typst` (Typst with Lete Sans Math). Replace `<renderer>` in the
    /// commands with one of them.
    macro_rules! renderer_tests {
        ($name:ident, $font:expr) => {
            mod $name {
                use super::*;
                const FONT: MathFont = $font;

            #[test]
            fn renders_common_formulas_on_whole_cells() {
                for latex in SAMPLES {
                    let image = render_display(latex, WHITE, CELL, 80, FONT)
                        .unwrap_or_else(|e| panic!("{latex}: {e}"));
                    assert_eq!(
                        size(&image),
                        (image.cols as u32 * 10, image.rows as u32 * 20)
                    );
                    let image = render_inline(latex, WHITE, CELL, 80, FONT)
                        .unwrap_or_else(|e| panic!("{latex}: {e}"));
                    assert_eq!(size(&image), (image.cols as u32 * 10, 20));
                }
            }

            #[test]
            fn tall_inline_math_fills_the_row_instead_of_shrinking_onto_the_baseline() {
                let frac = render_inline(r"\frac{a}{b}", WHITE, CELL, 80, FONT).unwrap();
                let display = render_inline(r"\displaystyle\frac{a}{b}", WHITE, CELL, 80, FONT).unwrap();
                let dfrac = render_inline(r"\dfrac{a}{b}", WHITE, CELL, 80, FONT).unwrap();
                for image in [&frac, &display, &dfrac] {
                    assert!(
                        ink_height(image) >= 17,
                        "fills the 20px row: {}",
                        ink_height(image)
                    );
                }
                // Formulas that fit stay on the baseline at full size.
                let flat = render_inline("x^2", WHITE, CELL, 80, FONT).unwrap();
                assert!(ink_height(&flat) < 17);
            }

            #[test]
            fn wide_math_is_drawn_smaller_to_fit() {
                let wide = "a + ".repeat(60) + "b";
                let image = render_display(&wide, WHITE, CELL, 40, FONT).unwrap();
                assert!(image.cols <= 40);
                let image = render_inline(&wide, WHITE, CELL, 40, FONT).unwrap();
                assert!(image.cols <= 40);
            }

            #[test]
            fn malformed_input_errors_quickly() {
                for latex in MALFORMED {
                    let start = Instant::now();
                    let _ = render_display(latex, WHITE, CELL, 80, FONT);
                    let _ = render_inline(latex, WHITE, CELL, 80, FONT);
                    assert!(start.elapsed() < Duration::from_secs(1), "{latex:?}");
                }
                assert!(render_inline(r"\frac{a}", WHITE, CELL, 80, FONT).is_err());
            }

            #[test]
            fn deep_nesting_does_not_overflow() {
                for depth in [50, 500, 5_000] {
                    let braces = format!("{}x{}", "{".repeat(depth), "}".repeat(depth));
                    let fracs = format!("{}x{}", r"\frac{1}{".repeat(depth), "}".repeat(depth));
                    let sups = format!("{}x", "x^{".repeat(depth)) + &"}".repeat(depth);
                    for latex in [braces, fracs, sups] {
                        let start = Instant::now();
                        let _ = render_display(&latex, WHITE, CELL, 80, FONT);
                        assert!(start.elapsed() < Duration::from_secs(2));
                    }
                }
            }

            #[test]
            fn long_input_is_not_quadratic() {
                let latex = "a + b = c \\\\ ".repeat(2_000);
                let start = Instant::now();
                let _ = render_display(&latex, WHITE, CELL, 80, FONT);
                assert!(
                    start.elapsed() < Duration::from_secs(2),
                    "{:?}",
                    start.elapsed()
                );
            }

            #[test]
            fn store_renders_each_formula_once_with_distinct_ids() {
                let mut store = MathImages::new(CELL, FONT);
                let a = store.display("x^2", WHITE, 80).unwrap();
                assert_eq!(store.display("x^2", WHITE, 80), Some(a));
                let b = store.inline("x^2", WHITE, 80).unwrap();
                assert_ne!(a.0, b.0);
                assert_eq!(store.images.len(), 2);
                assert_eq!(store.inline(r"\frac{a}", WHITE, 80), None);
                assert!(store.get_mut(a.0).is_some_and(|m| !m.uploaded));
            }

            #[test]
            fn katex_is_the_backup_and_source_the_last_resort() {
                // mitex can't convert \mathscr: Typst fails, RaTeX renders it.
                let mut store = MathImages::new(CELL, FONT);
                assert!(store.display(r"\mathscr{L}", WHITE, 80).is_some());
                assert!(store.inline(r"\mathscr{L}", WHITE, 80).is_some());
                // Invalid for both renderers: no image, the source is shown.
                assert_eq!(store.display(r"\frac{a}", WHITE, 80), None);
            }

            /// `cargo test --release tex::tests::<renderer>::bench -- --ignored --nocapture`
            #[test]
            #[ignore]
            fn bench() {
                let start = Instant::now();
                render_display("x", WHITE, CELL, 80, FONT).unwrap();
                eprintln!("first formula (cold start): {:?}", start.elapsed());
                for (label, display) in [("display", true), ("inline", false)] {
                    let n = 200;
                    let start = Instant::now();
                    let mut failed = 0;
                    for i in 0..n {
                        let latex = format!("{} + {i}", SAMPLES[i % SAMPLES.len()]);
                        let result = if display {
                            render_display(&latex, WHITE, CELL, 80, FONT)
                        } else {
                            render_inline(&latex, WHITE, CELL, 80, FONT)
                        };
                        failed += usize::from(result.is_err());
                    }
                    eprintln!(
                        "{label}: {:?}/formula over {n} unique formulas ({failed} failed)",
                        start.elapsed() / n as u32
                    );
                }
            }

            #[test]
            #[ignore]
            fn memory() {
                let rss = || {
                    std::fs::read_to_string("/proc/self/status")
                        .unwrap()
                        .lines()
                        .find(|l| l.starts_with("VmRSS"))
                        .unwrap()
                        .to_string()
                };
                eprintln!("before: {}", rss());
                for round in 0..4 {
                    for i in 0..500 {
                        let latex = format!("{} + {}", SAMPLES[i % SAMPLES.len()], round * 500 + i);
                        let _ = render_display(&latex, WHITE, CELL, 80, FONT);
                    }
                    eprintln!("after {} formulas: {}", (round + 1) * 500, rss());
                }
            }

            /// `cargo test --release tex::tests::<renderer>::coverage -- --ignored --nocapture`
            #[test]
            #[ignore]
            fn coverage() {
                let corpus = [
                    r#"x^2 + y^2 = z^2"#,
                    r#"a_{ij} + b_i^2"#,
                    r#"\frac{a}{b} + \tfrac{1}{2} + \dfrac{x}{y}"#,
                    r#"\sqrt{x} + \sqrt[3]{x}"#,
                    r#"\binom{n}{k} + \tbinom{n}{k}"#,
                    r#"\sum_{i=1}^n i + \prod_{k} k"#,
                    r#"\int_0^1 f(x)\,dx + \iint_D g + \oint_C h"#,
                    r#"\lim_{x \to 0} \frac{\sin x}{x}"#,
                    r#"\alpha \beta \gamma \delta \epsilon \varepsilon \zeta \eta \theta \vartheta \iota \kappa \lambda \mu \nu \xi \pi \varpi \rho \varrho \sigma \varsigma \tau \upsilon \phi \varphi \chi \psi \omega"#,
                    r#"\Gamma \Delta \Theta \Lambda \Xi \Pi \Sigma \Upsilon \Phi \Psi \Omega"#,
                    r#"\mathbb{R} \mathcal{H} \mathfrak{g} \mathbf{v} \mathit{x} \mathrm{d} \mathsf{A} \mathtt{x} \mathscr{L}"#,
                    r#"\boldsymbol{\alpha} + \bm{x}"#,
                    r#"\vec{v} \hat{x} \bar{x} \tilde{x} \dot{x} \ddot{x} \breve{x} \check{x} \acute{x} \grave{x}"#,
                    r#"\widehat{xyz} \widetilde{abc} \overline{AB} \underline{x}"#,
                    r#"\overbrace{a+b}^{n} \underbrace{c+d}_{m}"#,
                    r#"\overrightarrow{AB} \overleftarrow{AB}"#,
                    r#"\xrightarrow{f} \xleftarrow[g]{h}"#,
                    r#"\left( \frac{a}{b} \right) \left[ x \right] \left\{ y \right\} \left| z \right| \left\langle u \right\rangle"#,
                    r#"\bigl( x \bigr) \Big[ y \Big] \bigg\{ z \bigg\} \Bigg| w \Bigg|"#,
                    r#"\begin{pmatrix} a & b \\ c & d \end{pmatrix}"#,
                    r#"\begin{bmatrix} 1 & 0 \\ 0 & 1 \end{bmatrix}"#,
                    r#"\begin{vmatrix} a & b \\ c & d \end{vmatrix}"#,
                    r#"\begin{Bmatrix} a \end{Bmatrix} \begin{Vmatrix} a \end{Vmatrix}"#,
                    r#"\begin{matrix} a & b \\ c & d \end{matrix}"#,
                    r#"\begin{smallmatrix} a & b \\ c & d \end{smallmatrix}"#,
                    r#"f(x) = \begin{cases} 1 & x > 0 \\ 0 & \text{otherwise} \end{cases}"#,
                    r#"\begin{aligned} a &= b + c \\ &= d \end{aligned}"#,
                    r#"\begin{array}{cc} a & b \\ c & d \end{array}"#,
                    r#"\begin{gathered} a = b \\ c = d \end{gathered}"#,
                    r#"\begin{split} a &= b \\ &= c \end{split}"#,
                    r#"\leq \geq \neq \approx \equiv \sim \simeq \cong \propto \ll \gg"#,
                    r#"\in \notin \subset \subseteq \supset \supseteq \cup \cap \setminus \emptyset \varnothing"#,
                    r#"\forall \exists \nexists \neg \land \lor \implies \iff \Rightarrow \Leftrightarrow"#,
                    r#"\to \mapsto \leftarrow \rightarrow \uparrow \downarrow \hookrightarrow \rightharpoonup"#,
                    r#"\infty \partial \nabla \hbar \ell \Re \Im \aleph \wp"#,
                    r#"\cdot \times \div \pm \mp \circ \bullet \oplus \otimes \odot \star \ast"#,
                    r#"\ldots \cdots \vdots \ddots"#,
                    r#"\sin \cos \tan \log \ln \exp \det \max \min \sup \inf \arg \gcd"#,
                    r#"\operatorname{rank}(A) + \operatorname*{argmax}_x f"#,
                    r#"\text{if } x > 0 \text{ then } y"#,
                    r#"\quad \qquad \, \; \: \! \enspace"#,
                    r#"\not= \not\in"#,
                    r#"\stackrel{def}{=} \overset{!}{=} \underset{x}{\min}"#,
                    r#"\color{red}{x} + \textcolor{blue}{y}"#,
                    r#"\boxed{E = mc^2}"#,
                    r#"\phantom{x} + \hphantom{y}"#,
                    r#"\displaystyle\sum_{i} \textstyle\sum_{i}"#,
                    r#"\mid \nmid \parallel \perp \angle \triangle \square"#,
                    r#"\lfloor x \rfloor \lceil y \rceil"#,
                    r#"\langle a, b \rangle"#,
                    r#"a \bmod b + a \pmod{n}"#,
                    r#"\frac{\partial^2 u}{\partial t^2} = c^2 \nabla^2 u"#,
                    r#"{n \choose k} + {a \over b}"#,
                    r#"\overleftrightarrow{AB}"#,
                    r#"\cancel{x} + \sout{y}"#,
                    r#"\ce{H2O}"#,
                    r#"\tag{1} x = y"#,
                    r#"\label{eq} x"#,
                    r#"\href{https://x.org}{x}"#,
                    r#"\mathring{A} + \underleftarrow{AB}"#,
                    r#"\Set{x | x > 0}"#,
                    r#"\begin{CD} A @>f>> B \end{CD}"#,
                ];
                let mut failed = 0;
                for latex in corpus {
                    match render_display(latex, WHITE, CELL, 80, FONT) {
                        Ok(_) => eprintln!("OK   {latex}"),
                        Err(err) => {
                            failed += 1;
                            eprintln!("FAIL {latex}  --  {err}");
                        }
                    }
                }
                eprintln!("{failed}/{} failed", corpus.len());
            }

            /// Writes sample PNGs (on a dark background) for eyeballing:
            /// `CALCI_TEX_OUT=<dir> cargo test --release tex::tests::<renderer>::dump -- --ignored`
            #[test]
            #[ignore]
            fn dump() {
                let dir = std::env::var("CALCI_TEX_OUT").expect("set CALCI_TEX_OUT");
                std::fs::create_dir_all(&dir).unwrap();
                for (idx, latex) in SAMPLES.iter().enumerate() {
                    let image = render_display(latex, (0xd0, 0x87, 0x70), CELL, 80, FONT).unwrap();
                    std::fs::write(format!("{dir}/display_{idx:02}.png"), &image.png).unwrap();
                    let image = render_inline(latex, (0xd0, 0x87, 0x70), CELL, 80, FONT).unwrap();
                    std::fs::write(format!("{dir}/inline_{idx:02}.png"), &image.png).unwrap();
                }
            }
            }
        };
    }

    renderer_tests!(katex, MathFont::Katex);
    renderer_tests!(typst, MathFont::LeteSans);
}

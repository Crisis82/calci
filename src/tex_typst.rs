//! Math typeset with Typst and an OpenType math font (`math_font` other
//! than `katex`).
//!
//! LaTeX is converted to Typst math by `mitex`, then compiled against
//! mitex's Typst definitions (`assets/mitex`) and rendered with
//! `typst-render`. One world per math font is kept for the whole process,
//! so Typst's memoized library, font parsing and layouts are reused across
//! formulas.

use std::sync::{LazyLock, Mutex};

use typst::LibraryExt;
use typst::diag::{FileError, FileResult};
use typst::foundations::{Bytes, Datetime, Duration};
use typst::layout::{Abs, Frame, FrameItem};
use typst::syntax::{FileId, RootedPath, Source, VirtualPath, VirtualRoot};
use typst::text::{Font, FontBook};
use typst::utils::LazyHash;
use typst::{Library, World};
use typst_layout::PagedDocument;

use crate::config::MathFont;

/// mitex's Typst side, served at these paths inside the virtual project.
const SPECS: &[(&str, &str)] = &[
    (
        "/mitex/specs/mod.typ",
        include_str!("../assets/mitex/specs/mod.typ"),
    ),
    (
        "/mitex/specs/prelude.typ",
        include_str!("../assets/mitex/specs/prelude.typ"),
    ),
    (
        "/mitex/specs/latex/standard.typ",
        include_str!("../assets/mitex/specs/latex/standard.typ"),
    ),
];

fn font_bytes(font: MathFont) -> Option<&'static [u8]> {
    Some(match font {
        MathFont::Katex => return None,
        MathFont::Fira => include_bytes!("../assets/math-fonts/FiraMath-Regular.otf"),
        MathFont::NewCmSans => include_bytes!("../assets/math-fonts/NewCMSansMath-Regular.otf"),
        MathFont::KpMathSans => include_bytes!("../assets/math-fonts/KpMath-Sans.otf"),
        MathFont::LeteSans => include_bytes!("../assets/math-fonts/LeteSansMath.otf"),
    })
}

fn file_id(path: &str) -> FileId {
    RootedPath::new(
        VirtualRoot::Project,
        VirtualPath::new(path).expect("valid virtual path"),
    )
    .intern()
}

struct MathWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    font: Font,
    family: String,
    /// The formula being compiled; one fixed file id whose text changes
    /// (Typst interns file ids for the process lifetime).
    main: Source,
    specs: Vec<Source>,
}

impl MathWorld {
    fn new(math_font: MathFont) -> Result<Self, String> {
        let bytes = font_bytes(math_font).ok_or("KaTeX fonts are rendered by RaTeX")?;
        let font = Font::iter(Bytes::new(bytes))
            .next()
            .ok_or("failed to load math font")?;
        let family = font.info().family.clone();
        Ok(Self {
            library: LazyHash::new(Library::default()),
            book: LazyHash::new(FontBook::from_fonts([&font])),
            font,
            family,
            main: Source::new(file_id("/main.typ"), String::new()),
            specs: SPECS
                .iter()
                .map(|(path, text)| Source::new(file_id(path), (*text).to_string()))
                .collect(),
        })
    }
}

impl World for MathWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main.id()
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main.id() {
            return Ok(self.main.clone());
        }
        self.specs
            .iter()
            .find(|s| s.id() == id)
            .cloned()
            .ok_or_else(|| FileError::NotFound(id.vpath().get_without_slash().into()))
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        self.source(id)
            .map(|s| Bytes::from_string(s.text().to_string()))
    }

    fn font(&self, index: usize) -> Option<Font> {
        (index == 0).then(|| self.font.clone())
    }

    fn today(&self, _offset: Option<Duration>) -> Option<Datetime> {
        None
    }
}

/// Memoized results unused for this many compiles are dropped.
const EVICT_AFTER_COMPILES: usize = 10;

static WORLDS: LazyLock<Mutex<Vec<(MathFont, MathWorld)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Deepest LaTeX nesting rendered. Typst's layout and mitex's parser
/// recurse once per level and overflow the stack (an uncatchable abort)
/// somewhere past ~200 nested fractions; real formulas stay far below this.
const MAX_NESTING: usize = 64;

/// Deepest nesting of groups in `latex`: `{}` (not `\{`), `\left`/`\right`,
/// `\begin`/`\end`, `()` and `[]`.
fn nesting_depth(latex: &str) -> usize {
    let mut depth = 0usize;
    let mut deepest = 0usize;
    let mut chars = latex.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if !c.is_ascii_alphabetic() {
                        break;
                    }
                    name.push(c);
                    chars.next();
                }
                if name.is_empty() {
                    chars.next(); // `\{`, `\\`, `\,` ...: an escaped character.
                    continue;
                }
                match name.as_str() {
                    "left" | "begin" => depth += 1,
                    "right" | "end" => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        deepest = deepest.max(depth);
    }
    deepest
}

/// Symbol names mitex 0.2.4 emits that Typst 0.15 removed, with the current
/// name for the same character. Found by compiling every alias in mitex's
/// spec (`drift::list_stale_aliases`) and looking each character up in
/// Typst's symbol table (codex 0.3); `ℏ` has no name any more, so the
/// character itself is used.
const RENAMED_SYMBOLS: &[(&str, &str)] = &[
    ("angle.l", "chevron.l"),
    ("angle.r", "chevron.r"),
    ("arrow.l.dash", "arrow.l.dashed"),
    ("arrow.r.dash", "arrow.r.dashed"),
    ("ast.circle", "ast.op.o"),
    ("bracket.l.double", "bracket.l.stroked"),
    ("bracket.r.double", "bracket.r.stroked"),
    ("circle.nested", "compose.o"),
    ("dash.circle", "dash.o"),
    ("diff", "partial"),
    ("dot.circle", "dot.o"),
    ("dot.circle.big", "dot.o.big"),
    ("minus.circle", "minus.o"),
    ("ohm.inv", "Omega.inv"),
    ("planck.reduce", "ℏ"),
    ("plus.circle", "plus.o"),
    ("plus.circle.big", "plus.o.big"),
    ("sect", "inter"),
    ("sect.big", "inter.big"),
    ("sect.double", "inter.double"),
    ("sect.sq", "inter.sq"),
    ("times.circle", "times.o"),
    ("times.circle.big", "times.o.big"),
];

/// Replace stale symbol names in mitex's Typst output. Only whole
/// identifier chains outside string literals are renamed, so text such as
/// `\text{diff}` is left alone.
fn rename_stale_symbols(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut chars = code.char_indices().peekable();
    let mut prev: Option<char> = None;
    while let Some((start, ch)) = chars.next() {
        if ch == '"' {
            out.push(ch);
            let mut escaped = false;
            for (_, c) in chars.by_ref() {
                out.push(c);
                match c {
                    '\\' if !escaped => escaped = true,
                    '"' if !escaped => break,
                    _ => escaped = false,
                }
            }
            prev = Some('"');
            continue;
        }
        let starts_identifier = ch.is_ascii_alphabetic()
            && !prev
                .is_some_and(|p| p.is_ascii_alphanumeric() || p == '.' || p == '_' || p == '\\');
        if !starts_identifier {
            out.push(ch);
            prev = Some(ch);
            continue;
        }
        // Identifier with `.modifier` parts: `sect.big`.
        let mut end = start + ch.len_utf8();
        let mut last = ch;
        while let Some(&(i, c)) = chars.peek() {
            let continues = c.is_ascii_alphanumeric()
                || (c == '.'
                    && code[i + 1..]
                        .chars()
                        .next()
                        .is_some_and(|n| n.is_ascii_alphabetic()));
            if !continues {
                break;
            }
            end = i + c.len_utf8();
            last = c;
            chars.next();
        }
        let chain = &code[start..end];
        match RENAMED_SYMBOLS.iter().find(|(old, _)| *old == chain) {
            Some((_, new)) => out.push_str(new),
            None => out.push_str(chain),
        }
        prev = Some(last);
    }
    out
}

/// A rendered formula: its image and the distance from the image top to the
/// baseline, in pixels.
pub struct Typeset {
    pub image: image::RgbaImage,
    pub ascent: f32,
}

/// Escape `text` as the contents of a Typst string literal.
fn typst_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

pub fn typeset(
    latex: &str,
    display: bool,
    color: (u8, u8, u8),
    font_px: f32,
    math_font: MathFont,
) -> Result<Typeset, String> {
    let depth = nesting_depth(latex);
    if depth > MAX_NESTING {
        return Err(format!("nested {depth} levels deep (limit {MAX_NESTING})"));
    }
    let code = rename_stale_symbols(&mitex::convert_math(latex, None)?);
    let mut worlds = WORLDS
        .lock()
        .map_err(|_| "math world poisoned".to_string())?;
    if !worlds.iter().any(|(f, _)| *f == math_font) {
        worlds.push((math_font, MathWorld::new(math_font)?));
    }
    let world = &mut worlds
        .iter_mut()
        .find(|(f, _)| *f == math_font)
        .expect("just inserted")
        .1;

    let (r, g, b) = color;
    let text = format!(
        "#import \"/mitex/specs/mod.typ\": mitex-scope\n\
         #set page(width: auto, height: auto, margin: 0pt, fill: none)\n\
         #set text(font: \"{family}\", size: {font_px}pt, fill: rgb({r}, {g}, {b}), \
         top-edge: \"bounds\", bottom-edge: \"bounds\")\n\
         #show math.equation: set text(font: \"{family}\")\n\
         #math.equation(block: {display}, eval(\"$\" + \"{code}\" + \"$\", scope: mitex-scope))\n",
        family = world.family,
        code = typst_string(&code),
    );
    world.main.replace(&text);

    let world_ref: &MathWorld = world;
    let compiled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        typst::compile::<PagedDocument>(world_ref).output
    }));
    // Typst memoizes every compile; without eviction memory grows by ~20 MB
    // per 1000 formulas. Keep what the last few compiles used (the library
    // and font work they share), as the Typst CLI does in watch mode.
    typst::comemo::evict(EVICT_AFTER_COMPILES);
    let document = compiled
        .map_err(|_| "typst panicked".to_string())?
        .map_err(|errors| {
            errors
                .first()
                .map(|e| e.message.to_string())
                .unwrap_or_else(|| "typst error".to_string())
        })?;
    let page = document.pages().first().ok_or("no page")?;
    let baseline = find_baseline(&page.frame, Abs::zero()).unwrap_or(page.frame.height());

    let pixmap = typst_render::render(
        page,
        &typst_render::RenderOptions {
            pixel_per_pt: 1.0.into(),
            render_bleed: false,
        },
    );
    let mut image = image::RgbaImage::new(pixmap.width(), pixmap.height());
    for (dst, src) in image.pixels_mut().zip(pixmap.pixels()) {
        let c = src.demultiply();
        *dst = image::Rgba([c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok(Typeset {
        image,
        ascent: baseline.to_pt() as f32,
    })
}

/// The first baseline in the frame tree: the equation's line.
fn find_baseline(frame: &Frame, offset: Abs) -> Option<Abs> {
    for (pos, item) in frame.items() {
        if let FrameItem::Group(group) = item {
            if group.frame.has_baseline() {
                return Some(offset + pos.y + group.frame.baseline());
            }
            if let Some(found) = find_baseline(&group.frame, offset + pos.y) {
                return Some(found);
            }
        }
    }
    frame.has_baseline().then(|| offset + frame.baseline())
}

#[cfg(test)]
mod drift {
    use super::*;

    #[test]
    fn stale_symbols_are_renamed_outside_strings_only() {
        assert_eq!(
            rename_stale_symbols("sect.big  x  sect"),
            "inter.big  x  inter"
        );
        assert_eq!(
            rename_stale_symbols("angle.l  a  angle.r"),
            "chevron.l  a  chevron.r"
        );
        assert_eq!(
            rename_stale_symbols("diff x + diffuse"),
            "partial x + diffuse"
        );
        assert_eq!(
            rename_stale_symbols(r#"#textmath["diff \" sect"] sect"#),
            r#"#textmath["diff \" sect"] inter"#
        );
        assert_eq!(rename_stale_symbols("x_(sect)"), "x_(inter)");
    }

    #[test]
    fn nesting_depth_counts_groups_not_escaped_braces() {
        assert_eq!(nesting_depth(r"\frac{a}{b}"), 1);
        assert_eq!(nesting_depth(r"\left\{ \frac{1}{\frac{2}{3}} \right\}"), 3);
        assert_eq!(nesting_depth(r"\begin{cases} a \end{cases}"), 2);
        assert_eq!(nesting_depth(r"[a, b)"), 1);
        assert_eq!(nesting_depth(&"{".repeat(100)), 100);
    }

    #[test]
    fn every_stale_alias_compiles_after_renaming() {
        let mut world = MathWorld::new(MathFont::Fira).unwrap();
        for (old, _) in RENAMED_SYMBOLS {
            world.main.replace(&format!(
                "#import \"/mitex/specs/mod.typ\": mitex-scope\n\
                 #show math.equation: set text(font: \"Fira Math\")\n\
                 #math.equation(eval(\"$\" + \"{}\" + \"$\", scope: mitex-scope))",
                typst_string(&rename_stale_symbols(old))
            ));
            assert!(
                typst::compile::<PagedDocument>(&world).output.is_ok(),
                "{old}"
            );
        }
    }

    /// Lists mitex symbol aliases that Typst rejects:
    /// `cargo test --release drift -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn list_stale_aliases() {
        let standard = SPECS[2].1;
        let mut aliases: Vec<&str> = Vec::new();
        for marker in ["define-sym(\"", "alias: \""] {
            let mut rest = standard;
            while let Some(i) = rest.find(marker) {
                rest = &rest[i + marker.len()..];
                let end = rest.find('"').unwrap();
                let alias = &rest[..end];
                if !alias.is_empty() && !alias.starts_with('#') && !aliases.contains(&alias) {
                    aliases.push(alias);
                }
            }
        }
        let mut world = MathWorld::new(MathFont::Fira).unwrap();
        let mut stale = 0;
        for alias in &aliases {
            world.main.replace(&format!(
                "#import \"/mitex/specs/mod.typ\": mitex-scope\n\
                 #show math.equation: set text(font: \"Fira Math\")\n\
                 #math.equation(eval(\"$\" + \"{}\" + \"$\", scope: mitex-scope))",
                typst_string(alias)
            ));
            if let Err(errors) = typst::compile::<PagedDocument>(&world).output {
                let message = errors[0].message.to_string();
                if message.contains("unknown variable")
                    || message.contains("unknown symbol modifier")
                {
                    stale += 1;
                    eprintln!("STALE {alias:30} {message}");
                }
            }
        }
        eprintln!("{stale} stale of {} aliases", aliases.len());
    }
}

#[cfg(test)]
mod nesting_probe {
    /// `CALCI_REPEAT=<cmd> CALCI_N=<n>`: typeset `<cmd>` repeated n times + `x`.
    #[test]
    #[ignore]
    fn nesting_probe() {
        let unit = std::env::var("CALCI_REPEAT").unwrap();
        let n: usize = std::env::var("CALCI_N").unwrap().parse().unwrap();
        let latex = unit.repeat(n) + "x";
        let start = std::time::Instant::now();
        let out = super::typeset(
            &latex,
            true,
            (255, 255, 255),
            17.0,
            crate::config::MathFont::Fira,
        );
        eprintln!(
            "RESULT ok={} {:?} {:?}",
            out.is_ok(),
            start.elapsed(),
            out.err().map(|e| e.chars().take(60).collect::<String>())
        );
    }
}

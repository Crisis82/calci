//! Terminal backend that keeps the real screen in sync with ratatui's buffer.
//!
//! ratatui only redraws cells that changed and relies on the terminal's own
//! cursor movement between consecutive cells. That breaks whenever the
//! terminal and ratatui disagree about a character's width (emoji with
//! U+FE0F, keycaps, Nerd Font icons, ambiguous-width symbols): every later
//! character of the run lands one cell off, and since ratatui believes the
//! screen is correct those "random letters" are never repaired.
//!
//! ratatui 0.30 also emits an explicit blank for the cell after a U+FE0F
//! emoji (for terminals that draw it one cell wide), but queues it *after*
//! the emoji. In terminals that draw the emoji two cells wide (kitty), that
//! blank overwrites the following character instead.
//!
//! [`SafeBackend`] wraps the crossterm backend and fixes both: after any
//! character whose width terminals may disagree on, the next cell is placed
//! with an explicit cursor move, and trailing blanks of wide characters are
//! written before the character itself, so a two-cell glyph covers them and
//! a one-cell glyph leaves a clean blank.

use std::io::{self, Write};

use ratatui::backend::{Backend, ClearType, CrosstermBackend, IntoCrossterm, WindowSize};
use ratatui::buffer::Cell;
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{
    Attribute, Color as CColor, Colors, Print, SetAttribute, SetBackgroundColor, SetColors,
    SetForegroundColor,
};
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub struct SafeBackend<W: Write> {
    inner: CrosstermBackend<W>,
}

impl<W: Write> SafeBackend<W> {
    pub fn new(writer: W) -> Self {
        Self {
            inner: CrosstermBackend::new(writer),
        }
    }
}

/// Whether terminals may disagree with ratatui about this grapheme's width,
/// in which case the cell after it must be positioned explicitly.
fn needs_resync(symbol: &str) -> bool {
    let mut chars = symbol.chars();
    let Some(ch) = chars.next() else {
        return false;
    };
    if chars.next().is_some() {
        // Multi-codepoint grapheme: emoji sequences, combining marks, ZWJ.
        return true;
    }
    let cp = ch as u32;
    let unambiguous = cp < 0x7f
        // Latin/Greek/Cyrillic and friends, when single width
        || ((0xa0..0x2000).contains(&cp) && ch.width() == Some(1))
        // General punctuation (smart quotes, dashes, bullets)
        || (0x2000..0x2070).contains(&cp)
        // Box drawing and block elements (table borders)
        || (0x2500..0x25a0).contains(&cp);
    !unambiguous
}

/// Split ratatui's update list into runs that the crossterm backend can
/// draw relying on implicit cursor advance, reordering trailing blanks of
/// wide characters before the character. Each run starts with an explicit
/// cursor move.
fn safe_runs<'a>(updates: &[(u16, u16, &'a Cell)]) -> Vec<Vec<(u16, u16, &'a Cell)>> {
    let mut runs: Vec<Vec<(u16, u16, &'a Cell)>> = Vec::new();
    let mut current: Vec<(u16, u16, &'a Cell)> = Vec::new();
    let flush = |current: &mut Vec<(u16, u16, &'a Cell)>, runs: &mut Vec<_>| {
        if !current.is_empty() {
            runs.push(std::mem::take(current));
        }
    };
    let mut i = 0;
    while i < updates.len() {
        let (x, y, cell) = updates[i];
        let width = cell.symbol().width();
        if width > 1 {
            // ratatui queues explicit updates for the cells the wide
            // grapheme covers right after it; draw those first.
            let mut j = i + 1;
            while j < updates.len() {
                let (tx, ty, _) = updates[j];
                if ty != y || tx <= x || usize::from(tx - x) >= width {
                    break;
                }
                j += 1;
            }
            flush(&mut current, &mut runs);
            if j > i + 1 {
                runs.push(updates[i + 1..j].to_vec());
            }
            runs.push(vec![updates[i]]);
            i = j;
            continue;
        }
        current.push(updates[i]);
        if needs_resync(cell.symbol()) {
            flush(&mut current, &mut runs);
        }
        i += 1;
    }
    flush(&mut current, &mut runs);
    runs
}

/// Switch terminal attributes from `from` to `to` (as ratatui's crossterm
/// backend does: bold and dim share one reset, so the survivor is reapplied).
fn queue_modifier_diff(w: &mut impl Write, from: Modifier, to: Modifier) -> io::Result<()> {
    let removed = from - to;
    if removed.contains(Modifier::REVERSED) {
        queue!(w, SetAttribute(Attribute::NoReverse))?;
    }
    if removed.intersects(Modifier::BOLD | Modifier::DIM) {
        queue!(w, SetAttribute(Attribute::NormalIntensity))?;
        if to.contains(Modifier::DIM) {
            queue!(w, SetAttribute(Attribute::Dim))?;
        }
        if to.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(Attribute::Bold))?;
        }
    }
    if removed.contains(Modifier::ITALIC) {
        queue!(w, SetAttribute(Attribute::NoItalic))?;
    }
    if removed.contains(Modifier::UNDERLINED) {
        queue!(w, SetAttribute(Attribute::NoUnderline))?;
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        queue!(w, SetAttribute(Attribute::NotCrossedOut))?;
    }
    if removed.intersects(Modifier::SLOW_BLINK | Modifier::RAPID_BLINK) {
        queue!(w, SetAttribute(Attribute::NoBlink))?;
    }
    if removed.contains(Modifier::HIDDEN) {
        queue!(w, SetAttribute(Attribute::NoHidden))?;
    }
    let added = to - from;
    for (flag, attr) in [
        (Modifier::REVERSED, Attribute::Reverse),
        (Modifier::BOLD, Attribute::Bold),
        (Modifier::ITALIC, Attribute::Italic),
        (Modifier::UNDERLINED, Attribute::Underlined),
        (Modifier::DIM, Attribute::Dim),
        (Modifier::CROSSED_OUT, Attribute::CrossedOut),
        (Modifier::SLOW_BLINK, Attribute::SlowBlink),
        (Modifier::RAPID_BLINK, Attribute::RapidBlink),
        (Modifier::HIDDEN, Attribute::Hidden),
    ] {
        if added.contains(flag) {
            queue!(w, SetAttribute(attr))?;
        }
    }
    Ok(())
}

/// Raw writes (escape sequences via `execute!`) go straight to the terminal.
impl<W: Write> Write for SafeBackend<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.inner)
    }
}

impl<W: Write> Backend for SafeBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let updates: Vec<_> = content.collect();
        // Same output as ratatui's crossterm backend (colors and attributes
        // carried across cells), except that every run starts with an
        // explicit cursor move.
        let w = &mut self.inner;
        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut modifier = Modifier::empty();
        let mut last: Option<(u16, u16)> = None;
        for run in safe_runs(&updates) {
            for (i, (x, y, cell)) in run.into_iter().enumerate() {
                let contiguous = matches!(last, Some((lx, ly)) if x == lx + 1 && y == ly);
                if i == 0 || !contiguous {
                    queue!(w, MoveTo(x, y))?;
                }
                last = Some((x, y));
                if cell.modifier != modifier {
                    queue_modifier_diff(w, modifier, cell.modifier)?;
                    modifier = cell.modifier;
                }
                if cell.fg != fg || cell.bg != bg {
                    queue!(
                        w,
                        SetColors(Colors::new(
                            cell.fg.into_crossterm(),
                            cell.bg.into_crossterm()
                        ))
                    )?;
                    fg = cell.fg;
                    bg = cell.bg;
                }
                queue!(w, Print(cell.symbol()))?;
            }
        }
        queue!(
            w,
            SetForegroundColor(CColor::Reset),
            SetBackgroundColor(CColor::Reset),
            SetAttribute(Attribute::Reset),
        )
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(symbol: &str) -> Cell {
        let mut c = Cell::default();
        c.set_symbol(symbol);
        c
    }

    fn positions(runs: &[Vec<(u16, u16, &Cell)>]) -> Vec<Vec<(u16, String)>> {
        runs.iter()
            .map(|r| {
                r.iter()
                    .map(|(x, _, c)| (*x, c.symbol().to_string()))
                    .collect()
            })
            .collect()
    }

    #[test]
    fn plain_text_stays_one_run() {
        let cells: Vec<Cell> = "ab│“c”".chars().map(|c| cell(&c.to_string())).collect();
        let updates: Vec<_> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| (i as u16, 0, c))
            .collect();
        assert_eq!(safe_runs(&updates).len(), 1);
    }

    #[test]
    fn trailing_blank_of_vs16_emoji_is_drawn_before_it_and_next_cell_is_repositioned() {
        // What ratatui 0.30 emits for "⚠️x" over previously different content:
        // emoji at 0, explicit trailing blank at 1, then 'x' at 2.
        let (warn, blank, x) = (cell("⚠\u{fe0f}"), cell(" "), cell("x"));
        let updates = vec![(0, 0, &warn), (1, 0, &blank), (2, 0, &x)];
        assert_eq!(
            positions(&safe_runs(&updates)),
            vec![
                vec![(1, " ".to_string())],
                vec![(0, "⚠\u{fe0f}".to_string())],
                vec![(2, "x".to_string())],
            ]
        );
    }

    #[test]
    fn cell_after_ambiguous_symbol_gets_its_own_run() {
        let (a, icon, b) = (cell("a"), cell("\u{f0131}"), cell("b"));
        let updates = vec![(0, 0, &a), (1, 0, &icon), (2, 0, &b)];
        assert_eq!(
            positions(&safe_runs(&updates)),
            vec![
                vec![(0, "a".to_string()), (1, "\u{f0131}".to_string())],
                vec![(2, "b".to_string())],
            ]
        );
    }

    #[test]
    fn output_moves_cursor_explicitly_after_emoji() {
        #[derive(Clone, Default)]
        struct Shared(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);
        impl Write for Shared {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.borrow_mut().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let (warn, blank, x) = (cell("⚠\u{fe0f}"), cell(" "), cell("x"));
        let sink = Shared::default();
        let mut backend = SafeBackend::new(sink.clone());
        backend
            .draw(vec![(0, 0, &warn), (1, 0, &blank), (2, 0, &x)].into_iter())
            .unwrap();
        Backend::flush(&mut backend).unwrap();
        let out = String::from_utf8(sink.0.borrow().clone()).unwrap();
        // CSI row;col H is 1-based: blank at col 2, emoji at col 1, x at col 3.
        let blank_at = out.find("\x1b[1;2H").expect("move to trailing cell");
        let emoji_at = out.find("\x1b[1;1H").expect("move to emoji");
        let x_at = out.find("\x1b[1;3H").expect("explicit move after emoji");
        assert!(blank_at < emoji_at && emoji_at < x_at);
    }
}

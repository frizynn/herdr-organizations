//! The one frame helper both terminal views share: a cell buffer composed
//! like the design mockups (named ANSI colours only, so the user's palette
//! decides the hex), a row diff so only changed lines are written, and the
//! raw-mode guard. Input and state-file changes arrive on one channel, so the
//! process blocks without a timer.

use std::io::{self, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Context, Result};
use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    self, BeginSynchronizedUpdate, EndSynchronizedUpdate, EnterAlternateScreen,
    LeaveAlternateScreen,
};
use crossterm::{execute, queue};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
}

impl Style {
    pub const PLAIN: Style = Style {
        fg: None,
        bg: None,
        bold: false,
        dim: false,
    };

    pub const fn fg(color: Color) -> Style {
        Style {
            fg: Some(color),
            ..Style::PLAIN
        }
    }

    pub const fn bold(self) -> Style {
        Style { bold: true, ..self }
    }

    pub const fn dim(self) -> Style {
        Style { dim: true, ..self }
    }

    pub const fn on(self, bg: Color) -> Style {
        Style {
            bg: Some(bg),
            ..self
        }
    }
}

pub const DIM: Style = Style::PLAIN.dim();
pub const BOLD: Style = Style::PLAIN.bold();
/// Section headings: bold and dim, as in the mockups.
pub const HEAD: Style = Style::PLAIN.bold().dim();
pub const RED: Style = Style::fg(Color::Red);
pub const YELLOW: Style = Style::fg(Color::Yellow);
pub const BLUE: Style = Style::fg(Color::Blue);
pub const GREEN: Style = Style::fg(Color::Green);
pub const CYAN: Style = Style::fg(Color::Cyan);
pub const GREY: Style = Style::fg(Color::DarkGrey);
/// The selected row: colour 0 background, so blue review dots stay readable.
pub const SELECTED: Color = Color::Black;
pub const CHIP: Style = Style::PLAIN.on(Color::Black);
pub const PRIMARY: Style = Style::fg(Color::Black).on(Color::Blue).bold();

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cell {
    /// Empty for the trailing half of a wide character.
    ch: String,
    style: Style,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: " ".into(),
            style: Style::PLAIN,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    cells: Vec<Vec<Cell>>,
}

/// Display width of a string, control characters removed.
pub fn width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Cuts to `max` columns with an ellipsis, after removing terminal controls.
pub fn fit(text: &str, max: usize) -> String {
    fit_terminal_row(text, max)
}

fn is_bidi_or_line_control(character: char) -> bool {
    matches!(
        character as u32,
        0x061c
            | 0x200b..=0x200f
            | 0x2028..=0x202e
            | 0x2060..=0x206f
            | 0xfeff
    )
}

fn skip_osc(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(character) = chars.next() {
        if matches!(character, '\u{0007}' | '\u{009c}') {
            break;
        }
        if character == '\u{001b}' && chars.peek() == Some(&'\\') {
            chars.next();
            break;
        }
    }
}

fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for character in chars.by_ref() {
        if ('\u{0040}'..='\u{007e}').contains(&character) {
            break;
        }
    }
}

fn sanitize_terminal_text(value: &str) -> String {
    let mut chars = value.chars().peekable();
    let mut output = String::with_capacity(value.len());
    while let Some(character) = chars.next() {
        if character == '\u{001b}' {
            match chars.peek() {
                Some(']') => {
                    chars.next();
                    skip_osc(&mut chars);
                }
                Some('[') => {
                    chars.next();
                    skip_csi(&mut chars);
                }
                Some('P' | '^' | '_') => {
                    chars.next();
                    skip_osc(&mut chars);
                }
                _ => {}
            }
            continue;
        }
        if character == '\u{009d}' {
            skip_osc(&mut chars);
            continue;
        }
        if character == '\u{009b}' {
            skip_csi(&mut chars);
            continue;
        }
        if matches!(character as u32, 0x0090 | 0x009e | 0x009f) {
            skip_osc(&mut chars);
            continue;
        }
        if character.is_control() || is_bidi_or_line_control(character) {
            continue;
        }
        output.push(character);
    }
    output
}

pub fn fit_terminal_row(value: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let clean = sanitize_terminal_text(value);
    let max_chars = max_width.saturating_mul(4).max(16);
    let mut output = String::new();
    let mut width = 0usize;
    let mut truncated = false;
    for (char_count, character) in clean.chars().enumerate() {
        if char_count >= max_chars {
            truncated = true;
            break;
        }
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if width.saturating_add(character_width) > max_width {
            truncated = true;
            break;
        }
        output.push(character);
        width += character_width;
    }
    if truncated {
        while width.saturating_add(1) > max_width {
            let Some(character) = output.pop() else {
                break;
            };
            width = width.saturating_sub(UnicodeWidthChar::width(character).unwrap_or(0));
        }
        output.push('…');
    }
    output
}

impl Frame {
    pub fn new(width: usize, height: usize) -> Frame {
        Frame {
            width,
            height,
            cells: vec![vec![Cell::default(); width]; height],
        }
    }

    /// Writes `text` from column `x`; clipped at the frame edge. Returns the
    /// column after the text.
    pub fn put(&mut self, x: usize, y: usize, text: &str, style: Style) -> usize {
        if y >= self.height {
            return x;
        }
        let clean = fit(text, self.width.saturating_sub(x));
        let mut col = x;
        for ch in clean.chars() {
            let w = ch.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if col + w > self.width {
                break;
            }
            let row = &mut self.cells[y];
            let bg = style.bg.or(row[col].style.bg);
            row[col] = Cell {
                ch: ch.to_string(),
                style: Style { bg, ..style },
            };
            for extra in 1..w {
                row[col + extra] = Cell {
                    ch: String::new(),
                    style: Style { bg, ..style },
                };
            }
            col += w;
        }
        col
    }

    /// Text that ends on column `right` (exclusive).
    pub fn put_right(&mut self, right: usize, y: usize, text: &str, style: Style) {
        let w = width(text).min(right);
        self.put(right - w, y, text, style);
    }

    pub fn fill(&mut self, x: usize, y: usize, w: usize, bg: Option<Color>) {
        if y >= self.height {
            return;
        }
        for col in x..(x + w).min(self.width) {
            self.cells[y][col] = Cell {
                ch: " ".into(),
                style: Style { bg, ..Style::PLAIN },
            };
        }
    }

    /// A full-width selected row background.
    pub fn select_row(&mut self, y: usize) {
        self.fill(0, y, self.width, Some(SELECTED));
    }

    pub fn rule(&mut self, y: usize) {
        let line = "─".repeat(self.width);
        self.put(0, y, &line, GREY);
    }

    /// Dims everything already drawn (behind a dialog).
    pub fn dim_all(&mut self) {
        for row in &mut self.cells {
            for cell in row {
                cell.style.dim = true;
                cell.style.bold = false;
                cell.style.bg = None;
            }
        }
    }

    /// A bordered box; returns the inner origin.
    pub fn boxed(
        &mut self,
        x: usize,
        y: usize,
        w: usize,
        h: usize,
        title: &str,
        border: Style,
    ) -> (usize, usize) {
        if w < 2 || h < 2 {
            return (x, y);
        }
        for row in y..y + h {
            self.fill(x, row, w, None);
        }
        let inner = w - 2;
        self.put(x, y, &format!("┌{}┐", "─".repeat(inner)), border);
        self.put(x, y + h - 1, &format!("└{}┘", "─".repeat(inner)), border);
        for row in y + 1..y + h - 1 {
            self.put(x, row, "│", border);
            self.put(x + w - 1, row, "│", border);
        }
        if !title.is_empty() {
            self.put(x + 1, y, title, BOLD);
        }
        (x + 1, y + 1)
    }

    /// The hint bar: key first, words dimmed, joined by " · ". Pairs that do
    /// not fit are dropped from the end.
    pub fn hint(&mut self, y: usize, pairs: &[(String, &str)]) {
        let mut x = 1;
        for (i, (key, words)) in pairs.iter().enumerate() {
            let need = width(key) + 1 + width(words) + if i > 0 { 3 } else { 0 };
            if x + need > self.width {
                break;
            }
            if i > 0 {
                x = self.put(x + 1, y, "·", DIM) + 1;
            }
            x = self.put(x, y, key, BOLD) + 1;
            x = self.put(x, y, words, DIM);
        }
    }

    /// One answer or action chip: ` key text `.
    pub fn chip(&mut self, x: usize, y: usize, key: &str, text: &str) -> usize {
        let end = self.put(x, y, &format!(" {key} {text} "), CHIP);
        self.put(x + 1, y, key, BLUE.bold().on(SELECTED));
        end
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        self.cells
            .iter()
            .map(|row| row.iter().map(|c| c.ch.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[cfg(test)]
    pub fn style_at(&self, x: usize, y: usize) -> Style {
        self.cells[y][x].style
    }

    fn write_row(&self, out: &mut impl Write, y: usize) -> io::Result<()> {
        queue!(
            out,
            MoveTo(0, y as u16),
            ResetColor,
            SetAttribute(Attribute::Reset)
        )?;
        let mut current = Style::PLAIN;
        for cell in &self.cells[y] {
            if cell.ch.is_empty() {
                continue;
            }
            if cell.style != current {
                queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
                if let Some(fg) = cell.style.fg {
                    queue!(out, SetForegroundColor(fg))?;
                }
                if let Some(bg) = cell.style.bg {
                    queue!(out, SetBackgroundColor(bg))?;
                }
                if cell.style.bold {
                    queue!(out, SetAttribute(Attribute::Bold))?;
                }
                if cell.style.dim {
                    queue!(out, SetAttribute(Attribute::Dim))?;
                }
                current = cell.style;
            }
            queue!(out, Print(&cell.ch))?;
        }
        queue!(out, SetAttribute(Attribute::Reset), ResetColor)
    }
}

/// Raw mode, alternate screen and mouse capture for the life of the guard.
pub struct Terminal {
    last: Option<Frame>,
}

impl Terminal {
    pub fn enter() -> Result<Terminal> {
        terminal::enable_raw_mode().context("could not enable terminal input")?;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            event::EnableMouseCapture,
            Hide
        )
        .context("could not enter the Organizations view")?;
        Ok(Terminal { last: None })
    }

    pub fn size() -> (usize, usize) {
        terminal::size()
            .map(|(w, h)| (w as usize, h as usize))
            .unwrap_or((92, 30))
    }

    /// Writes only the rows that differ from the previous frame.
    pub fn draw(&mut self, frame: Frame) -> Result<()> {
        let mut out = io::stdout().lock();
        queue!(out, BeginSynchronizedUpdate, Hide)?;
        let resized = self
            .last
            .as_ref()
            .is_none_or(|l| l.width != frame.width || l.height != frame.height);
        if resized {
            queue!(out, terminal::Clear(terminal::ClearType::All))?;
        }
        for y in 0..frame.height {
            let same = !resized
                && self
                    .last
                    .as_ref()
                    .is_some_and(|l| l.cells[y] == frame.cells[y]);
            if !same {
                frame.write_row(&mut out, y)?;
            }
        }
        queue!(out, EndSynchronizedUpdate)?;
        out.flush()?;
        self.last = Some(frame);
        Ok(())
    }

    /// Forgets the previous frame so the next draw repaints everything.
    pub fn invalidate(&mut self) {
        self.last = None;
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            event::DisableMouseCapture,
            Show,
            LeaveAlternateScreen,
            SetAttribute(Attribute::Reset)
        );
        let _ = terminal::disable_raw_mode();
    }
}

/// Runs `f` with stdout and stderr sent to /dev/null, so a CLI mechanic that
/// prints (resolve, adopt) cannot scribble over the full-screen view.
pub fn quiet<T>(f: impl FnOnce() -> T) -> T {
    let _ = io::stdout().flush();
    // SAFETY: dup/dup2/close on the process's own standard descriptors; each
    // saved descriptor is restored and closed before returning.
    unsafe {
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY);
        let saved = [libc::dup(1), libc::dup(2)];
        if null >= 0 && saved.iter().all(|fd| *fd >= 0) {
            libc::dup2(null, 1);
            libc::dup2(null, 2);
        }
        let result = f();
        let _ = io::stdout().flush();
        for (fd, copy) in [1, 2].into_iter().zip(saved) {
            if copy >= 0 {
                libc::dup2(copy, fd);
                libc::close(copy);
            }
        }
        if null >= 0 {
            libc::close(null);
        }
        result
    }
}

pub enum Input {
    Terminal(Event),
    /// The projects root changed (the state file may be new).
    Changed,
}

/// One reader thread for terminal events and one for root changes. The
/// caller blocks on the receiver: nothing wakes it on a timer.
pub fn inputs(root: &Path) -> Receiver<Input> {
    let (tx, rx) = channel();
    let keys: Sender<Input> = tx.clone();
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if keys.send(Input::Terminal(event)).is_err() {
                return;
            }
        }
    });
    if let Ok(watcher) = crate::watch::Watcher::new(root) {
        std::thread::spawn(move || {
            while watcher.wait().is_ok() {
                if tx.send(Input::Changed).is_err() {
                    return;
                }
            }
        });
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_clips_and_keeps_wide_characters_on_the_grid() {
        let mut frame = Frame::new(10, 2);
        let end = frame.put(1, 0, "abc", RED);
        assert_eq!(end, 4);
        frame.put(8, 1, "wxyz", Style::PLAIN);
        assert_eq!(frame.text().lines().nth(1).unwrap(), "        w…");
        let mut wide = Frame::new(6, 1);
        wide.put(0, 0, "日本x", Style::PLAIN);
        assert_eq!(wide.text(), "日本x ");
    }

    #[test]
    fn hint_drops_pairs_that_do_not_fit() {
        let mut frame = Frame::new(24, 1);
        frame.hint(
            0,
            &[
                ("↵".into(), "open"),
                ("n".into(), "new"),
                ("esc".into(), "close"),
            ],
        );
        assert_eq!(frame.text().trim_end(), " ↵ open · n new");
    }

    #[test]
    fn selected_rows_keep_their_background_under_text() {
        let mut frame = Frame::new(8, 1);
        frame.select_row(0);
        frame.put(1, 0, "●", BLUE);
        assert_eq!(frame.style_at(1, 0).bg, Some(SELECTED));
        assert_eq!(frame.style_at(1, 0).fg, Some(Color::Blue));
    }
}

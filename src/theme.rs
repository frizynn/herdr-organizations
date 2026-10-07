//! How the menu and the tree look, by meaning, on the terminal's own 16
//! colors so they suit its background and the user's palette. A state
//! always shows a glyph and a word; color only adds to them. The user's
//! `<config dir>/theme.toml` overrides any slot:
//!
//! ```toml
//! needs_you = "magenta bold"
//! working   = "cyan"
//! muted     = "default dim"
//! ascii     = true            # ! * o + x instead of ! ● ○ ◆ ✓
//! ```
//!
//! A slot is a color (`black`, `red`, `green`, `yellow`, `blue`,
//! `magenta`, `cyan`, `white`, `grey`, the `bright_` forms, `default`, or
//! `#rrggbb`) and any of `bold`, `dim`, `italic`, `underline`, `reverse`.
//! `NO_COLOR`, or `TERM=dumb`, drops the colors and keeps the rest.

use std::path::Path;

use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};

use crate::paths::Env;
use crate::thread::Group;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub fg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

impl Style {
    pub fn parse(text: &str) -> Option<Style> {
        let mut style = Style::default();
        for word in text.split_whitespace() {
            match word {
                "bold" => style.bold = true,
                "dim" => style.dim = true,
                "italic" => style.italic = true,
                "underline" => style.underline = true,
                "reverse" => style.reverse = true,
                "default" => style.fg = None,
                color => style.fg = Some(color_named(color)?),
            }
        }
        Some(style)
    }

    /// `text` wrapped in this style's escapes, reset after.
    pub fn paint(&self, text: &str) -> String {
        if *self == Style::default() || text.is_empty() {
            return text.to_string();
        }
        let mut out = String::new();
        let mut attr = |on: bool, attribute: Attribute| {
            if on {
                out.push_str(&SetAttribute(attribute).to_string());
            }
        };
        attr(self.bold, Attribute::Bold);
        attr(self.dim, Attribute::Dim);
        attr(self.italic, Attribute::Italic);
        attr(self.underline, Attribute::Underlined);
        attr(self.reverse, Attribute::Reverse);
        if let Some(color) = self.fg {
            out.push_str(&SetForegroundColor(color).to_string());
        }
        out.push_str(text);
        out.push_str(&ResetColor.to_string());
        out.push_str(&SetAttribute(Attribute::Reset).to_string());
        out
    }

    /// This style with another one's attributes on top (the selection over
    /// a row, a state word inside it).
    pub fn with(self, other: Style) -> Style {
        Style {
            fg: other.fg.or(self.fg),
            bold: self.bold || other.bold,
            dim: self.dim || other.dim,
            italic: self.italic || other.italic,
            underline: self.underline || other.underline,
            reverse: self.reverse || other.reverse,
        }
    }
}

fn color_named(name: &str) -> Option<Color> {
    Some(match name {
        "black" => Color::Black,
        "red" => Color::DarkRed,
        "green" => Color::DarkGreen,
        "yellow" => Color::DarkYellow,
        "blue" => Color::DarkBlue,
        "magenta" => Color::DarkMagenta,
        "cyan" => Color::DarkCyan,
        "white" => Color::Grey,
        "grey" | "gray" => Color::DarkGrey,
        "bright_red" => Color::Red,
        "bright_green" => Color::Green,
        "bright_yellow" => Color::Yellow,
        "bright_blue" => Color::Blue,
        "bright_magenta" => Color::Magenta,
        "bright_cyan" => Color::Cyan,
        "bright_white" => Color::White,
        hex if hex.len() == 7 && hex.starts_with('#') => {
            let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
            Color::Rgb {
                r: byte(1)?,
                g: byte(3)?,
                b: byte(5)?,
            }
        }
        _ => return None,
    })
}

/// The slots, by meaning. Names are the keys of theme.toml.
#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub title: Style,
    pub heading: Style,
    pub text: Style,
    pub muted: Style,
    pub accent: Style,
    pub selection: Style,
    pub border: Style,
    pub needs_you: Style,
    pub review: Style,
    pub working: Style,
    pub landing: Style,
    pub idle: Style,
    pub done: Style,
    pub error: Style,
    pub ascii: bool,
    /// Problems found in theme.toml.
    pub errors: Vec<String>,
}

impl Default for Theme {
    fn default() -> Self {
        let s = |text: &str| Style::parse(text).expect("default style parses");
        Theme {
            title: s("bold"),
            heading: s("bold"),
            text: s("default"),
            muted: s("dim"),
            accent: s("blue bold"),
            selection: s("reverse"),
            border: s("dim"),
            needs_you: s("magenta bold"),
            review: s("blue"),
            working: s("cyan"),
            landing: s("green"),
            idle: s("default"),
            done: s("green dim"),
            error: s("red bold"),
            ascii: false,
            errors: Vec::new(),
        }
    }
}

impl Theme {
    pub fn load(config_dir: &Path, env: &Env) -> Theme {
        let mut theme = match std::fs::read_to_string(config_dir.join("theme.toml")) {
            Ok(text) => Theme::from_toml(&text),
            Err(_) => Theme::default(),
        };
        let no_color =
            env.var("NO_COLOR").is_some_and(|v| !v.is_empty()) || env.var("TERM") == Some("dumb");
        if no_color {
            theme.drop_colors();
        }
        theme
    }

    pub fn from_toml(text: &str) -> Theme {
        let mut theme = Theme::default();
        let table: toml::Table = match toml::from_str(text) {
            Ok(table) => table,
            Err(error) => {
                theme
                    .errors
                    .push(format!("theme.toml: {}", error.message()));
                return theme;
            }
        };
        for (key, value) in table {
            if key == "ascii" {
                match value.as_bool() {
                    Some(ascii) => theme.ascii = ascii,
                    None => theme
                        .errors
                        .push("theme.toml: ascii is true or false".into()),
                }
                continue;
            }
            let Some(style) = value.as_str().and_then(Style::parse) else {
                theme.errors.push(format!("theme.toml: {key}: not a style"));
                continue;
            };
            match theme.slot(&key) {
                Some(slot) => *slot = style,
                None => theme.errors.push(format!("theme.toml: no slot `{key}`")),
            }
        }
        theme
    }

    fn slot(&mut self, name: &str) -> Option<&mut Style> {
        Some(match name {
            "title" => &mut self.title,
            "heading" => &mut self.heading,
            "text" => &mut self.text,
            "muted" => &mut self.muted,
            "accent" => &mut self.accent,
            "selection" => &mut self.selection,
            "border" => &mut self.border,
            "needs_you" => &mut self.needs_you,
            "review" => &mut self.review,
            "working" => &mut self.working,
            "landing" => &mut self.landing,
            "idle" => &mut self.idle,
            "done" => &mut self.done,
            "error" => &mut self.error,
            _ => return None,
        })
    }

    fn drop_colors(&mut self) {
        for name in [
            "title",
            "heading",
            "text",
            "muted",
            "accent",
            "selection",
            "border",
            "needs_you",
            "review",
            "working",
            "landing",
            "idle",
            "done",
            "error",
        ] {
            if let Some(slot) = self.slot(name) {
                slot.fg = None;
            }
        }
    }

    /// A group's glyph, word and style: the same everywhere.
    pub fn state(&self, group: Group) -> (&'static str, &'static str, Style) {
        let (glyph, ascii, style) = match group {
            Group::WaitingOnYou => ("!", "!", self.needs_you),
            Group::ReadyForReview => ("◆", "+", self.review),
            Group::Working => ("●", "*", self.working),
            Group::Landing => ("↑", "^", self.landing),
            Group::Idle => ("○", "o", self.idle),
            Group::Resolved => ("✓", "v", self.done),
        };
        (
            if self.ascii { ascii } else { glyph },
            crate::sidebar::word(group),
            style,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styles_parse_colors_and_attributes() {
        let style = Style::parse("magenta bold").unwrap();
        assert_eq!(style.fg, Some(Color::DarkMagenta));
        assert!(style.bold && !style.dim);
        assert_eq!(
            Style::parse("#ff8800").unwrap().fg,
            Some(Color::Rgb {
                r: 255,
                g: 136,
                b: 0
            })
        );
        assert!(Style::parse("purple").is_none());
        assert_eq!(Style::default().paint("x"), "x");
        assert!(
            Style::parse("bold")
                .unwrap()
                .paint("x")
                .contains("\u{1b}[1m")
        );
    }

    #[test]
    fn theme_overrides_slots_and_reports_mistakes() {
        let theme = Theme::from_toml(
            "working = \"yellow\"\nascii = true\nnope = \"red\"\nidle = \"purple\"\n",
        );
        assert_eq!(theme.working.fg, Some(Color::DarkYellow));
        assert!(theme.ascii);
        assert_eq!(theme.errors.len(), 2, "{:?}", theme.errors);
        assert_eq!(theme.state(Group::Working).0, "*");
    }

    #[test]
    fn no_color_keeps_glyphs_words_and_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::for_test(dir.path(), &[("NO_COLOR", "1")]);
        let theme = Theme::load(dir.path(), &env);
        let (glyph, word, style) = theme.state(Group::WaitingOnYou);
        assert_eq!((glyph, word), ("!", "needs you"));
        assert_eq!(style.fg, None);
        assert!(style.bold);
    }
}

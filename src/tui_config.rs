//! `~/.config/herdr-projects/tui.toml`: the view settings and the user's
//! keybindings for the Organizations popup and dock. The Settings screen
//! writes the same file, so hand edits and screen edits stay one source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

pub const FILE: &str = "tui.toml";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Dock {
    #[default]
    Off,
    Right,
    Left,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Resolved {
    #[default]
    Count,
    List,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Notify {
    #[default]
    NeedsYouAndReview,
    NeedsYou,
    Off,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct View {
    pub dock: Dock,
    pub dock_width: u8,
    pub resolved: Resolved,
    pub notify: Notify,
    /// Digits 1-9 answer a waiting agent from the thread detail screen.
    pub answer: bool,
}

impl Default for View {
    fn default() -> Self {
        View {
            dock: Dock::Off,
            dock_width: 30,
            resolved: Resolved::Count,
            notify: Notify::NeedsYouAndReview,
            answer: true,
        }
    }
}

/// Every rebindable command. Digits 1-9 (jump, or answer from the thread
/// detail) are fixed; Settings can turn answering off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Open,
    Back,
    NewProject,
    NewCoordinator,
    NewThread,
    Board,
    Merge,
    Settings,
    Search,
    Adopt,
    Fold,
    GoToPane,
    Reply,
    OpenPr,
    Done,
    Filter,
    KeepWorktree,
    Doctor,
    NextField,
    PrevField,
    Help,
}

/// Where a command is read. Two commands may share a key only when they never
/// appear on the same screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Launcher,
    Form,
    Tree,
    Board,
    Detail,
    Confirm,
    Settings,
    Dock,
}

const ALL_SCREENS: &[Screen] = &[
    Screen::Launcher,
    Screen::Form,
    Screen::Tree,
    Screen::Board,
    Screen::Detail,
    Screen::Confirm,
    Screen::Settings,
    Screen::Dock,
];

impl Action {
    pub const ALL: [Action; 25] = [
        Action::Up,
        Action::Down,
        Action::Left,
        Action::Right,
        Action::Open,
        Action::Back,
        Action::NewProject,
        Action::NewCoordinator,
        Action::NewThread,
        Action::Board,
        Action::Merge,
        Action::Settings,
        Action::Search,
        Action::Adopt,
        Action::Fold,
        Action::GoToPane,
        Action::Reply,
        Action::OpenPr,
        Action::Done,
        Action::Filter,
        Action::KeepWorktree,
        Action::Doctor,
        Action::NextField,
        Action::PrevField,
        Action::Help,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::Up => "move up",
            Action::Down => "move down",
            Action::Left => "left / previous value",
            Action::Right => "right / next value",
            Action::Open => "open",
            Action::Back => "back, then close",
            Action::NewProject => "new project",
            Action::NewCoordinator => "new coordinator",
            Action::NewThread => "new thread",
            Action::Board => "threads board",
            Action::Merge => "merge",
            Action::Settings => "settings",
            Action::Search => "search",
            Action::Adopt => "adopt workspace",
            Action::Fold => "fold coordinator",
            Action::GoToPane => "go to pane",
            Action::Reply => "reply",
            Action::OpenPr => "open PR",
            Action::Done => "inbox item done",
            Action::Filter => "board filter",
            Action::KeepWorktree => "merge, keep worktree",
            Action::Doctor => "run doctor",
            Action::NextField => "next field",
            Action::PrevField => "previous field",
            Action::Help => "keys",
        }
    }

    fn default_keys(self) -> &'static [&'static str] {
        match self {
            Action::Up => &["up", "k"],
            Action::Down => &["down", "j"],
            Action::Left => &["left"],
            Action::Right => &["right"],
            Action::Open => &["enter"],
            Action::Back => &["esc"],
            Action::NewProject => &["n"],
            Action::NewCoordinator => &["c"],
            Action::NewThread => &["t"],
            Action::Board => &["b"],
            Action::Merge => &["m"],
            Action::Settings => &["s"],
            Action::Search => &["/"],
            Action::Adopt => &["a"],
            Action::Fold => &["space"],
            Action::GoToPane => &["g"],
            Action::Reply => &["r"],
            Action::OpenPr => &["p"],
            Action::Done => &["d"],
            Action::Filter => &["f"],
            Action::KeepWorktree => &["k"],
            Action::Doctor => &["d"],
            Action::NextField => &["tab"],
            Action::PrevField => &["shift+tab"],
            Action::Help => &["?"],
        }
    }

    /// The screens that read this command. Text entry on the form owns
    /// printable keys, so the form only reads non-printable commands.
    fn screens(self) -> &'static [Screen] {
        use Screen::*;
        match self {
            Action::Back | Action::Open => ALL_SCREENS,
            Action::Up | Action::Down => &[Launcher, Form, Tree, Board, Detail, Settings, Dock],
            Action::Left | Action::Right => &[Form, Board, Settings],
            Action::NewProject => &[Launcher, Tree, Board],
            Action::NewCoordinator => &[Launcher, Tree],
            Action::NewThread => &[Tree, Dock],
            Action::Board => &[Launcher, Tree],
            Action::Merge => &[Tree, Board, Detail, Dock],
            Action::Settings => &[Launcher],
            Action::Search => &[Launcher],
            Action::Adopt => &[Launcher],
            Action::Fold => &[Tree],
            Action::GoToPane => &[Tree, Board, Detail, Dock],
            Action::Reply => &[Tree, Board, Detail],
            Action::OpenPr => &[Tree, Detail],
            Action::Done => &[Detail],
            Action::Filter => &[Board],
            Action::KeepWorktree => &[Confirm],
            Action::Doctor => &[Settings],
            Action::NextField | Action::PrevField => &[Form],
            Action::Help => &[Dock],
        }
    }

    pub fn on(self, screen: Screen) -> bool {
        self.screens().contains(&screen)
    }
}

/// A normalized key name: `enter`, `esc`, `tab`, `shift+tab`, `space`,
/// `backspace`, arrows, `ctrl+x`, `alt+x`, or one printable character.
pub fn key_name(event: &KeyEvent) -> Option<String> {
    let base = match event.code {
        KeyCode::Enter => "enter".to_string(),
        KeyCode::Esc => "esc".to_string(),
        KeyCode::Tab => "tab".to_string(),
        KeyCode::BackTab => return Some("shift+tab".into()),
        KeyCode::Backspace => "backspace".to_string(),
        KeyCode::Delete => "delete".to_string(),
        KeyCode::Up => "up".to_string(),
        KeyCode::Down => "down".to_string(),
        KeyCode::Left => "left".to_string(),
        KeyCode::Right => "right".to_string(),
        KeyCode::Home => "home".to_string(),
        KeyCode::End => "end".to_string(),
        KeyCode::PageUp => "pageup".to_string(),
        KeyCode::PageDown => "pagedown".to_string(),
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_lowercase().collect(),
        KeyCode::F(n) => format!("f{n}"),
        _ => return None,
    };
    let mut parts = Vec::new();
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        parts.push("ctrl");
    }
    if event.modifiers.contains(KeyModifiers::ALT) {
        parts.push("alt");
    }
    // Shift is already part of an uppercase or symbol character.
    let printable = matches!(event.code, KeyCode::Char(c) if c != ' ');
    if event.modifiers.contains(KeyModifiers::SHIFT) && !printable {
        parts.push("shift");
    }
    if let KeyCode::Char(c) = event.code
        && c.is_uppercase()
    {
        parts.push("shift");
    }
    parts.push(&base);
    Some(parts.join("+"))
}

fn valid_key(name: &str) -> bool {
    let last = name.rsplit('+').next().unwrap_or("");
    let modifiers = name.split('+').count() - 1;
    let known = [
        "enter",
        "esc",
        "tab",
        "backspace",
        "delete",
        "up",
        "down",
        "left",
        "right",
        "home",
        "end",
        "pageup",
        "pagedown",
        "space",
    ];
    let single = last.chars().count() == 1;
    let function = last
        .strip_prefix('f')
        .is_some_and(|n| n.parse::<u8>().is_ok());
    (known.contains(&last) || single || function)
        && name
            .split('+')
            .take(modifiers)
            .all(|m| matches!(m, "ctrl" | "alt" | "shift"))
        // Digits jump or answer on every screen.
        && !(modifiers == 0 && last.chars().all(|c| c.is_ascii_digit()) && last != "0")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    map: BTreeMap<Action, Vec<String>>,
}

impl Default for Keys {
    fn default() -> Self {
        Keys {
            map: Action::ALL
                .iter()
                .map(|a| (*a, a.default_keys().iter().map(|k| k.to_string()).collect()))
                .collect(),
        }
    }
}

impl Keys {
    pub fn keys(&self, action: Action) -> &[String] {
        self.map.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The key shown in hints: the first binding.
    pub fn primary(&self, action: Action) -> &str {
        self.keys(action).first().map(String::as_str).unwrap_or("")
    }

    pub fn display(&self, action: Action) -> String {
        self.keys(action).join(" / ")
    }

    /// The command a key means on `screen`.
    pub fn action(&self, screen: Screen, key: &str) -> Option<Action> {
        Action::ALL
            .into_iter()
            .find(|a| a.on(screen) && self.keys(*a).iter().any(|k| k == key))
    }

    /// The command already holding `key` on a screen `action` shares.
    pub fn conflict(&self, action: Action, key: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|other| {
            *other != action
                && self.keys(*other).iter().any(|k| k == key)
                && action.screens().iter().any(|s| other.on(*s))
        })
    }

    /// Rebinds `action` to exactly `key`. Refused when invalid or when
    /// another command on a shared screen already uses it.
    pub fn set(&mut self, action: Action, key: &str) -> Result<(), String> {
        if !valid_key(key) {
            return Err(format!("`{key}` cannot be bound"));
        }
        if let Some(other) = self.conflict(action, key) {
            return Err(format!("`{key}` is already {}", other.label()));
        }
        self.map.insert(action, vec![key.to_string()]);
        Ok(())
    }

    pub fn reset(&mut self, action: Action) {
        self.map.insert(
            action,
            action
                .default_keys()
                .iter()
                .map(|k| k.to_string())
                .collect(),
        );
    }
}

/// The file shape. Keys are written only when they differ from the default,
/// so new defaults reach users who never changed that command.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    view: View,
    keys: BTreeMap<Action, KeyList>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum KeyList {
    One(String),
    Many(Vec<String>),
}

impl KeyList {
    fn into_vec(self) -> Vec<String> {
        match self {
            KeyList::One(key) => vec![key],
            KeyList::Many(keys) => keys,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub view: View,
    pub keys: Keys,
    /// Bindings from the file that were ignored, for the Settings screen.
    pub problems: Vec<String>,
}

pub fn path(config_dir: &Path) -> PathBuf {
    config_dir.join(FILE)
}

/// Missing file: defaults. A bad binding is reported and its default kept;
/// a file that does not parse is an error so a typo is never silently lost.
pub fn load(config_dir: &Path) -> Result<Config> {
    let path = path(config_dir);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Config::default());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    let file: File =
        toml::from_str(&text).with_context(|| format!("{} does not parse", path.display()))?;
    let mut config = Config {
        view: file.view,
        ..Config::default()
    };
    config.view.dock_width = config.view.dock_width.clamp(15, 50);
    for (action, list) in file.keys {
        let keys: Vec<String> = list
            .into_vec()
            .into_iter()
            .map(|k| k.to_lowercase())
            .collect();
        if keys.is_empty() {
            config
                .problems
                .push(format!("{}: no key given", action.label()));
            continue;
        }
        let bad = keys.iter().find(|k| !valid_key(k));
        if let Some(bad) = bad {
            config
                .problems
                .push(format!("{}: `{bad}` cannot be bound", action.label()));
            continue;
        }
        config.keys.map.insert(action, keys);
    }
    // Conflicts are checked after every binding is in, so file order does not matter.
    for action in Action::ALL {
        let clash = config
            .keys
            .keys(action)
            .iter()
            .find_map(|k| config.keys.conflict(action, k).map(|o| (k.clone(), o)));
        if let Some((key, other)) = clash
            && action > other
        {
            config.problems.push(format!(
                "{}: `{key}` is already {}; default kept",
                action.label(),
                other.label()
            ));
            config.keys.reset(action);
        }
    }
    Ok(config)
}

pub fn save(config_dir: &Path, config: &Config) -> Result<()> {
    let defaults = Keys::default();
    let keys = Action::ALL
        .into_iter()
        .filter(|a| config.keys.keys(*a) != defaults.keys(*a))
        .map(|a| {
            let keys = config.keys.keys(a).to_vec();
            let list = if keys.len() == 1 {
                KeyList::One(keys[0].clone())
            } else {
                KeyList::Many(keys)
            };
            (a, list)
        })
        .collect();
    let file = File {
        view: config.view.clone(),
        keys,
    };
    std::fs::create_dir_all(config_dir)
        .with_context(|| format!("could not create {}", config_dir.display()))?;
    let text = format!(
        "# Herdr Organizations popup and dock. Edit here or in Settings (s).\n# Keys: enter, esc, tab, shift+tab, space, up, down, left, right, ctrl+x, alt+x or one character.\n{}",
        toml::to_string(&file)?
    );
    crate::project::write_atomic(&path(config_dir), text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn defaults_match_the_mockups() {
        let keys = Keys::default();
        assert_eq!(keys.action(Screen::Launcher, "n"), Some(Action::NewProject));
        assert_eq!(
            keys.action(Screen::Launcher, "c"),
            Some(Action::NewCoordinator)
        );
        assert_eq!(keys.action(Screen::Tree, "t"), Some(Action::NewThread));
        assert_eq!(keys.action(Screen::Launcher, "b"), Some(Action::Board));
        assert_eq!(keys.action(Screen::Tree, "m"), Some(Action::Merge));
        assert_eq!(keys.action(Screen::Launcher, "s"), Some(Action::Settings));
        assert_eq!(keys.action(Screen::Launcher, "/"), Some(Action::Search));
        assert_eq!(keys.action(Screen::Tree, "esc"), Some(Action::Back));
        assert_eq!(keys.action(Screen::Detail, "d"), Some(Action::Done));
        assert_eq!(keys.action(Screen::Settings, "d"), Some(Action::Doctor));
        assert_eq!(
            keys.action(Screen::Confirm, "k"),
            Some(Action::KeepWorktree)
        );
        // `k` moves up everywhere else.
        assert_eq!(keys.action(Screen::Tree, "k"), Some(Action::Up));
    }

    #[test]
    fn no_default_conflicts() {
        let keys = Keys::default();
        for action in Action::ALL {
            for key in keys.keys(action) {
                assert_eq!(keys.conflict(action, key), None, "{action:?} {key}");
            }
        }
    }

    #[test]
    fn key_names_normalize_terminal_events() {
        assert_eq!(
            key_name(&key(KeyCode::Enter, KeyModifiers::NONE)).unwrap(),
            "enter"
        );
        assert_eq!(
            key_name(&key(KeyCode::BackTab, KeyModifiers::SHIFT)).unwrap(),
            "shift+tab"
        );
        assert_eq!(
            key_name(&key(KeyCode::Char(' '), KeyModifiers::NONE)).unwrap(),
            "space"
        );
        assert_eq!(
            key_name(&key(KeyCode::Char('x'), KeyModifiers::CONTROL)).unwrap(),
            "ctrl+x"
        );
        assert_eq!(
            key_name(&key(KeyCode::Char('N'), KeyModifiers::SHIFT)).unwrap(),
            "shift+n"
        );
        assert_eq!(
            key_name(&key(KeyCode::Char('?'), KeyModifiers::SHIFT)).unwrap(),
            "?"
        );
    }

    #[test]
    fn rebinding_refuses_conflicts_and_digits() {
        let mut keys = Keys::default();
        assert!(
            keys.set(Action::Board, "n")
                .unwrap_err()
                .contains("new project")
        );
        assert!(keys.set(Action::Board, "3").is_err());
        // `d` is free on the launcher because done and doctor live elsewhere.
        keys.set(Action::Board, "x").unwrap();
        assert_eq!(keys.action(Screen::Launcher, "x"), Some(Action::Board));
        assert_eq!(keys.action(Screen::Launcher, "b"), None);
        keys.reset(Action::Board);
        assert_eq!(keys.action(Screen::Launcher, "b"), Some(Action::Board));
    }

    #[test]
    fn file_round_trip_keeps_only_changed_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.keys.set(Action::Board, "w").unwrap();
        config.view.dock = Dock::Left;
        save(dir.path(), &config).unwrap();
        let text = std::fs::read_to_string(path(dir.path())).unwrap();
        assert!(text.contains("board = \"w\""), "{text}");
        assert!(!text.contains("new_project"), "{text}");
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.keys.primary(Action::Board), "w");
        assert_eq!(loaded.view.dock, Dock::Left);
        assert!(loaded.problems.is_empty());
    }

    #[test]
    fn bad_bindings_are_reported_and_defaults_kept() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            path(dir.path()),
            "[keys]\nboard = \"n\"\nmerge = \"hyper+q\"\nsettings = [\"s\", \"ctrl+o\"]\n",
        )
        .unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.keys.primary(Action::Board), "b");
        assert_eq!(loaded.keys.primary(Action::Merge), "m");
        assert_eq!(loaded.keys.keys(Action::Settings), ["s", "ctrl+o"]);
        assert_eq!(loaded.problems.len(), 2, "{:?}", loaded.problems);
    }

    #[test]
    fn a_file_that_does_not_parse_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(path(dir.path()), "[keys\n").unwrap();
        assert!(load(dir.path()).is_err());
    }
}

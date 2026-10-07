//! Every key the menu and the tree answer, as named actions per context,
//! with the user's overrides from `<config dir>/keymap.toml`:
//!
//! ```toml
//! [threads]
//! resolve = "X"              # one key
//! open_pr = ["o", "ctrl+o"]  # several
//! [global]
//! quit = ["esc"]             # `q` no longer quits
//! ```
//!
//! A section names a context, a key an action, a value one key or a list
//! (`[]` unbinds). Keys are written `q`, `S` (case counts), `enter`, `esc`,
//! `tab`, `shift+tab`, `space`, `backspace`, `up`, `down`, `left`, `right`,
//! `pgup`, `pgdn`, `home`, `end`, `f1`..`f12`, `ctrl+x`, `alt+x`. A list
//! context (threads, tasks, ...) also answers the `global` keys it does not
//! bind itself; a modal context (detail, confirm, input, choice, picker,
//! help) answers only its own. Two actions on one key in one context are
//! refused: that context keeps its defaults and the error is shown.

use std::collections::BTreeMap;
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Context {
    Global,
    Threads,
    Tasks,
    Inbox,
    Routines,
    Settings,
    Memory,
    Detail,
    Confirm,
    /// Typing a value or a filter: letters are text, not keys.
    Input,
    /// A list of options: pick one, or check several.
    Choice,
    Picker,
    Help,
}

impl Context {
    pub const ALL: [Context; 13] = [
        Context::Global,
        Context::Threads,
        Context::Tasks,
        Context::Inbox,
        Context::Routines,
        Context::Settings,
        Context::Memory,
        Context::Detail,
        Context::Confirm,
        Context::Input,
        Context::Choice,
        Context::Picker,
        Context::Help,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Context::Global => "global",
            Context::Threads => "threads",
            Context::Tasks => "tasks",
            Context::Inbox => "inbox",
            Context::Routines => "routines",
            Context::Settings => "settings",
            Context::Memory => "memory",
            Context::Detail => "detail",
            Context::Confirm => "confirm",
            Context::Input => "input",
            Context::Choice => "choice",
            Context::Picker => "picker",
            Context::Help => "help",
        }
    }

    /// List contexts fall back to `global`; modal ones stand alone.
    fn falls_back(self) -> bool {
        matches!(
            self,
            Context::Threads
                | Context::Tasks
                | Context::Inbox
                | Context::Routines
                | Context::Settings
                | Context::Memory
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    NextSection,
    PrevSection,
    Down,
    Up,
    PageDown,
    PageUp,
    Top,
    Bottom,
    Filter,
    SwitchProject,
    Help,
    Jump,
    Detail,
    ForwardNext,
    Stop,
    Ack,
    Restart,
    Resolve,
    OpenPr,
    Coordinator,
    Sweep,
    ToggleResolved,
    Delegate,
    TaskDone,
    TaskDrop,
    InboxDone,
    RoutineToggle,
    Edit,
    NewProfile,
    DeleteProfile,
    Yolo,
    PauseResume,
    Archive,
    DeleteProject,
    Back,
    Open,
    CopyPath,
    Yes,
    No,
    Submit,
    Cancel,
    Erase,
    Check,
    PrevChoice,
    NextChoice,
    NextField,
    PrevField,
}

/// One default binding: context, action, its config name, keys, help text.
type Default = (
    Context,
    Action,
    &'static str,
    &'static [&'static str],
    &'static str,
);

use Action as A;
use Context as C;

/// The defaults, in the order help shows them.
const DEFAULTS: &[Default] = &[
    (C::Global, A::Down, "down", &["down", "j"], "down"),
    (C::Global, A::Up, "up", &["up", "k"], "up"),
    (
        C::Global,
        A::NextSection,
        "next_section",
        &["tab", "right"],
        "next section",
    ),
    (
        C::Global,
        A::PrevSection,
        "prev_section",
        &["shift+tab", "left"],
        "previous section",
    ),
    (C::Global, A::PageDown, "page_down", &["pgdn"], "page down"),
    (C::Global, A::PageUp, "page_up", &["pgup"], "page up"),
    (C::Global, A::Top, "top", &["home", "g"], "first row"),
    (C::Global, A::Bottom, "bottom", &["end", "G"], "last row"),
    (C::Global, A::Filter, "filter", &["/"], "filter rows"),
    (
        C::Global,
        A::SwitchProject,
        "switch_project",
        &["P"],
        "switch project",
    ),
    (C::Global, A::Help, "help", &["?"], "all keys"),
    (C::Global, A::Quit, "quit", &["esc", "q", "ctrl+c"], "close"),
    (C::Threads, A::Jump, "jump", &["enter"], "go to its pane"),
    (C::Threads, A::Detail, "detail", &["i"], "report"),
    (
        C::Threads,
        A::ForwardNext,
        "forward_next",
        &["1", "2", "3", "4", "5", "6", "7", "8", "9"],
        "send Next line N",
    ),
    (C::Threads, A::Ack, "ack", &["a"], "mark seen"),
    (C::Threads, A::OpenPr, "open_pr", &["o"], "open PR"),
    (C::Threads, A::Stop, "stop", &["s"], "interrupt (Esc)"),
    (C::Threads, A::Restart, "restart", &["r"], "restart"),
    (C::Threads, A::Resolve, "resolve", &["x"], "resolve"),
    (
        C::Threads,
        A::Coordinator,
        "coordinator",
        &["c"],
        "go to coordinator",
    ),
    (C::Threads, A::Sweep, "sweep", &["S"], "clean up leftovers"),
    (
        C::Threads,
        A::ToggleResolved,
        "toggle_resolved",
        &["."],
        "show/hide resolved",
    ),
    (C::Tasks, A::Jump, "jump", &["enter"], "go to its thread"),
    (C::Tasks, A::Detail, "notes", &["i"], "notes"),
    (C::Tasks, A::Delegate, "delegate", &["d"], "delegate"),
    (C::Tasks, A::TaskDone, "done", &["m"], "mark done"),
    (C::Tasks, A::TaskDrop, "drop", &["D"], "drop"),
    (C::Inbox, A::Detail, "detail", &["enter"], "read"),
    (C::Inbox, A::InboxDone, "done", &["a"], "mark handled"),
    (
        C::Routines,
        A::RoutineToggle,
        "toggle",
        &["enter"],
        "on/off",
    ),
    (C::Routines, A::Detail, "prompt", &["i"], "prompt"),
    (C::Settings, A::Edit, "edit", &["enter"], "change"),
    (
        C::Settings,
        A::NewProfile,
        "new_profile",
        &["n"],
        "new profile",
    ),
    (
        C::Settings,
        A::DeleteProfile,
        "delete_profile",
        &["d"],
        "delete profile",
    ),
    (C::Settings, A::Yolo, "yolo", &["Y"], "yolo mode"),
    (
        C::Settings,
        A::PauseResume,
        "pause",
        &["p"],
        "pause/resume project",
    ),
    (
        C::Settings,
        A::Archive,
        "archive",
        &["A"],
        "archive project",
    ),
    (
        C::Settings,
        A::DeleteProject,
        "delete",
        &["X"],
        "delete project",
    ),
    (C::Memory, A::Detail, "read", &["enter"], "read"),
    (C::Detail, A::Back, "back", &["esc", "q"], "back"),
    (C::Detail, A::Down, "down", &["down", "j"], "down"),
    (C::Detail, A::Up, "up", &["up", "k"], "up"),
    (
        C::Detail,
        A::PageDown,
        "page_down",
        &["pgdn", "space"],
        "page down",
    ),
    (C::Detail, A::PageUp, "page_up", &["pgup"], "page up"),
    (C::Detail, A::Open, "open", &["enter"], "open file"),
    (C::Detail, A::CopyPath, "copy_path", &["y"], "copy path"),
    (C::Confirm, A::Yes, "yes", &["y", "Y"], "yes"),
    (C::Confirm, A::No, "no", &["n", "N", "esc", "enter"], "no"),
    (C::Input, A::Submit, "submit", &["enter"], "save"),
    (C::Input, A::Cancel, "cancel", &["esc", "ctrl+c"], "cancel"),
    (C::Input, A::Erase, "erase", &["backspace"], "erase"),
    (
        C::Input,
        A::NextField,
        "next_field",
        &["tab", "down"],
        "next field",
    ),
    (
        C::Input,
        A::PrevField,
        "prev_field",
        &["shift+tab", "up"],
        "previous field",
    ),
    (
        C::Input,
        A::PrevChoice,
        "prev_choice",
        &["left"],
        "previous option",
    ),
    (
        C::Input,
        A::NextChoice,
        "next_choice",
        &["right"],
        "next option",
    ),
    (C::Choice, A::Down, "down", &["down", "j"], "down"),
    (C::Choice, A::Up, "up", &["up", "k"], "up"),
    (C::Choice, A::Check, "check", &["space"], "check"),
    (C::Choice, A::Submit, "submit", &["enter"], "ok"),
    (C::Choice, A::Cancel, "cancel", &["esc", "q"], "cancel"),
    (C::Picker, A::Down, "down", &["down", "j"], "down"),
    (C::Picker, A::Up, "up", &["up", "k"], "up"),
    (C::Picker, A::Submit, "submit", &["enter"], "switch"),
    (C::Picker, A::Filter, "filter", &["/"], "filter"),
    (
        C::Picker,
        A::Cancel,
        "cancel",
        &["esc", "ctrl+c"],
        "clear filter / close",
    ),
    (C::Help, A::Back, "back", &["esc", "q", "?"], "back"),
    (C::Help, A::Down, "down", &["down", "j"], "down"),
    (C::Help, A::Up, "up", &["up", "k"], "up"),
];

/// A key as the user writes it: a code and its modifiers. Shift is part of
/// a character (`S`), never a separate modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key {
    code: KeyCode,
    mods: KeyModifiers,
}

impl Key {
    pub fn from_event(event: &KeyEvent) -> Key {
        let mut mods = event.modifiers & (KeyModifiers::CONTROL | KeyModifiers::ALT);
        let code = match event.code {
            KeyCode::BackTab => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            KeyCode::Tab if event.modifiers.contains(KeyModifiers::SHIFT) => {
                mods |= KeyModifiers::SHIFT;
                KeyCode::Tab
            }
            // Terminals report ctrl+letter in either case.
            KeyCode::Char(c) if mods.contains(KeyModifiers::CONTROL) => {
                KeyCode::Char(c.to_ascii_lowercase())
            }
            code => code,
        };
        Key { code, mods }
    }

    pub fn parse(text: &str) -> Option<Key> {
        let mut mods = KeyModifiers::NONE;
        let mut rest = text;
        loop {
            if let Some(r) = rest.strip_prefix("ctrl+") {
                mods |= KeyModifiers::CONTROL;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("alt+") {
                mods |= KeyModifiers::ALT;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("shift+") {
                mods |= KeyModifiers::SHIFT;
                rest = r;
            } else {
                break;
            }
        }
        let code = match rest {
            "enter" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "pgup" => KeyCode::PageUp,
            "pgdn" => KeyCode::PageDown,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "delete" => KeyCode::Delete,
            f if f.len() > 1 && f.starts_with('f') => KeyCode::F(f[1..].parse().ok()?),
            c if c.chars().count() == 1 => {
                let c = c.chars().next()?;
                KeyCode::Char(if mods.contains(KeyModifiers::CONTROL) {
                    c.to_ascii_lowercase()
                } else {
                    c
                })
            }
            _ => return None,
        };
        // Shift only qualifies tab; a shifted letter is written as the letter.
        if code != KeyCode::Tab {
            mods.remove(KeyModifiers::SHIFT);
        }
        Some(Key { code, mods })
    }

    /// How help writes the key: `enter`, `S`, `ctrl+c`, `↑`.
    pub fn label(&self) -> String {
        let base = match self.code {
            KeyCode::Enter => "↵".to_string(),
            KeyCode::Esc => "esc".into(),
            KeyCode::Tab => "tab".into(),
            KeyCode::Char(' ') => "space".into(),
            KeyCode::Backspace => "⌫".into(),
            KeyCode::Up => "↑".into(),
            KeyCode::Down => "↓".into(),
            KeyCode::Left => "←".into(),
            KeyCode::Right => "→".into(),
            KeyCode::PageUp => "pgup".into(),
            KeyCode::PageDown => "pgdn".into(),
            KeyCode::Home => "home".into(),
            KeyCode::End => "end".into(),
            KeyCode::Delete => "del".into(),
            KeyCode::F(n) => format!("f{n}"),
            KeyCode::Char(c) => c.to_string(),
            _ => "?".into(),
        };
        let mut label = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            label.push_str("ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            label.push_str("alt+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            label.push_str("shift+");
        }
        label + &base
    }

    /// The character typed, for input contexts.
    pub fn char(&self) -> Option<char> {
        match self.code {
            KeyCode::Char(c) if self.mods.is_empty() => Some(c),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Binding {
    pub action: Action,
    pub name: &'static str,
    pub keys: Vec<Key>,
    pub desc: &'static str,
}

#[derive(Debug, Clone)]
pub struct Keymap {
    contexts: BTreeMap<Context, Vec<Binding>>,
    /// Problems found in keymap.toml; the menu shows them once.
    pub errors: Vec<String>,
}

impl Keymap {
    pub fn defaults() -> Keymap {
        let mut contexts: BTreeMap<Context, Vec<Binding>> = BTreeMap::new();
        for (context, action, name, keys, desc) in DEFAULTS {
            contexts.entry(*context).or_default().push(Binding {
                action: *action,
                name,
                keys: keys
                    .iter()
                    .map(|k| Key::parse(k).expect("default key parses"))
                    .collect(),
                desc,
            });
        }
        Keymap {
            contexts,
            errors: Vec::new(),
        }
    }

    /// Defaults with `<config_dir>/keymap.toml` over them. A missing file
    /// is the defaults; a broken one is the defaults and an error.
    pub fn load(config_dir: &Path) -> Keymap {
        let path = config_dir.join("keymap.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => Keymap::from_toml(&text),
            Err(_) => Keymap::defaults(),
        }
    }

    pub fn from_toml(text: &str) -> Keymap {
        let mut keymap = Keymap::defaults();
        let table: toml::Table = match toml::from_str(text) {
            Ok(table) => table,
            Err(error) => {
                keymap
                    .errors
                    .push(format!("keymap.toml: {}", error.message()));
                return keymap;
            }
        };
        for (section, value) in table {
            let Some(context) = Context::ALL.iter().find(|c| c.name() == section).copied() else {
                keymap
                    .errors
                    .push(format!("keymap.toml: no context `{section}`"));
                continue;
            };
            let Some(entries) = value.as_table() else {
                keymap
                    .errors
                    .push(format!("keymap.toml: [{section}] is not a table"));
                continue;
            };
            let before = keymap.contexts.get(&context).cloned().unwrap_or_default();
            let mut failed = false;
            for (name, keys) in entries {
                let keys: Vec<&str> = match keys {
                    toml::Value::String(key) => vec![key.as_str()],
                    toml::Value::Array(list) => list.iter().filter_map(|v| v.as_str()).collect(),
                    _ => {
                        keymap.errors.push(format!(
                            "keymap.toml: [{section}] {name}: give a key or a list"
                        ));
                        failed = true;
                        continue;
                    }
                };
                let parsed: Option<Vec<Key>> = keys.iter().map(|k| Key::parse(k)).collect();
                let Some(parsed) = parsed else {
                    keymap.errors.push(format!(
                        "keymap.toml: [{section}] {name}: unknown key in {keys:?}"
                    ));
                    failed = true;
                    continue;
                };
                match keymap
                    .contexts
                    .get_mut(&context)
                    .and_then(|bindings| bindings.iter_mut().find(|b| b.name == name.as_str()))
                {
                    Some(binding) => binding.keys = parsed,
                    None => {
                        keymap
                            .errors
                            .push(format!("keymap.toml: [{section}] has no action `{name}`"));
                        failed = true;
                    }
                }
            }
            if let Some(conflict) = keymap.conflict(context) {
                keymap.errors.push(format!(
                    "keymap.toml: [{section}] {conflict}; using the defaults there"
                ));
                failed = true;
            }
            if failed {
                keymap.contexts.insert(context, before);
            }
        }
        keymap
    }

    /// Two actions on one key in `context`, or in a list context and the
    /// `global` keys it falls back to.
    fn conflict(&self, context: Context) -> Option<String> {
        let mine = self.contexts.get(&context)?;
        let mut seen: Vec<(Key, &str)> = Vec::new();
        for binding in mine {
            for key in &binding.keys {
                if let Some((_, other)) = seen.iter().find(|(k, _)| k == key) {
                    return Some(format!(
                        "`{}` is bound to both {other} and {}",
                        key.label(),
                        binding.name
                    ));
                }
                seen.push((*key, binding.name));
            }
        }
        None
    }

    /// The action `event` means in `context`.
    pub fn action(&self, context: Context, event: &KeyEvent) -> Option<Action> {
        let key = Key::from_event(event);
        let find = |context: Context| {
            self.contexts
                .get(&context)?
                .iter()
                .find(|b| b.keys.contains(&key))
                .map(|b| b.action)
        };
        find(context).or_else(|| {
            context
                .falls_back()
                .then(|| find(Context::Global))
                .flatten()
        })
    }

    /// The bindings `context` answers, its own first, as help shows them.
    pub fn bindings(&self, context: Context) -> Vec<&Binding> {
        let mut out: Vec<&Binding> = self
            .contexts
            .get(&context)
            .map(|b| b.iter().filter(|b| !b.keys.is_empty()).collect())
            .unwrap_or_default();
        if context.falls_back()
            && let Some(global) = self.contexts.get(&Context::Global)
        {
            for binding in global.iter().filter(|b| !b.keys.is_empty()) {
                if !out.iter().any(|b| b.action == binding.action) {
                    out.push(binding);
                }
            }
        }
        out
    }

    /// The first key bound to `action` in `context`, for prompts.
    pub fn key_for(&self, context: Context, action: Action) -> String {
        self.bindings(context)
            .into_iter()
            .find(|b| b.action == action)
            .and_then(|b| b.keys.first())
            .map(Key::label)
            .unwrap_or_default()
    }

    /// The help bar: `key desc` pairs that fit `width`, then `? help`. Keys
    /// with more than one binding show the first one.
    pub fn hint(&self, context: Context, width: usize) -> String {
        // Only the lists answer the help key.
        let help = if context.falls_back() {
            format!("{} help", self.key_for(Context::Global, Action::Help))
        } else {
            String::new()
        };
        let mut line = String::new();
        let skip = [
            Action::Down,
            Action::Up,
            Action::Help,
            Action::PageDown,
            Action::PageUp,
            Action::Top,
            Action::Bottom,
        ];
        for binding in self.bindings(context) {
            if skip.contains(&binding.action) {
                continue;
            }
            let keys = if binding.action == Action::ForwardNext {
                "1-9".to_string()
            } else {
                binding.keys[0].label()
            };
            let item = format!("{keys} {}", binding.desc);
            let room = width.saturating_sub(help.chars().count() + 4);
            if line.chars().count() + item.chars().count() + 2 > room {
                line.push_str("  …");
                break;
            }
            if !line.is_empty() {
                line.push_str("  ");
            }
            line.push_str(&item);
        }
        if !context.falls_back() {
            line
        } else if line.is_empty() {
            help
        } else {
            format!("{line}  {help}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn keys_parse_and_print_the_way_the_user_writes_them() {
        for (text, label) in [
            ("q", "q"),
            ("S", "S"),
            ("enter", "↵"),
            ("ctrl+c", "ctrl+c"),
            ("shift+tab", "shift+tab"),
            ("pgdn", "pgdn"),
            ("f5", "f5"),
        ] {
            assert_eq!(Key::parse(text).unwrap().label(), label, "{text}");
        }
        assert!(Key::parse("nope").is_none());
        // A shifted letter arrives as the letter with SHIFT; it is the letter.
        assert_eq!(
            Key::from_event(&event(KeyCode::Char('S'), KeyModifiers::SHIFT)),
            Key::parse("S").unwrap()
        );
        assert_eq!(
            Key::from_event(&event(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Key::parse("shift+tab").unwrap()
        );
    }

    #[test]
    fn list_contexts_fall_back_to_global_and_modal_ones_do_not() {
        let keymap = Keymap::defaults();
        let q = event(KeyCode::Char('q'), KeyModifiers::NONE);
        assert_eq!(keymap.action(Context::Threads, &q), Some(Action::Quit));
        assert_eq!(keymap.action(Context::Input, &q), None);
        assert_eq!(keymap.action(Context::Detail, &q), Some(Action::Back));
        let x = event(KeyCode::Char('x'), KeyModifiers::NONE);
        assert_eq!(keymap.action(Context::Threads, &x), Some(Action::Resolve));
        assert_eq!(keymap.action(Context::Tasks, &x), None);
    }

    #[test]
    fn the_defaults_have_no_conflicts() {
        let keymap = Keymap::defaults();
        for context in Context::ALL {
            assert_eq!(keymap.conflict(context), None, "{}", context.name());
            if context.falls_back() {
                let global = &keymap.contexts[&Context::Global];
                for binding in keymap.contexts.get(&context).into_iter().flatten() {
                    for key in &binding.keys {
                        if let Some(g) = global.iter().find(|g| g.keys.contains(key)) {
                            panic!(
                                "{}: `{}` shadows global {}",
                                context.name(),
                                key.label(),
                                g.name
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn overrides_rebind_unbind_and_refuse_conflicts() {
        let keymap = Keymap::from_toml(
            "[threads]\nresolve = \"X\"\nopen_pr = [\"o\", \"ctrl+o\"]\n[global]\nquit = [\"esc\"]\n",
        );
        assert!(keymap.errors.is_empty(), "{:?}", keymap.errors);
        let press = |c| event(KeyCode::Char(c), KeyModifiers::NONE);
        assert_eq!(
            keymap.action(Context::Threads, &press('X')),
            Some(Action::Resolve)
        );
        assert_eq!(keymap.action(Context::Threads, &press('x')), None);
        assert_eq!(keymap.action(Context::Threads, &press('q')), None);
        assert_eq!(
            keymap.action(
                Context::Threads,
                &event(KeyCode::Char('o'), KeyModifiers::CONTROL)
            ),
            Some(Action::OpenPr)
        );

        let broken = Keymap::from_toml(
            "[threads]\nresolve = \"a\"\n[nope]\nx = \"y\"\n[tasks]\nfly = \"f\"\n",
        );
        assert_eq!(broken.errors.len(), 3, "{:?}", broken.errors);
        assert!(
            broken.errors[0].contains("ack and resolve")
                || broken.errors.iter().any(|e| e.contains("bound to both"))
        );
        // The refused context keeps its defaults.
        assert_eq!(
            broken.action(
                Context::Threads,
                &event(KeyCode::Char('x'), KeyModifiers::NONE)
            ),
            Some(Action::Resolve)
        );
        assert_eq!(Keymap::from_toml("not toml [").errors.len(), 1);
    }

    #[test]
    fn the_help_bar_uses_the_users_keys_and_fits_the_width() {
        let keymap = Keymap::from_toml("[threads]\nresolve = \"X\"\n");
        let wide = keymap.hint(Context::Threads, 200);
        assert!(
            wide.contains("X resolve") && wide.ends_with("? help"),
            "{wide}"
        );
        let narrow = keymap.hint(Context::Threads, 40);
        assert!(narrow.chars().count() <= 40, "{narrow}");
        assert!(
            narrow.contains('…') && narrow.ends_with("? help"),
            "{narrow}"
        );
    }
}

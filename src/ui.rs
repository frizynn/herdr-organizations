//! The Organizations terminal UI: one process that exists only while its
//! popup (or the optional dock) is open. Launcher, New, project tree, board,
//! thread detail, merge confirm and settings are screens of the same
//! process; Herdr allows one popup at a time, so dialogs are drawn here.
//!
//! The view reads the ticker's state file and redraws on a key or when that
//! file changes. It never polls Herdr; it calls Herdr only for an action the
//! user asked for (`ui_ops`).

use std::collections::BTreeSet;

use anyhow::Result;
use crossterm::event::{Event, KeyEvent, KeyEventKind, MouseButton, MouseEventKind};
use crossterm::style::Color;
use serde::{Deserialize, Serialize};

use crate::organizations::ROOT_ID;
use crate::paths::Ctx;
use crate::state::{self, Counts, Node, ProjectView, Snapshot, Status};
use crate::term::{
    self, BLUE, BOLD, CYAN, DIM, Frame, GREEN, GREY, HEAD, Input, PRIMARY, RED, SELECTED, Style,
    Terminal, YELLOW,
};
use crate::thread::NodeRole;
use crate::tui_config::{self, Action, Config, Dock as DockSide, Notify, Resolved, Screen};
use crate::ui_ops;

pub const HARNESSES: [&str; 3] = ["claude", "codex", "opencode"];
const EFFORTS: [&str; 4] = ["", "low", "medium", "high"];
const HANDOFF: &str = "ui-handoff";

/// How the popup should open, written by the action that opened it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Handoff {
    /// `launcher`, `new`, `tree`, `pick` or `adopt`.
    pub screen: String,
    pub slug: String,
    pub node: String,
    /// For `pick`: the command run on the chosen project.
    pub command: String,
    pub pane_id: String,
    pub workspace_label: String,
    pub workspace_cwd: String,
}

/// The invocation's correlation id when Herdr gave a safe one: the action
/// and the popup it opens share it, so two invocations never swap handoffs.
fn correlation(ctx: &Ctx) -> Option<String> {
    let context: serde_json::Value = ctx
        .env
        .var("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|json| serde_json::from_str(json).ok())?;
    let id = context["correlation_id"].as_str()?;
    (!id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then(|| id.to_string())
}

fn handoff_path(dir: &str, correlation: Option<&str>) -> std::path::PathBuf {
    let name = match correlation {
        Some(id) => format!("{HANDOFF}-{id}.json"),
        None => format!("{HANDOFF}.json"),
    };
    std::path::Path::new(dir).join(name)
}

pub fn write_handoff(ctx: &Ctx, handoff: &Handoff) -> Result<()> {
    let Some(dir) = ctx.env.var("HERDR_PLUGIN_STATE_DIR") else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    crate::project::write_json(&handoff_path(dir, correlation(ctx).as_deref()), handoff)
}

/// Reads and removes this popup's handoff; a stale one (over 30 s) is ignored.
pub fn take_handoff(ctx: &Ctx) -> Handoff {
    let Some(dir) = ctx.env.var("HERDR_PLUGIN_STATE_DIR") else {
        return Handoff::default();
    };
    let correlation = correlation(ctx);
    for path in [
        handoff_path(dir, correlation.as_deref()),
        handoff_path(dir, None),
    ] {
        let fresh = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|age| age.as_secs() < 30);
        let Some(fresh) = fresh else {
            continue;
        };
        let handoff = crate::project::read_json(&path).filter(|_| fresh);
        let _ = std::fs::remove_file(&path);
        if let Some(handoff) = handoff {
            return handoff;
        }
    }
    Handoff::default()
}

// ------------------------------------------------------------------ model

#[derive(Debug, Clone, PartialEq)]
enum Target {
    NeedsYou(String, String),
    Project(String),
    Node(String, String),
    NewCoordinator(String),
    NewThread(String),
    NewProject,
    Adopt,
    Resolved(String, String),
    Setting(usize),
    Key(Action),
    Inbox(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Project,
    Coordinator,
    Thread,
    Workspace,
}

impl Kind {
    const ALL: [Kind; 4] = [
        Kind::Project,
        Kind::Coordinator,
        Kind::Thread,
        Kind::Workspace,
    ];

    fn label(self) -> (&'static str, &'static str) {
        match self {
            Kind::Project => ("Project", "coordinator + threads"),
            Kind::Coordinator => ("Coordinator", "inside a project"),
            Kind::Thread => ("Thread", "one agent, one task"),
            Kind::Workspace => ("Workspace", "folder, no coordinator"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Field {
    Text {
        label: &'static str,
        value: String,
        hint: &'static str,
    },
    Choice {
        label: &'static str,
        options: Vec<String>,
        shown: Vec<String>,
        at: usize,
    },
}

impl Field {
    fn text(label: &'static str, value: &str, hint: &'static str) -> Field {
        Field::Text {
            label,
            value: value.into(),
            hint,
        }
    }

    fn choice(label: &'static str, options: Vec<String>, shown: Vec<String>, at: usize) -> Field {
        Field::Choice {
            label,
            options,
            shown,
            at,
        }
    }

    fn value(&self) -> String {
        match self {
            Field::Text { value, .. } => value.clone(),
            Field::Choice { options, at, .. } => options.get(*at).cloned().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Form {
    kind: Kind,
    fields: Vec<Field>,
    focus: usize,
    adopt_pane: String,
    adopt_cwd: String,
}

#[derive(Debug, Clone, PartialEq)]
enum View {
    Launcher {
        sel: usize,
    },
    Tree {
        slug: String,
        sel: usize,
        folded: BTreeSet<String>,
    },
    Board {
        slug: String,
        col: usize,
        row: usize,
        filter: usize,
    },
    Detail {
        slug: String,
        id: String,
        sel: usize,
        output: Vec<String>,
    },
    Settings {
        sel: usize,
        capture: bool,
    },
    Form(Form),
    Confirm {
        slug: String,
        id: String,
    },
    Reply {
        slug: String,
        id: String,
        text: String,
    },
    Search {
        text: String,
    },
    Help,
    Dock {
        sel: usize,
    },
}

impl View {
    fn screen(&self) -> Screen {
        match self {
            View::Launcher { .. } | View::Search { .. } => Screen::Launcher,
            View::Tree { .. } => Screen::Tree,
            View::Board { .. } => Screen::Board,
            View::Detail { .. } => Screen::Detail,
            View::Settings { .. } => Screen::Settings,
            View::Form(_) | View::Reply { .. } => Screen::Form,
            View::Confirm { .. } => Screen::Confirm,
            View::Help | View::Dock { .. } => Screen::Dock,
        }
    }

    fn overlay(&self) -> bool {
        matches!(self, View::Confirm { .. } | View::Reply { .. } | View::Help)
    }
}

pub enum Mode {
    Popup,
    Dock { slug: String, workspace: String },
}

struct App<'a> {
    ctx: &'a Ctx<'a>,
    mode: Mode,
    snap: Snapshot,
    config: Config,
    stack: Vec<View>,
    message: String,
    close: bool,
    search: String,
    pick: String,
    current_workspace: (String, String),
    /// Rows of the last frame that hold a selectable target: (row, index).
    rows: Vec<(usize, usize)>,
    last_click: Option<(usize, std::time::Instant)>,
}

// ------------------------------------------------------------------ lines

struct Span {
    /// Negative: right-aligned, -1 ends on the last column.
    col: isize,
    text: String,
    style: Style,
}

#[derive(Default)]
struct Line {
    spans: Vec<Span>,
    target: Option<Target>,
}

impl Line {
    fn new() -> Line {
        Line::default()
    }

    fn at(mut self, col: isize, text: impl Into<String>, style: Style) -> Line {
        self.spans.push(Span {
            col,
            text: text.into(),
            style,
        });
        self
    }

    fn target(mut self, target: Target) -> Line {
        self.target = Some(target);
        self
    }
}

fn draw_line(frame: &mut Frame, y: usize, line: &Line, selected: bool) {
    if selected {
        frame.select_row(y);
        frame.put(1, y, "›", BOLD);
    }
    for span in &line.spans {
        let style = if selected && span.style == Style::PLAIN && span.col >= 0 {
            BOLD
        } else {
            span.style
        };
        if span.col < 0 {
            let right = (frame.width as isize + span.col + 1).max(0) as usize;
            frame.put_right(right, y, &span.text, style);
        } else {
            frame.put(span.col as usize, y, &span.text, style);
        }
    }
}

/// Draws `lines` into rows `top..top+height`, scrolled so the selected
/// target stays visible. Returns the (row, target index) pairs drawn.
fn draw_list(
    frame: &mut Frame,
    top: usize,
    height: usize,
    lines: &[Line],
    sel: usize,
) -> Vec<(usize, usize)> {
    let mut target_line = None;
    let mut index = 0;
    for (i, line) in lines.iter().enumerate() {
        if line.target.is_some() {
            if index == sel {
                target_line = Some(i);
            }
            index += 1;
        }
    }
    let start = match target_line {
        Some(i) if i >= height => i + 1 - height,
        _ => 0,
    };
    let mut rows = Vec::new();
    let mut index = lines[..start].iter().filter(|l| l.target.is_some()).count();
    for (offset, line) in lines.iter().skip(start).take(height).enumerate() {
        let selected = line.target.is_some() && Some(start + offset) == target_line;
        draw_line(frame, top + offset, line, selected);
        if line.target.is_some() {
            rows.push((top + offset, index));
            index += 1;
        }
    }
    rows
}

fn targets(lines: &[Line]) -> Vec<Target> {
    lines.iter().filter_map(|l| l.target.clone()).collect()
}

fn dot(status: Status) -> (&'static str, Style) {
    match status {
        Status::Need => ("●", RED),
        Status::Work => ("●", YELLOW),
        Status::Review => ("●", BLUE),
        Status::Done => ("✓", GREEN),
        Status::Idle => ("○", GREY),
    }
}

fn status_style(status: Status) -> Style {
    dot(status).1
}

/// `●2 ●3 ●1 ✓1` style counts, one colour each, starting at `col`.
fn counts_spans(mut line: Line, col: isize, counts: &Counts) -> Line {
    let mut x = col;
    for status in [
        Status::Need,
        Status::Work,
        Status::Review,
        Status::Done,
        Status::Idle,
    ] {
        let n = counts.get(status);
        if n == 0 {
            continue;
        }
        let (glyph, style) = dot(status);
        let text = format!("{glyph}{n}");
        let w = text.chars().count() as isize + 2;
        line = line.at(x, text, style);
        x += w;
    }
    line
}

fn counts_header(counts: &Counts) -> Line {
    let mut line = Line::new();
    let mut x = 1;
    for (status, word) in [
        (Status::Need, "need you"),
        (Status::Work, "working"),
        (Status::Review, "to review"),
        (Status::Done, "resolved"),
        (Status::Idle, "idle"),
    ] {
        let n = counts.get(status);
        if n == 0 && matches!(status, Status::Idle | Status::Done) {
            continue;
        }
        let (glyph, style) = dot(status);
        line = line.at(x, glyph, style);
        let text = format!("{n} {word}");
        line = line.at(x + 2, text.clone(), Style::PLAIN);
        x += 2 + text.chars().count() as isize + 3;
    }
    line
}

fn coordinator_name(project: &ProjectView, id: &str) -> String {
    match project.node(id) {
        Some(n) if n.id == ROOT_ID => "Coordinator".into(),
        Some(n) => n.title.clone(),
        None => String::new(),
    }
}

fn profile(node: &Node) -> String {
    let mut parts = vec![node.harness.clone()];
    let model = [node.model.as_str(), node.effort.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    if !model.is_empty() {
        parts.push(model);
    }
    parts.retain(|p| !p.is_empty());
    parts.join(" · ")
}

fn visible_threads<'p>(
    project: &'p ProjectView,
    coordinator: &str,
    resolved: Resolved,
) -> (Vec<&'p Node>, usize) {
    let all: Vec<&Node> = project
        .threads
        .iter()
        .filter(|t| t.coordinator == coordinator)
        .collect();
    let done = all.iter().filter(|t| t.status == Status::Done).count();
    if resolved == Resolved::Count && done > 1 {
        (
            all.into_iter()
                .filter(|t| t.status != Status::Done)
                .collect(),
            done,
        )
    } else {
        (all, 0)
    }
}

impl<'a> App<'a> {
    fn keys(&self) -> &tui_config::Keys {
        &self.config.keys
    }

    fn hint(&self, pairs: &[(Action, &'static str)]) -> Vec<(String, &'static str)> {
        pairs
            .iter()
            .map(|(a, words)| (self.key_label(*a), *words))
            .collect()
    }

    fn key_label(&self, action: Action) -> String {
        match self.keys().primary(action) {
            "enter" => "↵".into(),
            "up" => "↑".into(),
            "down" => "↓".into(),
            "left" => "←".into(),
            "right" => "→".into(),
            key => key.to_string(),
        }
    }

    fn project(&self, slug: &str) -> Option<&ProjectView> {
        self.snap.project(slug)
    }

    fn reload(&mut self) {
        self.snap = ui_ops::load_snapshot(self.ctx);
    }

    // -------------------------------------------------------------- launcher

    fn needs_you(&self) -> Vec<(&ProjectView, &Node)> {
        let mut items: Vec<(&ProjectView, &Node)> = self
            .snap
            .projects
            .iter()
            .flat_map(|p| {
                p.threads
                    .iter()
                    .chain(p.coordinators.iter().skip(1))
                    .map(move |t| (p, t))
            })
            .filter(|(_, t)| t.status == Status::Need)
            .filter(|(p, t)| self.matches(p, t))
            .collect();
        items.sort_by(|a, b| a.1.since.cmp(&b.1.since));
        items
    }

    fn matches(&self, project: &ProjectView, node: &Node) -> bool {
        let needle = self.search.to_lowercase();
        needle.is_empty()
            || node.title.to_lowercase().contains(&needle)
            || project.name.to_lowercase().contains(&needle)
            || node.workspace.to_lowercase().contains(&needle)
    }

    /// Coordinators numbered 1-9 across projects, in launcher order.
    fn numbered(&self) -> Vec<(String, String)> {
        self.snap
            .projects
            .iter()
            .flat_map(|p| {
                p.coordinators
                    .iter()
                    .map(move |c| (p.slug.clone(), c.id.clone()))
            })
            .take(9)
            .collect()
    }

    fn launcher_lines(&self) -> Vec<Line> {
        let mut lines = Vec::new();
        let needs = self.needs_you();
        if !needs.is_empty() {
            lines.push(
                Line::new()
                    .at(1, "NEEDS YOU", HEAD)
                    .at(-2, "oldest first", DIM),
            );
            for (p, t) in needs.iter().take(6) {
                lines.push(
                    Line::new()
                        .at(3, "●", RED)
                        .at(5, term::fit(&t.title, 13), Style::PLAIN)
                        .at(19, term::fit(&coordinator_name(p, &t.coordinator), 17), DIM)
                        .at(37, term::fit(&t.text, 38), Style::PLAIN)
                        .at(-2, state::age(&t.since), DIM)
                        .target(Target::NeedsYou(p.slug.clone(), t.id.clone())),
                );
            }
            lines.push(Line::new());
        }
        let coordinators: usize = self
            .snap
            .projects
            .iter()
            .map(|p| p.coordinators.len())
            .sum();
        let threads: usize = self
            .snap
            .projects
            .iter()
            .map(|p| p.threads.iter().filter(|t| !t.resolved).count())
            .sum();
        lines.push(Line::new().at(1, "PROJECTS", HEAD).at(
            -2,
            format!(
                "{} project{} · {coordinators} coordinators · {threads} threads",
                self.snap.projects.len(),
                if self.snap.projects.len() == 1 {
                    ""
                } else {
                    "s"
                }
            ),
            DIM,
        ));
        let numbered = self.numbered();
        for p in &self.snap.projects {
            let coords: Vec<&Node> = p
                .coordinators
                .iter()
                .filter(|c| self.matches(p, c) || self.search.is_empty())
                .collect();
            if !self.search.is_empty()
                && coords.is_empty()
                && !p.name.to_lowercase().contains(&self.search.to_lowercase())
            {
                continue;
            }
            let mut head = Line::new().at(3, term::fit(&p.name, 22), BOLD).at(
                27,
                term::fit(&p.repo.replace(&home(), "~"), 30),
                DIM,
            );
            if p.status != "active" {
                head = head.at(-2, p.status.clone(), DIM);
            } else {
                head = counts_spans(head, -22, &p.counts);
            }
            lines.push(head.target(Target::Project(p.slug.clone())));
            for (i, c) in coords.iter().enumerate() {
                let digit = numbered
                    .iter()
                    .position(|(s, id)| *s == p.slug && *id == c.id)
                    .map(|n| (n + 1).to_string())
                    .unwrap_or_default();
                let (glyph, style) = dot(c.status);
                let branch = if i + 1 == coords.len() { "└" } else { "├" };
                let title = if c.id == ROOT_ID {
                    "Coordinator".to_string()
                } else {
                    c.title.clone()
                };
                let detail = if c.id == ROOT_ID {
                    profile(c)
                } else {
                    c.workspace.clone()
                };
                let mut line = Line::new()
                    .at(2, digit, DIM)
                    .at(4, branch, GREY)
                    .at(6, glyph, style)
                    .at(8, term::fit(&title, 18), Style::PLAIN)
                    .at(27, term::fit(&detail, 30), DIM);
                // The project row already carries the root's numbers.
                line = if c.counts.total() > 0 && c.id != ROOT_ID {
                    counts_spans(line, -22, &c.counts)
                } else {
                    line.at(-22, c.status.word(), status_style(c.status))
                };
                lines.push(line.target(Target::Node(p.slug.clone(), c.id.clone())));
            }
            lines.push(
                Line::new()
                    .at(4, "+", BLUE)
                    .at(
                        6,
                        format!("New coordinator in {}", term::fit(&p.name, 40)),
                        Style::PLAIN,
                    )
                    .at(-2, self.key_label(Action::NewCoordinator), DIM)
                    .target(Target::NewCoordinator(p.slug.clone())),
            );
        }
        lines.push(
            Line::new()
                .at(4, "+", BLUE)
                .at(6, "New project", Style::PLAIN)
                .at(-2, self.key_label(Action::NewProject), DIM)
                .target(Target::NewProject),
        );
        let outside: Vec<&state::Workspace> = self
            .snap
            .workspaces
            .iter()
            .filter(|w| w.project.is_empty())
            .collect();
        if !outside.is_empty() {
            lines.push(Line::new());
            lines.push(
                Line::new()
                    .at(1, "WORKSPACES", HEAD)
                    .at(12, "not in a project", DIM),
            );
            for chunk in outside.chunks(3) {
                let mut line = Line::new();
                for (i, w) in chunk.iter().enumerate() {
                    let x = 2 + i as isize * 26;
                    let (glyph, style) = herdr_dot(&w.agent_status);
                    let tabs = format!("{} tab{}", w.tabs, if w.tabs == 1 { "" } else { "s" });
                    line = line
                        .at(x, glyph, style)
                        .at(x + 2, term::fit(&w.label, 14), Style::PLAIN)
                        .at(x + 17, tabs, DIM);
                }
                lines.push(line);
            }
        }
        lines
    }

    fn draw_launcher(&mut self, frame: &mut Frame, sel: usize) {
        if self.snap.projects.is_empty() {
            return self.draw_empty(frame, sel);
        }
        let all = self.snap.counts();
        draw_line(frame, 0, &counts_header(&all), false);
        let updated = match &self.snap.ticker {
            Some(_) => format!("updated {} ago", ago(&self.snap.generated)),
            None => "ticker not running".into(),
        };
        frame.put_right(frame.width - 1, 0, &updated, DIM);
        if !self.pick.is_empty() {
            frame.put(1, 1, &format!("Choose a project to {}", self.pick), BLUE);
        }
        let lines = self.launcher_lines();
        let height = frame.height.saturating_sub(4);
        self.rows = draw_list(frame, 2, height, &lines, sel);
        let pairs = self.hint(&[
            (Action::Open, "open"),
            (Action::NewProject, "new"),
            (Action::Adopt, "adopt"),
            (Action::Search, "search"),
            (Action::Board, "board"),
            (Action::Settings, "settings"),
            (Action::Back, "close"),
        ]);
        let mut pairs = pairs;
        pairs.insert(2, ("1-9".into(), "jump"));
        frame.hint(frame.height - 1, &pairs);
    }

    fn draw_empty(&mut self, frame: &mut Frame, sel: usize) {
        frame.put(1, 1, "No projects yet", BOLD);
        frame.put(
            1,
            2,
            "A project is one coordinator agent that splits work into threads:",
            DIM,
        );
        frame.put(
            1,
            3,
            "one agent per task, each in its own worktree and tab.",
            DIM,
        );
        let mut lines = vec![
            Line::new()
                .at(3, "+", BLUE)
                .at(5, "New project", Style::PLAIN)
                .at(-2, self.key_label(Action::NewProject), DIM)
                .target(Target::NewProject),
        ];
        if !self.current_workspace.1.is_empty() {
            lines.push(
                Line::new()
                    .at(3, "+", BLUE)
                    .at(
                        5,
                        format!(
                            "Turn workspace \"{}\" into a project",
                            term::fit(&self.current_workspace.1, 30)
                        ),
                        Style::PLAIN,
                    )
                    .at(-2, self.key_label(Action::Adopt), DIM)
                    .target(Target::Adopt),
            );
        }
        self.rows = draw_list(frame, 5, 2, &lines, sel);
        let y = 8;
        frame.put(1, y, "WORKSPACES", HEAD);
        frame.put(
            12,
            y,
            &format!("{} · none in a project", self.snap.workspaces.len()),
            DIM,
        );
        for (i, w) in self.snap.workspaces.iter().enumerate() {
            let row = y + 1 + i / 2;
            if row + 1 >= frame.height {
                break;
            }
            let x = if i % 2 == 0 { 3 } else { 37 };
            let (glyph, style) = herdr_dot(&w.agent_status);
            frame.put(x, row, glyph, style);
            frame.put(x + 2, row, &term::fit(&w.label, 21), Style::PLAIN);
            frame.put(
                x + 24,
                row,
                &format!("{} tab{}", w.tabs, if w.tabs == 1 { "" } else { "s" }),
                DIM,
            );
        }
        let pairs = self.hint(&[
            (Action::Open, "select"),
            (Action::NewProject, "new project"),
            (Action::Adopt, "adopt workspace"),
            (Action::Back, "close"),
        ]);
        frame.hint(frame.height - 1, &pairs);
    }

    // -------------------------------------------------------------- tree

    fn tree_lines(&self, project: &ProjectView, folded: &BTreeSet<String>) -> Vec<Line> {
        let resolved = self.config.view.resolved;
        let mut lines = Vec::new();
        for c in &project.coordinators {
            let root = c.id == ROOT_ID;
            if root {
                let (glyph, style) = dot(c.status);
                lines.push(
                    Line::new()
                        .at(3, glyph, style)
                        .at(5, "Coordinator", BOLD)
                        .at(26, c.status.word(), style)
                        .at(37, term::fit(&c.text, 40), Style::PLAIN)
                        .target(Target::Node(project.slug.clone(), c.id.clone())),
                );
            } else {
                let indent = (c.depth.saturating_sub(1) * 2) as isize;
                let fold = if folded.contains(&c.id) { "▸" } else { "▾" };
                let (glyph, style) = dot(c.status);
                let line = Line::new()
                    .at(1 + indent, fold, DIM)
                    .at(3 + indent, glyph, style)
                    .at(5 + indent, term::fit(&c.title, 20), BOLD)
                    .at(
                        26,
                        term::fit(&format!("coordinator · {}", c.workspace), 30),
                        DIM,
                    );
                lines.push(
                    counts_spans(line, -24, &c.counts)
                        .target(Target::Node(project.slug.clone(), c.id.clone())),
                );
                if folded.contains(&c.id) {
                    continue;
                }
            }
            let (threads, done) = visible_threads(project, &c.id, resolved);
            let total = threads.len() + usize::from(done > 0);
            for (i, t) in threads.iter().enumerate() {
                let (glyph, style) = dot(t.status);
                let branch = if i + 1 == total { "└─" } else { "├─" };
                lines.push(
                    Line::new()
                        .at(3, branch, GREY)
                        .at(6, glyph, style)
                        .at(8, term::fit(&t.title, 17), Style::PLAIN)
                        .at(26, t.status.word(), style)
                        .at(37, term::fit(&t.text, 44), Style::PLAIN)
                        .at(-2, state::age(&t.since), DIM)
                        .target(Target::Node(project.slug.clone(), t.id.clone())),
                );
            }
            if done > 0 {
                lines.push(
                    Line::new()
                        .at(3, "└─", GREY)
                        .at(6, "✓", GREEN)
                        .at(8, format!("{done} resolved"), DIM)
                        .target(Target::Resolved(project.slug.clone(), c.id.clone())),
                );
            }
        }
        lines.push(
            Line::new()
                .at(3, "+", BLUE)
                .at(5, "New coordinator", Style::PLAIN)
                .at(-2, self.key_label(Action::NewCoordinator), DIM)
                .target(Target::NewCoordinator(project.slug.clone())),
        );
        lines
    }

    fn draw_tree(&mut self, frame: &mut Frame, slug: &str, sel: usize, folded: &BTreeSet<String>) {
        let Some(project) = self.project(slug).cloned() else {
            frame.put(1, 1, "This project is gone.", RED);
            return;
        };
        let root = &project.coordinators[0];
        frame.put(1, 0, &profile(root), DIM);
        if !project.thread_harness.is_empty() {
            frame.put(26, 0, &format!("workers {}", project.thread_harness), DIM);
        }
        frame.put_right(frame.width - 1, 0, &project.repo.replace(&home(), "~"), DIM);
        draw_line(frame, 1, &counts_header(&project.counts), false);
        let lines = self.tree_lines(&project, folded);
        let detail = 8;
        let height = frame.height.saturating_sub(3 + detail + 1);
        self.rows = draw_list(frame, 3, height, &lines, sel);
        let y = frame.height - 1 - detail;
        frame.rule(y);
        let selected = targets(&lines).get(sel).cloned();
        let actions = match selected {
            Some(Target::Node(_, id)) => self.draw_node_summary(frame, &project, &id, y + 1),
            _ => Vec::new(),
        };
        let mut pairs = self.hint(&[(Action::Open, "go to pane"), (Action::Fold, "fold")]);
        pairs.extend(actions);
        pairs.extend(self.hint(&[
            (Action::NewThread, "thread"),
            (Action::Board, "board"),
            (Action::Back, "back"),
        ]));
        frame.hint(frame.height - 1, &pairs);
    }

    /// The selected node under the rule: who it is, what it says, and the
    /// one-key actions that apply. Returns the hint pairs for those actions.
    fn draw_node_summary(
        &self,
        frame: &mut Frame,
        project: &ProjectView,
        id: &str,
        y: usize,
    ) -> Vec<(String, &'static str)> {
        let Some(node) = project.node(id) else {
            return Vec::new();
        };
        let name = if node.id == ROOT_ID {
            "Coordinator"
        } else {
            node.title.as_str()
        };
        let x = frame.put(1, y, name, BOLD) + 1;
        let mut meta = vec![profile(node)];
        if !node.tab.is_empty() {
            meta.push(format!("tab {}", node.tab));
        }
        if !node.workspace.is_empty() {
            meta.push(node.workspace.clone());
        }
        meta.retain(|m| !m.is_empty());
        frame.put(x, y, &format!("· {}", meta.join(" · ")), DIM);
        let place = if node.worktree.is_empty() {
            node.branch.clone()
        } else {
            node.worktree.clone()
        };
        frame.put(1, y + 1, &place.replace(&home(), "~"), DIM);
        let mut text_y = y + 3;
        if let Some(pr) = &node.pr {
            frame.put(1, text_y, &format!("PR #{}", pr.number), BOLD);
            let mut x = 1 + 4 + pr.number.len() + 2;
            if pr.approved() {
                x = frame.put(x, text_y, "✓ approved", GREEN) + 2;
            }
            if pr.checks() > 0 {
                let style = if pr.failed > 0 {
                    RED
                } else if pr.pending > 0 {
                    YELLOW
                } else {
                    GREEN
                };
                x = frame.put(
                    x,
                    text_y,
                    &format!("checks {}/{}", pr.passed, pr.checks()),
                    style,
                ) + 2;
            }
            if pr.additions + pr.deletions > 0 {
                x = frame.put(x, text_y, &format!("+{}", pr.additions), GREEN) + 1;
                frame.put(x, text_y, &format!("−{}", pr.deletions), RED);
            }
            text_y += 1;
        } else {
            frame.put(
                1,
                text_y,
                &term::fit(&node.text, frame.width - 2),
                Style::PLAIN,
            );
            text_y += 1;
        }
        let chip_y = text_y + 1;
        let mut x = 1;
        let mut pairs = Vec::new();
        if node.status == Status::Need {
            x = frame.chip(x, chip_y, "1-9", "answer") + 1;
            x = frame.chip(x, chip_y, &self.key_label(Action::Reply), "Reply…") + 1;
            pairs.push(("1-9".into(), "answer"));
        } else if node.pr.as_ref().is_some_and(|p| !p.merged()) && node.id != ROOT_ID {
            x = frame.chip(x, chip_y, &self.key_label(Action::Merge), "Merge") + 1;
            x = frame.chip(x, chip_y, &self.key_label(Action::OpenPr), "Open PR") + 1;
            pairs.extend(self.hint(&[(Action::Merge, "merge")]));
        }
        frame.chip(x, chip_y, &self.key_label(Action::GoToPane), "Go to pane");
        pairs
    }

    // -------------------------------------------------------------- board

    fn board_filters(project: &ProjectView) -> Vec<(String, String)> {
        let mut filters = vec![("all".to_string(), String::new())];
        filters.extend(
            project
                .coordinators
                .iter()
                .skip(1)
                .map(|c| (c.title.clone(), c.id.clone())),
        );
        filters
    }

    fn board_columns(project: &ProjectView, filter: &str) -> [Vec<Node>; 4] {
        let pick = |status: Status| {
            project
                .threads
                .iter()
                .filter(|t| t.status == status)
                .filter(|t| filter.is_empty() || t.coordinator == filter)
                .cloned()
                .collect::<Vec<_>>()
        };
        [
            pick(Status::Need),
            pick(Status::Work),
            pick(Status::Review),
            pick(Status::Done),
        ]
    }

    fn draw_board(&mut self, frame: &mut Frame, slug: &str, col: usize, row: usize, filter: usize) {
        let Some(project) = self.project(slug).cloned() else {
            return;
        };
        let filters = Self::board_filters(&project);
        let filter = filter.min(filters.len() - 1);
        let mut x = 1;
        for (i, (name, _)) in filters.iter().enumerate() {
            let style = if i == filter { term::CHIP.bold() } else { DIM };
            x = frame.put(x, 0, &format!(" {} ", term::fit(name, 18)), style) + 1;
        }
        let shown = project.threads.iter().filter(|t| !t.resolved).count();
        frame.put_right(frame.width - 1, 0, &format!("{shown} threads"), DIM);
        let columns = Self::board_columns(&project, &filters[filter].1);
        let cw = (frame.width.saturating_sub(2)) / 4;
        let card_rows = frame.height.saturating_sub(13);
        for (i, (status, title)) in [
            (Status::Need, "NEEDS YOU"),
            (Status::Work, "WORKING"),
            (Status::Review, "REVIEW"),
            (Status::Done, "RESOLVED"),
        ]
        .into_iter()
        .enumerate()
        {
            let cx = 1 + i * cw;
            if i > 0 {
                for y in 2..2 + card_rows + 2 {
                    frame.put(cx - 1, y, "│", GREY);
                }
            }
            let (glyph, style) = dot(status);
            frame.put(cx + 1, 2, glyph, style);
            let end = frame.put(cx + 3, 2, title, BOLD);
            frame.put(end + 1, 2, &columns[i].len().to_string(), DIM);
            frame.put(cx, 3, &"─".repeat(cw.saturating_sub(1)), GREY);
            let fits = card_rows / 4;
            for (n, t) in columns[i].iter().take(fits).enumerate() {
                let y = 4 + n * 4;
                let selected = i == col && n == row;
                if selected {
                    for k in 0..3 {
                        frame.fill(cx, y + k, cw.saturating_sub(1), Some(SELECTED));
                    }
                    frame.put(cx, y, "›", BOLD);
                }
                frame.put(cx + 1, y, &term::fit(&t.title, cw.saturating_sub(6)), BOLD);
                frame.put_right(cx + cw - 2, y, &state::age(&t.since), DIM);
                frame.put(
                    cx + 1,
                    y + 1,
                    &term::fit(&coordinator_name(&project, &t.coordinator), cw - 3),
                    DIM,
                );
                let text_style = match t.status {
                    Status::Review => BLUE,
                    Status::Done => GREEN,
                    _ => Style::PLAIN,
                };
                frame.put(cx + 1, y + 2, &term::fit(&t.text, cw - 3), text_style);
            }
            if columns[i].len() > fits {
                frame.put(
                    cx + 1,
                    4 + fits * 4,
                    &format!("+{} more", columns[i].len() - fits),
                    DIM,
                );
            }
        }
        let y = 4 + card_rows;
        let idle: Vec<String> = project
            .threads
            .iter()
            .filter(|t| t.status == Status::Idle)
            .map(|t| {
                format!(
                    "{} · {}",
                    t.title,
                    coordinator_name(&project, &t.coordinator)
                )
            })
            .collect();
        frame.put(1, y, "○", GREY);
        let end = frame.put(3, y, "IDLE", BOLD);
        let end = frame.put(end + 1, y, &idle.len().to_string(), DIM);
        frame.put(
            end + 3,
            y,
            &term::fit(&idle.join("   "), frame.width.saturating_sub(end + 4)),
            DIM,
        );
        frame.put(1, y + 1, "◆", BLUE);
        let mut x = frame.put(3, y + 1, "COORDINATORS", BOLD) + 3;
        for c in &project.coordinators {
            let (glyph, style) = dot(c.status);
            let name = if c.id == ROOT_ID {
                "Coordinator"
            } else {
                c.title.as_str()
            };
            if x + 3 + name.chars().count() >= frame.width {
                break;
            }
            frame.put(x, y + 1, glyph, style);
            x = frame.put(x + 2, y + 1, name, Style::PLAIN) + 3;
        }
        frame.rule(y + 3);
        let selected = columns.get(col).and_then(|c| c.get(row));
        if let Some(t) = selected {
            frame.put(1, y + 4, "›", BOLD);
            let x = frame.put(3, y + 4, &t.title, BOLD) + 1;
            frame.put(x, y + 4, &format!("{} · {}", profile(t), t.workspace), DIM);
            frame.put(3, y + 5, &term::fit(&t.text, frame.width - 4), Style::PLAIN);
        }
        let mut pairs = vec![("←→↑↓".to_string(), "move")];
        pairs.extend(self.hint(&[(Action::Open, "open"), (Action::GoToPane, "go to pane")]));
        pairs.push(("1-9".into(), "answer"));
        pairs.extend(self.hint(&[
            (Action::Merge, "merge"),
            (Action::Filter, "filter"),
            (Action::Back, "back"),
        ]));
        frame.hint(frame.height - 1, &pairs);
        self.rows.clear();
    }

    // -------------------------------------------------------------- detail

    fn draw_detail(
        &mut self,
        frame: &mut Frame,
        slug: &str,
        id: &str,
        sel: usize,
        output: &[String],
    ) {
        let Some(project) = self.project(slug).cloned() else {
            return;
        };
        let Some(node) = project.node(id).cloned() else {
            frame.put(1, 0, "This thread is gone.", RED);
            return;
        };
        let (glyph, style) = dot(node.status);
        frame.put(1, 0, glyph, style);
        let x = frame.put(3, 0, node.status.word(), style.bold());
        let path = if node.id == ROOT_ID {
            format!("{} › Coordinator", project.name)
        } else {
            format!(
                "{} › {}",
                coordinator_name(&project, &node.coordinator),
                node.title
            )
        };
        let x = frame.put(x + 1, 0, "·", DIM);
        frame.put(x + 1, 0, &path, Style::PLAIN);
        let age = state::age(&node.since);
        if !age.is_empty() {
            let word = if node.status == Status::Need {
                "waiting"
            } else {
                "since"
            };
            frame.put_right(frame.width - 1, 0, &format!("{word} {age}"), DIM);
        }
        let mut meta = vec![profile(&node)];
        if !node.tab.is_empty() {
            meta.push(format!("tab {} in {}", node.tab, node.workspace));
        }
        meta.retain(|m| !m.is_empty());
        frame.put(1, 1, &meta.join(" · "), DIM);
        let mut branch = Vec::new();
        if !node.branch.is_empty() {
            branch.push(format!("branch {}", node.branch));
        }
        if !node.worktree.is_empty() {
            branch.push(node.worktree.replace(&home(), "~"));
        }
        frame.put(1, 2, &branch.join(" · "), DIM);
        let mut y = 4;
        if node.status == Status::Need {
            frame.put(1, y, "QUESTION", HEAD);
            let lines: Vec<&String> = if output.is_empty() {
                Vec::new()
            } else {
                output.iter().collect()
            };
            if lines.is_empty() {
                frame.put(
                    1,
                    y + 1,
                    &term::fit(&node.text, frame.width - 2),
                    Style::PLAIN,
                );
                y += 2;
            } else {
                for (i, line) in lines.iter().enumerate() {
                    frame.put(
                        1,
                        y + 1 + i,
                        &term::fit(line, frame.width - 2),
                        Style::PLAIN,
                    );
                }
                y += 1 + lines.len();
            }
            let x = frame.chip(1, y + 1, "1-9", "pick that option") + 1;
            frame.chip(x, y + 1, &self.key_label(Action::Reply), "Reply…");
            y += 3;
        } else if !node.text.is_empty() {
            frame.put(1, y, "NOW", HEAD);
            frame.put(
                1,
                y + 1,
                &term::fit(&node.text, frame.width - 2),
                Style::PLAIN,
            );
            y += 3;
        }
        if let Some(pr) = &node.pr {
            frame.put(1, y, "PR / CI", HEAD);
            let state_style = if pr.merged() || pr.approved() {
                GREEN
            } else {
                YELLOW
            };
            frame.put(1, y + 1, "●", state_style);
            let mut x = frame.put(3, y + 1, &format!("#{}", pr.number), BOLD) + 1;
            let word = if pr.merged() {
                "merged"
            } else if pr.draft {
                "draft"
            } else if pr.approved() {
                "approved"
            } else {
                "open"
            };
            x = frame.put(x, y + 1, word, DIM) + 2;
            if pr.checks() > 0 {
                let (glyph, st) = if pr.failed > 0 {
                    ("✗", RED)
                } else if pr.pending > 0 {
                    ("◐", YELLOW)
                } else {
                    ("✓", GREEN)
                };
                frame.put(x, y + 1, glyph, st);
                let detail = if pr.pending > 0 {
                    format!("checks {}/{} running", pr.passed, pr.checks())
                } else {
                    format!("checks {}/{}", pr.passed, pr.checks())
                };
                x = frame.put(x + 2, y + 1, &detail, Style::PLAIN) + 2;
            }
            if pr.additions + pr.deletions > 0 {
                x = frame.put(x, y + 1, &format!("+{}", pr.additions), GREEN) + 1;
                frame.put(x, y + 1, &format!("−{}", pr.deletions), RED);
            }
            frame.put_right(
                frame.width - 1,
                y + 1,
                &format!("{} open", self.key_label(Action::OpenPr)),
                DIM,
            );
            if let Some(blocker) = &pr.blocker
                && !pr.merged()
            {
                frame.put(
                    3,
                    y + 2,
                    &term::fit(&format!("not mergeable: {blocker}"), frame.width - 4),
                    DIM,
                );
                y += 1;
            }
            y += 3;
        }
        let items: Vec<&state::InboxItem> = project
            .inbox
            .iter()
            .filter(|i| i.subject == node.id)
            .collect();
        let mut lines = Vec::new();
        if !items.is_empty() {
            frame.put(1, y, "INBOX", HEAD);
            frame.put(7, y, &items.len().to_string(), DIM);
            for item in &items {
                lines.push(
                    Line::new()
                        .at(3, term::fit(&item.kind, 12), BLUE)
                        .at(17, state::age(&item.created), DIM)
                        .at(
                            22,
                            term::fit(&item.summary, frame.width.saturating_sub(24)),
                            Style::PLAIN,
                        )
                        .target(Target::Inbox(item.id.clone())),
                );
            }
            let height = 3.min(frame.height.saturating_sub(y + 6));
            self.rows = draw_list(frame, y + 1, height, &lines, sel);
            y += 2 + height;
        } else {
            self.rows.clear();
        }
        if node.status != Status::Need && !output.is_empty() && y + 3 < frame.height {
            frame.put(1, y, "LAST OUTPUT", HEAD);
            let pane = if node.tab.is_empty() {
                node.pane_id.clone()
            } else {
                format!("pane {}", node.tab)
            };
            frame.put_right(frame.width - 1, y, &pane, DIM);
            for (i, line) in output.iter().rev().take(2).rev().enumerate() {
                frame.put(1, y + 1 + i, "│", GREY);
                frame.put(3, y + 1 + i, &term::fit(line, frame.width - 4), DIM);
            }
        }
        let mut pairs = Vec::new();
        if node.status == Status::Need {
            pairs.push(("1-9".to_string(), "answer"));
        }
        pairs.extend(self.hint(&[(Action::Reply, "reply"), (Action::GoToPane, "go to pane")]));
        if node.pr.as_ref().is_some_and(|p| !p.merged()) {
            pairs.extend(self.hint(&[(Action::OpenPr, "open PR"), (Action::Merge, "merge")]));
        }
        if !items.is_empty() {
            pairs.extend(self.hint(&[(Action::Done, "done")]));
        }
        pairs.extend(self.hint(&[(Action::Back, "back")]));
        frame.hint(frame.height - 1, &pairs);
    }

    // -------------------------------------------------------------- settings

    fn settings_lines(&self) -> Vec<Line> {
        let v = &self.config.view;
        let mut lines = vec![Line::new().at(1, "VIEW", HEAD)];
        let rows: [(&str, String, &str); 5] = [
            (
                "Dock",
                match v.dock {
                    DockSide::Off => "off",
                    DockSide::Right => "right",
                    DockSide::Left => "left",
                }
                .into(),
                "off · right · left",
            ),
            ("Dock width", format!("{}%", v.dock_width), "15-50%"),
            (
                "Resolved threads",
                match v.resolved {
                    Resolved::Count => "count only",
                    Resolved::List => "list",
                }
                .into(),
                "count · list",
            ),
            (
                "Notify me on",
                match v.notify {
                    Notify::NeedsYouAndReview => "needs you, review",
                    Notify::NeedsYou => "needs you",
                    Notify::Off => "off",
                }
                .into(),
                "needs you, review · needs you · off",
            ),
            (
                "Launcher key",
                "prefix+a".into(),
                "set in Herdr's config.toml",
            ),
        ];
        for (i, (label, value, range)) in rows.into_iter().enumerate() {
            let line = Line::new().at(3, label, Style::PLAIN);
            let line = if i == 4 {
                line.at(26, value, DIM)
            } else {
                let close = 27 + value.chars().count() as isize;
                line.at(24, "‹", DIM)
                    .at(26, value, BLUE.bold())
                    .at(close, "›", DIM)
            };
            lines.push(line.at(-2, range, DIM).target(Target::Setting(i)));
        }
        lines.push(Line::new());
        lines.push(
            Line::new()
                .at(1, "KEYS", HEAD)
                .at(-2, "↵ rebind · backspace default", DIM),
        );
        for action in Action::ALL {
            lines.push(
                Line::new()
                    .at(3, action.label(), Style::PLAIN)
                    .at(26, self.keys().display(action), BLUE)
                    .target(Target::Key(action)),
            );
        }
        for problem in &self.config.problems {
            lines.push(Line::new().at(3, term::fit(problem, 80), RED));
        }
        lines.push(Line::new());
        lines.push(Line::new().at(1, "STATUS", HEAD));
        match &self.snap.ticker {
            Some(t) => {
                let events = if t.events {
                    "events connected"
                } else {
                    "events down, reconciling every minute"
                };
                lines.push(
                    Line::new()
                        .at(3, "ticker", Style::PLAIN)
                        .at(17, "●", GREEN)
                        .at(
                            19,
                            format!(
                                "running · {:.1} MB peak · {events}",
                                t.peak_rss_kb as f64 / 1024.0
                            ),
                            Style::PLAIN,
                        ),
                );
                if !t.last_event.is_empty() {
                    lines.push(Line::new().at(3, "last change", Style::PLAIN).at(
                        19,
                        format!("{} ago · {}", ago(&t.last_event_at), t.last_event),
                        DIM,
                    ));
                }
                if !t.herdr.is_empty() {
                    lines.push(Line::new().at(3, "herdr", Style::PLAIN).at(
                        19,
                        t.herdr.clone(),
                        DIM,
                    ));
                }
            }
            None => lines.push(
                Line::new()
                    .at(3, "ticker", Style::PLAIN)
                    .at(17, "○", GREY)
                    .at(19, "not running; views show recorded state", DIM),
            ),
        }
        let coordinators: usize = self
            .snap
            .projects
            .iter()
            .map(|p| p.coordinators.len())
            .sum();
        let threads: usize = self.snap.projects.iter().map(|p| p.threads.len()).sum();
        lines.push(Line::new().at(3, "projects", Style::PLAIN).at(
            19,
            format!(
                "{} · {coordinators} coordinators · {threads} threads",
                self.snap.projects.len()
            ),
            Style::PLAIN,
        ));
        lines.push(Line::new().at(3, "root", Style::PLAIN).at(
            19,
            self.ctx.root.display().to_string().replace(&home(), "~"),
            DIM,
        ));
        lines.push(
            Line::new().at(3, "config", Style::PLAIN).at(
                19,
                tui_config::path(&self.ctx.config_dir)
                    .display()
                    .to_string()
                    .replace(&home(), "~"),
                DIM,
            ),
        );
        lines
    }

    fn draw_settings(&mut self, frame: &mut Frame, sel: usize, capture: bool) {
        let lines = self.settings_lines();
        let height = frame.height.saturating_sub(2);
        self.rows = draw_list(frame, 0, height, &lines, sel);
        if capture {
            frame.hint(
                frame.height - 1,
                &[
                    ("press a key".into(), "to bind it"),
                    ("esc".into(), "cancel"),
                ],
            );
        } else {
            let mut pairs = vec![("↑↓".to_string(), "move"), ("←→".to_string(), "change")];
            pairs.extend(self.hint(&[(Action::Doctor, "run doctor"), (Action::Back, "back")]));
            frame.hint(frame.height - 1, &pairs);
        }
    }

    // -------------------------------------------------------------- form

    fn draw_form(&mut self, frame: &mut Frame, form: &Form) {
        let split = 26;
        for y in 0..frame.height.saturating_sub(3) {
            frame.put(split, y, "│", GREY);
        }
        frame.put(1, 0, "CREATE", HEAD);
        for (i, kind) in Kind::ALL.iter().enumerate() {
            let y = 1 + i * 3;
            let (name, sub) = kind.label();
            if *kind == form.kind {
                frame.fill(0, y, split, Some(SELECTED));
                frame.fill(0, y + 1, split, Some(SELECTED));
                frame.put(1, y, "›", BOLD);
                frame.put(3, y, name, BOLD);
            } else {
                frame.put(3, y, name, Style::PLAIN);
            }
            frame.put(3, y + 1, sub, DIM);
        }
        frame.put(1, frame.height.saturating_sub(5), "Saved in", DIM);
        let root = self.ctx.root.display().to_string().replace(&home(), "~");
        frame.put(
            1,
            frame.height.saturating_sub(4),
            &term::fit(&root, split - 2),
            DIM,
        );
        let fx = split + 2;
        let fw = frame.width.saturating_sub(fx + 1);
        let title = match (form.kind, form.adopt_pane.is_empty()) {
            (Kind::Project, false) => "Project from this workspace",
            (Kind::Project, true) => "New project",
            (Kind::Coordinator, _) => "New coordinator",
            (Kind::Thread, _) => "New thread",
            (Kind::Workspace, _) => "New workspace",
        };
        frame.put(fx, 0, title, BOLD);
        let mut y = 2;
        for (i, field) in form.fields.iter().enumerate() {
            let focused = i == form.focus;
            let label_style = if focused { BLUE.bold() } else { DIM };
            match field {
                Field::Text { label, value, hint } => {
                    frame.put(fx, y, label, label_style);
                    frame.fill(fx, y + 1, fw, Some(SELECTED));
                    if value.is_empty() && !focused {
                        frame.put(fx + 1, y + 1, hint, DIM.on(SELECTED));
                    } else {
                        let shown = tail(value, fw.saturating_sub(3));
                        let end = frame.put(fx + 1, y + 1, &shown, Style::PLAIN.on(SELECTED));
                        if focused {
                            frame.put(end, y + 1, "▏", BLUE.bold().on(SELECTED));
                        }
                    }
                    y += 2;
                    if i == 0
                        && let Some((ok, text)) = self.validate(form)
                    {
                        frame.put(
                            fx,
                            y,
                            if ok { "✓" } else { "✗" },
                            if ok { GREEN } else { RED },
                        );
                        frame.put(fx + 2, y, &term::fit(&text, fw - 2), DIM);
                        y += 1;
                    }
                }
                Field::Choice {
                    label, shown, at, ..
                } => {
                    frame.put(fx, y, label, label_style);
                    let mut x = fx;
                    for (n, option) in shown.iter().enumerate() {
                        let style = if n == *at { PRIMARY } else { term::CHIP };
                        if x + option.chars().count() + 2 > fx + fw {
                            break;
                        }
                        x = frame.put(x, y + 1, &format!(" {option} "), style) + 1;
                    }
                    y += 2;
                }
            }
            y += 1;
            if y + 6 > frame.height {
                break;
            }
        }
        let preview = self.preview(form);
        let py = frame.height.saturating_sub(5);
        frame.put(fx, py, &term::fit(&preview, fw), DIM);
        let by = py + 1;
        frame.put_right(fx + fw - 11, by, " esc cancel ", term::CHIP);
        frame.put_right(fx + fw, by, " ↵ create ", PRIMARY);
        frame.hint(
            frame.height - 1,
            &[
                (self.key_label(Action::NextField), "next field"),
                (self.key_label(Action::PrevField), "back"),
                ("←→".into(), "choose"),
                ("↑↓".into(), "what to create"),
                ("↵".into(), "create"),
                ("esc".into(), "cancel"),
            ],
        );
        self.rows.clear();
    }

    fn form_value(form: &Form, label: &str) -> String {
        form.fields
            .iter()
            .find(|f| match f {
                Field::Text { label: l, .. } | Field::Choice { label: l, .. } => *l == label,
            })
            .map(Field::value)
            .unwrap_or_default()
    }

    fn validate(&self, form: &Form) -> Option<(bool, String)> {
        match form.kind {
            Kind::Project => {
                let name = Self::form_value(form, "Name");
                if name.trim().is_empty() {
                    return None;
                }
                Some(match crate::project::slug_from_name(&name) {
                    Ok(slug) if self.ctx.root.join(&slug).exists() => {
                        (false, format!("`{slug}` already exists"))
                    }
                    Ok(slug) => (
                        true,
                        format!(
                            "free · {}",
                            self.ctx
                                .root
                                .join(&slug)
                                .display()
                                .to_string()
                                .replace(&home(), "~")
                        ),
                    ),
                    Err(error) => (false, format!("{error:#}")),
                })
            }
            Kind::Workspace => {
                let folder = Self::form_value(form, "Folder");
                if folder.trim().is_empty() {
                    return None;
                }
                let path = self.ctx.env.expand_tilde(folder.trim());
                Some(if path.is_dir() {
                    (true, "folder exists".into())
                } else {
                    (false, "not a folder".into())
                })
            }
            _ => None,
        }
    }

    fn new_project(form: &Form) -> ui_ops::NewProject {
        ui_ops::NewProject {
            name: Self::form_value(form, "Name"),
            goal: Self::form_value(form, "Goal"),
            repos: Self::form_value(form, "Repositories")
                .split([',', ' '])
                .map(str::to_string)
                .filter(|r| !r.is_empty())
                .collect(),
            coordinator_agent: Self::form_value(form, "Coordinator"),
            thread_agent: Self::form_value(form, "Threads run on"),
            adopt_pane: form.adopt_pane.clone(),
            adopt_cwd: form.adopt_cwd.clone(),
        }
    }

    fn new_node(form: &Form) -> ui_ops::NewNode {
        let parent = match form.kind {
            Kind::Thread => Self::form_value(form, "Under"),
            _ => ROOT_ID.into(),
        };
        ui_ops::NewNode {
            slug: Self::form_value(form, "Project"),
            parent: if parent.is_empty() {
                ROOT_ID.into()
            } else {
                parent
            },
            role: if form.kind == Kind::Thread {
                NodeRole::Worker
            } else {
                NodeRole::Coordinator
            },
            title: Self::form_value(form, "Title"),
            task: Self::form_value(form, "Task"),
            repo: Self::form_value(form, "Repository"),
            harness: Self::form_value(form, "Runs on"),
            model: Self::form_value(form, "Model"),
            effort: Self::form_value(form, "Effort"),
        }
    }

    fn preview(&self, form: &Form) -> String {
        match form.kind {
            Kind::Project => ui_ops::preview_project(&Self::new_project(form)),
            Kind::Coordinator | Kind::Thread => ui_ops::preview_node(&Self::new_node(form)),
            Kind::Workspace => format!(
                "$ herdr workspace create --cwd {} --label {:?}",
                Self::form_value(form, "Folder"),
                Self::form_value(form, "Label")
            ),
        }
    }

    fn project_choice(&self, slug: &str) -> Field {
        let slugs: Vec<String> = self.snap.projects.iter().map(|p| p.slug.clone()).collect();
        let names: Vec<String> = self.snap.projects.iter().map(|p| p.name.clone()).collect();
        let at = slugs.iter().position(|s| s == slug).unwrap_or(0);
        Field::choice("Project", slugs, names, at)
    }

    fn coordinator_choice(&self, slug: &str, parent: &str) -> Field {
        let project = self.project(slug);
        let ids: Vec<String> = project
            .map(|p| p.coordinators.iter().map(|c| c.id.clone()).collect())
            .unwrap_or_else(|| vec![ROOT_ID.into()]);
        let names: Vec<String> = project
            .map(|p| {
                p.coordinators
                    .iter()
                    .map(|c| coordinator_name(p, &c.id))
                    .collect()
            })
            .unwrap_or_else(|| vec!["Coordinator".into()]);
        let at = ids.iter().position(|i| i == parent).unwrap_or(0);
        Field::choice("Under", ids, names, at)
    }

    fn harness_choice(label: &'static str, inherit: bool, current: &str) -> Field {
        let mut options: Vec<String> = HARNESSES.iter().map(|s| s.to_string()).collect();
        let mut shown = options.clone();
        if inherit {
            options.insert(0, String::new());
            shown.insert(0, "inherit".into());
        }
        let at = options.iter().position(|o| o == current).unwrap_or(0);
        Field::choice(label, options, shown, at)
    }

    fn form(&self, kind: Kind, slug: &str, parent: &str) -> Form {
        let slug = if slug.is_empty() {
            self.snap
                .projects
                .first()
                .map(|p| p.slug.clone())
                .unwrap_or_default()
        } else {
            slug.to_string()
        };
        let efforts = EFFORTS.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let effort_names = EFFORTS
            .iter()
            .map(|s| {
                if s.is_empty() {
                    "inherit".to_string()
                } else {
                    s.to_string()
                }
            })
            .collect();
        let fields = match kind {
            Kind::Project => vec![
                Field::text("Name", "", "Panel mayorista"),
                Field::text("Goal", "", "What should be true when this project is done?"),
                Field::text("Repositories", "", "~/dev/app, ~/dev/api"),
                Self::harness_choice("Coordinator", false, "claude"),
                Self::harness_choice("Threads run on", false, "claude"),
            ],
            Kind::Coordinator => vec![
                Field::text("Title", "", "rediseño mobile"),
                self.project_choice(&slug),
                Field::text("Task", "", "What this coordinator owns"),
                Field::text("Repository", "", "optional: its own worktree and workspace"),
                Self::harness_choice("Runs on", true, ""),
                Field::text("Model", "", "inherit"),
                Field::choice("Effort", efforts, effort_names, 0),
            ],
            Kind::Thread => vec![
                Field::text("Title", "", "panel depo"),
                self.project_choice(&slug),
                self.coordinator_choice(&slug, parent),
                Field::text("Task", "", "One bounded task"),
                Field::text("Repository", "", "optional: a worktree; empty is a tab"),
                Self::harness_choice("Runs on", true, ""),
                Field::text("Model", "", "inherit"),
                Field::choice("Effort", efforts, effort_names, 0),
            ],
            Kind::Workspace => vec![
                Field::text("Folder", "", "~/dev/app"),
                Field::text("Label", "", "app"),
            ],
        };
        Form {
            kind,
            fields,
            focus: 0,
            adopt_pane: String::new(),
            adopt_cwd: String::new(),
        }
    }

    fn submit(&mut self, form: &Form) -> bool {
        let result = match form.kind {
            Kind::Project => {
                let new = Self::new_project(form);
                if new.name.trim().is_empty() {
                    Err(anyhow::anyhow!("give the project a name"))
                } else {
                    ui_ops::create_project(self.ctx, &new).map(|slug| format!("opened `{slug}`"))
                }
            }
            Kind::Coordinator | Kind::Thread => {
                let new = Self::new_node(form);
                if new.title.trim().is_empty() || new.slug.is_empty() {
                    Err(anyhow::anyhow!("a title and a project are required"))
                } else {
                    ui_ops::create_node(self.ctx, &new)
                        .map(|t| format!("started {} {}", t.id, t.title))
                }
            }
            Kind::Workspace => ui_ops::create_workspace(
                self.ctx,
                &Self::form_value(form, "Folder"),
                &Self::form_value(form, "Label"),
            )
            .map(|_| "workspace created".to_string()),
        };
        match result {
            Ok(message) => {
                self.message = message;
                true
            }
            Err(error) => {
                self.message = format!("{error:#}");
                false
            }
        }
    }

    // -------------------------------------------------------------- dock

    fn dock_coordinator(&self) -> Option<(ProjectView, Node)> {
        let Mode::Dock { slug, workspace } = &self.mode else {
            return None;
        };
        let project = self.project(slug)?.clone();
        let coordinator = project
            .coordinators
            .iter()
            .rev()
            .find(|c| c.workspace_id == *workspace)
            .or_else(|| project.coordinators.first())?
            .clone();
        Some((project, coordinator))
    }

    fn dock_lines(&self, project: &ProjectView, coordinator: &Node, width: usize) -> Vec<Line> {
        let mut lines = Vec::new();
        let subtree: Vec<&Node> = project
            .threads
            .iter()
            .filter(|t| is_under(project, t, &coordinator.id))
            .collect();
        for (status, title) in [
            (Status::Need, "NEEDS YOU"),
            (Status::Review, "REVIEW"),
            (Status::Work, "WORKING"),
            (Status::Idle, "IDLE"),
        ] {
            let group: Vec<&&Node> = subtree.iter().filter(|t| t.status == status).collect();
            if group.is_empty() {
                continue;
            }
            let (glyph, style) = dot(status);
            lines.push(Line::new().at(1, title, HEAD).at(
                2 + title.len() as isize,
                group.len().to_string(),
                style,
            ));
            for t in group {
                lines.push(
                    Line::new()
                        .at(1, glyph, style)
                        .at(
                            3,
                            term::fit(&t.title, width.saturating_sub(9)),
                            Style::PLAIN,
                        )
                        .at(-2, state::age(&t.since), DIM)
                        .target(Target::Node(project.slug.clone(), t.id.clone())),
                );
                if !t.text.is_empty() {
                    lines.push(Line::new().at(3, term::fit(&t.text, width.saturating_sub(4)), DIM));
                }
            }
            lines.push(Line::new());
        }
        let done = subtree.iter().filter(|t| t.status == Status::Done).count();
        if done > 0 {
            lines.push(Line::new().at(1, "▸", DIM).at(3, "RESOLVED", HEAD).at(
                12,
                done.to_string(),
                GREEN,
            ));
            lines.push(Line::new());
        }
        let ids: BTreeSet<&str> = subtree.iter().map(|t| t.id.as_str()).collect();
        let items: Vec<&state::InboxItem> = project
            .inbox
            .iter()
            .filter(|i| ids.contains(i.subject.as_str()))
            .collect();
        if !items.is_empty() {
            lines.push(
                Line::new()
                    .at(1, "INBOX", HEAD)
                    .at(7, items.len().to_string(), DIM),
            );
            for item in items.iter().take(4) {
                let who = project
                    .node(&item.subject)
                    .map(|n| n.title.clone())
                    .unwrap_or_default();
                lines.push(
                    Line::new()
                        .at(1, term::fit(&who, width.saturating_sub(8)), BLUE)
                        .at(-2, state::age(&item.created), DIM),
                );
                lines.push(Line::new().at(
                    1,
                    term::fit(&item.summary, width.saturating_sub(2)),
                    DIM,
                ));
            }
            lines.push(Line::new());
        }
        lines.push(
            Line::new()
                .at(1, "+", BLUE)
                .at(3, "New thread", Style::PLAIN)
                .at(-2, self.key_label(Action::NewThread), DIM)
                .target(Target::NewThread(project.slug.clone())),
        );
        lines
    }

    fn draw_dock(&mut self, frame: &mut Frame, sel: usize) {
        let Some((project, coordinator)) = self.dock_coordinator() else {
            frame.put(1, 0, "No project here", DIM);
            return;
        };
        let name = if coordinator.id == ROOT_ID {
            project.name.clone()
        } else {
            coordinator.title.clone()
        };
        frame.put(1, 0, &name, BOLD);
        frame.put(1, 1, &format!("coordinator · {}", coordinator.harness), DIM);
        let (glyph, style) = dot(coordinator.status);
        frame.put_right(frame.width - 1, 1, glyph, style);
        let lines = self.dock_lines(&project, &coordinator, frame.width);
        let height = frame.height.saturating_sub(5);
        self.rows = draw_list(frame, 3, height, &lines, sel);
        let pairs = self.hint(&[
            (Action::Open, "go"),
            (Action::Merge, "merge"),
            (Action::Help, "keys"),
        ]);
        frame.hint(frame.height - 1, &pairs);
    }

    fn draw_help(&self, frame: &mut Frame) {
        frame.dim_all();
        let w = frame.width.saturating_sub(4).min(44);
        let h = frame.height.saturating_sub(2).min(Action::ALL.len() + 2);
        let (x, y) = frame.boxed(2, 1, w, h, " Keys ", BLUE);
        for (i, action) in Action::ALL
            .iter()
            .filter(|a| a.on(Screen::Dock) || a.on(Screen::Tree))
            .enumerate()
        {
            if i + 2 >= h {
                break;
            }
            frame.put(x + 1, y + i, &self.keys().display(*action), BLUE);
            frame.put(x + 14, y + i, action.label(), DIM);
        }
    }

    // -------------------------------------------------------------- dialogs

    fn draw_confirm(&self, frame: &mut Frame, slug: &str, id: &str) {
        frame.dim_all();
        let Some(project) = self.project(slug) else {
            return;
        };
        let Some(node) = project.node(id) else {
            return;
        };
        let Some(pr) = &node.pr else {
            return;
        };
        let w = frame.width.saturating_sub(4).min(62);
        let x0 = (frame.width - w) / 2;
        let y0 = frame.height.saturating_sub(14) / 2;
        let (x, y) = frame.boxed(x0, y0, w, 13, "", BLUE);
        frame.put(x0 + 1, y0, &format!(" Merge PR #{} ", pr.number), BOLD);
        frame.put(x + 1, y, &term::fit(&node.title, w - 4), BOLD);
        frame.put(x + 1, y + 1, &format!("{} → base", node.branch), DIM);
        let mut cx = x + 1;
        if pr.approved() {
            cx = frame.put(cx, y + 3, "✓ approved", GREEN) + 3;
        }
        if pr.checks() > 0 {
            let style = if pr.failed + pr.pending == 0 {
                GREEN
            } else {
                YELLOW
            };
            cx = frame.put(
                cx,
                y + 3,
                &format!("✓ checks {}/{} passed", pr.passed, pr.checks()),
                style,
            ) + 3;
        }
        cx = frame.put(cx, y + 3, &format!("+{}", pr.additions), GREEN) + 1;
        frame.put(cx, y + 3, &format!("−{}", pr.deletions), RED);
        match &pr.blocker {
            Some(blocker) => {
                frame.put(x + 1, y + 5, "Not mergeable now:", RED);
                frame.put(x + 1, y + 6, &term::fit(blocker, w - 4), Style::PLAIN);
                frame.put(x + 1, y + 10, " esc back ", term::CHIP);
            }
            None => {
                frame.put(x + 1, y + 5, "Squash and merge, then", Style::PLAIN);
                frame.put(x + 3, y + 6, "·", DIM);
                frame.put(
                    x + 5,
                    y + 6,
                    &format!("close thread {} and its view", node.title),
                    Style::PLAIN,
                );
                frame.put(x + 3, y + 7, "·", DIM);
                let tree = if node.worktree.is_empty() {
                    "nothing to remove (no worktree)".to_string()
                } else {
                    format!("remove worktree {}", node.branch)
                };
                frame.put(x + 5, y + 7, &term::fit(&tree, w - 8), Style::PLAIN);
                frame.put(x + 3, y + 8, "·", DIM);
                frame.put(
                    x + 5,
                    y + 8,
                    &format!(
                        "tell coordinator {}",
                        coordinator_name(project, &node.coordinator)
                    ),
                    Style::PLAIN,
                );
                frame.put(x + 1, y + 10, " ↵ merge ", PRIMARY);
                frame.put(x + 11, y + 10, " esc cancel ", term::CHIP);
                frame.put_right(
                    x + w - 3,
                    y + 10,
                    &format!("{} keep worktree", self.key_label(Action::KeepWorktree)),
                    DIM,
                );
            }
        }
        frame.fill(0, frame.height - 1, frame.width, None);
        let pairs = if pr.blocker.is_some() {
            self.hint(&[(Action::Back, "back")])
        } else {
            self.hint(&[
                (Action::Open, "merge"),
                (Action::KeepWorktree, "merge, keep worktree"),
                (Action::Back, "cancel"),
            ])
        };
        frame.hint(frame.height - 1, &pairs);
    }

    fn draw_reply(&self, frame: &mut Frame, slug: &str, id: &str, text: &str) {
        frame.dim_all();
        let name = self
            .project(slug)
            .and_then(|p| p.node(id))
            .map(|n| {
                if n.id == ROOT_ID {
                    "Coordinator".to_string()
                } else {
                    n.title.clone()
                }
            })
            .unwrap_or_default();
        let w = frame.width.saturating_sub(4);
        let y0 = frame.height.saturating_sub(6) / 2;
        let (x, y) = frame.boxed(2, y0, w, 5, &format!(" Reply to {name} "), BLUE);
        frame.fill(x + 1, y + 1, w - 4, Some(SELECTED));
        let end = frame.put(x + 2, y + 1, &tail(text, w - 8), Style::PLAIN.on(SELECTED));
        frame.put(end, y + 1, "▏", BLUE.bold().on(SELECTED));
        frame.fill(0, frame.height - 1, frame.width, None);
        frame.hint(
            frame.height - 1,
            &[("↵".into(), "send"), ("esc".into(), "cancel")],
        );
    }

    // -------------------------------------------------------------- frame

    fn render(&mut self, width: usize, height: usize) -> Frame {
        let mut frame = Frame::new(width, height);
        let (min_w, min_h) = match self.mode {
            Mode::Popup => (40, 12),
            Mode::Dock { .. } => (16, 6),
        };
        if width < min_w || height < min_h {
            frame.put(0, 0, "Too small; make this larger", DIM);
            self.rows.clear();
            return frame;
        }
        let base = self.stack.iter().rposition(|v| !v.overlay()).unwrap_or(0);
        let view = self.stack[base].clone();
        match &view {
            View::Launcher { sel } => self.draw_launcher(&mut frame, *sel),
            View::Tree { slug, sel, folded } => self.draw_tree(&mut frame, slug, *sel, folded),
            View::Board {
                slug,
                col,
                row,
                filter,
            } => self.draw_board(&mut frame, slug, *col, *row, *filter),
            View::Detail {
                slug,
                id,
                sel,
                output,
            } => self.draw_detail(&mut frame, slug, id, *sel, output),
            View::Settings { sel, capture } => self.draw_settings(&mut frame, *sel, *capture),
            View::Form(form) => self.draw_form(&mut frame, form),
            View::Dock { sel } => self.draw_dock(&mut frame, *sel),
            View::Search { .. } | View::Confirm { .. } | View::Reply { .. } | View::Help => {}
        }
        if let Some(top) = self.stack.last().cloned()
            && top.overlay()
        {
            match &top {
                View::Confirm { slug, id } => self.draw_confirm(&mut frame, slug, id),
                View::Reply { slug, id, text } => self.draw_reply(&mut frame, slug, id, text),
                View::Help => self.draw_help(&mut frame),
                _ => {}
            }
        }
        if let Some(View::Search { text }) = self.stack.last() {
            let y = height - 1;
            frame.fill(0, y, width, None);
            let end = frame.put(1, y, "/", BOLD);
            let end = frame.put(end + 1, y, text, Style::PLAIN);
            frame.put(end, y, "▏", BLUE.bold());
        } else if !self.message.is_empty() {
            let y = height.saturating_sub(2);
            frame.fill(0, y, width, None);
            let style = if self.message.starts_with("merged")
                || self.message.starts_with("started")
                || self.message.starts_with("opened")
            {
                GREEN
            } else {
                Style::fg(Color::Yellow)
            };
            frame.put(1, y, &self.message, style);
        }
        frame
    }

    // -------------------------------------------------------------- input

    fn top(&mut self) -> &mut View {
        self.stack.last_mut().expect("the stack is never empty")
    }

    fn back(&mut self) {
        if self.stack.len() > 1 {
            self.stack.pop();
        } else if matches!(self.mode, Mode::Popup) {
            self.close = true;
        }
    }

    fn selected_target(&self) -> Option<Target> {
        let width = 92;
        match self.stack.last()? {
            View::Launcher { sel } => {
                if self.snap.projects.is_empty() {
                    let mut t = vec![Target::NewProject];
                    if !self.current_workspace.1.is_empty() {
                        t.push(Target::Adopt);
                    }
                    t.get(*sel).cloned()
                } else {
                    targets(&self.launcher_lines()).get(*sel).cloned()
                }
            }
            View::Tree { slug, sel, folded } => {
                let project = self.project(slug)?;
                targets(&self.tree_lines(project, folded))
                    .get(*sel)
                    .cloned()
            }
            View::Settings { sel, .. } => targets(&self.settings_lines()).get(*sel).cloned(),
            View::Dock { sel } => {
                let (project, coordinator) = self.dock_coordinator()?;
                targets(&self.dock_lines(&project, &coordinator, width))
                    .get(*sel)
                    .cloned()
            }
            View::Detail { slug, id, sel, .. } => {
                let project = self.project(slug)?;
                project
                    .inbox
                    .iter()
                    .filter(|i| i.subject == *id)
                    .nth(*sel)
                    .map(|i| Target::Inbox(i.id.clone()))
            }
            _ => None,
        }
    }

    fn target_count(&self) -> usize {
        match self.stack.last() {
            Some(View::Launcher { .. }) if self.snap.projects.is_empty() => {
                1 + usize::from(!self.current_workspace.1.is_empty())
            }
            Some(View::Launcher { .. }) => targets(&self.launcher_lines()).len(),
            Some(View::Tree { slug, folded, .. }) => self
                .project(slug)
                .map(|p| targets(&self.tree_lines(p, folded)).len())
                .unwrap_or(0),
            Some(View::Settings { .. }) => targets(&self.settings_lines()).len(),
            Some(View::Dock { .. }) => self
                .dock_coordinator()
                .map(|(p, c)| targets(&self.dock_lines(&p, &c, 92)).len())
                .unwrap_or(0),
            Some(View::Detail { slug, id, .. }) => self
                .project(slug)
                .map(|p| p.inbox.iter().filter(|i| i.subject == *id).count())
                .unwrap_or(0),
            _ => 0,
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let count = self.target_count();
        let sel = match self.top() {
            View::Launcher { sel }
            | View::Tree { sel, .. }
            | View::Settings { sel, .. }
            | View::Dock { sel }
            | View::Detail { sel, .. } => sel,
            _ => return,
        };
        if count == 0 {
            *sel = 0;
            return;
        }
        *sel = (*sel as isize + delta).clamp(0, count as isize - 1) as usize;
    }

    fn set_sel(&mut self, index: usize) {
        if let View::Launcher { sel }
        | View::Tree { sel, .. }
        | View::Settings { sel, .. }
        | View::Dock { sel }
        | View::Detail { sel, .. } = self.top()
        {
            *sel = index;
        }
    }

    fn open_tree(&mut self, slug: &str, node: Option<&str>) {
        let folded = BTreeSet::new();
        let sel = match (self.project(slug), node) {
            (Some(p), Some(id)) => targets(&self.tree_lines(p, &folded))
                .iter()
                .position(|t| *t == Target::Node(slug.to_string(), id.to_string()))
                .unwrap_or(0),
            _ => 0,
        };
        self.stack.push(View::Tree {
            slug: slug.into(),
            sel,
            folded,
        });
    }

    fn open_detail(&mut self, slug: &str, id: &str) {
        let output = ui_ops::last_output(self.ctx, slug, id, 3);
        self.stack.push(View::Detail {
            slug: slug.into(),
            id: id.into(),
            sel: 0,
            output,
        });
    }

    fn go(&mut self, slug: &str, id: &str) {
        match ui_ops::go_to_pane(self.ctx, slug, id) {
            Ok(_) if matches!(self.mode, Mode::Popup) => self.close = true,
            Ok(_) => self.message.clear(),
            Err(error) => self.message = format!("{error:#}"),
        }
    }

    /// The node an action applies to on the current screen.
    fn focused_node(&self) -> Option<(String, String)> {
        match self.stack.last()? {
            View::Detail { slug, id, .. } => Some((slug.clone(), id.clone())),
            View::Board {
                slug,
                col,
                row,
                filter,
            } => {
                let project = self.project(slug)?;
                let filters = Self::board_filters(project);
                let filter = &filters.get(*filter)?.1;
                let columns = Self::board_columns(project, filter);
                columns
                    .get(*col)?
                    .get(*row)
                    .map(|t| (slug.clone(), t.id.clone()))
            }
            _ => match self.selected_target()? {
                Target::Node(slug, id) | Target::NeedsYou(slug, id) => Some((slug, id)),
                _ => None,
            },
        }
    }

    fn start_merge(&mut self) {
        let Some((slug, id)) = self.focused_node() else {
            return;
        };
        let has_pr = self
            .project(&slug)
            .and_then(|p| p.node(&id))
            .and_then(|n| n.pr.as_ref())
            .is_some_and(|p| !p.merged());
        if has_pr {
            self.stack.push(View::Confirm { slug, id });
        } else {
            self.message = "this thread has no open pull request".into();
        }
    }

    fn new_form(&mut self, kind: Kind, slug: &str, parent: &str) {
        let form = self.form(kind, slug, parent);
        self.stack.push(View::Form(form));
    }

    fn current_slug(&self) -> String {
        for view in self.stack.iter().rev() {
            match view {
                View::Tree { slug, .. } | View::Board { slug, .. } | View::Detail { slug, .. } => {
                    return slug.clone();
                }
                _ => {}
            }
        }
        if let Mode::Dock { slug, .. } = &self.mode {
            return slug.clone();
        }
        match self.selected_target() {
            Some(
                Target::Project(s)
                | Target::Node(s, _)
                | Target::NewCoordinator(s)
                | Target::NeedsYou(s, _),
            ) => s,
            _ => String::new(),
        }
    }

    fn adopt_current(&mut self) {
        let (id, label) = self.current_workspace.clone();
        if id.is_empty() {
            self.message = "Herdr did not say which workspace this popup was opened from".into();
            return;
        }
        if self
            .snap
            .workspaces
            .iter()
            .any(|w| w.id == id && !w.project.is_empty())
        {
            self.message = format!("\"{label}\" already belongs to a project");
            return;
        }
        match ui_ops::workspace_agent(self.ctx, &id) {
            Ok((pane, cwd)) => {
                let mut form = self.form(Kind::Project, "", "");
                if let Field::Text { value, .. } = &mut form.fields[0] {
                    *value = label;
                }
                form.adopt_pane = pane;
                form.adopt_cwd = cwd;
                self.stack.push(View::Form(form));
            }
            Err(error) => self.message = format!("{error:#}"),
        }
    }

    fn activate(&mut self) {
        let Some(target) = self.selected_target() else {
            return;
        };
        if !self.pick.is_empty()
            && let Target::Project(slug) | Target::Node(slug, _) = &target
        {
            let command = self.pick.clone();
            let slug = slug.clone();
            let result = term::quiet(|| crate::actions::run_on_slug(self.ctx, &command, &slug));
            match result {
                Ok(()) => self.close = true,
                Err(error) => self.message = format!("{error:#}"),
            }
            return;
        }
        match target {
            Target::NeedsYou(slug, id) => self.open_detail(&slug, &id),
            Target::Project(slug) => self.open_tree(&slug, None),
            Target::Node(slug, id) => {
                let on_launcher = matches!(self.stack.last(), Some(View::Launcher { .. }));
                if on_launcher {
                    self.open_tree(&slug, Some(&id));
                } else {
                    self.go(&slug, &id);
                }
            }
            Target::NewThread(slug) => self.dock_new_thread(&slug),
            Target::NewCoordinator(slug) => self.new_form(Kind::Coordinator, &slug, ROOT_ID),
            Target::NewProject => self.new_form(Kind::Project, "", ""),
            Target::Adopt => self.adopt_current(),
            Target::Resolved(_, _) => {
                self.config.view.resolved = Resolved::List;
                self.message = "showing resolved threads (Settings keeps the choice)".into();
            }
            Target::Setting(i) => self.change_setting(i, 1),
            Target::Key(_) => {
                if let View::Settings { capture, .. } = self.top() {
                    *capture = true;
                }
            }
            Target::Inbox(_) => {}
        }
    }

    fn dock_new_thread(&mut self, slug: &str) {
        let parent = self
            .dock_coordinator()
            .map(|(_, c)| c.id)
            .unwrap_or_else(|| ROOT_ID.into());
        let handoff = Handoff {
            screen: "new-thread".into(),
            slug: slug.into(),
            node: parent,
            ..Handoff::default()
        };
        let opened =
            write_handoff(self.ctx, &handoff).and_then(|_| crate::actions::open_popup(self.ctx));
        if let Err(error) = opened {
            self.message = format!("{error:#}");
        }
    }

    fn change_setting(&mut self, index: usize, delta: i32) {
        let v = &mut self.config.view;
        let cycle = |n: usize, at: usize| ((at as i32 + delta).rem_euclid(n as i32)) as usize;
        match index {
            0 => {
                let all = [DockSide::Off, DockSide::Right, DockSide::Left];
                let at = all.iter().position(|d| *d == v.dock).unwrap_or(0);
                v.dock = all[cycle(3, at)];
            }
            1 => v.dock_width = (v.dock_width as i32 + delta * 5).clamp(15, 50) as u8,
            2 => {
                v.resolved = match v.resolved {
                    Resolved::Count => Resolved::List,
                    Resolved::List => Resolved::Count,
                }
            }
            3 => {
                let all = [Notify::NeedsYouAndReview, Notify::NeedsYou, Notify::Off];
                let at = all.iter().position(|n| *n == v.notify).unwrap_or(0);
                v.notify = all[cycle(3, at)];
            }
            _ => return,
        }
        self.save_config();
    }

    fn save_config(&mut self) {
        if let Err(error) = tui_config::save(&self.ctx.config_dir, &self.config) {
            self.message = format!("could not save: {error:#}");
        }
    }

    fn run_doctor(&mut self) {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let out = std::process::Command::new(exe)
            .arg("--root")
            .arg(&self.ctx.root)
            .arg("doctor")
            .output();
        self.message = match out {
            Ok(out) if out.status.success() => "doctor: all required checks passed".into(),
            Ok(out) => {
                let text = String::from_utf8_lossy(&out.stdout);
                let failed = text
                    .lines()
                    .find(|l| l.contains("FAIL"))
                    .unwrap_or("some checks failed");
                format!("doctor: {}", failed.trim())
            }
            Err(error) => format!("doctor did not run: {error}"),
        };
    }

    fn handle_text(field: &mut String, key: &KeyEvent) -> bool {
        use crossterm::event::{KeyCode, KeyModifiers};
        match key.code {
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                field.push(c);
                true
            }
            KeyCode::Backspace => {
                field.pop();
                true
            }
            _ => false,
        }
    }

    fn on_key(&mut self, key: KeyEvent) {
        if key.kind != KeyEventKind::Press {
            return;
        }
        let Some(name) = tui_config::key_name(&key) else {
            return;
        };
        let top = self.stack.last().cloned().expect("never empty");
        self.message.clear();
        match top {
            View::Search { mut text } => {
                if name == "esc" {
                    self.search.clear();
                    self.stack.pop();
                } else if name == "enter" {
                    self.stack.pop();
                } else if Self::handle_text(&mut text, &key) {
                    self.search = text.clone();
                    *self.top() = View::Search { text };
                    if let Some(View::Launcher { sel }) = self.stack.iter_mut().rev().nth(1) {
                        *sel = 0;
                    }
                }
                return;
            }
            View::Reply { slug, id, mut text } => {
                if name == "esc" {
                    self.stack.pop();
                } else if name == "enter" {
                    self.stack.pop();
                    match ui_ops::reply(self.ctx, &slug, &id, &text) {
                        Ok(()) => self.message = "sent".into(),
                        Err(error) => self.message = format!("{error:#}"),
                    }
                } else if Self::handle_text(&mut text, &key) {
                    *self.top() = View::Reply { slug, id, text };
                }
                return;
            }
            View::Settings { capture: true, .. } => {
                if name != "esc"
                    && let Some(Target::Key(action)) = self.selected_target()
                {
                    match self.config.keys.set(action, &name) {
                        Ok(()) => self.save_config(),
                        Err(error) => self.message = error,
                    }
                }
                if let View::Settings { capture, .. } = self.top() {
                    *capture = false;
                }
                return;
            }
            View::Form(mut form) => {
                self.on_form_key(&mut form, &key, &name);
                if let Some(View::Form(current)) = self.stack.last_mut() {
                    *current = form;
                }
                return;
            }
            _ => {}
        }
        let screen = top.screen();
        if let Some(digit) = name
            .chars()
            .next()
            .filter(|c| name.len() == 1 && c.is_ascii_digit() && *c != '0')
        {
            self.on_digit(digit);
            return;
        }
        if matches!(top, View::Settings { .. }) && matches!(name.as_str(), "backspace" | "delete") {
            if let Some(Target::Key(action)) = self.selected_target() {
                self.config.keys.reset(action);
                self.save_config();
            }
            return;
        }
        let Some(action) = self.keys().action(screen, &name) else {
            return;
        };
        self.on_action(action, &top);
    }

    fn on_digit(&mut self, digit: char) {
        let n = digit.to_digit(10).unwrap_or(1) as usize;
        match self.stack.last() {
            Some(View::Launcher { .. }) => {
                if let Some((slug, id)) = self.numbered().get(n - 1).cloned() {
                    self.open_tree(&slug, Some(&id));
                }
            }
            Some(View::Confirm { .. } | View::Settings { .. } | View::Help) => {}
            _ => {
                let Some((slug, id)) = self.focused_node() else {
                    return;
                };
                let needs = self
                    .project(&slug)
                    .and_then(|p| p.node(&id))
                    .is_some_and(|n| n.status == Status::Need);
                if !needs {
                    self.message = "this agent is not waiting on you".into();
                    return;
                }
                match ui_ops::answer(self.ctx, &slug, &id, digit) {
                    Ok(()) => self.message = format!("sent {digit}"),
                    Err(error) => self.message = format!("{error:#}"),
                }
            }
        }
    }

    fn on_action(&mut self, action: Action, top: &View) {
        match (action, top) {
            (Action::Back, View::Help | View::Confirm { .. }) => {
                self.stack.pop();
            }
            (Action::Back, _) => self.back(),
            (Action::Up, View::Board { .. }) => self.board_move(0, -1),
            (Action::Down, View::Board { .. }) => self.board_move(0, 1),
            (Action::Left, View::Board { .. }) => self.board_move(-1, 0),
            (Action::Right, View::Board { .. }) => self.board_move(1, 0),
            (Action::Up, _) => self.move_sel(-1),
            (Action::Down, _) => self.move_sel(1),
            (Action::Left | Action::Right, View::Settings { .. }) => {
                if let Some(Target::Setting(i)) = self.selected_target() {
                    self.change_setting(i, if action == Action::Left { -1 } else { 1 });
                }
            }
            (Action::Open | Action::KeepWorktree, View::Confirm { slug, id }) => {
                let blocked = self
                    .project(slug)
                    .and_then(|p| p.node(id))
                    .and_then(|n| n.pr.as_ref())
                    .is_some_and(|p| p.blocker.is_some());
                if blocked {
                    return;
                }
                let (slug, id) = (slug.clone(), id.clone());
                self.stack.pop();
                self.message =
                    match ui_ops::merge(self.ctx, &slug, &id, action == Action::KeepWorktree) {
                        Ok(message) => message,
                        Err(error) => format!("{error:#}"),
                    };
                self.reload();
            }
            (Action::Open, View::Board { .. }) => {
                if let Some((slug, id)) = self.focused_node() {
                    self.open_detail(&slug, &id);
                }
            }
            (Action::Open, View::Detail { .. }) => {}
            (Action::Open, _) => self.activate(),
            (Action::NewProject, _) => self.new_form(Kind::Project, "", ""),
            (Action::NewCoordinator, _) => {
                let slug = self.current_slug();
                self.new_form(Kind::Coordinator, &slug, ROOT_ID);
            }
            (Action::NewThread, View::Dock { .. }) => {
                let slug = self.current_slug();
                self.dock_new_thread(&slug);
            }
            (Action::NewThread, _) => {
                let slug = self.current_slug();
                let parent = match self.selected_target() {
                    Some(Target::Node(_, id)) => self
                        .project(&slug)
                        .and_then(|p| p.node(&id))
                        .map(|n| {
                            if n.role == "coordinator" {
                                n.id.clone()
                            } else {
                                n.coordinator.clone()
                            }
                        })
                        .unwrap_or_else(|| ROOT_ID.into()),
                    _ => ROOT_ID.into(),
                };
                self.new_form(Kind::Thread, &slug, &parent);
            }
            (Action::Board, _) => {
                let slug = self.current_slug();
                let slug = if slug.is_empty() {
                    self.snap
                        .projects
                        .first()
                        .map(|p| p.slug.clone())
                        .unwrap_or_default()
                } else {
                    slug
                };
                if !slug.is_empty() {
                    self.stack.push(View::Board {
                        slug,
                        col: 0,
                        row: 0,
                        filter: 0,
                    });
                }
            }
            (Action::Merge, _) => self.start_merge(),
            (Action::Settings, _) => self.stack.push(View::Settings {
                sel: 0,
                capture: false,
            }),
            (Action::Search, _) => self.stack.push(View::Search {
                text: self.search.clone(),
            }),
            (Action::Adopt, _) => self.adopt_current(),
            (Action::Fold, View::Tree { .. }) => {
                if let Some(Target::Node(_, id)) = self.selected_target()
                    && id != ROOT_ID
                    && let View::Tree { folded, .. } = self.top()
                    && !folded.remove(&id)
                {
                    folded.insert(id);
                }
            }
            (Action::GoToPane, _) => {
                if let Some((slug, id)) = self.focused_node() {
                    self.go(&slug, &id);
                }
            }
            (Action::Reply, _) => {
                if let Some((slug, id)) = self.focused_node() {
                    self.stack.push(View::Reply {
                        slug,
                        id,
                        text: String::new(),
                    });
                }
            }
            (Action::OpenPr, _) => {
                let url = self
                    .focused_node()
                    .and_then(|(s, id)| self.project(&s)?.node(&id)?.pr.clone())
                    .map(|p| p.url);
                if let Some(url) = url
                    && let Err(error) = ui_ops::open_pr(self.ctx, &url)
                {
                    self.message = format!("{error:#}");
                }
            }
            (Action::Done, View::Detail { slug, .. }) => {
                if let Some(Target::Inbox(item)) = self.selected_target() {
                    let slug = slug.clone();
                    match ui_ops::inbox_done(self.ctx, &slug, &item) {
                        Ok(()) => self.reload(),
                        Err(error) => self.message = format!("{error:#}"),
                    }
                }
            }
            (Action::Filter, View::Board { slug, filter, .. }) => {
                let count = self
                    .project(slug)
                    .map(|p| Self::board_filters(p).len())
                    .unwrap_or(1);
                let next = (filter + 1) % count.max(1);
                if let View::Board { filter, row, .. } = self.top() {
                    *filter = next;
                    *row = 0;
                }
            }
            (Action::Doctor, View::Settings { .. }) => self.run_doctor(),
            (Action::Help, _) => self.stack.push(View::Help),
            _ => {}
        }
    }

    fn board_move(&mut self, dc: isize, dr: isize) {
        let Some(View::Board {
            slug,
            col,
            row,
            filter,
        }) = self.stack.last().cloned()
        else {
            return;
        };
        let Some(project) = self.project(&slug) else {
            return;
        };
        let filters = Self::board_filters(project);
        let columns = Self::board_columns(project, &filters[filter.min(filters.len() - 1)].1);
        let col = (col as isize + dc).clamp(0, 3) as usize;
        let len = columns[col].len();
        let row = if len == 0 {
            0
        } else {
            (row as isize + dr).clamp(0, len as isize - 1) as usize
        };
        if let View::Board { col: c, row: r, .. } = self.top() {
            *c = col;
            *r = row;
        }
    }

    fn on_form_key(&mut self, form: &mut Form, key: &KeyEvent, name: &str) {
        let action = self.keys().action(Screen::Form, name);
        let printable =
            name.chars().count() == 1 || name.starts_with("shift+") && name.chars().count() == 7;
        match action {
            Some(Action::Back) => {
                self.stack.pop();
                return;
            }
            Some(Action::Open) => {
                if self.submit(form) {
                    self.close = matches!(self.mode, Mode::Popup) && form.kind != Kind::Workspace;
                    self.reload();
                    if !self.close {
                        self.stack.pop();
                    }
                }
                return;
            }
            Some(Action::NextField) => {
                form.focus = (form.focus + 1) % form.fields.len();
                return;
            }
            Some(Action::PrevField) => {
                form.focus = (form.focus + form.fields.len() - 1) % form.fields.len();
                return;
            }
            Some(Action::Up | Action::Down) if !printable && form.adopt_pane.is_empty() => {
                let at = Kind::ALL.iter().position(|k| *k == form.kind).unwrap_or(0);
                let delta: isize = if action == Some(Action::Up) { -1 } else { 1 };
                let next = (at as isize + delta).rem_euclid(Kind::ALL.len() as isize) as usize;
                let slug = Self::form_value(form, "Project");
                *form = self.form(Kind::ALL[next], &slug, ROOT_ID);
                return;
            }
            _ => {}
        }
        let project_changed = match &mut form.fields[form.focus] {
            Field::Text { value, .. } => {
                Self::handle_text(value, key);
                false
            }
            Field::Choice {
                options, at, label, ..
            } => {
                let n = options.len().max(1);
                match action {
                    Some(Action::Left) => *at = (*at + n - 1) % n,
                    Some(Action::Right) => *at = (*at + 1) % n,
                    _ => {}
                }
                *label == "Project"
            }
        };
        if project_changed && form.kind == Kind::Thread {
            let slug = Self::form_value(form, "Project");
            let index = form
                .fields
                .iter()
                .position(|f| matches!(f, Field::Choice { label: "Under", .. }));
            if let Some(index) = index {
                form.fields[index] = self.coordinator_choice(&slug, ROOT_ID);
            }
        }
    }

    fn on_mouse(&mut self, row: usize) {
        let Some(index) = self.rows.iter().find(|(y, _)| *y == row).map(|(_, i)| *i) else {
            return;
        };
        let now = std::time::Instant::now();
        let double = self.last_click.is_some_and(|(previous, at)| {
            previous == index && now.duration_since(at).as_millis() < 500
        });
        self.set_sel(index);
        self.last_click = Some((index, now));
        if double {
            self.last_click = None;
            self.activate();
        }
    }
}

fn is_under(project: &ProjectView, node: &Node, coordinator: &str) -> bool {
    if coordinator == ROOT_ID {
        return true;
    }
    let mut owner = node.coordinator.clone();
    for _ in 0..64 {
        if owner == coordinator {
            return true;
        }
        match project.node(&owner) {
            Some(c) if c.id != ROOT_ID => owner = c.coordinator.clone(),
            _ => return false,
        }
    }
    false
}

/// Herdr's own status icons for plain workspaces (done is teal there).
fn herdr_dot(status: &str) -> (&'static str, Style) {
    match status {
        "working" => ("●", YELLOW),
        "blocked" => ("●", RED),
        "done" => ("●", CYAN),
        _ => ("○", GREY),
    }
}

fn ago(since: &str) -> String {
    let secs = crate::thread::seconds_since(since, jiff::Timestamp::now()).max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h", secs / 3600),
    }
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "\u{0}".into())
}

/// The end of a text that is too long for its field, so the cursor stays visible.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        text.to_string()
    } else {
        format!(
            "…{}",
            text.chars().skip(count + 1 - max).collect::<String>()
        )
    }
}

fn current_workspace(ctx: &Ctx) -> (String, String) {
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Context {
        workspace_id: String,
        workspace_label: String,
    }
    let context: Context = ctx
        .env
        .var("HERDR_PLUGIN_CONTEXT_JSON")
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or_default();
    let id = ctx
        .env
        .var("HERDR_WORKSPACE_ID")
        .map(str::to_string)
        .unwrap_or(context.workspace_id);
    (id, context.workspace_label)
}

fn new_app<'a>(ctx: &'a Ctx<'a>, mode: Mode) -> App<'a> {
    let (config, problem) = match tui_config::load(&ctx.config_dir) {
        Ok(config) => (config, String::new()),
        Err(error) => (Config::default(), format!("{error:#}; using defaults")),
    };
    App {
        ctx,
        mode,
        snap: ui_ops::load_snapshot(ctx),
        config,
        stack: Vec::new(),
        message: problem,
        close: false,
        search: String::new(),
        pick: String::new(),
        current_workspace: current_workspace(ctx),
        rows: Vec::new(),
        last_click: None,
    }
}

fn event_loop(app: &mut App) -> Result<()> {
    let mut terminal = Terminal::enter()?;
    let inputs = term::inputs(&app.ctx.root);
    let mut state_mtime = std::fs::metadata(state::path(&app.ctx.root))
        .and_then(|m| m.modified())
        .ok();
    let mut identity_pending = matches!(app.mode, Mode::Dock { .. });
    loop {
        let (w, h) = Terminal::size();
        let frame = app.render(w, h);
        terminal.draw(frame)?;
        if identity_pending {
            identity_pending = false;
            crate::dock::report_identity(app.ctx);
        }
        if app.close {
            return Ok(());
        }
        match inputs.recv() {
            Ok(Input::Terminal(Event::Key(key))) => app.on_key(key),
            Ok(Input::Terminal(Event::Mouse(mouse))) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    app.on_mouse(mouse.row as usize);
                }
            }
            Ok(Input::Terminal(Event::Resize(_, _))) => terminal.invalidate(),
            Ok(Input::Terminal(_)) => {}
            Ok(Input::Changed) => {
                let mtime = std::fs::metadata(state::path(&app.ctx.root))
                    .and_then(|m| m.modified())
                    .ok();
                if mtime == state_mtime {
                    continue;
                }
                state_mtime = mtime;
                app.reload();
            }
            Err(_) => return Ok(()),
        }
    }
}

/// The popup: opened by the launcher key or an action.
pub fn run_popup(ctx: &Ctx) -> Result<()> {
    let mut app = new_app(ctx, Mode::Popup);
    let handoff = take_handoff(ctx);
    app.stack.push(View::Launcher { sel: 0 });
    match handoff.screen.as_str() {
        "new" => app.new_form(Kind::Project, "", ""),
        "new-thread" => app.new_form(Kind::Thread, &handoff.slug, &handoff.node),
        "tree" if app.project(&handoff.slug).is_some() => app.open_tree(&handoff.slug, None),
        "pick" => app.pick = handoff.command.clone(),
        "adopt" => {
            let mut form = app.form(Kind::Project, "", "");
            if let Field::Text { value, .. } = &mut form.fields[0] {
                *value = handoff.workspace_label.clone();
            }
            form.adopt_pane = handoff.pane_id.clone();
            form.adopt_cwd = handoff.workspace_cwd.clone();
            app.stack.push(View::Form(form));
        }
        _ => {}
    }
    event_loop(&mut app)
}

/// The optional dock: a split pane next to one coordinator's agents.
pub fn run_dock(ctx: &Ctx, slug: String, workspace: String) -> Result<()> {
    let mut app = new_app(ctx, Mode::Dock { slug, workspace });
    app.stack.push(View::Dock { sel: 0 });
    event_loop(&mut app)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Pr, Ticker};

    fn snapshot() -> Snapshot {
        let node = |id: &str, title: &str, role: &str, coordinator: &str, status: Status| Node {
            id: id.into(),
            title: title.into(),
            role: role.into(),
            coordinator: coordinator.into(),
            status,
            text: format!("{title} text"),
            since: crate::project::now(),
            harness: "claude".into(),
            ..Node::default()
        };
        let mut root = node("root", "Coordinator", "coordinator", "root", Status::Work);
        root.harness = "codex".into();
        root.counts = Counts {
            need: 1,
            work: 1,
            review: 1,
            ..Counts::default()
        };
        let mut mobile = node(
            "t-0001",
            "rediseño mobile",
            "coordinator",
            "root",
            Status::Work,
        );
        mobile.depth = 1;
        mobile.workspace = "AWAM rediseno mobile".into();
        mobile.counts = Counts {
            need: 1,
            work: 1,
            review: 1,
            ..Counts::default()
        };
        let mut merca = node("t-0004", "panel merca", "worker", "t-0001", Status::Review);
        merca.pr = Some(Pr {
            number: "1342".into(),
            url: "https://github.com/o/r/pull/1342".into(),
            review: "APPROVED".into(),
            ..Pr::default()
        });
        Snapshot {
            schema_version: state::SCHEMA_VERSION,
            generated: crate::project::now(),
            ticker: Some(Ticker {
                events: true,
                ..Ticker::default()
            }),
            projects: vec![ProjectView {
                slug: "awam".into(),
                name: "AWAM Comercio SaaS".into(),
                repo: "/x/comercio-saas".into(),
                status: "active".into(),
                coordinators: vec![root, mobile],
                threads: vec![
                    node("t-0002", "panel depo", "worker", "t-0001", Status::Need),
                    node("t-0003", "landing", "worker", "t-0001", Status::Work),
                    merca,
                ],
                counts: Counts {
                    need: 1,
                    work: 1,
                    review: 1,
                    ..Counts::default()
                },
                ..ProjectView::default()
            }],
            workspaces: vec![state::Workspace {
                id: "w9".into(),
                label: "Developer".into(),
                tabs: 1,
                ..state::Workspace::default()
            }],
        }
    }

    fn with_app(test: impl FnOnce(&mut App)) {
        let world = crate::scenarios::World::new();
        let ctx = world.ctx();
        let mut app = new_app(&ctx, Mode::Popup);
        app.snap = snapshot();
        app.stack.push(View::Launcher { sel: 0 });
        test(&mut app);
    }

    fn press(app: &mut App, code: crossterm::event::KeyCode) {
        app.on_key(KeyEvent::new(code, crossterm::event::KeyModifiers::NONE));
    }

    #[test]
    fn launcher_puts_needs_you_first_and_nests_coordinators_under_projects() {
        with_app(|app| {
            let text = app.render(84, 22).text();
            let lines: Vec<&str> = text.lines().collect();
            assert!(lines[2].contains("NEEDS YOU"), "{text}");
            assert!(lines[3].contains("› ● panel depo"), "{text}");
            assert!(text.contains("AWAM Comercio SaaS"));
            assert!(text.contains("1 ├ ● Coordinator"), "{text}");
            assert!(text.contains("2 └ ● rediseño mobile"), "{text}");
            assert!(text.contains("+ New coordinator in AWAM Comercio SaaS"));
            assert!(text.contains("WORKSPACES"));
            assert!(lines[21].contains("↵ open · n new · 1-9 jump"), "{text}");
        });
    }

    #[test]
    fn digits_jump_to_a_coordinator_and_escape_goes_back_then_closes() {
        with_app(|app| {
            press(app, crossterm::event::KeyCode::Char('2'));
            assert!(matches!(app.stack.last(), Some(View::Tree { .. })));
            let text = app.render(90, 29).text();
            assert!(text.contains("▾ ● rediseño mobile"), "{text}");
            assert!(text.contains("├─ ● panel depo"), "{text}");
            press(app, crossterm::event::KeyCode::Esc);
            assert!(matches!(app.stack.last(), Some(View::Launcher { .. })));
            press(app, crossterm::event::KeyCode::Esc);
            assert!(app.close);
        });
    }

    #[test]
    fn merge_opens_a_dialog_in_the_same_process() {
        with_app(|app| {
            app.open_tree("awam", Some("t-0004"));
            press(app, crossterm::event::KeyCode::Char('m'));
            assert!(matches!(app.stack.last(), Some(View::Confirm { .. })));
            let text = app.render(90, 29).text();
            assert!(text.contains("Merge PR #1342"), "{text}");
            assert!(text.contains("Squash and merge, then"), "{text}");
            assert!(text.contains("tell coordinator rediseño mobile"), "{text}");
            press(app, crossterm::event::KeyCode::Esc);
            assert!(matches!(app.stack.last(), Some(View::Tree { .. })));
        });
    }

    #[test]
    fn user_keybindings_change_dispatch_and_hints() {
        with_app(|app| {
            app.config.keys.set(Action::Board, "w").unwrap();
            let text = app.render(84, 22).text();
            assert!(text.contains("w board"), "{text}");
            press(app, crossterm::event::KeyCode::Char('b'));
            assert!(matches!(app.stack.last(), Some(View::Launcher { .. })));
            press(app, crossterm::event::KeyCode::Char('w'));
            assert!(matches!(app.stack.last(), Some(View::Board { .. })));
        });
    }

    #[test]
    fn settings_rebind_captures_the_next_key_and_saves_it() {
        with_app(|app| {
            press(app, crossterm::event::KeyCode::Char('s'));
            let keys_at = targets(&app.settings_lines())
                .iter()
                .position(|t| *t == Target::Key(Action::Board))
                .unwrap();
            app.set_sel(keys_at);
            press(app, crossterm::event::KeyCode::Enter);
            press(app, crossterm::event::KeyCode::Char('x'));
            assert_eq!(app.config.keys.primary(Action::Board), "x");
            let saved = tui_config::load(&app.ctx.config_dir).unwrap();
            assert_eq!(saved.keys.primary(Action::Board), "x");
            // A conflicting key is refused with a message.
            press(app, crossterm::event::KeyCode::Enter);
            press(app, crossterm::event::KeyCode::Char('n'));
            assert_eq!(app.config.keys.primary(Action::Board), "x");
            assert!(app.message.contains("new project"));
        });
    }

    #[test]
    fn board_shows_four_columns_and_moves_between_them() {
        with_app(|app| {
            press(app, crossterm::event::KeyCode::Char('b'));
            let text = app.render(92, 28).text();
            for title in ["NEEDS YOU", "WORKING", "REVIEW", "RESOLVED", "COORDINATORS"] {
                assert!(text.contains(title), "{title}: {text}");
            }
            press(app, crossterm::event::KeyCode::Right);
            assert_eq!(app.focused_node().unwrap().1, "t-0003");
        });
    }

    #[test]
    fn the_new_form_previews_the_cli_call_and_validates_the_name() {
        with_app(|app| {
            press(app, crossterm::event::KeyCode::Char('n'));
            for c in "Panel mayorista".chars() {
                press(app, crossterm::event::KeyCode::Char(c));
            }
            let text = app.render(90, 28).text();
            assert!(text.contains("✓ free"), "{text}");
            assert!(
                text.contains("$ herdr-organizations new \"Panel mayorista\""),
                "{text}"
            );
            // Printable keys type into the field instead of running commands.
            assert!(matches!(app.stack.last(), Some(View::Form(_))));
        });
    }

    #[test]
    fn every_screen_renders_at_small_and_odd_sizes() {
        with_app(|app| {
            let views = [
                View::Launcher { sel: 0 },
                View::Tree {
                    slug: "awam".into(),
                    sel: 3,
                    folded: BTreeSet::new(),
                },
                View::Board {
                    slug: "awam".into(),
                    col: 2,
                    row: 0,
                    filter: 1,
                },
                View::Detail {
                    slug: "awam".into(),
                    id: "t-0004".into(),
                    sel: 0,
                    output: vec!["$ ok".into()],
                },
                View::Settings {
                    sel: 2,
                    capture: false,
                },
                View::Dock { sel: 0 },
            ];
            for view in views {
                app.stack = vec![view.clone()];
                for (w, h) in [(0, 0), (1, 1), (39, 11), (40, 12), (61, 17), (200, 60)] {
                    let frame = app.render(w, h);
                    assert_eq!((frame.width, frame.height), (w, h), "{view:?}");
                }
            }
            app.stack = vec![View::Form(app.form(Kind::Thread, "awam", "t-0001"))];
            for (w, h) in [(40, 12), (92, 31)] {
                app.render(w, h);
            }
        });
    }

    #[test]
    fn empty_state_offers_new_and_adopt() {
        with_app(|app| {
            app.snap.projects.clear();
            app.current_workspace = ("w9".into(), "Developer".into());
            let text = app.render(70, 16).text();
            assert!(text.contains("No projects yet"));
            assert!(
                text.contains("Turn workspace \"Developer\" into a project"),
                "{text}"
            );
        });
    }
}

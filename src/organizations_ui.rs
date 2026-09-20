//! Recursive Organizations picker shown inside Herdr's plugin popup.

use std::collections::{BTreeMap, HashSet};
use std::io::{self, IsTerminal, Write};
use std::ops::Range;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use crossterm::execute;
use crossterm::style::{Attribute, SetAttribute};
use crossterm::terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen};
use unicode_width::UnicodeWidthChar;

use crate::herdr::Herdr;
use crate::organizations::{self, TreeEntry};
use crate::paths::{Ctx, SessionFlags};
use crate::project::{self, Project};
use crate::thread::{self, Thread};
use crate::{coordinator, threads};

const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(500);
const MAX_RENDERED_PROJECTS: usize = 200;
const MAX_RENDERED_NODES: usize = 1_000;
const MAX_PRINT_ALL_ROWS: usize = 2_000;

struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode().context("could not enable terminal input")?;
        let guard = Self;
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            event::EnableMouseCapture
        )
        .context("could not enter Organizations view")?;
        Ok(guard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(
            io::stdout(),
            event::DisableMouseCapture,
            LeaveAlternateScreen,
            SetAttribute(Attribute::Reset)
        );
        let _ = terminal::disable_raw_mode();
    }
}

#[derive(Debug, Clone)]
struct ProjectChoice {
    project: Project,
    label: String,
}

#[derive(Debug, Clone)]
struct TreeView {
    project: Project,
    entries: Vec<TreeEntry>,
    omitted_nodes: usize,
}

#[derive(Debug, Clone)]
enum Screen {
    Projects {
        choices: Vec<ProjectChoice>,
        selected: usize,
    },
    Tree {
        view: TreeView,
        selected: usize,
    },
}

pub fn run(ctx: &Ctx) -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return print_all(ctx);
    }

    let choices = project_choices(ctx);
    let _terminal = TerminalGuard::enter()?;
    let mut screen = Screen::Projects {
        choices,
        selected: 0,
    };
    let mut message = String::new();
    let mut last_click: Option<(usize, Instant)> = None;

    loop {
        render(&mut io::stdout(), &screen, &message)?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let event = event::read()?;
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                last_click = None;
                message.clear();
                match key.code {
                    KeyCode::Esc | KeyCode::Char('q') => match screen {
                        Screen::Tree { .. } => {
                            screen = Screen::Projects {
                                choices: project_choices(ctx),
                                selected: 0,
                            }
                        }
                        Screen::Projects { .. } => break,
                    },
                    KeyCode::Up | KeyCode::Char('k') => change_selection(&mut screen, -1),
                    KeyCode::Down | KeyCode::Char('j') => change_selection(&mut screen, 1),
                    KeyCode::Enter => {
                        if activate(ctx, &mut screen, &mut message) {
                            break;
                        }
                    }
                    KeyCode::Char('r') => refresh(ctx, &mut screen, &mut message),
                    _ => {}
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                let now = Instant::now();
                let height = terminal::size()?.1 as usize;
                if let Some(index) = mouse_selection(&screen, mouse.row as usize, height) {
                    let same_double = last_click.is_some_and(|(previous, at)| {
                        previous == index && now.duration_since(at) <= DOUBLE_CLICK_WINDOW
                    });
                    select_index(&mut screen, index);
                    last_click = Some((index, now));
                    message.clear();
                    if same_double {
                        if activate(ctx, &mut screen, &mut message) {
                            break;
                        }
                        last_click = None;
                    }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn project_choices(ctx: &Ctx) -> Vec<ProjectChoice> {
    project::list_slugs(&ctx.root)
        .into_iter()
        .take(MAX_RENDERED_PROJECTS)
        .filter_map(|slug| {
            let project = Project::load(&ctx.root, &slug).ok()?;
            let label = project
                .read_project_md()
                .map(|(settings, _)| project::display_name(&settings.name, &slug))
                .unwrap_or_else(|_| project::humanize(&slug));
            Some(ProjectChoice { project, label })
        })
        .collect()
}

fn open_tree(project: Project) -> Result<TreeView> {
    let mut entries = operational_tree(&project)?;
    let omitted_nodes = entries.len().saturating_sub(MAX_RENDERED_NODES);
    entries.truncate(MAX_RENDERED_NODES);
    Ok(TreeView {
        entries,
        omitted_nodes,
        project,
    })
}

/// The popup is an operational view. Resolved leaves remain persisted for
/// history but disappear from the tree. A resolved ancestor stays visible
/// while it still gives structure to an active descendant.
fn operational_tree(project: &Project) -> Result<Vec<TreeEntry>> {
    tree_entries(project, false)
}

pub(crate) fn tree_entries(project: &Project, show_resolved: bool) -> Result<Vec<TreeEntry>> {
    let all = organizations::tree(project)?;
    if show_resolved {
        return Ok(all);
    }
    let parents: BTreeMap<_, _> = all
        .iter()
        .map(|entry| {
            (
                entry.thread.id.clone(),
                organizations::parent_id(&entry.thread).to_string(),
            )
        })
        .collect();
    let mut visible: HashSet<String> = all
        .iter()
        .filter(|entry| entry.thread.status != thread::Status::Resolved)
        .map(|entry| entry.thread.id.clone())
        .collect();
    let mut pending: Vec<_> = visible.iter().cloned().collect();
    while let Some(id) = pending.pop() {
        let Some(parent) = parents.get(&id) else {
            continue;
        };
        if parent != organizations::ROOT_ID && visible.insert(parent.clone()) {
            pending.push(parent.clone());
        }
    }
    let records: Vec<_> = all
        .into_iter()
        .filter(|entry| visible.contains(&entry.thread.id))
        .map(|entry| entry.thread)
        .collect();
    organizations::tree_from(&records)
}

fn print_all(ctx: &Ctx) -> Result<()> {
    let choices = project_choices(ctx);
    if choices.is_empty() {
        println!("No organizations yet.");
        return Ok(());
    }
    let width = terminal_width();
    println!("Herdr Organizations");
    let mut printed_rows = 1usize;
    let mut limited = false;
    for choice in choices {
        if printed_rows >= MAX_PRINT_ALL_ROWS - 1 {
            limited = true;
            break;
        }
        let view = open_tree(choice.project)?;
        println!(
            "{}",
            fit_terminal_row(&format!("{} ({})", choice.label, view.project.slug), width)
        );
        printed_rows += 1;
        let rows = tree_lines_with_width(&view.project, &view.entries, view.omitted_nodes, width);
        for line in rows {
            if printed_rows >= MAX_PRINT_ALL_ROWS - 1 {
                limited = true;
                break;
            }
            println!("{}", fit_terminal_row(&format!("  {line}"), width));
            printed_rows += 1;
        }
        if limited {
            break;
        }
    }
    if limited {
        println!(
            "{}",
            fit_terminal_row("Output limited after 2,000 rows.", width)
        );
    }
    Ok(())
}

fn render(writer: &mut impl Write, screen: &Screen, message: &str) -> Result<()> {
    let height = terminal::size()
        .map(|(_, height)| height as usize)
        .unwrap_or(24);
    let width = terminal_width();
    let visible_rows = visible_row_capacity(height, 2);
    execute!(writer, MoveTo(0, 0), Clear(ClearType::All))?;
    match screen {
        Screen::Projects { choices, selected } => {
            write_display_line(writer, "Herdr Organizations", width)?;
            write_display_line(writer, "↑/k ↓/j move   Enter browse   Esc/q close", width)?;
            if choices.is_empty() {
                write_display_line(writer, "No organizations yet.", width)?;
            } else {
                let range = visible_range(choices.len(), *selected, visible_rows);
                for (index, choice) in choices.iter().enumerate().take(range.end).skip(range.start)
                {
                    write_selectable_line(
                        writer,
                        &format!("{}  ({})", choice.label, choice.project.slug),
                        width,
                        index == *selected,
                    )?;
                }
            }
        }
        Screen::Tree { view, selected } => {
            let label = view
                .project
                .read_project_md()
                .map(|(settings, _)| project::display_name(&settings.name, &view.project.slug))
                .unwrap_or_else(|_| project::humanize(&view.project.slug));
            write_display_line(
                writer,
                &format!("Herdr Organizations / {label} ({})", view.project.slug),
                width,
            )?;
            write_display_line(
                writer,
                "↑/k ↓/j move   Enter open   r refresh   Esc/q projects",
                width,
            )?;
            let rows =
                tree_lines_with_width(&view.project, &view.entries, view.omitted_nodes, width);
            let range = visible_range(rows.len(), *selected, visible_rows);
            for (index, row) in rows.iter().enumerate().take(range.end).skip(range.start) {
                write_selectable_line(writer, row, width, index == *selected)?;
            }
        }
    }
    if !message.is_empty() {
        execute!(
            writer,
            MoveTo(0, height.saturating_sub(1).min(u16::MAX as usize) as u16)
        )?;
        write!(writer, "{}", fit_terminal_row(message, width))?;
    }
    writer.flush()?;
    Ok(())
}

fn terminal_width() -> usize {
    terminal::size()
        .map(|(width, _)| width as usize)
        .unwrap_or(120)
        .max(1)
}

fn write_display_line(writer: &mut impl Write, value: &str, width: usize) -> Result<()> {
    write!(writer, "{}\r\n", fit_terminal_row(value, width))?;
    Ok(())
}

fn write_selectable_line(
    writer: &mut impl Write,
    value: &str,
    width: usize,
    selected: bool,
) -> Result<()> {
    if selected {
        execute!(writer, SetAttribute(Attribute::Reverse))?;
    }
    write!(writer, "{}", fit_terminal_row(value, width))?;
    if selected {
        execute!(writer, SetAttribute(Attribute::Reset))?;
    }
    write!(writer, "\r\n")?;
    Ok(())
}

#[cfg(test)]
fn tree_lines(project: &Project, entries: &[TreeEntry]) -> Vec<String> {
    tree_lines_with_width(project, entries, 0, terminal_width())
}

fn tree_lines_with_width(
    project: &Project,
    entries: &[TreeEntry],
    omitted_nodes: usize,
    width: usize,
) -> Vec<String> {
    let coordinator_state = project.coordinator().map_or_else(
        || "not opened".to_string(),
        |record| {
            if record.pane_id.is_empty() {
                "not placed".to_string()
            } else if record.prime_pending {
                "priming pending".to_string()
            } else {
                "pane recorded".to_string()
            }
        },
    );
    let shown_count = entries.len().min(MAX_RENDERED_NODES);
    let omitted_count = omitted_nodes.saturating_add(entries.len() - shown_count);
    let mut root = format!(
        "Project coordinator  [{}]  {}",
        project.status(),
        coordinator_state,
    );
    if omitted_count > 0 {
        root.push_str(&format!(
            "  showing {shown_count} of {} nodes",
            shown_count.saturating_add(omitted_count)
        ));
    }
    let mut output = vec![fit_terminal_row(&root, width)];
    for entry in entries.iter().take(MAX_RENDERED_NODES) {
        let state = thread::Group::from_token(&entry.thread.last_group)
            .map(thread::Group::label)
            .unwrap_or_else(|| match entry.thread.status {
                thread::Status::Starting => "Starting",
                thread::Status::Open => "Not polled yet",
                thread::Status::Failed => "Waiting on you",
                thread::Status::Resolved => "Resolved",
            });
        let row = format!(
            "{}{}  [{}]  {}  {}",
            entry.prefix,
            entry.thread.title,
            state,
            entry.thread.role.as_str(),
            entry.thread.id,
        );
        output.push(fit_terminal_row(&row, width));
    }
    output
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

pub(crate) fn fit_terminal_row(value: &str, max_width: usize) -> String {
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

fn change_selection(screen: &mut Screen, movement: isize) {
    match screen {
        Screen::Projects { choices, selected } => move_selection(selected, choices.len(), movement),
        Screen::Tree { view, selected } => {
            move_selection(selected, view.entries.len() + 1, movement)
        }
    }
}

fn move_selection(selected: &mut usize, count: usize, movement: isize) {
    if count == 0 {
        *selected = 0;
    } else if movement < 0 {
        *selected = selected.saturating_sub(movement.unsigned_abs());
    } else {
        *selected = selected.saturating_add(movement as usize).min(count - 1);
    }
}

fn select_index(screen: &mut Screen, index: usize) {
    match screen {
        Screen::Projects { choices, selected } if index < choices.len() => *selected = index,
        Screen::Tree { view, selected } if index <= view.entries.len() => *selected = index,
        _ => {}
    }
}

fn mouse_selection(screen: &Screen, row: usize, height: usize) -> Option<usize> {
    let first_row = 2;
    let content_row = row.checked_sub(first_row)?;
    let visible_rows = visible_row_capacity(height, first_row);
    if content_row >= visible_rows {
        return None;
    }
    let (count, selected) = match screen {
        Screen::Projects { choices, selected } => (choices.len(), *selected),
        Screen::Tree { view, selected } => (view.entries.len() + 1, *selected),
    };
    let range = visible_range(count, selected, visible_rows);
    let index = range.start + content_row;
    (index < range.end).then_some(index)
}

fn visible_row_capacity(height: usize, first_row: usize) -> usize {
    height.saturating_sub(first_row + 1).max(1)
}

fn visible_range(count: usize, selected: usize, capacity: usize) -> Range<usize> {
    if count <= capacity {
        return 0..count;
    }
    let capacity = capacity.max(1).min(count);
    let start = selected.saturating_sub(capacity - 1).min(count - capacity);
    start..start + capacity
}

/// Returns true when the popup should close because the selected session was
/// opened successfully.
fn activate(ctx: &Ctx, screen: &mut Screen, message: &mut String) -> bool {
    match screen {
        Screen::Projects { choices, selected } => {
            if let Some(choice) = choices.get(*selected) {
                match open_tree(choice.project.clone()) {
                    Ok(view) => {
                        *screen = Screen::Tree { view, selected: 0 };
                        message.clear();
                    }
                    Err(error) => *message = format!("Could not read organization tree: {error:#}"),
                }
            }
            false
        }
        Screen::Tree { view, selected } => {
            let node = selected
                .checked_sub(1)
                .and_then(|index| view.entries.get(index).map(|entry| &entry.thread));
            match focus_node(ctx, &view.project, node) {
                Ok(_) => true,
                Err(error) => {
                    *message = format!("Could not open selection: {error:#}");
                    false
                }
            }
        }
    }
}

fn refresh(ctx: &Ctx, screen: &mut Screen, message: &mut String) {
    if let Screen::Tree { view, selected } = screen {
        match open_tree(view.project.clone()) {
            Ok(refreshed) => {
                let max = refreshed.entries.len();
                *view = refreshed;
                *selected = (*selected).min(max);
                message.clear();
            }
            Err(error) => *message = format!("Could not refresh organization tree: {error:#}"),
        }
    } else {
        *screen = Screen::Projects {
            choices: project_choices(ctx),
            selected: 0,
        };
        message.clear();
    }
}

pub(crate) fn focus_node(ctx: &Ctx, project: &Project, node: Option<&Thread>) -> Result<String> {
    let recorded_socket = project.coordinator().and_then(|record| {
        (!record.socket.is_empty()).then(|| std::path::PathBuf::from(record.socket))
    });
    let options = coordinator::OpenOptions {
        session: SessionFlags {
            session: None,
            socket: recorded_socket.filter(|socket| socket.exists()),
        },
        reprime: false,
        rebind: true,
    };
    coordinator::open(ctx, &project.slug, &options)
        .context("could not open the project coordinator")?;

    let project = Project::load(&ctx.root, &project.slug)?;
    let coordinator = project
        .coordinator()
        .context("the project coordinator has not been placed")?;
    let Some(selected) = node else {
        let view = threads::session_view(ctx, &project)
            .context("the project's Herdr session did not become reachable")?;
        if view
            .agents
            .iter()
            .any(|agent| coordinator::agent_matches(&coordinator, agent))
        {
            return focus_live_pane(&view.herdr, &coordinator.pane_id, true);
        }
        if view
            .panes
            .iter()
            .any(|pane| coordinator::pane_matches(&coordinator, pane))
        {
            view.herdr
                .tab_focus(&coordinator.tab_id)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            return Ok(coordinator.pane_id);
        }
        bail!("the project coordinator has no live pane after opening");
    };

    let mut record = thread::load(&project, &selected.id)?;
    if record.status == thread::Status::Resolved {
        bail!(
            "{} is resolved and cannot be reopened automatically",
            record.id
        );
    }
    let view = threads::session_view(ctx, &project)
        .context("the project's Herdr session did not become reachable")?;
    let herdr = view.herdr.on_machine(&record.machine);
    let (agents, panes) = if record.is_remote() {
        (
            herdr
                .agent_list()
                .context("could not list agents on the node's machine")?,
            herdr
                .pane_list()
                .context("could not list panes on the node's machine")?,
        )
    } else {
        (view.agents, view.panes)
    };

    if agents
        .iter()
        .any(|agent| thread::agent_matches(&record, agent))
    {
        return focus_live_pane(&herdr, &record.pane_id, true);
    }
    if panes.iter().any(|pane| thread::pane_matches(&record, pane)) {
        herdr
            .tab_focus(&record.tab_id)
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        return Ok(record.pane_id);
    }

    record = threads::restart(ctx, &project.slug, &record.id)
        .with_context(|| format!("could not reopen {}", record.id))?;
    view.herdr
        .on_machine(&record.machine)
        .tab_focus(&record.tab_id)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(record.pane_id)
}

fn focus_live_pane(herdr: &Herdr<'_>, pane_id: &str, is_live: bool) -> Result<String> {
    if pane_id.is_empty() || !is_live {
        bail!("the selected node has no live pane");
    }
    herdr
        .agent_focus(pane_id)
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(pane_id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn navigation_clamps_and_mouse_rows_map_to_visible_items() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let screen = Screen::Projects {
            choices: project_choices(&world.ctx()),
            selected: 0,
        };
        assert_eq!(mouse_selection(&screen, 1, 10), None);
        assert_eq!(mouse_selection(&screen, 2, 10), Some(0));
        assert_eq!(mouse_selection(&screen, 3, 10), None);
        let mut selected = 0;
        move_selection(&mut selected, 2, -1);
        assert_eq!(selected, 0);
        move_selection(&mut selected, 2, 1);
        assert_eq!(selected, 1);
        move_selection(&mut selected, 2, 20);
        assert_eq!(selected, 1);
        assert_eq!(visible_range(20, 0, 5), 0..5);
        assert_eq!(visible_range(20, 15, 5), 11..16);
        assert_eq!(visible_range(20, 19, 5), 15..20);
        assert!(project.dir().is_dir());
    }

    #[test]
    fn tree_rows_lead_with_titles_and_keep_compact_operational_metadata() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let coordinator = organizations::tree_from(&[
            Thread {
                id: "t-0001".into(),
                parent_id: organizations::ROOT_ID.into(),
                role: thread::NodeRole::Coordinator,
                can_spawn: true,
                title: "Area lead".into(),
                ..Thread::default()
            },
            Thread {
                id: "t-0002".into(),
                parent_id: "t-0001".into(),
                title: "Builder".into(),
                ..Thread::default()
            },
        ])
        .unwrap();
        let lines = tree_lines(&project, &coordinator);
        assert!(lines[0].starts_with("Project coordinator  [active]"));
        assert!(lines[1].contains("Area lead  [Starting]  coordinator  t-0001"));
        assert!(lines[2].contains("└─ Builder  [Starting]  worker  t-0002"));
        assert!(!lines.iter().any(|line| line.contains("parent=")));
    }

    #[test]
    fn tree_screen_renders_the_root_and_recursive_rows_without_a_terminal() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let entries = organizations::tree_from(&[
            Thread {
                id: "t-0001".into(),
                parent_id: organizations::ROOT_ID.into(),
                role: thread::NodeRole::Coordinator,
                can_spawn: true,
                title: "Area lead".into(),
                ..Thread::default()
            },
            Thread {
                id: "t-0002".into(),
                parent_id: "t-0001".into(),
                title: "Builder".into(),
                ..Thread::default()
            },
        ])
        .unwrap();
        let screen = Screen::Tree {
            view: TreeView {
                project,
                entries,
                omitted_nodes: 0,
            },
            selected: 0,
        };
        let mut output = Vec::new();
        render(&mut output, &screen, "").unwrap();
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("Project coordinator  [active]"));
        assert!(rendered.contains("└─ Area lead  [Starting]  coordinator  t-0001"));
        assert!(rendered.contains("   └─ Builder  [Starting]  worker  t-0002"));
    }

    #[test]
    fn operational_tree_hides_resolved_leaves_but_keeps_needed_ancestors() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        crate::thread::allocate(&project, |thread| {
            thread.id = "t-0001".into();
            thread.parent_id = organizations::ROOT_ID.into();
            thread.role = crate::thread::NodeRole::Coordinator;
            thread.can_spawn = true;
            thread.status = thread::Status::Resolved;
        })
        .unwrap();
        crate::thread::allocate(&project, |thread| {
            thread.id = "t-0002".into();
            thread.parent_id = "t-0001".into();
            thread.status = thread::Status::Open;
        })
        .unwrap();
        crate::thread::allocate(&project, |thread| {
            thread.id = "t-0003".into();
            thread.parent_id = organizations::ROOT_ID.into();
            thread.status = thread::Status::Resolved;
        })
        .unwrap();

        let entries = operational_tree(&project).unwrap();

        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.thread.id.as_str())
                .collect::<Vec<_>>(),
            ["t-0001", "t-0002"]
        );
        assert_eq!(entries[1].prefix, "   └─ ");
    }

    #[test]
    fn rendered_rows_return_to_column_zero_and_reset_selection_before_newline() {
        let mut output = Vec::new();

        write_display_line(&mut output, "header", 80).unwrap();
        write_selectable_line(&mut output, "selected", 80, true).unwrap();

        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.starts_with("header\r\n"));
        assert!(rendered.ends_with("selected\x1b[0m\r\n"));
        assert!(
            rendered
                .as_bytes()
                .windows(2)
                .filter(|bytes| bytes[1] == b'\n')
                .all(|bytes| bytes[0] == b'\r')
        );
    }

    #[test]
    fn mouse_selection_resolves_rows_after_the_tree_scrolls() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let entries = (1..=20)
            .map(|number| TreeEntry {
                thread: Thread {
                    id: format!("t-{number:04}"),
                    parent_id: organizations::ROOT_ID.into(),
                    ..Thread::default()
                },
                depth: 1,
                tree_order: number,
                is_last: number == 20,
                prefix: "├─ ".into(),
            })
            .collect();
        let screen = Screen::Tree {
            view: TreeView {
                project,
                entries,
                omitted_nodes: 0,
            },
            selected: 15,
        };
        assert_eq!(mouse_selection(&screen, 2, 8), Some(11));
        assert_eq!(mouse_selection(&screen, 6, 8), Some(15));
        assert_eq!(mouse_selection(&screen, 7, 8), None);
    }

    #[test]
    fn focus_selection_sends_an_explicit_pane_focus_request() {
        let world = crate::scenarios::World::new();
        world
            .runner
            .on("agent focus", crate::runner::fake::ok(r#"{"result":{}}"#));
        let ctx = world.ctx();
        let herdr = Herdr::new(ctx.env.herdr_bin(), "socket", ctx.runner);
        focus_live_pane(&herdr, "w1:p7", true).unwrap();
        assert!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().contains("agent focus w1:p7"))
        );
        assert!(focus_live_pane(&herdr, "w1:p7", false).is_err());
    }

    #[test]
    fn tab_focus_uses_the_exact_herdr_command() {
        let world = crate::scenarios::World::new();
        world
            .runner
            .on("tab focus", crate::runner::fake::ok(r#"{"result":{}}"#));
        let ctx = world.ctx();
        let herdr = Herdr::new(ctx.env.herdr_bin(), "socket", ctx.runner);

        herdr.tab_focus("w1:t7").unwrap();

        assert!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|call| call.display().ends_with("tab focus w1:t7"))
        );
    }

    #[test]
    fn opening_a_live_node_focuses_it_and_tells_the_popup_to_close() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let thread_cwd = world.home.path().join("thread");
        let worker = world.thread(&project, &thread_cwd, |_| {});
        *world.panes.borrow_mut() = format!(
            "[{},{}]",
            world.coordinator_pane(&project),
            crate::scenarios::pane_json(
                &worker.workspace_id,
                &worker.tab_id,
                &worker.pane_id,
                &worker.cwd,
            )
        );
        *world.agents.borrow_mut() = format!(
            "[{},{}]",
            crate::scenarios::agent_json(
                "w1",
                "w1:t1",
                "w1:p1",
                &project.canonical_dir().to_string_lossy(),
                "hp-demo-coordinator",
                "idle",
            ),
            crate::scenarios::agent_json(
                &worker.workspace_id,
                &worker.tab_id,
                &worker.pane_id,
                &worker.cwd,
                &worker.agent_name,
                "idle",
            )
        );
        world
            .runner
            .on("agent focus", crate::runner::fake::ok(r#"{"result":{}}"#));
        let entries = organizations::tree(&project).unwrap();
        let mut screen = Screen::Tree {
            view: TreeView {
                project,
                entries,
                omitted_nodes: 0,
            },
            selected: 1,
        };
        let mut message = String::new();

        assert!(activate(&world.ctx(), &mut screen, &mut message));
        assert!(message.is_empty());
        assert!(world.runner.calls.borrow().iter().any(|call| {
            call.display()
                .ends_with(&format!("agent focus {}", worker.pane_id))
        }));
    }

    #[test]
    fn opening_a_closed_tab_node_recreates_and_focuses_its_tab() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let worker = crate::thread::allocate(&project, |thread| {
            thread.title = "Closed task".into();
            thread.status = thread::Status::Open;
            thread.kind = thread::Kind::Tab;
            thread.agent = "codex".into();
            thread.workspace_id = "w1".into();
            thread.tab_id = "w1:old".into();
            thread.pane_id = "w1:old-pane".into();
        })
        .unwrap();
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        *world.agents.borrow_mut() = format!(
            "[{}]",
            crate::scenarios::agent_json(
                "w1",
                "w1:t1",
                "w1:p1",
                &project.canonical_dir().to_string_lossy(),
                "hp-demo-coordinator",
                "idle",
            )
        );
        world
            .runner
            .on("agent focus", crate::runner::fake::ok(r#"{"result":{}}"#));
        world.runner.on(
            "tab create",
            crate::runner::fake::ok(
                r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2"}}}"#,
            ),
        );
        world
            .runner
            .on("tab focus", crate::runner::fake::ok(r#"{"result":{}}"#));

        let opened = focus_node(&world.ctx(), &project, Some(&worker)).unwrap();

        assert_eq!(opened, "w1:p2");
        assert!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|call| { call.display().ends_with("tab focus w1:t2") })
        );
        let reopened = thread::load(&project, &worker.id).unwrap();
        assert_eq!(reopened.tab_id, "w1:t2");
        assert_eq!(reopened.pane_id, "w1:p2");
        assert!(reopened.prompt_pending);
    }

    #[test]
    fn opening_a_coordinator_at_a_shell_focuses_its_existing_tab() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
        world
            .runner
            .on("tab focus", crate::runner::fake::ok(r#"{"result":{}}"#));

        let opened = focus_node(&world.ctx(), &project, None).unwrap();

        assert_eq!(opened, "w1:p1");
        assert!(
            world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|call| { call.display().ends_with("tab focus w1:t1") })
        );
    }

    #[test]
    fn terminal_text_removes_osc_controls_newlines_and_bidi_marks() {
        let hostile = concat!(
            "safe",
            "\u{001b}]0;owned title\u{0007}",
            "\u{001b}[31mred\u{001b}[0m",
            "\u{202e}rtl",
            "\u{2066}isolate\u{2069}",
            "\u{0085}tail\nnext"
        );
        let clean = sanitize_terminal_text(hostile);
        assert_eq!(clean, "saferedrtlisolatetailnext");
        assert!(!clean.chars().any(char::is_control));
        assert!(!clean.chars().any(is_bidi_or_line_control));
    }

    #[test]
    fn display_width_truncation_handles_wide_and_huge_text() {
        assert_eq!(fit_terminal_row("日".repeat(8).as_str(), 5), "日日…");
        let huge = "x".repeat(50_000);
        let row = fit_terminal_row(&huge, 31);
        assert_eq!(
            UnicodeWidthChar::width(row.chars().last().unwrap()),
            Some(1)
        );
        assert!(row.chars().count() <= 124);
        assert!(UnicodeWidthChar::width(row.chars().next().unwrap()).unwrap() <= 2);
    }

    #[test]
    fn hostile_and_huge_titles_are_sanitized_without_changing_stored_titles() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let title = format!("Lead\u{001b}]2;owned\u{0007}\u{202e}\n{}", "界".repeat(200));
        let entries = organizations::tree_from(&[Thread {
            id: "t-0001".into(),
            parent_id: organizations::ROOT_ID.into(),
            role: thread::NodeRole::Coordinator,
            can_spawn: true,
            title: title.clone(),
            ..Thread::default()
        }])
        .unwrap();
        let rows = tree_lines_with_width(&project, &entries, 0, 24);

        assert_eq!(entries[0].thread.title, title);
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|row| UnicodeWidthStr::width(row.as_str()) <= 24)
        );
        assert!(!rows[1].contains("\u{001b}"));
        assert!(!rows[1].contains("\u{202e}"));
        assert!(!rows[1].contains('\n'));
        assert!(rows[1].ends_with('…'));
    }

    #[test]
    fn very_large_trees_render_only_a_bounded_number_of_rows() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let entries = (1..=MAX_RENDERED_NODES + 250)
            .map(|number| TreeEntry {
                thread: Thread {
                    id: format!("t-{number:04}"),
                    parent_id: organizations::ROOT_ID.into(),
                    title: "Long node title".repeat(100),
                    ..Thread::default()
                },
                depth: 1,
                tree_order: number,
                is_last: number == MAX_RENDERED_NODES + 250,
                prefix: "├─ ".into(),
            })
            .collect::<Vec<_>>();
        let rows = tree_lines_with_width(&project, &entries, 0, 120);

        assert_eq!(rows.len(), MAX_RENDERED_NODES + 1);
        assert!(rows[0].contains("showing 1000 of 1250 nodes"));
        assert!(
            rows.iter()
                .all(|row| UnicodeWidthStr::width(row.as_str()) <= 120)
        );
    }
}
